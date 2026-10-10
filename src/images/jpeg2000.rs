use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageBuffer, Limits, metadata::Orientation};
use jpeg2k::{ColorSpace, DecodeParameters, DumpImage, Image, ImageComponent};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

pub(super) fn is_jpeg2000(path: &Path) -> Result<bool> {
    let mut reader =
        BufReader::new(File::open(path).with_context(|| format!("opening {}", path.display()))?);
    Ok(
        jpeg2k::format::j2k_detect_format(reader.fill_buf().context("reading image header")?)
            .is_ok(),
    )
}

pub(super) fn decode(path: &Path) -> Result<DynamicImage> {
    let mut limits = Limits::default();
    let mut bytes = Vec::new();
    File::open(path)
        .context("opening JPEG2000 image")?
        .take(limits.max_alloc.unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .context("reading JPEG2000 image")?;
    limits
        .reserve_usize(bytes.len())
        .context("JPEG2000 input exceeds the image allocation limit")?;
    let decoder = DumpImage::from_bytes_with(&bytes, DecodeParameters::new().strict(true))
        .context("reading JPEG2000 header")?;
    layout(&decoder.img, limits.clone())?;
    decoder.decode().context("decoding JPEG2000 pixels")?;
    let (width, height, gray16) = layout(&decoder.img, limits)?;
    let components = decoder.img.components();
    // OpenJPEG applies JP2 channel definitions only after decoding.
    let alpha = matches!(components.len(), 2 | 4);
    ensure!(
        components.iter().enumerate().all(|(index, component)| {
            component.is_alpha() == (alpha && index == components.len() - 1)
        }),
        "unsupported JPEG2000 alpha layout"
    );
    let planes: Vec<_> = components.iter().map(ImageComponent::data).collect();
    let mut image = if gray16 {
        let samples = planes[0]
            .iter()
            .map(|&value| sample(&components[0], value))
            .collect::<Result<Vec<_>>>()?;
        DynamicImage::ImageLuma16(
            ImageBuffer::from_vec(width, height, samples)
                .context("invalid JPEG2000 grayscale buffer")?,
        )
    } else {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for index in 0..planes[0].len() {
            let mut values = [0, 0, 0, 255];
            for (channel, (component, plane)) in components.iter().zip(&planes).enumerate() {
                let value = u32::from(sample(component, plane[index])?);
                let maximum = (1u32 << component.precision()) - 1;
                values[channel] = ((value * 255 + maximum / 2) / maximum) as u8;
            }
            pixels.extend_from_slice(&match components.len() {
                1 => [values[0], values[0], values[0], 255],
                2 => [values[0], values[0], values[0], values[1]],
                _ => values,
            });
        }
        DynamicImage::ImageRgba8(
            ImageBuffer::from_vec(width, height, pixels)
                .context("invalid JPEG2000 display buffer")?,
        )
    };
    image.apply_orientation(orientation(&bytes)?);
    Ok(image)
}

fn layout(image: &Image, mut limits: Limits) -> Result<(u32, u32, bool)> {
    ensure!(
        (1..=4).contains(&image.num_components()),
        "unsupported JPEG2000 component count"
    );
    ensure!(
        matches!(
            image.color_space(),
            ColorSpace::Unknown | ColorSpace::Unspecified | ColorSpace::Gray | ColorSpace::SRGB
        ),
        "unsupported JPEG2000 color space: {:?}",
        image.color_space()
    );
    let components = image.components();
    let (width, height) = (image.width(), image.height());
    let count = width
        .checked_mul(height)
        .filter(|&count| count > 0)
        .context("JPEG2000 image is empty or too large")?;
    ensure!(
        width == image.orig_width()
            && height == image.orig_height()
            && components.iter().all(|component| {
                component.width() == width
                    && component.height() == height
                    && (1..=16).contains(&component.precision())
            }),
        "unsupported JPEG2000 component dimensions or precision"
    );
    let gray16 = components.len() == 1 && components[0].precision() > 8;
    let bytes_per_pixel = u64::from(image.num_components()) * 4 + if gray16 { 2 } else { 4 };
    limits.check_dimensions(width, height)?;
    limits
        .reserve(u64::from(count) * bytes_per_pixel)
        .context("JPEG2000 pixels exceed the image allocation limit")?;
    Ok((width, height, gray16))
}

fn sample(component: &ImageComponent, value: i32) -> Result<u16> {
    let bits = component.precision();
    let offset = if component.is_signed() {
        1i64 << (bits - 1)
    } else {
        0
    };
    let value = i64::from(value) + offset;
    ensure!(
        (0..(1i64 << bits)).contains(&value),
        "JPEG2000 sample is outside its declared precision"
    );
    Ok(value as u16)
}

fn orientation(mut bytes: &[u8]) -> Result<Orientation> {
    if !bytes.starts_with(jpeg2k::format::JP2_RFC3745_MAGIC) {
        return Ok(Orientation::NoTransforms);
    }
    while !bytes.is_empty() {
        let header = bytes.get(..8).context("truncated JPEG2000 box")?;
        let size = u32::from_be_bytes(header[..4].try_into()?);
        let (header_size, length) = match size {
            0 => (8, bytes.len()),
            1 => (
                16,
                usize::try_from(u64::from_be_bytes(
                    bytes
                        .get(8..16)
                        .context("truncated extended JPEG2000 box")?
                        .try_into()?,
                ))?,
            ),
            size => (8, size as usize),
        };
        let payload = bytes
            .get(header_size..length)
            .context("invalid JPEG2000 box length")?;
        if &header[4..8] == b"uuid" && payload.starts_with(b"JpgTiffExif->JP2") {
            return Ok(
                Orientation::from_exif_chunk(&payload[16..]).unwrap_or(Orientation::NoTransforms)
            );
        }
        bytes = &bytes[length..];
    }
    Ok(Orientation::NoTransforms)
}
