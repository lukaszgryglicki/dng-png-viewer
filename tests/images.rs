use dng_png_viewer::images::{self, Inspection, LoadedImage, Source};
use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, Rgb, Rgba};
use std::{collections::HashSet, fs, path::Path};
use tempfile::tempdir;

fn png(
    path: &Path,
    width: u32,
    height: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
) {
    let mut encoder = png::Encoder::new(fs::File::create(path).unwrap(), width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    encoder.set_source_gamma(png::ScaledFloat::new(1.0));
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(data).unwrap();
    writer.finish().unwrap();
}

fn gray(samples: &[u16]) -> LoadedImage {
    LoadedImage::new(DynamicImage::ImageLuma16(
        ImageBuffer::from_vec(samples.len() as u32, 1, samples.to_vec()).unwrap(),
    ))
    .unwrap()
}

#[test]
fn every_code_and_window_is_saturating_not_wrapping() {
    for shift in 0..=8 {
        for sample in 0..=u16::MAX {
            let expected = (u32::from(sample) / 2u32.pow(shift)).min(255) as u8;
            assert_eq!(images::window_sample(sample, shift).unwrap(), expected);
        }
    }
    for shift in [9, 16, 32, u32::MAX] {
        assert!(images::window_sample(65535, shift).is_err());
    }
}

#[test]
fn twelve_bit_example_defaults_to_the_full_range_window() {
    let Inspection::Dr(info) = gray(&[0, 1, 255, 256, 4095]).info else {
        panic!()
    };
    assert_eq!((info.minimum, info.maximum, info.code_bits), (0, 4095, 12));
    assert_eq!(
        (info.max_shift, info.initial_shift, info.occupied_levels),
        (4, 4, 5)
    );
    assert_eq!(info.sample_span_bits, 12.0);
}

#[test]
fn all_depths_default_to_the_window_without_extra_highlight_clipping() {
    for bits in 0..=16u32 {
        let maximum = ((1u32 << bits) - 1) as u16;
        let Inspection::Dr(info) = gray(&[0, maximum]).info else {
            panic!()
        };
        assert_eq!(info.code_bits, bits);
        assert_eq!(info.max_shift, bits.saturating_sub(8));
        assert_eq!(info.initial_shift, info.max_shift);
        assert!(u32::from(maximum) / 2u32.pow(info.initial_shift) <= 255);
    }
}

#[test]
fn constant_samples_and_black_offsets_do_not_invent_physical_dr() {
    for value in [0, 1, 255, 256, 32768, 65535] {
        let Inspection::Dr(info) = gray(&[value; 10]).info else {
            panic!()
        };
        assert_eq!(
            (info.minimum, info.maximum, info.occupied_levels),
            (value, value, 1)
        );
        assert_eq!(info.sample_span_bits, 0.0);
    }
    let Inspection::Dr(info) = gray(&[2048, 4095]).info else {
        panic!()
    };
    assert_eq!((info.code_bits, info.sample_span_bits), (12, 11.0));
    assert_eq!(images::window_sample(2048, 4).unwrap(), 128);
    let Inspection::Dr(info) = gray(&[0, 65535]).info else {
        panic!()
    };
    assert_eq!((info.code_bits, info.occupied_levels), (16, 2));
}

#[test]
fn png_preserves_every_sixteen_bit_code_and_linear_gamma() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("all.PNG");
    let samples: Vec<u16> = (0..=65535).collect();
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
    png(
        &path,
        256,
        256,
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &bytes,
    );
    let before = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let loaded = images::decode(&path).unwrap();
    assert_eq!(loaded.image.as_luma16().unwrap().as_raw(), &samples);
    let Inspection::Dr(info) = &loaded.info else {
        panic!()
    };
    assert_eq!(
        (info.width, info.height, info.occupied_levels),
        (256, 256, 65536)
    );
    assert_eq!((info.max_shift, info.initial_shift), (8, 8));
    assert_eq!(images::inspect(&path).unwrap(), loaded.info);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
}

