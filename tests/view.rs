use clap::ValueEnum;
use dng_png_viewer::{
    backend,
    images::{Inspection, LoadedImage},
    view::{self, DownsampleFilter, Effect, Key, Mode, Size, View},
};
use image::{DynamicImage, ImageBuffer, Rgba};
use sdl2::keyboard::Keycode;
use std::{collections::HashSet, ffi::OsStr, path::PathBuf};

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
            shift: f64::from(shift),
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
    assert_eq!(
        (view.mode, view.shift, view.brightness_step),
        (Mode::Fit, 4.0, 0.2)
    );
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
    assert_eq!(view.shift, 0.0);
    assert_eq!(
        view.handle(Key::Up, &image.info, image.size(), size(100, 100)),
        Effect::None
    );
    for _ in 0..20 {
        view.handle(Key::Down, &image.info, image.size(), size(100, 100));
    }
    assert_eq!(view.shift, 4.0);
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
    assert_eq!((view.mode, view.shift), (Mode::Native, 3.8));
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
        assert_eq!(view.shift, 3.8);
    }
    view.handle(Key::Right, &image.info, image.size(), viewport);
    view.handle(Key::Native, &image.info, image.size(), viewport);
    assert_eq!((view.pan_x, view.pan_y), (0, 0));
    view.handle(Key::Right, &image.info, image.size(), viewport);
    view.handle(Key::Fit, &image.info, image.size(), viewport);
    assert_eq!(
        (view.mode, view.shift, view.pan_x, view.pan_y),
        (Mode::Fit, 3.8, 0, 0)
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
        assert_eq!(view.shift, 0.0);
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
fn area_downsample_averages_every_pixel_in_a_four_by_four_block() {
    for position in 0..16 {
        let mut samples = vec![0; 16];
        samples[position] = 240;
        let image = gray(4, 4, samples);
        assert_eq!(
            view::render(&image, &View::default(), size(1, 1)).unwrap(),
            [15; 3],
            "source pixel {position} must contribute equally"
        );
    }
}

#[test]
fn area_downsample_weights_fractional_footprints_and_clips_image_edges() {
    for (width, height, viewport) in [(5, 1, size(2, 1)), (1, 5, size(1, 2))] {
        let image = gray(width, height, vec![0, 0, 100, 200, 200]);
        assert_eq!(
            view::render(&image, &View::default(), viewport).unwrap(),
            [20, 20, 20, 180, 180, 180]
        );
    }
    let image = gray(5, 5, (0..25).map(|i| (i % 5) * 40 + (i / 5) * 4).collect());
    assert_eq!(
        view::render(&image, &View::default(), size(2, 2)).unwrap(),
        [35, 35, 35, 131, 131, 131, 45, 45, 45, 141, 141, 141]
    );
}

#[test]
fn area_downsample_preserves_letterboxing_without_darkening_image_edges() {
    for (width, height) in [(5, 3), (3, 5)] {
        let image = gray(width, height, vec![77; 15]);
        let frame = view::render(&image, &View::default(), size(3, 3)).unwrap();
        for y in 0..3 {
            for x in 0..3 {
                let inside = if width > height { y == 1 } else { x == 1 };
                let expected = if inside { 77 } else { 0 };
                let offset = (y * 3 + x) * 3;
                assert_eq!(&frame[offset..offset + 3], &[expected; 3]);
            }
        }
    }
}

#[test]
fn area_downsample_averages_premultiplied_color_over_the_whole_footprint() {
    let pixels = ImageBuffer::from_fn(4, 4, |x, y| {
        if (1..=2).contains(&x) && (1..=2).contains(&y) {
            Rgba([255, 0, 0, 0])
        } else {
            Rgba([0, 64, 255, 128])
        }
    });
    let image = LoadedImage::new(DynamicImage::ImageRgba8(pixels)).unwrap();
    assert_eq!(
        view::render(&image, &View::default(), size(1, 1)).unwrap(),
        [0, 24, 96]
    );
    assert_eq!(
        view::render_with_downsample(
            &image,
            &View::default(),
            size(1, 1),
            DownsampleFilter::Bilinear
        )
        .unwrap(),
        [0; 3]
    );
}

#[test]
fn area_downsample_preserves_all_fractional_windows_before_averaging() {
    let samples: Vec<u16> = (0..=65535).collect();
    let image = gray(256, 256, samples.clone());
    for divisions in [4, 5] {
        for tick in 0..=8 * divisions {
            let shift = f64::from(tick) / f64::from(divisions);
            let view = View {
                shift,
                ..View::default()
            };
            let frame = view::render(&image, &view, size(64, 64)).unwrap();
            for y in 0..64 {
                for x in 0..64 {
                    let mut sum = 0u32;
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let sample = samples[(y * 4 + dy) * 256 + x * 4 + dx];
                            sum += (f64::from(sample) / shift.exp2()).floor().min(255.0) as u32;
                        }
                    }
                    let expected = ((sum + 8) / 16) as u8;
                    let offset = (y * 64 + x) * 3;
                    assert_eq!(
                        &frame[offset..offset + 3],
                        &[expected; 3],
                        "pixel ({x}, {y}), shift {shift}"
                    );
                }
            }
        }
    }
    assert_eq!(image.image.as_luma16().unwrap().as_raw(), &samples);
}

