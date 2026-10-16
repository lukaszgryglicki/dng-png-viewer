use crate::{
    backend,
    images::{self, Source},
    view::{DEFAULT_BRIGHTNESS_STEP, DownsampleFilter},
};
use anyhow::{Result, ensure};
use clap::{ArgGroup, CommandFactory, FromArgMatches, Parser};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Debug, Parser)]
#[command(version, about = "Native fullscreen Gray16 bit-window viewer for macOS, X11 and KMSDRM consoles.",
    group(ArgGroup::new("sources").required(true).multiple(true).args(["images", "directories"])),
    after_help = "Fit mode: LEFT/RIGHT previous/next; UP brighter, DOWN darker; SPACE resets to darkest; Z native 1:1.\n\
        Native mode: arrows pan toward the requested image edge; SPACE centers; X returns to fit.\n\
        ESC, Q or Ctrl+C exits with cleanup in both desktop and console modes.\n\
        DR starts at the full-range window; UP/DOWN adjust by --brightness-step bits.\n\
        FreeBSD console: active physical text VT only, not SSH/tmux; switching VTs exits.\n\
        Single-dash long options work: -dir /photos -preload 5 -brightness-step 0.2 -display-dr.\n\
        Pass directories for recursive discovery; quote globs to expand them here, not in the shell.\n\
        The complete image list has no count limit and is shuffled only once with --shuffle.")]
pub struct Cli {
    #[arg(
        value_name = "INPUT",
        help = "Image file, recursive directory, or quoted glob"
    )]
    pub images: Vec<PathBuf>,
    #[arg(
        long = "dir",
        value_name = "DIRECTORY",
        help = "Recursively add images from a literal directory (also accepted positionally)"
    )]
    pub directories: Vec<PathBuf>,
    #[arg(
        long,
        help = "Shuffle once after expanding and deduplicating all inputs"
    )]
    pub shuffle: bool,
    #[arg(
        long,
        help = "Print image/window information as JSON lines without opening a viewer"
    )]
    pub analyze: bool,
    #[arg(
        long,
        visible_alias = "dr",
        overrides_with_all = ["display_dr", "no_display_dr"],
        help = "Calculate and display noise-limited DR for Gray16 images using dng-monochrome's estimator (default)"
    )]
    pub display_dr: bool,
    #[arg(
        long,
        visible_alias = "no-dr",
        overrides_with_all = ["display_dr", "no_display_dr"],
        help = "Skip noise-limited DR calculation and display"
    )]
    pub no_display_dr: bool,
    #[arg(
        long,
        default_value_t = 5,
        value_name = "N",
        help = "Preload N images on each side in the background; 0 disables preloading"
    )]
    pub preload: usize,
    #[arg(
        long,
        default_value_t = DEFAULT_BRIGHTNESS_STEP,
        value_parser = brightness_step,
        value_name = "BITS",
        help = "Positive brightness-window step in bits (up to 8)"
    )]
    pub brightness_step: f64,
    #[arg(
        long,
        value_enum,
        ignore_case = true,
        default_value = "area",
        value_name = "FILTER",
        help = "Shrinking filter: area/full average, legacy bilinear, or random/middle/corner pixel"
    )]
    pub downsample: DownsampleFilter,
    #[arg(skip)]
    pub sources: Vec<Source>,
}

impl Cli {
    pub fn try_parse_compat(args: impl IntoIterator<Item = OsString>) -> Result<Self, clap::Error> {
        let mut options = true;
        let mut value_next = false;
        let args = args.into_iter().enumerate().map(|(index, arg)| {
            if index == 0 || !options {
                return arg;
            }
            if value_next {
                value_next = false;
                return arg;
            }
            if arg == "--" {
                options = false;
                return arg;
            }
            let bytes = arg.as_encoded_bytes();
            let names = [
                "dir",
                "shuffle",
                "analyze",
                "display-dr",
                "dr",
                "no-display-dr",
                "no-dr",
                "preload",
                "brightness-step",
                "downsample",
                "help",
                "version",
            ];
            for name in names {
                let single = format!("-{name}");
                let double = format!("--{name}");
                let separate = bytes == single.as_bytes() || bytes == double.as_bytes();
                if separate && matches!(name, "dir" | "preload" | "brightness-step" | "downsample")
                {
                    value_next = true;
                }
                if bytes == single.as_bytes() || bytes.starts_with(format!("{single}=").as_bytes())
                {
                    let mut normalized = OsString::from("-");
                    normalized.push(arg);
                    return normalized;
                }
            }
            arg
        });
        let matches = Self::command().try_get_matches_from(args)?;
        let mut cli = Self::from_arg_matches(&matches)?;
        let mut ordered = Vec::new();
        if let Some(indices) = matches.indices_of("images") {
            ordered.extend(
                indices
                    .zip(cli.images.iter().cloned())
                    .map(|(i, path)| (i, Source::File(path))),
            );
        }
        if let Some(indices) = matches.indices_of("directories") {
            ordered.extend(
                indices
                    .zip(cli.directories.iter().cloned())
                    .map(|(i, path)| (i, Source::Directory(path))),
            );
        }
        ordered.sort_by_key(|(index, _)| *index);
        cli.sources = ordered.into_iter().map(|(_, source)| source).collect();
        Ok(cli)
    }

    pub fn playlist(&self) -> Result<Vec<PathBuf>> {
        let mut paths = images::discover(&self.sources)?;
        if self.shuffle {
            fastrand::shuffle(&mut paths);
        }
        Ok(paths)
    }

    pub fn run(self) -> Result<()> {
        let paths = self.playlist()?;
        let display_dr = self.display_dr || !self.no_display_dr;
        if self.analyze {
            let mut errors = 0;
            for path in paths {
                match images::decode_with_dr(&path, display_dr) {
                    Ok(image) => {
                        let mut record = serde_json::json!({"source": path.to_string_lossy(), "image": image.info});
                        if display_dr {
                            record["dynamic_range"] = serde_json::to_value(image.dynamic_range)?;
                        }
                        serde_json::to_writer(io::stdout().lock(), &record)?;
                        writeln!(io::stdout().lock())?;
                    }
                    Err(error) => {
                        errors += 1;
                        eprintln!("{}: {error:#}", path.display());
                    }
                }
            }
            ensure!(errors == 0, "{errors} image(s) could not be inspected");
            return Ok(());
        }
        backend::run_with_options(
            paths,
            self.preload,
            self.brightness_step,
            self.downsample,
            display_dr,
        )
    }
}

fn brightness_step(value: &str) -> Result<f64, String> {
    let step: f64 = value.parse().map_err(|_| "expected a number of bits")?;
    if step.is_finite() && step > 0.0 && step <= 8.0 {
        Ok(step)
    } else {
        Err("brightness step must be finite, greater than 0 and at most 8 bits".into())
    }
}
