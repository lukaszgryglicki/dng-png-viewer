#![cfg(unix)]

use std::{
    fs::{self, FileTimes},
    os::unix::fs::PermissionsExt,
    process::Command,
    time::SystemTime,
};
use tempfile::tempdir;

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
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Makefile"), include_str!("../Makefile")).unwrap();
        fs::create_dir(dir.path().join(".cargo")).unwrap();
        fs::write(dir.path().join(".cargo/config.toml"), []).unwrap();
        fs::write(dir.path().join(".cargo/heif.cmake"), []).unwrap();
        fs::write(dir.path().join(".cargo/sdl2.cmake"), []).unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir(&tools).unwrap();
        for (name, body) in [
            ("uname", "printf '%s\\n' \"$MOCK_OS\"\n"),
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
                         mkdir -p target/static/release\n\
                         : > target/static/release/dng-png-viewer\n\
                       fi\n",
            ),
        ] {
            let path = tools.join(name);
            fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut paths = vec![tools];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let path = std::env::join_paths(paths).unwrap();
        let run = |libraries: &str| {
            Command::new("make")
                .args(["static", "CARGO=cargo", "JOBS=2"])
                .current_dir(dir.path())
                .env("PATH", &path)
                .env_remove("CARGO_TARGET_DIR")
                .env("MOCK_OS", os)
                .env("MOCK_LIBRARIES", libraries)
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
