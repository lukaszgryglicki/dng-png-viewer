use dng_png_viewer::{
    backend,
    images::{Inspection, LoadedImage},
    view::{self, Effect, Key, Mode, Size, View},
};
use image::{DynamicImage, ImageBuffer, Rgba};
use sdl2::keyboard::Keycode;
use std::{ffi::OsStr, path::PathBuf};

fn size(width: u32, height: u32) -> Size {
    Size { width, height }
}

fn gray(width: u32, height: u32, samples: Vec<u16>) -> LoadedImage {
    LoadedImage::new(DynamicImage::ImageLuma16(
        ImageBuffer::from_vec(width, height, samples).unwrap(),
    ))
    .unwrap()
}

#[test]
fn renderer_preserves_all_codes_for_every_native_window() {
    let image = gray(256, 256, (0..=65535).collect());
    for shift in 0..=8 {
        let view = View {
            mode: Mode::Native,
            shift,
            ..View::default()
        };
        let frame = view::render(&image, &view, size(256, 256)).unwrap();
        for (value, pixel) in frame.chunks_exact(3).enumerate() {
            assert_eq!(
                pixel,
                &[(value / 2usize.pow(shift)).min(255) as u8; 3],
                "code {value}, shift {shift}"
            );
        }
    }
}

#[test]
fn default_window_and_fit_keys_follow_the_specification() {
    let image = gray(2, 1, vec![0, 4095]);
    let mut view = View::new(&image.info);
    assert_eq!((view.mode, view.shift), (Mode::Fit, 4));
    for (key, effect) in [
        (Key::Left, Effect::Previous),
        (Key::Right, Effect::Next),
        (Key::Quit, Effect::Quit),
    ] {
        assert_eq!(
            view.handle(key, &image.info, image.size(), size(100, 100)),
            effect
        );
    }
    for _ in 0..20 {
        view.handle(Key::Up, &image.info, image.size(), size(100, 100));
    }
    assert_eq!(view.shift, 0);
    assert_eq!(
        view.handle(Key::Up, &image.info, image.size(), size(100, 100)),
        Effect::None
    );
    for _ in 0..20 {
        view.handle(Key::Down, &image.info, image.size(), size(100, 100));
    }
    assert_eq!(view.shift, 4);
    assert_eq!(
        view.handle(Key::Down, &image.info, image.size(), size(100, 100)),
        Effect::None
    );
}

#[test]
fn native_arrows_pan_toward_the_requested_edge_and_x_preserves_window() {
    let image = gray(1000, 800, vec![4095; 800000]);
    let viewport = size(200, 100);
    let mut view = View::new(&image.info);
    view.handle(Key::Up, &image.info, image.size(), viewport);
    view.handle(Key::Native, &image.info, image.size(), viewport);
    assert_eq!((view.mode, view.shift), (Mode::Native, 3));
    for (key, expected) in [
        (Key::Left, (64, 0)),
        (Key::Up, (64, 64)),
        (Key::Right, (0, 64)),
        (Key::Down, (0, 0)),
    ] {
        assert_eq!(
            view.handle(key, &image.info, image.size(), viewport),
            Effect::Redraw
        );
        assert_eq!((view.pan_x, view.pan_y), expected);
        assert_eq!(view.shift, 3);
    }
    view.handle(Key::Right, &image.info, image.size(), viewport);
    view.handle(Key::Native, &image.info, image.size(), viewport);
    assert_eq!((view.pan_x, view.pan_y), (0, 0));
    view.handle(Key::Right, &image.info, image.size(), viewport);
    view.handle(Key::Fit, &image.info, image.size(), viewport);
    assert_eq!(
        (view.mode, view.shift, view.pan_x, view.pan_y),
        (Mode::Fit, 3, 0, 0)
    );
    assert_eq!(
        view.handle(Key::Right, &image.info, image.size(), viewport),
        Effect::Next
    );
}

#[test]
fn normal_images_ignore_dr_keys_but_still_zoom_and_pan() {
    let image = LoadedImage::new(DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
        200,
        200,
        Rgba([255; 4]),
    )))
    .unwrap();
    let mut view = View::new(&image.info);
    assert!(matches!(image.info, Inspection::Standard { .. }));
    for key in [Key::Up, Key::Down] {
        assert_eq!(
            view.handle(key, &image.info, image.size(), size(10, 10)),
            Effect::None
        );
        assert_eq!(view.shift, 0);
    }
    view.handle(Key::Native, &image.info, image.size(), size(10, 10));
    assert_eq!(
        view.handle(Key::Up, &image.info, image.size(), size(10, 10)),
        Effect::Redraw
    );
    assert_eq!(view.pan_y, 64);
}

