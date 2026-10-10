use anyhow::{Context, Result, ensure};
use globset::{Glob, GlobBuilder, GlobSetBuilder};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

mod dng;
mod heif;
mod jpeg2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A positional file, directory, or quoted glob.
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
                    | "dng"
                    | "heic"
                    | "heif"
                    | "hif"
                    | "avif"
                    | "jp2"
                    | "j2k"
                    | "j2c"
                    | "jpc"
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
            Source::File(path) => match fs::symlink_metadata(path) {
                Ok(_) => append_input(path, &mut append)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && has_glob(path) => {
                    for matched in glob_paths(path)? {
                        append_input(&matched, &mut append)?;
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("resolving {}", path.display()));
                }
            },
            Source::Directory(root) => append_directory(root, &mut append)?,
        }
    }
    ensure!(
        !images.is_empty(),
        "no images found; pass files, directories, or quoted globs"
    );
    Ok(images)
}

fn append_input(path: &Path, append: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
    if path.is_dir() {
        append_directory(path, append)
    } else {
        append(path)
    }
}

fn append_directory(root: &Path, append: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
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
    found.sort_unstable();
    for path in found {
        append(&path)?;
    }
    Ok(())
}

fn has_glob(path: &Path) -> bool {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
}

fn compile_glob(path: &Path) -> Result<Glob> {
    let pattern = path.to_str().context("glob pattern must be valid UTF-8")?;
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid glob pattern: {}", path.display()))
}

fn glob_paths(input: &Path) -> Result<Vec<PathBuf>> {
    let mut root = PathBuf::new();
    let mut pattern = PathBuf::new();
    let mut prefixes = GlobSetBuilder::new();
    for component in input.components() {
        if pattern.as_os_str().is_empty() && !has_glob(Path::new(component.as_os_str())) {
            root.push(component);
        } else {
            pattern.push(component);
            prefixes.add(compile_glob(&pattern)?);
        }
    }
    let matcher = compile_glob(&pattern)?.compile_matcher();
    let prefixes = prefixes.build().context("compiling glob prefixes")?;
    let recursive = pattern
        .as_os_str()
        .as_encoded_bytes()
        .windows(2)
        .any(|bytes| bytes == b"**");
    let max_depth = if recursive {
        usize::MAX
    } else {
        pattern.components().count()
    };
    let directories_only = input
        .as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|&byte| std::path::is_separator(char::from(byte)));
    if root.as_os_str().is_empty() {
        root.push(".");
    }
    let root = root
        .canonicalize()
        .with_context(|| format!("resolving glob directory {}", root.display()))?;
    ensure!(root.is_dir(), "not a directory: {}", root.display());
    let mut walker = WalkDir::new(&root)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter();
    let mut found = Vec::new();
    while let Some(entry) = walker.next() {
        let entry = entry.with_context(|| format!("walking glob {}", input.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .context("glob entry outside its root")?;
        if entry.file_type().is_dir() && !prefixes.is_match(relative) {
            walker.skip_current_dir();
            continue;
        }
        if matcher.is_match(relative) {
            if directories_only
                && !fs::metadata(entry.path())
                    .with_context(|| format!("following {}", entry.path().display()))?
                    .is_dir()
            {
                continue;
            }
            found.push(entry.into_path());
        }
    }
    ensure!(
        !found.is_empty(),
        "glob matched no paths: {}",
        input.display()
    );
    found.sort_unstable();
    Ok(found)
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
                initial_shift: max_shift,
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
    if jpeg2000::is_jpeg2000(path)? {
        return LoadedImage::new(jpeg2000::decode(path)?);
    }
    if heif::is_heif(path)? {
        return LoadedImage::new(heif::decode(path)?);
    }
    let reader = ImageReader::open(path)
        .with_context(|| format!("opening {:?}", path))?
        .with_guessed_format()
        .context("detecting image format")?;
    if reader.format() == Some(ImageFormat::Tiff) && dng::is_dng(path)? {
        return LoadedImage::new(dng::decode(path)?);
    }
    let mut decoder = reader.into_decoder().context("opening image decoder")?;
    let orientation = decoder.orientation().context("reading image orientation")?;
    let mut image = DynamicImage::from_decoder(decoder).context("decoding image pixels")?;
    image.apply_orientation(orientation);
    LoadedImage::new(image)
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    Ok(decode(path)?.info)
}

pub(crate) struct GrayWindow {
    scale: f64,
}

impl GrayWindow {
    pub fn new(shift: f64) -> Result<Self> {
        ensure!(
            shift.is_finite() && (0.0..=8.0).contains(&shift),
            "an 8-bit window in Gray16 requires a finite shift from 0 to 8"
        );
        Ok(Self {
            scale: (-shift).exp2(),
        })
    }

    pub fn sample(&self, sample: u16) -> u8 {
        (f64::from(sample) * self.scale).min(255.0) as u8
    }
}

pub fn window_sample(sample: u16, shift: impl Into<f64>) -> Result<u8> {
    Ok(GrayWindow::new(shift.into())?.sample(sample))
}
