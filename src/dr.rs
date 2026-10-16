use anyhow::{Context, Result, ensure};
use dng_monochrome::{
    noise::DynamicRange,
    range::{self, Histogram, RangeOptions},
    raw::{MonoImage, SensorMetadata},
};
use image::{DynamicImage, ImageBuffer};
use rawler::{decoders::WellKnownIFD, formats::tiff::Value, rawsource::RawSource, tags::DngTag};
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
    let DynamicImage::ImageLuma16(pixels) = image else {
        return Ok((image, None));
    };
    let (width, height) = pixels.dimensions();
    let calibration = calibration.unwrap_or_default();
    let input = MonoImage {
        pixels: pixels.into_raw(),
        metadata: SensorMetadata {
            make: String::new(),
            model: String::new(),
            raw_width: width as usize,
            raw_height: height as usize,
            width: width as usize,
            height: height as usize,
            crop: [0, 0, width as usize, height as usize],
            orientation: 1,
            storage_bits: 16,
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
    let image = ImageBuffer::from_vec(width, height, input.pixels)
        .context("restoring grayscale pixels after DR estimation")?;
    Ok((DynamicImage::ImageLuma16(image), Some(dynamic_range)))
}