#[test]
fn parallel_histogram_merges_chunks_exactly() {
    let samples: Vec<u16> = (0..800000).map(|index| (index % 65536) as u16).collect();
    let loaded = LoadedImage::new(DynamicImage::ImageLuma16(
        ImageBuffer::from_vec(1000, 800, samples).unwrap(),
    ))
    .unwrap();
    let Inspection::Dr(info) = loaded.info else {
        panic!()
    };
    assert_eq!(
        (info.minimum, info.maximum, info.occupied_levels),
        (0, 65535, 65536)
    );
}

#[test]
fn ordinary_png_variants_do_not_enable_dr() {
    let dir = tempdir().unwrap();
    for (name, color, depth, data) in [
        (
            "gray8",
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            vec![0, 255],
        ),
        (
            "gray1",
            png::ColorType::Grayscale,
            png::BitDepth::One,
            vec![0b01000000],
        ),
        (
            "rgb8",
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            vec![0, 0, 0, 255, 255, 255],
        ),
        (
            "rgb16",
            png::ColorType::Rgb,
            png::BitDepth::Sixteen,
            vec![0, 1, 0, 1, 0, 1, 255, 255, 255, 255, 255, 255],
        ),
        (
            "rgba8",
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            vec![255, 0, 0, 128, 0, 255, 0, 255],
        ),
        (
            "rgba16",
            png::ColorType::Rgba,
            png::BitDepth::Sixteen,
            vec![255; 16],
        ),
        (
            "grayalpha8",
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            vec![128, 255, 255, 0],
        ),
        (
            "grayalpha16",
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Sixteen,
            vec![0, 1, 255, 255, 255, 255, 0, 0],
        ),
    ] {
        let path = dir.path().join(format!("{name}.png"));
        png(&path, 2, 1, color, depth, &data);
        let loaded = images::decode(&path).unwrap();
        assert!(
            matches!(
                loaded.info,
                Inspection::Standard {
                    width: 2,
                    height: 1,
                    ..
                }
            ),
            "{name}"
        );
        assert!(loaded.image.as_rgba8().is_some(), "{name}");
    }
}

#[test]
fn palette_transparency_and_gray16_transparency_use_normal_display() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("palette.png");
    let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 2, 1);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
    encoder.set_trns(vec![0, 255]);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 1])
        .unwrap();
    let loaded = images::decode(&path).unwrap();
    assert!(matches!(loaded.info, Inspection::Standard { .. }));
    assert_eq!(
        loaded.image.as_rgba8().unwrap().as_raw(),
        &[255, 0, 0, 0, 0, 255, 0, 255]
    );

    let path = dir.path().join("gray-transparent.png");
    let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 2, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Sixteen);
    encoder.set_trns(vec![0, 1]);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 1, 255, 255])
        .unwrap();
    let loaded = images::decode(&path).unwrap();
    assert!(matches!(loaded.info, Inspection::Standard { .. }));
    assert_eq!(loaded.image.as_rgba8().unwrap().get_pixel(0, 0).0[3], 0);
}

#[test]
fn ordinary_codecs_decode_without_external_programs() {
    let dir = tempdir().unwrap();
    for (extension, format) in [
        ("jpg", ImageFormat::Jpeg),
        ("bmp", ImageFormat::Bmp),
        ("gif", ImageFormat::Gif),
        ("tiff", ImageFormat::Tiff),
        ("webp", ImageFormat::WebP),
        ("ppm", ImageFormat::Pnm),
        ("tga", ImageFormat::Tga),
        ("qoi", ImageFormat::Qoi),
        ("ico", ImageFormat::Ico),
        ("ff", ImageFormat::Farbfeld),
        ("hdr", ImageFormat::Hdr),
        ("exr", ImageFormat::OpenExr),
    ] {
        let path = dir.path().join(format!("normal.{extension}"));
        let data = match format {
            ImageFormat::Farbfeld => DynamicImage::ImageRgba16(ImageBuffer::from_pixel(
                4,
                3,
                Rgba([65535, 16384, 32768, 65535]),
            )),
            ImageFormat::Hdr | ImageFormat::OpenExr => {
                DynamicImage::ImageRgb32F(ImageBuffer::from_pixel(4, 3, Rgb([1.0, 0.25, 0.5])))
            }
            ImageFormat::Ico => {
                DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 3, Rgba([255, 64, 128, 255])))
            }
            _ => DynamicImage::ImageRgb8(ImageBuffer::from_pixel(4, 3, Rgb([255, 64, 128]))),
        };
        data.save_with_format(&path, format).unwrap();
        assert!(images::supported_extension(&path));
        let loaded = images::decode(&path).unwrap();
        assert!(
            matches!(
                loaded.info,
                Inspection::Standard {
                    width: 4,
                    height: 3,
                    ..
                }
            ),
            "{extension}"
        );
        assert!(loaded.image.as_rgba8().is_some(), "{extension}");
    }
}

