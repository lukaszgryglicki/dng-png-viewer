use super::Exposure;
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageBuffer, Limits};
use jxl_oxide::{
    AllocTracker, AuxBoxData, EnumColourEncoding, InitializeResult, JxlImage, JxlThreadPool,
    RenderingIntent, image::BitDepth,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

pub(super) fn is_jpegxl(path: &Path) -> Result<bool> {
    let mut reader =
        BufReader::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
    let header = reader.fill_buf().context("reading image header")?;
    Ok(header.starts_with(b"\xff\x0a") || header.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n"))
}

pub(super) fn decode(path: &Path) -> Result<(DynamicImage, Exposure)> {
    let mut limits = Limits::default();
    let mut bytes = Vec::new();
    File::open(path)
        .context("opening JPEG XL image")?
        .take(limits.max_alloc.unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .context("reading JPEG XL image")?;
    limits
        .reserve_usize(bytes.len())
        .context("JPEG XL input exceeds the image allocation limit")?;
    let budget = limits
        .max_alloc
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    let tracker = AllocTracker::with_limit(budget);
    // Preload workers already run concurrently; do not create a pool per image.
    let mut uninit = JxlImage::builder()
        .pool(JxlThreadPool::none())
        .alloc_tracker(tracker.clone())
        .build_uninit();
    let mut offset = 0;
    let mut image = loop {
        let end = bytes.len().min(offset + 4096);
        let consumed = uninit
            .feed_bytes(&bytes[offset..end])
            .map_err(|error| anyhow::anyhow!("reading JPEG XL header: {error}"))?;
        ensure!(consumed > 0, "truncated JPEG XL header");
        offset += consumed;
        match uninit
            .try_init()
            .map_err(|error| anyhow::anyhow!("reading JPEG XL header: {error}"))?
        {
            InitializeResult::NeedMoreData(next) => uninit = next,
            InitializeResult::Initialized(image) => break image,
        }
    };
    let (width, height) = (image.width(), image.height());
    let count = width
        .checked_mul(height)
        .filter(|&count| count > 0)
        .context("JPEG XL image is empty or too large")?;
    let metadata = &image.image_header().metadata;
    let gray16 = image.pixel_format().is_grayscale()
        && !image.pixel_format().has_alpha()
        && matches!(
            metadata.bit_depth,
            BitDepth::IntegerSample {
                bits_per_sample: 9..=16
            }
        );
    let bits = metadata.bit_depth.bits_per_sample();
    let rgb16 = !image.pixel_format().is_grayscale()
        && matches!(
            metadata.bit_depth,
            BitDepth::IntegerSample {
                bits_per_sample: 9..=16
            }
        );
    let associated_alpha = metadata
        .ec_info
        .iter()
        .find_map(|channel| channel.alpha_associated())
        .unwrap_or(false);
    let channels = metadata.ec_info.len() + if metadata.grayscale() { 1 } else { 3 };
    let sample_bytes = u64::try_from(channels)?
        .checked_mul(4)
        .and_then(|size| size.checked_mul(u64::from(count)))
        .context("JPEG XL allocation size overflow")?;
    limits.check_dimensions(width, height)?;
    limits
        .reserve(
            sample_bytes
                .checked_add(
                    u64::from(count)
                        * if gray16 {
                            2
                        } else if rgb16 {
                            8
                        } else {
                            4
                        },
                )
                .context("JPEG XL allocation size overflow")?,
        )
        .context("JPEG XL pixels exceed the image allocation limit")?;
    let working_bytes = u64::from(count)
        .checked_mul(u64::try_from(channels.max(3))?)
        .and_then(|size| size.checked_mul(8))
        .context("JPEG XL working allocation size overflow")?;
    if image.pixel_format().has_black() {
        image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
    }
    while image.num_loaded_keyframes() == 0 {
        let end = bytes.len().min(offset + 4096);
        let consumed = image
            .feed_bytes(&bytes[offset..end])
            .map_err(|error| anyhow::anyhow!("reading JPEG XL frame: {error}"))?;
        ensure!(consumed > 0, "truncated JPEG XL frame");
        offset += consumed;
    }
    let exposure = read_exposure(&mut image, &bytes, offset);
    // Rendering can retain two RGB grid sets even for grayscale input.
    let input_len = bytes.len();
    drop(bytes);
    tracker.expand_limit(input_len + usize::try_from(working_bytes)?);
    let render = image
        .render_frame(0)
        .map_err(|error| anyhow::anyhow!("decoding JPEG XL pixels: {error}"))?;
    let mut stream = render.stream();
    ensure!(
        (stream.width(), stream.height()) == (width, height),
        "unexpected JPEG XL frame dimensions"
    );
    if gray16 {
        ensure!(
            stream.channels() == 1,
            "unexpected JPEG XL grayscale layout"
        );
        let mut samples = vec![0u16; count as usize];
        ensure!(
            stream.write_to_buffer(&mut samples) == samples.len(),
            "incomplete JPEG XL grayscale buffer"
        );
        // Integer streams normalize to 16 bits; restore the original code values.
        let maximum = (1u32 << bits) - 1;
        for value in &mut samples {
            *value = ((u32::from(*value) * maximum + 32767) / 65535) as u16;
        }
        return Ok((
            DynamicImage::ImageLuma16(
                ImageBuffer::from_vec(width, height, samples)
                    .context("invalid JPEG XL grayscale buffer")?,
            ),
            exposure,
        ));
    }
    let channels = stream.channels() as usize;
    ensure!(
        (1..=4).contains(&channels),
        "unsupported JPEG XL display layout"
    );
    let alpha = matches!(channels, 2 | 4);
    if rgb16 {
        ensure!((3..=4).contains(&channels), "unexpected JPEG XL RGB layout");
        let maximum = ((1u32 << bits) - 1) as f32;
        let mut pixels = vec![0u16; count as usize * channels];
        let mut row = vec![0f32; width as usize * channels];
        for output in pixels.chunks_exact_mut(width as usize * channels) {
            ensure!(
                stream.write_to_buffer(&mut row) == row.len(),
                "incomplete JPEG XL high-depth RGB buffer"
            );
            for (source, target) in row
                .chunks_exact(channels)
                .zip(output.chunks_exact_mut(channels))
            {
                let opacity = if alpha { source[3] } else { 1.0 };
                let scale = if associated_alpha {
                    if opacity > 0.0 {
                        maximum / opacity
                    } else {
                        0.0
                    }
                } else {
                    maximum
                };
                for channel in 0..3 {
                    target[channel] = (source[channel] * scale).round().clamp(0.0, maximum) as u16;
                }
                if alpha {
                    target[3] = (opacity * 65535.0).round().clamp(0.0, 65535.0) as u16;
                }
            }
        }
        let image = if alpha {
            DynamicImage::ImageRgba16(
                ImageBuffer::from_vec(width, height, pixels)
                    .context("invalid JPEG XL high-depth RGBA buffer")?,
            )
        } else {
            DynamicImage::ImageRgb16(
                ImageBuffer::from_vec(width, height, pixels)
                    .context("invalid JPEG XL high-depth RGB buffer")?,
            )
        };
        return Ok((image, exposure));
    }
    let mut pixels = vec![0; count as usize * 4];
    let mut row = vec![0f32; width as usize * channels];
    for output in pixels.chunks_exact_mut(width as usize * 4) {
        ensure!(
            stream.write_to_buffer(&mut row) == row.len(),
            "incomplete JPEG XL display buffer"
        );
        for (source, target) in row.chunks_exact(channels).zip(output.chunks_exact_mut(4)) {
            let opacity = if alpha { source[channels - 1] } else { 1.0 };
            let scale = if associated_alpha {
                if opacity > 0.0 { 255.0 / opacity } else { 0.0 }
            } else {
                255.0
            };
            for channel in 0..3 {
                let value = source[if channels <= 2 { 0 } else { channel }];
                target[channel] = (value * scale).round().clamp(0.0, 255.0) as u8;
            }
            target[3] = (opacity * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    let image = DynamicImage::ImageRgba8(
        ImageBuffer::from_vec(width, height, pixels).context("invalid JPEG XL display buffer")?,
    );
    let image = match channels {
        1 => DynamicImage::ImageLuma8(image.into_luma8()),
        2 => DynamicImage::ImageLumaA8(image.into_luma_alpha8()),
        _ => image,
    };
    Ok((image, exposure))
}

fn read_exposure(image: &mut JxlImage, bytes: &[u8], mut offset: usize) -> Exposure {
    while offset < bytes.len() && matches!(image.aux_boxes().first_exif(), Ok(AuxBoxData::Decoding))
    {
        let end = bytes.len().min(offset + 4096);
        match image.feed_bytes(&bytes[offset..end]) {
            Ok(0) | Err(_) => break,
            Ok(consumed) => offset += consumed,
        }
    }
    if offset == bytes.len() {
        let _ = image.finalize();
    }
    match image.aux_boxes().first_exif() {
        Ok(AuxBoxData::Data(exif)) => exif
            .payload()
            .get(exif.tiff_header_offset() as usize..)
            .map(Exposure::from_exif)
            .unwrap_or_default(),
        _ => Exposure::default(),
    }
}