#[test]
fn panning_is_clamped_and_smaller_images_stay_centered() {
    for (image, viewport) in [(size(101, 99), size(10, 20)), (size(5, 7), size(10, 20))] {
        let mut view = View {
            mode: Mode::Native,
            pan_x: i64::MAX,
            pan_y: i64::MIN,
            ..View::default()
        };
        view.clamp_pan(Some(image), viewport);
        let layout = view.layout(image, viewport);
        assert_eq!(layout.scale, 1.0);
        if image.width > viewport.width {
            assert_eq!((layout.left, layout.top), (0.0, -79.0));
        } else {
            assert_eq!(
                (layout.left, layout.top, view.pan_x, view.pan_y),
                (2.0, 6.0, 0, 0)
            );
        }
    }
}

#[test]
fn native_one_to_one_is_integer_aligned_even_for_odd_dimensions() {
    for source in 1..=9 {
        let image = gray(
            source,
            source,
            (0..source * source).map(|n| n as u16).collect(),
        );
        for screen in 1..=9 {
            let viewport = size(screen, screen);
            let view = View {
                mode: Mode::Native,
                ..View::default()
            };
            let layout = view.layout(image.size(), viewport);
            assert_eq!(layout.left.fract(), 0.0);
            let frame = view::render(&image, &view, viewport).unwrap();
            for y in 0..screen {
                for x in 0..screen {
                    let (sx, sy) = (
                        i64::from(x) - layout.left as i64,
                        i64::from(y) - layout.top as i64,
                    );
                    let expected =
                        if sx >= 0 && sy >= 0 && sx < i64::from(source) && sy < i64::from(source) {
                            (sy * i64::from(source) + sx) as u8
                        } else {
                            0
                        };
                    let offset = ((y * screen + x) * 3) as usize;
                    assert_eq!(&frame[offset..offset + 3], &[expected; 3]);
                }
            }
        }
    }
}

#[test]
fn fit_preserves_aspect_and_letterboxes() {
    let image = gray(2, 1, vec![255, 255]);
    let view = View::default();
    let layout = view.layout(image.size(), size(4, 4));
    assert_eq!((layout.left, layout.top, layout.scale), (0.0, 1.0, 2.0));
    let frame = view::render(&image, &view, size(4, 4)).unwrap();
    assert_eq!(&frame[..12], &[0; 12]);
    assert_eq!(&frame[12..36], &[255; 24]);
    assert_eq!(&frame[36..], &[0; 12]);
}

#[test]
fn windowing_precedes_bilinear_resampling() {
    let image = gray(2, 2, vec![0, 0, 0, 65535]);
    assert_eq!(
        view::render(&image, &View::default(), size(1, 1)).unwrap(),
        [64; 3]
    );
    let image = gray(2, 2, vec![0, 64, 128, 255]);
    let frame = view::render(&image, &View::default(), size(3, 3)).unwrap();
    assert_eq!(&frame[12..15], &[112; 3]);
}

#[test]
fn alpha_is_composited_before_interpolation_without_color_fringes() {
    let image = LoadedImage::new(DynamicImage::ImageRgba8(
        ImageBuffer::from_vec(2, 1, vec![255, 0, 0, 0, 0, 255, 0, 255]).unwrap(),
    ))
    .unwrap();
    let frame = view::render(&image, &View::default(), size(1, 1)).unwrap();
    assert_eq!(frame, [0, 128, 0]);
    let image = LoadedImage::new(DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
        1,
        1,
        Rgba([255, 128, 64, 128]),
    )))
    .unwrap();
    assert_eq!(
        view::render(&image, &View::default(), size(1, 1)).unwrap(),
        [128, 64, 32]
    );
}

#[test]
fn invalid_viewports_and_window_values_are_errors() {
    let image = gray(1, 1, vec![1]);
    for viewport in [size(0, 1), size(1, 0), size(65536, 65536)] {
        assert!(view::render(&image, &View::default(), viewport).is_err());
    }
    assert!(size(7680, 4320).frame_bytes().is_ok());
    assert!(
        view::render(
            &image,
            &View {
                shift: 9,
                ..View::default()
            },
            size(1, 1)
        )
        .is_err()
    );
}