#[test]
fn windowing_precedes_spatial_resampling() {
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
fn bilinear_downsample_retains_the_original_four_pixel_interpolation() {
    let image = gray(5, 1, vec![0, 0, 100, 200, 200]);
    assert_eq!(
        view::render_with_downsample(
            &image,
            &View::default(),
            size(2, 1),
            DownsampleFilter::Bilinear
        )
        .unwrap(),
        [0, 0, 0, 200, 200, 200]
    );
    let image = gray(4, 4, (0..16).map(|n| n * 16).collect());
    assert_eq!(
        view::render_with_downsample(
            &image,
            &View::default(),
            size(1, 1),
            DownsampleFilter::Bilinear
        )
        .unwrap(),
        [120; 3]
    );
}

#[test]
fn point_downsample_selects_the_requested_source_pixel_before_windowing() {
    let image = gray(8, 8, (0..64).map(|n| n * 1001).collect());
    for (filter, dx, dy) in [
        (DownsampleFilter::Middle, 2, 2),
        (DownsampleFilter::Ne, 3, 0),
        (DownsampleFilter::Nw, 0, 0),
        (DownsampleFilter::Se, 3, 3),
        (DownsampleFilter::Sw, 0, 3),
    ] {
        for shift in [0.0, 3.8, 7.75, 8.0] {
            let view = View {
                shift,
                ..View::default()
            };
            let frame = view::render_with_downsample(&image, &view, size(2, 2), filter).unwrap();
            for y in 0..2 {
                for x in 0..2 {
                    let sample = ((y * 4 + dy) * 8 + x * 4 + dx) * 1001;
                    let expected = (f64::from(sample) / shift.exp2()).floor().min(255.0) as u8;
                    let offset = ((y * 2 + x) * 3) as usize;
                    assert_eq!(
                        &frame[offset..offset + 3],
                        &[expected; 3],
                        "{filter:?}/{shift}"
                    );
                }
            }
        }
    }
}

#[test]
fn point_downsample_handles_fractional_footprints_and_single_source_axes() {
    for (filter, horizontal, vertical) in [
        (DownsampleFilter::Middle, [40, 160], [40, 160]),
        (DownsampleFilter::Ne, [90, 250], [10, 90]),
        (DownsampleFilter::Nw, [10, 90], [10, 90]),
        (DownsampleFilter::Se, [90, 250], [90, 250]),
        (DownsampleFilter::Sw, [10, 90], [90, 250]),
    ] {
        for (width, height, viewport, expected) in
            [(5, 1, size(2, 1), horizontal), (1, 5, size(1, 2), vertical)]
        {
            let image = gray(width, height, vec![10, 40, 90, 160, 250]);
            assert_eq!(
                view::render_with_downsample(&image, &View::default(), viewport, filter).unwrap(),
                expected
                    .into_iter()
                    .flat_map(|value| [value; 3])
                    .collect::<Vec<_>>(),
                "{filter:?}/{width}x{height}"
            );
        }
    }
}

#[test]
fn random_downsample_is_repeatable_and_preserves_fractional_source_precision() {
    let samples = (0..128 * 128)
        .map(|n| ((n / 128 % 4) * 4 + n % 4) * 4109)
        .collect();
    let image = gray(128, 128, samples);
    let mut view = View::new(&image.info);
    let viewport = size(32, 32);
    let frame =
        view::render_with_downsample(&image, &view, viewport, DownsampleFilter::Random).unwrap();
    assert_eq!(
        frame,
        view::render_with_downsample(&image, &view, viewport, DownsampleFilter::Random).unwrap()
    );
    let observed: HashSet<_> = frame
        .as_chunks::<3>()
        .0
        .iter()
        .map(|pixel| pixel[0])
        .collect();
    assert_eq!(observed, (0..16).map(|n| n * 16).collect());
    view.shift = 7.8;
    let brighter =
        view::render_with_downsample(&image, &view, viewport, DownsampleFilter::Random).unwrap();
    for (before, after) in frame
        .as_chunks::<3>()
        .0
        .iter()
        .zip(brighter.as_chunks::<3>().0)
    {
        let source = u32::from(before[0] / 16) * 4109;
        let expected = (f64::from(source) / view.shift.exp2()).floor().min(255.0) as u8;
        assert_eq!(after, &[expected; 3]);
    }
}

#[test]
fn all_downsample_filters_composite_alpha_over_black() {
    for (alpha, expected) in [(0, [0; 3]), (128, [128, 64, 32]), (255, [255, 128, 64])] {
        let image = LoadedImage::new(DynamicImage::ImageRgba8(ImageBuffer::from_pixel(
            4,
            4,
            Rgba([255, 128, 64, alpha]),
        )))
        .unwrap();
        for &filter in DownsampleFilter::value_variants() {
            assert_eq!(
                view::render_with_downsample(&image, &View::default(), size(1, 1), filter).unwrap(),
                expected,
                "{filter:?}/{alpha}"
            );
        }
    }
}

#[test]
fn downsample_filter_does_not_change_native_pixels_or_enlargement() {
    let gray = gray(5, 3, (0..15).map(|n| n * 4096).collect());
    let color = LoadedImage::new(DynamicImage::ImageRgba8(ImageBuffer::from_fn(
        5,
        3,
        |x, y| Rgba([(x * 50) as u8, (y * 80) as u8, 255, (x * 50) as u8]),
    )))
    .unwrap();
    for image in [&gray, &color] {
        for (mode, viewport) in [
            (Mode::Native, size(2, 2)),
            (Mode::Native, size(8, 8)),
            (Mode::Fit, size(5, 3)),
            (Mode::Fit, size(9, 7)),
        ] {
            let view = View {
                mode,
                shift: 3.8,
                pan_x: 1,
                pan_y: -1,
                ..View::default()
            };
            let expected =
                view::render_with_downsample(image, &view, viewport, DownsampleFilter::Bilinear)
                    .unwrap();
            for &filter in DownsampleFilter::value_variants() {
                assert_eq!(
                    view::render_with_downsample(image, &view, viewport, filter).unwrap(),
                    expected,
                    "{filter:?}/{mode:?}"
                );
            }
        }
    }
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
                shift: 9.0,
                ..View::default()
            },
            size(1, 1)
        )
        .is_err()
    );
}

