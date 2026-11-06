use exif::{Exif, In, Reader, Tag, Value};
use std::{fmt, fs::File, io::BufReader, path::Path};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Exposure {
    pub iso: Option<u32>,
    pub seconds: Option<f64>,
    pub aperture: Option<f64>,
}

impl Exposure {
    pub(super) fn from_path(path: &Path) -> Self {
        let Ok(file) = File::open(path) else {
            return Self::default();
        };
        Self::from_result(
            Reader::new()
                .continue_on_error(true)
                .read_from_container(&mut BufReader::new(file)),
        )
    }

    pub(super) fn from_exif(bytes: &[u8]) -> Self {
        let bytes = bytes.strip_prefix(b"Exif\0\0").unwrap_or(bytes);
        Self::from_result(
            Reader::new()
                .continue_on_error(true)
                .read_raw(bytes.to_vec()),
        )
    }

    pub(super) fn from_offset_exif(bytes: &[u8]) -> Self {
        let Some(offset) = bytes.get(..4) else {
            return Self::default();
        };
        let offset = u32::from_be_bytes(offset.try_into().unwrap()) as usize;
        bytes[4..]
            .get(offset..)
            .map(Self::from_exif)
            .unwrap_or_default()
    }

    fn from_result(result: Result<Exif, exif::Error>) -> Self {
        let Ok(exif) = result.or_else(|error| error.distill_partial_result(|_| {})) else {
            return Self::default();
        };
        let integer = |tag| {
            exif.get_field(tag, In::PRIMARY)?
                .value
                .get_uint(0)
                .filter(|&value| value > 0)
        };
        let rational = |tag| match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Rational(values) => values
                .first()
                .filter(|value| value.denom != 0)
                .map(|value| value.to_f64()),
            Value::SRational(values) => values
                .first()
                .filter(|value| value.denom != 0)
                .map(|value| value.to_f64()),
            _ => None,
        };
        Self {
            iso: integer(Tag::ISOSpeed)
                .or_else(|| integer(Tag::StandardOutputSensitivity))
                .or_else(|| integer(Tag::RecommendedExposureIndex))
                .or_else(|| integer(Tag::PhotographicSensitivity).filter(|&value| value != 65535)),
            seconds: rational(Tag::ExposureTime).filter(positive).or_else(|| {
                rational(Tag::ShutterSpeedValue)
                    .map(|value| (-value).exp2())
                    .filter(positive)
            }),
            aperture: rational(Tag::FNumber).filter(positive).or_else(|| {
                rational(Tag::ApertureValue)
                    .map(|value| (value / 2.0).exp2())
                    .filter(positive)
            }),
        }
    }
}

fn positive(value: &f64) -> bool {
    value.is_finite() && *value > 0.0
}

fn decimal(value: f64, precision: usize) -> String {
    let text = format!("{value:.precision$}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "0" && value != 0.0 {
        value.to_string()
    } else {
        text.to_owned()
    }
}

impl fmt::Display for Exposure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(iso) = self.iso.filter(|&value| value > 0) {
            parts.push(if iso >= 25000 {
                format!("ISO {}K", decimal(f64::from(iso) / 1000.0, 3))
            } else {
                format!("ISO {iso}")
            });
        }
        if let Some(seconds) = self.seconds.filter(positive) {
            parts.push(if seconds < 1.0 && seconds.recip().is_finite() {
                format!("1/{}s", decimal(seconds.recip(), 3))
            } else {
                format!("{}s", decimal(seconds, 3))
            });
        }
        if let Some(aperture) = self.aperture.filter(positive) {
            parts.push(format!("f{}", decimal(aperture, 2)));
        }
        formatter.write_str(&parts.join(" | "))
    }
}
