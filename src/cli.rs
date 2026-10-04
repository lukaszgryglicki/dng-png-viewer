use crate::{
    backend,
    images::{self, Source},
};
use anyhow::{Result, ensure};
use clap::{ArgGroup, CommandFactory, FromArgMatches, Parser};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

#[derive(Debug, Parser)]
#[command(version, about = "Native fullscreen Gray16 bit-window viewer for X11 and KMSDRM consoles.",
    group(ArgGroup::new("sources").required(true).multiple(true).args(["images", "directories"])),
    after_help = "Fit mode: LEFT/RIGHT previous/next; UP brighter, DOWN darker; Z native 1:1.\n\
        Native mode: arrows pan; X returns to fit. ESC exits.\n\
        Single-dash long options work: -dir /photos -shuffle.\n\
        Use --dir instead of a shell glob that exceeds the OS argument limit.")]
pub struct Cli {
    #[arg(value_name = "IMAGE")]
    pub images: Vec<PathBuf>,
    #[arg(
        long = "dir",
        value_name = "DIRECTORY",
        help = "Recursively add images; may be repeated and mixed with filenames"
    )]
    pub directories: Vec<PathBuf>,
    #[arg(long, help = "Shuffle the complete deduplicated playlist")]
    pub shuffle: bool,
    #[arg(
        long,
        help = "Print image/window information as JSON lines without opening a viewer"
    )]
    pub analyze: bool,
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
            let names = ["dir", "shuffle", "analyze", "help", "version"];
            for name in names {
                let single = format!("-{name}");
                let double = format!("--{name}");
                let separate = bytes == single.as_bytes() || bytes == double.as_bytes();
                if separate && name == "dir" {
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
        if self.analyze {
            let mut errors = 0;
            for path in paths {
                match images::inspect(&path) {
                    Ok(info) => {
                        let record =
                            serde_json::json!({"source": path.to_string_lossy(), "image": info});
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
        backend::run(paths)
    }
}