#[test]
fn sixteen_bit_tiff_and_pgm_preserve_native_samples() {
    let dir = tempdir().unwrap();
    let data = gray(&[0, 1, 255, 256, 257, 4095, 65535]);
    for (extension, format) in [("tif", ImageFormat::Tiff), ("pgm", ImageFormat::Pnm)] {
        let path = dir.path().join(format!("gray.{extension}"));
        data.image.save_with_format(&path, format).unwrap();
        let loaded = images::decode(&path).unwrap();
        assert_eq!(loaded.info, data.info);
        assert_eq!(loaded.image.as_luma16(), data.image.as_luma16());
    }
}

#[test]
fn animated_gif_displays_its_first_frame() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("animation.gif");
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(fs::File::create(&path).unwrap());
        encoder
            .encode_frames([
                image::Frame::new(ImageBuffer::from_pixel(3, 2, Rgba([255, 0, 0, 255]))),
                image::Frame::new(ImageBuffer::from_pixel(3, 2, Rgba([0, 255, 0, 255]))),
            ])
            .unwrap();
    }
    let loaded = images::decode(&path).unwrap();
    assert_eq!(
        loaded.image.as_rgba8().unwrap().get_pixel(1, 1).0,
        [255, 0, 0, 255]
    );
}

#[test]
fn jpeg_exif_orientation_is_applied() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("original.jpg");
    let data = ImageBuffer::from_fn(16, 8, |x, y| Rgb([(x * 16) as u8, (y * 32) as u8, 80]));
    let mut encoded = Vec::new();
    jpeg_encoder::Encoder::new(&mut encoded, 100)
        .encode(data.as_raw(), 16, 8, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    fs::write(&path, &encoded).unwrap();
    let original = images::decode(&path).unwrap().image;
    for orientation in 1..=8u8 {
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
        let mut tagged = vec![255, 216, 255, 225];
        tagged.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        tagged.extend(exif);
        tagged.extend_from_slice(&encoded[2..]);
        let path = dir.path().join(format!("oriented-{orientation}.jpg"));
        fs::write(&path, tagged).unwrap();
        let loaded = images::decode(&path).unwrap();
        let expected = match orientation {
            1 => original.clone(),
            2 => original.fliph(),
            3 => original.rotate180(),
            4 => original.flipv(),
            5 => original.rotate90().fliph(),
            6 => original.rotate90(),
            7 => original.rotate90().flipv(),
            8 => original.rotate270(),
            _ => unreachable!(),
        };
        assert_eq!(
            loaded.image.as_rgba8(),
            expected.as_rgba8(),
            "orientation {orientation}"
        );
    }
}

#[test]
fn invalid_empty_and_truncated_images_are_errors() {
    let dir = tempdir().unwrap();
    assert!(images::decode(dir.path()).is_err());
    assert!(images::decode(&dir.path().join("missing.png")).is_err());
    for data in [
        vec![],
        b"not an image".to_vec(),
        b"\x89PNG\r\n\x1a\n".to_vec(),
    ] {
        let path = dir.path().join("bad.png");
        fs::write(&path, data).unwrap();
        assert!(images::decode(&path).is_err());
    }
    assert!(
        LoadedImage::new(DynamicImage::ImageLuma16(ImageBuffer::<Luma<u16>, _>::new(
            0, 0
        )))
        .is_err()
    );
}