#[test]
fn hud_handles_small_viewports_long_unicode_labels_and_both_font_sizes() {
    for viewport in [size(1, 1), size(16, 17), size(40, 40), size(1000, 100)] {
        let mut frame = vec![90; viewport.frame_bytes().unwrap()];
        view::draw_hud(
            &mut frame,
            viewport,
            &[
                "very long title with é and 東京",
                "second line",
                "third line",
            ],
        );
        assert_eq!(frame.len(), viewport.frame_bytes().unwrap());
        if viewport.width <= 16 {
            assert!(frame.iter().all(|&value| value == 90));
        } else {
            assert!(frame.contains(&30));
        }
    }
}

#[test]
fn sdl_keys_and_display_selection_are_explicit() {
    for (code, expected) in [
        (Keycode::Left, Key::Left),
        (Keycode::Right, Key::Right),
        (Keycode::Up, Key::Up),
        (Keycode::Down, Key::Down),
        (Keycode::Z, Key::Native),
        (Keycode::X, Key::Fit),
        (Keycode::Escape, Key::Quit),
        (Keycode::Q, Key::Quit),
    ] {
        assert_eq!(backend::key(code), Some(expected));
    }
    assert_eq!(backend::key(Keycode::Space), None);
    use sdl2::keyboard::Mod;
    for modifiers in [Mod::LCTRLMOD, Mod::RCTRLMOD, Mod::LCTRLMOD | Mod::LSHIFTMOD] {
        assert_eq!(backend::key_press(Keycode::C, modifiers), Some(Key::Quit));
    }
    for modifiers in [Mod::NOMOD, Mod::LSHIFTMOD, Mod::LALTMOD] {
        assert_eq!(backend::key_press(Keycode::C, modifiers), None);
    }
    assert_eq!(
        backend::key_press(Keycode::Q, Mod::LSHIFTMOD),
        Some(Key::Quit)
    );
    assert_eq!(
        backend::video_driver(Some(OsStr::new(":1")), Some(OsStr::new("wayland-0"))),
        "x11"
    );
    assert_eq!(
        backend::video_driver(Some(OsStr::new("")), Some(OsStr::new("wayland-0"))),
        "wayland"
    );
    assert_eq!(backend::video_driver(None, None), "KMSDRM");
    assert_eq!(
        backend::video_driver(Some(OsStr::new("")), Some(OsStr::new(""))),
        "KMSDRM"
    );
    assert_eq!(
        backend::selected_driver(Some(OsStr::new("")), Some(OsStr::new(":1")), None).unwrap(),
        "x11"
    );
    assert_eq!(
        backend::selected_driver(Some(OsStr::new("kmsdrm")), Some(OsStr::new(":1")), None).unwrap(),
        "KMSDRM"
    );
    assert_eq!(
        backend::selected_driver(Some(OsStr::new("dummy")), None, None).unwrap(),
        "dummy"
    );
    assert!(backend::selected_driver(Some(OsStr::new("x11,KMSDRM")), None, None).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(backend::selected_driver(Some(OsStr::from_bytes(b"\xff")), None, None).is_err());
    }
}

#[cfg(any(target_os = "freebsd", target_os = "linux"))]
#[test]
fn linked_sdl2_includes_both_desktop_and_console_backends() {
    let drivers: Vec<_> = sdl2::video::drivers().collect();
    assert!(drivers.contains(&"x11"), "SDL2 lacks X11: {drivers:?}");
    assert!(
        drivers.contains(&"KMSDRM"),
        "SDL2 lacks KMSDRM: {drivers:?}"
    );
}

#[test]
fn captions_distinguish_loading_normal_native_and_dr_without_control_characters() {
    let paths = vec![PathBuf::from("somewhere/picture\n.png")];
    let image = gray(1, 1, vec![65535]);
    let mut view = View::new(&image.info);
    let title = backend::title(0, &paths, Some(&image), &view, "");
    assert!(title.contains("1/1 | picture .png | FIT | Gray16 1x1 | 16 code bits | window 8-16"));
    assert!(!title.contains('\n'));
    view.mode = Mode::Native;
    assert!(backend::title(0, &paths, Some(&image), &view, "").contains("| 1:1 |"));
    assert!(backend::title(0, &paths, None, &view, "Loading...").ends_with("Loading..."));
    let normal = LoadedImage::new(DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
        1,
        2,
        Rgba([255; 4]),
    )))
    .unwrap();
    assert!(backend::title(0, &paths, Some(&normal), &view, "").contains("standard 1x2"));
}
