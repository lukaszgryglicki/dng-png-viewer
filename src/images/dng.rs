use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageBuffer, metadata::Orientation};
use rawler::{
    RawImageData,
    imgop::{Dim2, Point, Rect, develop::RawDevelop},
    rawimage::RawPhotometricInterpretation,
};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

pub(super) fn is_dng(path: &Path) -> Result<bool> {
    let mut file = BufReader::new(File::open(path).context("opening TIFF header")?);
    let mut header = [0; 8];
    file.read_exact(&mut header)
        .context("reading TIFF header")?;
    let little = match &header[..4] {
        b"II\x2a\0" => true,
        b"MM\0\x2a" => false,
        _ => return Ok(false),
    };
    let short = |bytes| {
        if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    let offset = [header[4], header[5], header[6], header[7]];
    let offset = if little {
        u32::from_le_bytes(offset)
    } else {
        u32::from_be_bytes(offset)
    };
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut count = [0; 2];
    file.read_exact(&mut count)
        .context("reading TIFF directory")?;
    for _ in 0..short(count) {
        let mut entry = [0; 12];
        file.read_exact(&mut entry).context("reading TIFF tag")?;
        if short([entry[0], entry[1]]) == 50706 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn decode(path: &Path) -> Result<DynamicImage> {
    let raw = rawler::decode_file(path).context("decoding DNG raw samples")?;
    ensure!(raw.camera.mode == "dng", "input is not a DNG");
    let count = match &raw.data {
        RawImageData::Integer(data) => data.len(),
        RawImageData::Float(data) => data.len(),
    };
    ensure!(
        raw.width > 0
            && raw.height > 0
            && raw
                .width
                .checked_mul(raw.height)
                .and_then(|n| n.checked_mul(raw.cpp))
                == Some(count),
        "invalid DNG dimensions or sample count"
    );
    let area = raw
        .crop_area
        .or(raw.active_area)
        .unwrap_or_else(|| Rect::new(Point::zero(), Dim2::new(raw.width, raw.height)));
    for area in [Some(area), raw.active_area].into_iter().flatten() {
        ensure!(
            area.d.w > 0
                && area.d.h > 0
                && area
                    .p
                    .x
                    .checked_add(area.d.w)
                    .is_some_and(|end| end <= raw.width)
                && area
                    .p
                    .y
                    .checked_add(area.d.h)
                    .is_some_and(|end| end <= raw.height),
            "DNG crop lies outside the raw image"
        );
    }
    let orientation = match raw.orientation {
        rawler::Orientation::Unknown => Orientation::NoTransforms,
        _ => Orientation::from_exif(raw.orientation.to_u16().try_into()?)
            .context("invalid DNG orientation")?,
    };
    let monochrome = raw.cpp == 1
        && matches!(
            raw.photometric,
            RawPhotometricInterpretation::BlackIsZero | RawPhotometricInterpretation::LinearRaw
        );
    let mut image = if monochrome && matches!(&raw.data, RawImageData::Integer(_)) {
        let RawImageData::Integer(data) = raw.data else {
            unreachable!()
        };
        let image = DynamicImage::ImageLuma16(
            ImageBuffer::from_vec(raw.width.try_into()?, raw.height.try_into()?, data)
                .context("invalid monochrome DNG buffer")?,
        );
        if area.p.x == 0 && area.p.y == 0 && area.d.w == raw.width && area.d.h == raw.height {
            image
        } else {
            image.crop_imm(
                area.p.x.try_into()?,
                area.p.y.try_into()?,
                area.d.w.try_into()?,
                area.d.h.try_into()?,
            )
        }
    } else {
        let developed =
            std::panic::catch_unwind(|| RawDevelop::default().develop_intermediate(&raw))
                .map_err(|_| {
                    anyhow::anyhow!("DNG developer rejected malformed or unsupported raw data")
                })?
                .context("developing DNG for color display")?
                .to_dynamic_image()
                .context("invalid developed DNG buffer")?;
        DynamicImage::ImageRgba8(developed.into_rgba8())
    };
    image.apply_orientation(orientation);
    Ok(image)
}
