use clap::error::ErrorKind;
use dng_png_viewer::{cli::Cli, images::Source};
use std::{collections::HashSet, ffi::OsString, fs, path::PathBuf, process::Command};
use tempfile::tempdir;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_compat(
        std::iter::once("viewer")
            .chain(args.iter().copied())
            .map(OsString::from),
    )
}

#[test]
fn every_option_accepts_single_and_double_dash_forms() {
    for prefix in ["-", "--"] {
        let cli = parse(&[
            &format!("{prefix}dir"),
            "one",
            &format!("{prefix}dir=two"),
            &format!("{prefix}shuffle"),
            &format!("{prefix}analyze"),
            "a.png",
        ])
        .unwrap();
        assert!(cli.shuffle && cli.analyze);
        assert_eq!(
            cli.sources,
            vec![
                Source::Directory("one".into()),
                Source::Directory("two".into()),
                Source::File("a.png".into())
            ]
        );
        assert_eq!(
            parse(&[&format!("{prefix}help")]).unwrap_err().kind(),
            ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse(&[&format!("{prefix}version")]).unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }
    assert_eq!(parse(&["-h"]).unwrap_err().kind(), ErrorKind::DisplayHelp);
    assert_eq!(
        parse(&["-V"]).unwrap_err().kind(),
        ErrorKind::DisplayVersion
    );
}

#[test]
fn mixed_sources_keep_their_original_argument_order() {
    let cli = parse(&[
        "first.png",
        "second.png",
        "-dir",
        "one",
        "third.jpg",
        "--dir=two",
        "last.png",
    ])
    .unwrap();
    assert_eq!(
        cli.sources,
        vec![
            Source::File("first.png".into()),
            Source::File("second.png".into()),
            Source::Directory("one".into()),
            Source::File("third.jpg".into()),
            Source::Directory("two".into()),
            Source::File("last.png".into()),
        ]
    );
}

#[test]
fn double_dash_protects_option_like_filenames() {
    let cli = parse(&["--", "-shuffle", "-dir", "--help"]).unwrap();
    assert!(!cli.shuffle);
    assert_eq!(
        cli.images,
        vec![PathBuf::from("-shuffle"), "-dir".into(), "--help".into()]
    );
    let cli = parse(&["-dir=-folder"]).unwrap();
    assert_eq!(cli.directories, vec![PathBuf::from("-folder")]);
}

#[test]
fn invalid_options_missing_sources_and_player_options_fail() {
    for args in [
        vec![],
        vec!["--shuffle"],
        vec!["--analyze"],
        vec!["-dir"],
        vec!["--unknown", "a.png"],
        vec!["--shuffle=true", "a.png"],
        vec!["--mpv", "mpv", "a.png"],
        vec!["--mpv-arg=--vo=gpu", "a.png"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
}

#[test]
fn shuffle_applies_to_all_sources_after_deduplication() {
    let dir = tempdir().unwrap();
    for index in 0..40 {
        fs::write(dir.path().join(format!("{index:02}.png")), []).unwrap();
    }
    let first = dir.path().join("39.png");
    let mut cli = Cli::try_parse_compat([
        OsString::from("viewer"),
        first.into_os_string(),
        OsString::from("-dir"),
        dir.path().into(),
    ])
    .unwrap();
    let ordered = cli.playlist().unwrap();
    cli.shuffle = true;
    fastrand::seed(9123);
    let shuffled = cli.playlist().unwrap();
    fastrand::seed(9123);
    assert_eq!(cli.playlist().unwrap(), shuffled);
    assert_ne!(shuffled, ordered);
    assert_eq!(
        shuffled.iter().collect::<HashSet<_>>(),
        ordered.iter().collect::<HashSet<_>>()
    );
    assert_eq!(shuffled.len(), 40);
}

#[cfg(unix)]
#[test]
fn option_normalization_preserves_non_utf8_directory_bytes() {
    use std::os::unix::ffi::OsStringExt;
    for flag in [b"-dir=".as_slice(), b"--dir=".as_slice()] {
        let mut arg = flag.to_vec();
        arg.extend_from_slice(b"folder-\xff");
        let cli =
            Cli::try_parse_compat([OsString::from("viewer"), OsString::from_vec(arg)]).unwrap();
        assert_eq!(
            cli.directories[0].as_os_str().as_encoded_bytes(),
            b"folder-\xff"
        );
    }
}

#[test]
fn analyze_runs_headlessly_without_a_player_or_a_valid_video_driver() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("gray.png");
    let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 2, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Sixteen);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 0, 15, 255])
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dng-png-viewer"))
        .args(["-analyze", "-dir"])
        .arg(dir.path())
        .env("PATH", "")
        .env("SDL_VIDEODRIVER", "not-a-driver")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["source"],
        path.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(value["image"]["mode"], "dr");
    assert_eq!(value["image"]["maximum"], 4095);
    assert_eq!(value["image"]["max_shift"], 4);
    assert_eq!(value["image"]["initial_shift"], 2);
}

#[test]
fn analyze_reports_bad_images_and_continues_to_valid_ones_but_exits_nonzero() {
    let dir = tempdir().unwrap();
    let bad = dir.path().join("bad.png");
    fs::write(&bad, b"broken").unwrap();
    let good = dir.path().join("good.pgm");
    fs::write(&good, b"P5\n1 1\n255\n\x80").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dng-png-viewer"))
        .arg("--analyze")
        .arg(&bad)
        .arg(&good)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["image"]["mode"], "standard");
    assert!(String::from_utf8_lossy(&output.stderr).contains("bad.png"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 image(s) could not be inspected"));
}

#[test]
fn actual_help_version_and_empty_directory_exit_codes() {
    for option in ["-help", "--help", "-version", "--version"] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_dng-png-viewer"))
                .arg(option)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let dir = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dng-png-viewer"))
        .arg("-dir")
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no images found"));
}