#[test]
fn default_and_quarter_bit_steps_render_exact_samples_and_clamp_at_both_ends() {
    let image = gray(256, 256, (0..=65535).collect());
    let viewport = image.size();
    for (divisions, step) in [(5, 0.2), (4, 0.25)] {
        let mut view = View {
            brightness_step: step,
            ..View::new(&image.info)
        };
        for key in [Key::Up, Key::Down] {
            for tick in 0..=8 * divisions {
                let tick = if key == Key::Up {
                    8 * divisions - tick
                } else {
                    tick
                };
                let shift = f64::from(tick) / f64::from(divisions);
                assert!((view.shift - shift).abs() < 1e-12, "{step}/{key:?}/{shift}");
                if tick % divisions == 0 {
                    assert_eq!(view.shift, shift);
                }
                view.handle(Key::Native, &image.info, viewport, viewport);
                let frame = view::render(&image, &view, viewport).unwrap();
                for (sample, pixel) in frame.chunks_exact(3).enumerate() {
                    let expected = (sample as f64 / shift.exp2()).floor().min(255.0) as u8;
                    assert_eq!(
                        pixel, &[expected; 3],
                        "step {step}, sample {sample}, shift {shift}"
                    );
                }
                view.handle(Key::Fit, &image.info, viewport, viewport);
                view.handle(key, &image.info, viewport, viewport);
            }
            assert_eq!(
                view.handle(key, &image.info, viewport, viewport),
                Effect::None
            );
        }
    }
}

