use super::Exposure;
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageBuffer};
use libheif_rs::{
    ColorSpace, DecodingOptions, FileTypeResult, HeifContext, LibHeif, Plane, RgbChroma,
    StreamReader,
};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

pub(super) fn is_heif(path: &Path) -> Result<bool> {
    let mut reader =
        BufReader::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
    let header = reader.fill_buf().context("reading image header")?;
    Ok(header.len() >= 12
        && matches!(
            libheif_rs::check_file_type(header),
            FileTypeResult::Supported | FileTypeResult::Unsupported
        ))
}

pub(super) fn decode(path: &Path) -> Result<(DynamicImage, Exposure)> {
    let library = LibHeif::new_checked().context("initializing HEIF codecs")?;
    let file = File::open(path).context("opening HEIF image")?;
    let length = file.metadata()?.len();
    let mut context =
        HeifContext::read_from_reader(Box::new(StreamReader::new(BufReader::new(file), length)))
            .context("reading HEIF container")?;
    context.set_max_decoding_threads(1);
    let handle = context
        .primary_image_handle()
        .context("finding primary HEIF image")?;
    let mut metadata_ids = [0];
    let exposure = if handle.metadata_block_ids(&mut metadata_ids, u32::from_be_bytes(*b"Exif")) > 0
    {
        handle
            .metadata(metadata_ids[0])
            .ok()
            .map(|bytes| Exposure::from_offset_exif(&bytes))
            .unwrap_or_default()
    } else {
        Exposure::default()
    };
    let monochrome = handle
        .preferred_decoding_colorspace()
        .context("reading HEIF color space")?
        == ColorSpace::Monochrome;
    let gray16 = monochrome && !handle.has_alpha_channel() && handle.luma_bits_per_pixel() > 8;
    let rgb16 = !monochrome
        && handle
            .luma_bits_per_pixel()
            .max(handle.chroma_bits_per_pixel())
            > 8;
    let alpha = handle.has_alpha_channel();
    let mut options = DecodingOptions::new().context("allocating HEIF decode options")?;
    options.set_strict_decoding(true);
    options.set_convert_hdr_to_8bit(!(gray16 || rgb16));
    let decoded = library
        .decode(
            &handle,
            if gray16 {
                ColorSpace::Monochrome
            } else if rgb16 {
                ColorSpace::Rgb(if alpha {
                    RgbChroma::HdrRgbaBe
                } else {
                    RgbChroma::HdrRgbBe
                })
            } else {
                ColorSpace::Rgb(RgbChroma::Rgba)
            },
            Some(options),
        )
        .context("decoding HEIF pixels with the in-process HEVC/AV1 decoder")?;
    if gray16 {
        let plane = decoded
            .planes()
            .y
            .context("HEIF image has no grayscale plane")?;
        ensure!(
            (9..=16).contains(&plane.bits_per_pixel) && plane.storage_bits_per_pixel == 16,
            "unsupported HEIF grayscale depth"
        );
        let (width, height) = (plane.width, plane.height);
        let bytes = packed_plane(plane, 2)?;
        let samples = bytes
            .chunks_exact(2)
            .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
            .collect();
        Ok((
            DynamicImage::ImageLuma16(
                ImageBuffer::from_vec(width, height, samples)
                    .context("invalid HEIF grayscale buffer")?,
            ),
            exposure,
        ))
    } else if rgb16 {
        let plane = decoded
            .planes()
            .interleaved
            .context("HEIF image has no high-depth RGB plane")?;
        let channels = if alpha { 4 } else { 3 };
        ensure!(
            (9..=16).contains(&plane.bits_per_pixel)
                && usize::from(plane.storage_bits_per_pixel) == channels * 16,
            "unsupported HEIF RGB depth"
        );
        let maximum = (1u32 << plane.bits_per_pixel) - 1;
        let (width, height) = (plane.width, plane.height);
        let mut pixels: Vec<u16> = packed_plane(plane, channels * 2)?
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        if alpha {
            for pixel in pixels.chunks_exact_mut(4) {
                let opacity = u32::from(pixel[3]);
                if decoded.is_premultiplied_alpha() {
                    for value in &mut pixel[..3] {
                        *value = (u32::from(*value) * maximum + opacity / 2)
                            .checked_div(opacity)
                            .unwrap_or(0)
                            .min(maximum) as u16;
                    }
                }
                pixel[3] = ((opacity * 65535 + maximum / 2) / maximum) as u16;
            }
        }
        let image = if alpha {
            DynamicImage::ImageRgba16(
                ImageBuffer::from_vec(width, height, pixels)
                    .context("invalid HEIF high-depth RGBA buffer")?,
            )
        } else {
            DynamicImage::ImageRgb16(
                ImageBuffer::from_vec(width, height, pixels)
                    .context("invalid HEIF high-depth RGB buffer")?,
            )
        };
        Ok((image, exposure))
    } else {
        let plane = decoded
            .planes()
            .interleaved
            .context("HEIF image has no RGBA plane")?;
        ensure!(
            plane.bits_per_pixel == 8 && plane.storage_bits_per_pixel == 32,
            "unsupported HEIF display depth"
        );
        let (width, height) = (plane.width, plane.height);
        let mut pixels = packed_plane(plane, 4)?;
        if decoded.is_premultiplied_alpha() {
            for pixel in pixels.chunks_exact_mut(4) {
                let alpha = u32::from(pixel[3]);
                for value in &mut pixel[..3] {
                    *value = (u32::from(*value) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0)
                        .min(255) as u8;
                }
            }
        }
        let image = DynamicImage::ImageRgba8(
            ImageBuffer::from_vec(width, height, pixels).context("invalid HEIF RGBA buffer")?,
        );
        let image = if monochrome {
            if alpha {
                DynamicImage::ImageLumaA8(image.into_luma_alpha8())
            } else {
                DynamicImage::ImageLuma8(image.into_luma8())
            }
        } else {
            image
        };
        Ok((image, exposure))
    }
}

fn packed_plane(plane: Plane<&[u8]>, pixel_bytes: usize) -> Result<Vec<u8>> {
    let row_bytes = (plane.width as usize)
        .checked_mul(pixel_bytes)
        .context("HEIF row is too wide")?;
    let length = plane
        .stride
        .checked_mul(plane.height as usize)
        .context("HEIF plane is too large")?;
    ensure!(
        row_bytes > 0
            && plane.height > 0
            && plane.stride >= row_bytes
            && plane.data.len() >= length,
        "invalid HEIF plane dimensions or stride"
    );
    Ok(plane
        .data
        .chunks_exact(plane.stride)
        .take(plane.height as usize)
        .flat_map(|row| row[..row_bytes].iter().copied())
        .collect())
}
