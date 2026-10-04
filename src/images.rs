use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder, ImageReader};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Directory(PathBuf),
}

pub fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "jpe"
                    | "jfif"
                    | "gif"
                    | "bmp"
                    | "dib"
                    | "webp"
                    | "tif"
                    | "tiff"
                    | "pbm"
                    | "pgm"
                    | "ppm"
                    | "pnm"
                    | "pam"
                    | "exr"
                    | "hdr"
                    | "tga"
                    | "qoi"
                    | "ico"
                    | "ff"
            )
        })
}

pub fn discover(sources: &[Source]) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    let mut append = |path: &Path| -> Result<()> {
        let path = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        ensure!(
            path.is_file(),
            "not a regular image file: {}",
            path.display()
        );
        if seen.insert(path.clone()) {
            images.push(path);
        }
        Ok(())
    };
    for source in sources {
        match source {
            Source::File(path) => append(path)?,
            Source::Directory(root) => {
                let root = root
                    .canonicalize()
                    .with_context(|| format!("resolving directory {}", root.display()))?;
                ensure!(root.is_dir(), "not a directory: {}", root.display());
                let mut found = Vec::new();
                for entry in WalkDir::new(&root).follow_links(false) {
                    let entry = entry.with_context(|| format!("walking {}", root.display()))?;
                    if !supported_extension(entry.path()) {
                        continue;
                    }
                    let regular = if entry.file_type().is_symlink() {
                        fs::metadata(entry.path())
                            .with_context(|| format!("following {}", entry.path().display()))?
                            .is_file()
                    } else {
                        entry.file_type().is_file()
                    };
                    if regular {
                        found.push(entry.into_path());
                    }
                }
                found.sort();
                for path in found {
                    append(&path)?;
                }
            }
        }
    }
    ensure!(
        !images.is_empty(),
        "no images found; pass filenames or --dir DIRECTORY"
    );
    Ok(images)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Inspection {
    Dr(Gray16),
    Standard {
        width: u32,
        height: u32,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Gray16 {
    pub width: u32,
    pub height: u32,
    pub minimum: u16,
    pub maximum: u16,
    pub occupied_levels: u32,
    pub sample_span_bits: f64,
    pub code_bits: u32,
    pub max_shift: u32,
    pub initial_shift: u32,
}

#[derive(Debug)]
pub struct LoadedImage {
    pub image: DynamicImage,
    pub info: Inspection,
}

impl LoadedImage {
    pub fn new(image: DynamicImage) -> Result<Self> {
        let (width, height) = (image.width(), image.height());
        ensure!(width > 0 && height > 0, "cannot view an empty image");
        if let DynamicImage::ImageLuma16(pixels) = &image {
            let seen = pixels
                .as_raw()
                .par_chunks(262144)
                .map(|chunk| {
                    let mut seen = vec![false; 65536];
                    for &sample in chunk {
                        seen[usize::from(sample)] = true;
                    }
                    seen
                })
                .reduce(
                    || vec![false; 65536],
                    |mut left, right| {
                        for (left, right) in left.iter_mut().zip(right) {
                            *left |= right;
                        }
                        left
                    },
                );
            let minimum = seen.iter().position(|&used| used).expect("nonempty image") as u16;
            let maximum = seen.iter().rposition(|&used| used).expect("nonempty image") as u16;
            let code_bits = u16::BITS - maximum.leading_zeros();
            let max_shift = code_bits.saturating_sub(8);
            let info = Inspection::Dr(Gray16 {
                width,
                height,
                minimum,
                maximum,
                occupied_levels: seen.iter().filter(|&&used| used).count() as u32,
                sample_span_bits: f64::from(u32::from(maximum) - u32::from(minimum) + 1).log2(),
                code_bits,
                max_shift,
                initial_shift: max_shift / 2,
            });
            Ok(Self { image, info })
        } else {
            let info = Inspection::Standard {
                width,
                height,
                reason: format!(
                    "{:?}; standard 8-bit display, without DR scrolling",
                    image.color()
                ),
            };
            Ok(Self {
                image: DynamicImage::ImageRgba8(image.into_rgba8()),
                info,
            })
        }
    }

    pub fn size(&self) -> crate::view::Size {
        crate::view::Size {
            width: self.image.width(),
            height: self.image.height(),
        }
    }
}

pub fn decode(path: &Path) -> Result<LoadedImage> {
    ensure!(
        path.is_file(),
        "not a regular image file: {}",
        path.display()
    );
    let reader = ImageReader::open(path)
        .with_context(|| format!("opening {:?}", path))?
        .with_guessed_format()
        .context("detecting image format")?;
    let mut decoder = reader.into_decoder().context("opening image decoder")?;
    let orientation = decoder.orientation().context("reading image orientation")?;
    let mut image = DynamicImage::from_decoder(decoder).context("decoding image pixels")?;
    image.apply_orientation(orientation);
    LoadedImage::new(image)
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    Ok(decode(path)?.info)
}

pub fn window_sample(sample: u16, shift: u32) -> Result<u8> {
    ensure!(
        shift <= 8,
        "an 8-bit window in Gray16 requires a shift from 0 to 8"
    );
    Ok((sample >> shift).min(255) as u8)
}