#[test]
fn roundoff_correction_does_not_discard_tiny_configured_steps() {
    let image = gray(1, 1, vec![65535]);
    for step in [1e-15, 1e-12] {
        let mut view = View {
            brightness_step: step,
            ..View::new(&image.info)
        };
        view.handle(Key::Up, &image.info, image.size(), image.size());
        assert_eq!(view.shift, 8.0 - step);
        assert!(view.shift < 8.0);
        view.shift = 0.0;
        view.handle(Key::Down, &image.info, image.size(), image.size());
        assert_eq!(view.shift, step);
    }
}

#[test]
fn configurable_steps_clip_before_interpolation_and_preserve_zoom_behavior() {
    let image = gray(2, 2, vec![0, 0, 0, 65535]);
    for step in [0.1, 0.2, 0.25, 0.5, 1.0, 3.0, 8.0] {
        let mut view = View {
            brightness_step: step,
            ..View::new(&image.info)
        };
        view.handle(Key::Up, &image.info, image.size(), size(1, 1));
        assert_eq!(view.shift, (8.0 - step).max(0.0));
        assert_eq!(view::render(&image, &view, size(1, 1)).unwrap(), [64; 3]);
        let shift = view.shift;
        view.handle(Key::Native, &image.info, image.size(), size(1, 1));
        view.handle(Key::Up, &image.info, image.size(), size(1, 1));
        view.handle(Key::Fit, &image.info, image.size(), size(1, 1));
        assert_eq!(view.shift, shift);
        for _ in 0..100 {
            view.handle(Key::Up, &image.info, image.size(), size(1, 1));
        }
        assert_eq!(view.shift, 0.0);
        for _ in 0..100 {
            view.handle(Key::Down, &image.info, image.size(), size(1, 1));
        }
        assert_eq!(view.shift, 8.0);
    }
    for shift in [-0.25, f64::NAN, f64::INFINITY] {
        assert!(
            view::render(
                &image,
                &View {
                    shift,
                    ..View::default()
                },
                size(1, 1)
            )
            .is_err()
        );
    }
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
        if cfg!(target_os = "macos") {
            "cocoa"
        } else {
            "x11"
        }
    );
    assert_eq!(
        backend::video_driver(Some(OsStr::new("")), Some(OsStr::new("wayland-0"))),
        if cfg!(target_os = "macos") {
            "cocoa"
        } else {
            "wayland"
        }
    );
    let native_driver = if cfg!(target_os = "macos") {
        "cocoa"
    } else {
        "KMSDRM"
    };
    assert_eq!(backend::video_driver(None, None), native_driver);
    assert_eq!(
        backend::video_driver(Some(OsStr::new("")), Some(OsStr::new(""))),
        native_driver
    );
    assert_eq!(
        backend::selected_driver(Some(OsStr::new("")), Some(OsStr::new(":1")), None).unwrap(),
        if cfg!(target_os = "macos") {
            "cocoa"
        } else {
            "x11"
        }
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

#[cfg(target_os = "macos")]
#[test]
fn linked_sdl2_includes_native_cocoa_backend() {
    let drivers: Vec<_> = sdl2::video::drivers().collect();
    assert!(drivers.contains(&"cocoa"), "SDL2 lacks Cocoa: {drivers:?}");
    assert_eq!(backend::selected_driver(None, None, None).unwrap(), "cocoa");
    assert_eq!(
        backend::selected_driver(Some(OsStr::new("dummy")), None, None).unwrap(),
        "dummy"
    );
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
    view.shift = 7.75;
    assert!(backend::title(0, &paths, Some(&image), &view, "").contains("window 7.75-15.75"));
    view.shift = 0.1 + 0.2;
    assert!(backend::title(0, &paths, Some(&image), &view, "").contains("window 0.3-8.3"));
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
