#![cfg(unix)]

use std::{
    fs::{self, FileTimes},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::SystemTime,
};
use tempfile::{TempDir, tempdir};

fn make_fixture() -> TempDir {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Makefile"), include_str!("../Makefile")).unwrap();
    fs::create_dir(dir.path().join(".cargo")).unwrap();
    fs::create_dir(dir.path().join("scripts")).unwrap();
    fs::write(
        dir.path().join("scripts/requirements.sh"),
        include_str!("../scripts/requirements.sh"),
    )
    .unwrap();
    for name in ["config.toml", "heif.cmake", "sdl2.cmake"] {
        fs::write(dir.path().join(".cargo").join(name), []).unwrap();
    }
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    for (name, body) in [
        ("uname", "printf '%s\\n' \"$MOCK_OS\"\n"),
        (
            "rustc",
            "printf 'rustc %s (fixture)\\n' \"${MOCK_RUST_VERSION:-1.98.1}\"\n",
        ),
        (
            "cmake",
            "test \"$1\" = --version\n\
             printf 'cmake version %s\\n' \"${MOCK_CMAKE_VERSION:-3.31.0}\"\n",
        ),
        (
            "pkg-config",
            "for package in \"$@\"; do\n\
               if test \"$package\" = \"${MOCK_MISSING_PACKAGE:-}\"; then\n\
                 echo \"Missing package: $package\" >&2; exit 1\n\
               fi\n\
             done\n",
        ),
        ("id", "echo 1000\n"),
        (
            "sudo",
            "printf '%s\\n' \"$*\" >> privilege-calls\nexec \"$@\"\n",
        ),
        ("xcode-select", "exit 0\n"),
        (
            "apt-get",
            "printf '%s\\n' \"$*\" >> package-calls\n\
             exit \"${MOCK_PACKAGE_STATUS:-0}\"\n",
        ),
        (
            "pkg",
            "if test \"$1\" = info; then exit 1; fi\n\
             printf '%s\\n' \"$*\" >> package-calls\n\
             exit \"${MOCK_PACKAGE_STATUS:-0}\"\n",
        ),
        (
            "brew",
            "if test \"$1\" = list; then exit 1; fi\n\
             printf '%s\\n' \"$*\" >> package-calls\n\
             exit \"${MOCK_PACKAGE_STATUS:-0}\"\n",
        ),
        (
            "ldd",
            "echo ldd >> inspections\nprintf '%s\\n' \"$MOCK_LIBRARIES\"\n",
        ),
        (
            "otool",
            "echo otool >> inspections\nprintf '%s\\n' \"$MOCK_LIBRARIES\"\n",
        ),
        (
            "cargo",
            "printf '%s %s\\n' \"${CARGO_TARGET_DIR:-target}\" \"$*\" >> calls\n\
             if test \"$1\" = build; then\n\
               target=\"${CARGO_TARGET_DIR:-target}\"\n\
               mkdir -p \"$target/release\"\n\
               printf '%s executable\\n' \"$target\" > \"$target/release/dng-png-viewer\"\n\
             elif test \"$1\" = clean && test \"$#\" -eq 1; then\n\
               rm -rf target\n\
             fi\n",
        ),
    ] {
        let path = tools.join(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

fn make_command(dir: &Path, os: &str, libraries: &str) -> Command {
    let mut paths = vec![dir.join("tools")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = Command::new("make");
    command
        .args(["CARGO=cargo", "JOBS=2", "INSTALL_DIR=installed scripts"])
        .current_dir(dir)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("HOME", dir)
        .env_remove("CARGO_TARGET_DIR")
        .env("MOCK_OS", os)
        .env("MOCK_LIBRARIES", libraries);
    command
}

#[test]
fn install_reports_missing_dependencies_before_building_or_cleaning_anything() {
    for (variable, value, message) in [
        ("MOCK_MISSING_PACKAGE", "sdl2", "Missing SDL2"),
        ("MOCK_MISSING_PACKAGE", "libde265", "Missing SDL2"),
        ("MOCK_MISSING_PACKAGE", "gbm", "Missing X11/DRM/GBM/EGL"),
        ("MOCK_RUST_VERSION", "1.88.0", "Rust 1.89+"),
        ("MOCK_CMAKE_VERSION", "3.21.0", "CMake 3.22+"),
    ] {
        let dir = make_fixture();
        let result = make_command(dir.path(), "Linux", "")
            .arg("install")
            .env(variable, value)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{variable}: {result:?}");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(message), "{error}");
        assert!(error.contains("make requirements"), "{error}");
        assert!(!dir.path().join("calls").exists());
        assert!(!dir.path().join("package-calls").exists());
        assert!(!dir.path().join("privilege-calls").exists());
    }
}

#[test]
fn requirements_installs_native_dependencies_on_each_supported_platform() {
    for os in ["FreeBSD", "Linux", "Darwin"] {
        let dir = make_fixture();
        let result = make_command(dir.path(), os, "")
            .arg("requirements")
            .output()
            .unwrap();
        assert!(result.status.success(), "{os}: {result:?}");
        let packages = fs::read_to_string(dir.path().join("package-calls")).unwrap();
        for dependency in ["cmake", "sdl2", "x265", "aom", "libde265"] {
            assert!(packages.contains(dependency), "{packages}");
        }
        if os == "Linux" {
            assert!(packages.contains("aom-tools"));
            assert!(packages.contains("libnuma-dev"));
        }
        assert!(!packages.contains("musl"));
        assert_eq!(dir.path().join("privilege-calls").exists(), os != "Darwin");
        assert!(!dir.path().join("calls").exists());
    }
}

#[test]
fn requirements_reports_package_manager_failure_without_building() {
    let dir = make_fixture();
    let result = make_command(dir.path(), "Linux", "")
        .arg("requirements")
        .env("MOCK_PACKAGE_STATUS", "23")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!dir.path().join("calls").exists());
}

#[test]
fn bundled_heif_requires_both_decoders_and_disables_external_decoder_plugins() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("heif.cmake"),
        include_str!("../.cargo/heif.cmake"),
    )
    .unwrap();
    fs::write(
        dir.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.22)\n\
         project(codec_check NONE)\n\
         include(${CMAKE_CURRENT_SOURCE_DIR}/heif.cmake)\n\
         set(CMAKE_MODULE_PATH ${CMAKE_CURRENT_SOURCE_DIR})\n\
         find_package(LIBDE265)\n\
         find_package(AOM)\n\
         if(NOT WITH_LIBDE265 OR NOT WITH_AOM_DECODER OR WITH_LIBDE265_PLUGIN OR WITH_AOM_DECODER_PLUGIN)\n\
           message(FATAL_ERROR \"required built-in decoder configuration missing\")\n\
         endif()\n",
    ).unwrap();
    for package in ["LIBDE265", "AOM"] {
        let optional_config = if package == "AOM" {
            "find_package(AOM QUIET CONFIG PATHS ${CMAKE_CURRENT_LIST_DIR}/missing NO_DEFAULT_PATH)\n"
        } else {
            ""
        };
        fs::write(
            dir.path().join(format!("Find{package}.cmake")),
            format!(
                "{optional_config}include(FindPackageHandleStandardArgs)\n\
                 find_package_handle_standard_args({package} REQUIRED_VARS AVAILABLE_{package})\n\
                 set({package}_DECODER_FOUND ${{{package}_FOUND}})\n"
            ),
        )
        .unwrap();
    }
    for (de265, aom) in [("ON", "ON"), ("OFF", "ON"), ("ON", "OFF")] {
        let result = Command::new("cmake")
            .arg("-S")
            .arg(dir.path())
            .arg("-B")
            .arg(dir.path().join(format!("{de265}-{aom}")))
            .arg(format!("-DAVAILABLE_LIBDE265={de265}"))
            .arg(format!("-DAVAILABLE_AOM={aom}"))
            .output()
            .unwrap();
        assert_eq!(
            result.status.success(),
            de265 == "ON" && aom == "ON",
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn bundled_sdl_config_accepts_legacy_minimum_with_current_cmake() {
    assert!(
        include_str!("../.cargo/config.toml").contains("CMAKE_POLICY_VERSION_MINIMUM = \"3.5\"")
    );
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("sdl2.cmake"),
        include_str!("../.cargo/sdl2.cmake"),
    )
    .unwrap();
    fs::write(
        dir.path().join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 2.8.11)\n\
         project(sdl_check NONE)\n\
         if(SDL_HIDAPI OR SDL_JOYSTICK OR SDL_HAPTIC OR NOT CMAKE_C_STANDARD EQUAL 99)\n\
           message(FATAL_ERROR \"required SDL toolchain configuration missing\")\n\
         endif()\n",
    )
    .unwrap();
    let result = Command::new("cmake")
        .arg("-S")
        .arg(dir.path())
        .arg("-B")
        .arg(dir.path().join("build"))
        .arg(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            dir.path().join("sdl2.cmake").display()
        ))
        .env("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn make_refreshes_native_profiles_once_and_checks_platform_specific_sdl_linkage() {
    for os in ["FreeBSD", "Linux", "Darwin"] {
        let dir = make_fixture();
        let run = |libraries: &str| {
            make_command(dir.path(), os, libraries)
                .arg("static")
                .output()
                .unwrap()
        };
        for _ in 0..2 {
            let result = run("program:\n/usr/lib/libSystem.B.dylib");
            assert!(
                result.status.success(),
                "{os}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.matches(" clean ").count(), 4, "{calls}");
        assert!(calls.contains("target clean -p libheif-sys\n"));
        assert!(calls.contains("target clean --release -p libheif-sys\n"));
        assert!(calls.contains("target/static clean --release -p libheif-sys\n"));
        assert!(calls.contains("target/static clean --release -p sdl2-sys\n"));
        let expected_inspector = if os == "Darwin" { "otool\n" } else { "ldd\n" };
        assert_eq!(
            fs::read_to_string(dir.path().join("inspections")).unwrap(),
            expected_inspector.repeat(2)
        );
        fs::File::open(dir.path().join("target/.heif-config"))
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
        assert!(run("program:\n/usr/lib/libSystem.B.dylib").status.success());
        let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.matches(" clean ").count(), 7);
        fs::File::open(dir.path().join("target/static/.sdl2-config"))
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .unwrap();
        assert!(run("program:\n/usr/lib/libSystem.B.dylib").status.success());
        let calls = fs::read_to_string(dir.path().join("calls")).unwrap();
        assert_eq!(calls.matches(" clean ").count(), 8);
        for dynamic_sdl in [
            "/usr/local/lib/libSDL2-2.0.so.0",
            "/opt/homebrew/lib/libSDL2.dylib",
            "/Library/Frameworks/SDL2.framework/Versions/A/SDL2",
        ] {
            let result = run(dynamic_sdl);
            assert!(!result.status.success());
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("SDL2 was not linked statically")
            );
        }
    }
}

#[test]
fn install_copies_both_profiles_and_clean_preserves_all_three_executables() {
    assert!(include_str!("../Makefile").contains("INSTALL_DIR ?= /data/scripts"));
    for os in ["FreeBSD", "Linux", "Darwin"] {
        for goals in [
            &["install"][..],
            &["release", "static", "install", "clean"][..],
        ] {
            let dir = make_fixture();
            let result = make_command(dir.path(), os, "program:\n/usr/lib/libSystem.B.dylib")
                .args(goals)
                .output()
                .unwrap();
            assert!(result.status.success(), "{os}: {result:?}");
            for (name, expected) in [
                ("dng-png-viewer", "target executable\n"),
                ("dng-png-viewer.static", "target/static executable\n"),
                (
                    "installed scripts/dng-png-viewer",
                    "target/static executable\n",
                ),
            ] {
                let path = dir.path().join(name);
                assert_eq!(fs::read_to_string(&path).unwrap(), expected);
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o755
                );
            }
            assert_eq!(
                fs::read_to_string(dir.path().join("calls"))
                    .unwrap()
                    .lines()
                    .filter(|line| line.contains(" build "))
                    .count(),
                2
            );
            assert_eq!(
                dir.path().join("target").exists(),
                !goals.contains(&"clean")
            );
            assert!(
                !dir.path()
                    .join("installed scripts/dng-png-viewer.static")
                    .exists()
            );
        }
    }
}

#[test]
fn install_reports_destination_errors() {
    let dir = make_fixture();
    let result = make_command(dir.path(), "FreeBSD", "/usr/lib/libc.so")
        .args(["install", "INSTALL_DIR=Makefile"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty());
}