#[test]
fn format_is_detected_from_content_for_explicit_files() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("without-extension");
    png(
        &path,
        1,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &[0, 17],
    );
    assert!(!images::supported_extension(&path));
    assert_eq!(
        images::decode(&path)
            .unwrap()
            .image
            .as_luma16()
            .unwrap()
            .as_raw(),
        &[17]
    );
    assert_eq!(
        images::discover(&[Source::File(path.clone())]).unwrap(),
        vec![path.canonicalize().unwrap()]
    );
}

#[test]
fn directories_are_sorted_recursive_case_insensitive_and_deduplicated() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("nested");
    fs::create_dir(&sub).unwrap();
    let first = dir.path().join("z.PNG");
    let second = dir.path().join("a.jpg");
    let third = sub.join("b.TIFF");
    for path in [&first, &second, &third, &dir.path().join("notes.txt")] {
        fs::write(path, []).unwrap();
    }
    let found = images::discover(&[
        Source::File(first.clone()),
        Source::Directory(dir.path().into()),
        Source::File(third.clone()),
        Source::Directory(sub),
    ])
    .unwrap();
    assert_eq!(
        found,
        vec![
            first.canonicalize().unwrap(),
            second.canonicalize().unwrap(),
            third.canonicalize().unwrap()
        ]
    );
}

#[test]
fn directory_errors_and_unsupported_formats_are_explicit() {
    let dir = tempdir().unwrap();
    let text = dir.path().join("notes.txt");
    fs::write(&text, []).unwrap();
    for sources in [
        vec![],
        vec![Source::Directory(dir.path().into())],
        vec![Source::Directory(text)],
        vec![Source::File(dir.path().into())],
        vec![Source::Directory(dir.path().join("missing"))],
    ] {
        assert!(images::discover(&sources).is_err());
    }
    for name in [
        "raw.DNG",
        "photo.heic",
        "photo.avif",
        "photo.jxl",
        "notes.txt",
    ] {
        assert!(!images::supported_extension(Path::new(name)));
    }
}

#[cfg(unix)]
#[test]
fn symlink_files_deduplicate_without_recursing_directory_loops() {
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let image = dir.path().join("one.png");
    fs::write(&image, []).unwrap();
    symlink(&image, dir.path().join("copy.png")).unwrap();
    symlink(dir.path(), dir.path().join("loop.png")).unwrap();
    symlink(dir.path(), dir.path().join("loop")).unwrap();
    assert_eq!(
        images::discover(&[Source::Directory(dir.path().into())]).unwrap(),
        vec![image.canonicalize().unwrap()]
    );
    symlink(dir.path().join("missing"), dir.path().join("broken.png")).unwrap();
    assert!(images::discover(&[Source::Directory(dir.path().into())]).is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_remain_byte_safe() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let dir = tempdir().unwrap();
    let path = dir
        .path()
        .join(OsString::from_vec(b"image-\xff.png".to_vec()));
    png(
        &path,
        1,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &[0, 1],
    );
    let found = images::discover(&[Source::Directory(dir.path().into())]).unwrap();
    assert_eq!(found, vec![path.canonicalize().unwrap()]);
    assert!(matches!(
        images::decode(&found[0]).unwrap().info,
        Inspection::Dr(_)
    ));
}

#[test]
fn a_large_directory_becomes_one_in_memory_playlist() {
    let dir = tempdir().unwrap();
    for index in 0..2048 {
        fs::write(dir.path().join(format!("{index:04}.png")), []).unwrap();
    }
    let found = images::discover(&[Source::Directory(dir.path().into())]).unwrap();
    assert_eq!(found.len(), 2048);
    assert_eq!(found.iter().collect::<HashSet<_>>().len(), 2048);
}
