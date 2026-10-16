use anyhow::{Context, Result, ensure};
use dng_monochrome::{
    noise::DynamicRange,
    range::{self, Histogram, RangeOptions},
    raw::{MonoImage, SensorMetadata},
};
use image::{DynamicImage, ImageBuffer, Pixel, Rgb};
use rawler::{decoders::WellKnownIFD, formats::tiff::Value, rawsource::RawSource, tags::DngTag};
use rayon::prelude::*;
use std::path::Path;

pub(crate) struct Calibration {
    black: f64,
    white: u16,
    noise_profile: Option<[f64; 2]>,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            black: 0.0,
            white: u16::MAX,
            noise_profile: None,
        }
    }
}

impl Calibration {
    pub(crate) fn from_dng(path: &Path, raw: &rawler::RawImage) -> Result<Self> {
        let levels = raw.blacklevel.as_vec();
        let black = f64::from(*levels.first().context("missing black level")?);
        ensure!(
            black.is_finite() && levels.iter().all(|level| f64::from(*level) == black),
            "non-uniform or invalid black levels are not supported for DR estimation"
        );
        let white = *raw.whitelevel.0.first().context("missing white level")?;
        ensure!(
            white <= u32::from(u16::MAX) && black >= 0.0 && black < f64::from(white),
            "invalid black/white calibration: {black}/{white}"
        );
        let noise_profile = std::panic::catch_unwind(|| -> Result<Option<[f64; 2]>> {
            let source = RawSource::new(path).context("opening DNG noise metadata")?;
            let decoder = rawler::get_decoder(&source)?;
            let Some(ifd) = decoder.ifd(WellKnownIFD::Raw)? else {
                return Ok(None);
            };
            let Some(entry) = ifd.get_entry(DngTag::NoiseProfile) else {
                return Ok(None);
            };
            let Value::Double(values) = &entry.value else {
                anyhow::bail!("monochrome NoiseProfile must contain two DOUBLE coefficients");
            };
            ensure!(
                values.len() == 2
                    && values
                        .iter()
                        .all(|value| value.is_finite() && *value >= 0.0),
                "monochrome NoiseProfile must contain two finite, nonnegative coefficients"
            );
            ensure!(
                values.iter().any(|value| *value > 0.0),
                "NoiseProfile cannot have zero total noise"
            );
            let span = f64::from(white) - black;
            ensure!(
                (values[0] * span * 65535.0 + values[1] * span * span).is_finite(),
                "NoiseProfile coefficients overflow the raw-code noise model"
            );
            Ok(Some([values[0], values[1]]))
        })
        .map_err(|_| {
            anyhow::anyhow!("DNG noise metadata decoder panicked on malformed metadata")
        })??;
        Ok(Self {
            black,
            white: white as u16,
            noise_profile,
        })
    }
}

pub(crate) fn estimate(
    image: DynamicImage,
    calibration: Option<Calibration>,
) -> Result<(DynamicImage, Option<DynamicRange>)> {
    let (width, height) = (image.width(), image.height());
    let (pixels, calibration, original) = match image {
        DynamicImage::ImageLuma16(pixels) => {
            (pixels.into_raw(), calibration.unwrap_or_default(), None)
        }
        image => {
            ensure!(
                calibration.is_none(),
                "DNG DR calibration requires native Gray16 samples"
            );
            let pixels = image
                .as_rgba8()
                .context("expected standard RGBA8 display pixels for luminance DR")?
                .as_raw()
                .par_chunks_exact(4)
                .map(|pixel| {
                    let alpha = u16::from(pixel[3]);
                    let rgb = Rgb(std::array::from_fn(|channel| {
                        ((u16::from(pixel[channel]) * alpha + 127) / 255) as u8
                    }));
                    u16::from(rgb.to_luma().0[0])
                })
                .collect();
            (
                pixels,
                Calibration {
                    black: 0.0,
                    white: 255,
                    noise_profile: None,
                },
                Some(image),
            )
        }
    };
    let input = MonoImage {
        pixels,
        metadata: SensorMetadata {
            make: String::new(),
            model: String::new(),
            raw_width: width as usize,
            raw_height: height as usize,
            width: width as usize,
            height: height as usize,
            crop: [0, 0, width as usize, height as usize],
            orientation: 1,
            storage_bits: if original.is_some() { 8 } else { 16 },
            black_level: calibration.black,
            white_level: calibration.white,
            iso: None,
            standard_output_sensitivity: None,
            exposure_seconds: None,
            aperture_f_number: None,
            aperture_value_apex: None,
            focal_length_mm: None,
            lens_model: None,
            noise_profile: calibration.noise_profile,
        },
        source_metadata: Default::default(),
    };
    let histogram = Histogram::new(&input.pixels)?;
    let dynamic_range = range::analyze(&input, &histogram, RangeOptions::default())
        .context("estimating noise-limited DR")?
        .dynamic_range;
    let image = match original {
        Some(image) => image,
        None => DynamicImage::ImageLuma16(
            ImageBuffer::from_vec(width, height, input.pixels)
                .context("restoring grayscale pixels after DR estimation")?,
        ),
    };
    Ok((image, Some(dynamic_range)))
}
