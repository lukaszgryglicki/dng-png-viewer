use dng_png_viewer::images::{self, Inspection};
use image::{DynamicImage, ImageBuffer, metadata::Orientation};
use libheif_rs::{
    Channel, Chroma, ColorSpace, CompressionFormat, EncoderParameterValue, EncoderQuality,
    EncodingOptions, HeifContext, Image, ImageOrientation, LibHeif,
};
use rawler::formats::tiff::{DirectoryWriter, Rational, SRational, TiffWriter, Value};
use std::{fs, path::Path};
use tempfile::tempdir;

struct Dng {
    width: u32,
    height: u32,
    bits: u16,
    cpp: u16,
    cfa: bool,
    orientation: u16,
    crop: Option<[u32; 4]>,
    preview: bool,
    samples: Vec<u16>,
}

impl Dng {
    fn gray(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            bits: 16,
            cpp: 1,
            cfa: false,
            orientation: 1,
            crop: None,
            preview: false,
            samples: (0..width * height).map(|n| n as u16).collect(),
        }
    }

    fn write(&self, path: &Path) {
        let mut bytes = Vec::new();
        for row in self
            .samples
            .chunks_exact(self.width as usize * self.cpp as usize)
        {
            if self.bits == 16 {
                bytes.extend(row.iter().flat_map(|sample| sample.to_ne_bytes()));
            } else {
                let (mut byte, mut used) = (0u8, 0);
                for &sample in row {
                    for bit in (0..self.bits).rev() {
                        byte = (byte << 1) | ((sample >> bit) & 1) as u8;
                        used += 1;
                        if used == 8 {
                            bytes.push(byte);
                            (byte, used) = (0, 0);
                        }
                    }
                }
                if used > 0 {
                    bytes.push(byte << (8 - used));
                }
            }
        }
        let mut writer = TiffWriter::new(fs::File::create(path).unwrap()).unwrap();
        let offset = writer.write_data(&bytes).unwrap();
        let mut raw = DirectoryWriter::new();
        raw.add_tag(254u16, 0u32);
        raw.add_tag(256u16, self.width);
        raw.add_tag(257u16, self.height);
        raw.add_tag(258u16, Value::Short(vec![self.bits; self.cpp as usize]));
        raw.add_tag(259u16, 1u16);
        raw.add_tag(262u16, if self.cfa { 32803u16 } else { 34892u16 });
        raw.add_tag(271u16, "Synthetic");
        raw.add_tag(272u16, "Viewer fixture");
        raw.add_tag(273u16, offset);
        if self.orientation > 0 {
            raw.add_tag(274u16, self.orientation);
        }
        raw.add_tag(277u16, self.cpp);
        raw.add_tag(278u16, self.height);
        raw.add_tag(279u16, bytes.len() as u32);
        raw.add_tag(284u16, 1u16);
        raw.add_tag(50706u16, Value::Byte(vec![1, 4, 0, 0]));
        raw.add_tag(50714u16, Value::Long(vec![1023u32; self.cpp as usize]));
        raw.add_tag(
            50717u16,
            Value::Long(vec![(1u32 << self.bits) - 1; self.cpp as usize]),
        );
        if let Some([x, y, width, height]) = self.crop {
            raw.add_tag(50719u16, [x, y]);
            raw.add_tag(50720u16, [width, height]);
        }
        if self.cfa {
            raw.add_tag(33421u16, [2u16, 2]);
            raw.add_tag(33422u16, Value::Byte(vec![0, 1, 1, 2]));
        }
        if self.cfa || self.cpp > 1 {
            raw.add_tag(
                50721u16,
                Value::SRational(
                    [1, 0, 0, 0, 1, 0, 0, 0, 1]
                        .map(|n| SRational { n, d: 1 })
                        .to_vec(),
                ),
            );
            raw.add_tag(50728u16, Value::Rational(vec![Rational { n: 1, d: 1 }; 3]));
            raw.add_tag(50778u16, 21u16);
        }
        if self.preview {
            let raw_offset = raw.build(&mut writer).unwrap();
            let preview_offset = writer.write_data(&[255, 0, 0]).unwrap();
            let mut preview = DirectoryWriter::new();
            preview.add_tag(254u16, 1u32);
            preview.add_tag(256u16, 1u32);
            preview.add_tag(257u16, 1u32);
            preview.add_tag(258u16, [8u16; 3]);
            preview.add_tag(259u16, 1u16);
            preview.add_tag(262u16, 2u16);
            preview.add_tag(271u16, "Synthetic");
            preview.add_tag(272u16, "Viewer fixture");
            preview.add_tag(273u16, preview_offset);
            preview.add_tag(274u16, self.orientation.max(1));
            preview.add_tag(277u16, 3u16);
            preview.add_tag(278u16, 1u32);
            preview.add_tag(279u16, 3u32);
            preview.add_tag(330u16, raw_offset);
            preview.add_tag(50706u16, Value::Byte(vec![1, 4, 0, 0]));
            writer.build(preview).unwrap();
        } else {
            writer.build(raw).unwrap();
        }
    }
}

fn heif(
    path: &Path,
    bits: u8,
    color: bool,
    alpha: bool,
    format: CompressionFormat,
    orientation: ImageOrientation,
) -> Vec<u16> {
    let (width, height) = (65, 17);
    let maximum = (1u32 << bits) - 1;
    let samples: Vec<u16> = (0..width * height)
        .map(|n| ((n * 53) % (maximum + 1)) as u16)
        .collect();
    let mut image = Image::new(
        width,
        height,
        if color {
            ColorSpace::YCbCr(Chroma::C444)
        } else {
            ColorSpace::Monochrome
        },
    )
    .unwrap();
    let mut channels = vec![Channel::Y];
    if color {
        channels.extend([Channel::Cb, Channel::Cr]);
    }
    if alpha {
        channels.push(Channel::Alpha);
    }
    for channel in channels {
        image.create_plane(channel, width, height, bits).unwrap();
        let planes = image.planes_mut();
        let plane = match channel {
            Channel::Y => planes.y.unwrap(),
            Channel::Cb => planes.cb.unwrap(),
            Channel::Cr => planes.cr.unwrap(),
            Channel::Alpha => planes.a.unwrap(),
            _ => unreachable!(),
        };
        for y in 0..height as usize {
            for x in 0..width as usize {
                let value = match channel {
                    Channel::Y => samples[y * width as usize + x],
                    Channel::Cb => (maximum / 4) as u16,
                    Channel::Cr => (maximum * 3 / 4) as u16,
                    Channel::Alpha => 1u16 << (bits - 1),
                    _ => unreachable!(),
                };
                if bits > 8 {
                    let offset = y * plane.stride + x * 2;
                    plane.data[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
                } else {
                    plane.data[y * plane.stride + x] = value as u8;
                }
            }
        }
    }
    encode_heif(path, &image, format, alpha, orientation);
    samples
}

fn encode_heif(
    path: &Path,
    image: &Image,
    format: CompressionFormat,
    alpha: bool,
    orientation: ImageOrientation,
) {
    let library = LibHeif::new_checked().unwrap();
    let mut encoder = library.encoder_for_format(format).unwrap();
    encoder.set_quality(EncoderQuality::LossLess).unwrap();
    match format {
        CompressionFormat::Hevc => {
            for (name, value) in [("x265:pools", "none"), ("x265:frame-threads", "1")] {
                encoder
                    .set_parameter_value(name, EncoderParameterValue::String(value.to_owned()))
                    .unwrap();
            }
        }
        CompressionFormat::Av1 => {
            encoder
                .set_parameter_value("threads", EncoderParameterValue::Int(1))
                .unwrap();
            encoder
                .set_parameter_value("tune", EncoderParameterValue::String("psnr".into()))
                .unwrap();
        }
        _ => unreachable!(),
    }
    let mut options = EncodingOptions::new().unwrap();
    options.set_save_alpha_channel(alpha);
    options.set_image_orientation(orientation);
    let mut context = HeifContext::new().unwrap();
    context
        .encode_image(image, &mut encoder, Some(options))
        .unwrap();
    context.write_to_file(path.to_str().unwrap()).unwrap();
}

#[test]
fn dng_preserves_all_integer_codes_without_black_subtraction_or_preview_conversion() {
    let directory = tempdir().unwrap();
    for preview in [false, true] {
        let mut source = Dng::gray(256, 256);
        source.preview = preview;
        let path = directory.path().join(format!("gray-{preview}.DNG"));
        source.write(&path);
        let before = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let decoded = images::decode(&path).unwrap();
        assert_eq!(decoded.image.as_luma16().unwrap().as_raw(), &source.samples);
        let Inspection::Dr(info) = decoded.info else {
            panic!("DNG must use raw Gray16");
        };
        assert_eq!(
            (info.minimum, info.maximum, info.occupied_levels),
            (0, 65535, 65536)
        );
        assert_eq!((info.code_bits, info.initial_shift), (16, 8));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        let no_extension = directory.path().join(format!("raw-{preview}"));
        fs::copy(&path, &no_extension).unwrap();
        assert_eq!(
            images::decode(&no_extension)
                .unwrap()
                .image
                .as_luma16()
                .unwrap()
                .as_raw(),
            &source.samples,
        );
    }
}

#[test]
fn packed_dng_depths_crop_and_all_orientations_are_preserved() {
    let directory = tempdir().unwrap();
    for bits in [12, 14, 16] {
        let mut source = Dng::gray(8, 2);
        source.bits = bits;
        source.samples = (0..16)
            .map(|n| (n * ((1u32 << bits) - 1) / 15) as u16)
            .collect();
        source.crop = Some([1, 0, 6, 2]);
        let original = DynamicImage::ImageLuma16(
            ImageBuffer::from_vec(source.width, source.height, source.samples.clone()).unwrap(),
        )
        .crop_imm(1, 0, 6, 2);
        for orientation in 0..=8 {
            source.orientation = orientation;
            let path = directory
                .path()
                .join(format!("raw-{bits}-{orientation}.dng"));
            source.write(&path);
            let mut expected = original.clone();
            expected.apply_orientation(Orientation::from_exif(orientation.max(1) as u8).unwrap());
            let decoded = images::decode(&path).unwrap();
            assert_eq!(
                decoded.image.as_luma16(),
                expected.as_luma16(),
                "{bits}/{orientation}"
            );
        }
    }
}

#[test]
fn color_linear_and_cfa_dngs_are_developed_without_dr_controls() {
    let directory = tempdir().unwrap();
    for cfa in [false, true] {
        let mut source = Dng::gray(24, 16);
        source.cfa = cfa;
        source.cpp = if cfa { 1 } else { 3 };
        source.samples = if cfa {
            (0..source.height)
                .flat_map(|y| {
                    (0..source.width).map(move |x| match (x % 2, y % 2) {
                        (0, 0) => 45000,
                        (1, 1) => 6000,
                        _ => 18000,
                    })
                })
                .collect()
        } else {
            [45000, 18000, 6000].repeat((source.width * source.height) as usize)
        };
        let path = directory.path().join(format!("color-{cfa}.dng"));
        source.write(&path);
        let decoded = images::decode(&path).unwrap();
        assert!(matches!(
            decoded.info,
            Inspection::Standard {
                width: 24,
                height: 16,
                ..
            }
        ));
        let pixel = decoded.image.as_rgba8().unwrap().get_pixel(12, 8).0;
        assert!(pixel[0] > pixel[1] && pixel[0] > pixel[2], "{pixel:?}");

        source.crop = Some([2, 2, 20, 12]);
        source.orientation = 6;
        let cropped = directory.path().join(format!("cropped-color-{cfa}.dng"));
        source.write(&cropped);
        let decoded = images::decode(&cropped).unwrap();
        assert!(matches!(
            decoded.info,
            Inspection::Standard {
                width: 12,
                height: 20,
                ..
            }
        ));
    }
}

#[test]
fn high_bit_depth_monochrome_heic_and_avif_preserve_decoded_codes() {
    let directory = tempdir().unwrap();
    for (extension, format) in [
        ("heic", CompressionFormat::Hevc),
        ("avif", CompressionFormat::Av1),
    ] {
        for bits in [10, 12] {
            let path = directory.path().join(format!("gray-{bits}.{extension}"));
            let expected = heif(&path, bits, false, false, format, ImageOrientation::Normal);
            let decoded = images::decode(&path)
                .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
            assert_eq!(
                decoded.image.as_luma16().unwrap().as_raw(),
                &expected,
                "{extension}/{bits}"
            );
            let Inspection::Dr(info) = decoded.info else {
                panic!();
            };
            assert_eq!(
                (info.width, info.height, info.code_bits),
                (65, 17, u32::from(bits))
            );
            let without_extension = directory.path().join("heif-without-extension");
            fs::copy(&path, &without_extension).unwrap();
            assert_eq!(
                images::decode(&without_extension)
                    .unwrap()
                    .image
                    .as_luma16()
                    .unwrap()
                    .as_raw(),
                &expected
            );
        }
    }
}

#[test]
fn color_and_alpha_heic_including_ten_and_twelve_bits_use_standard_display() {
    let directory = tempdir().unwrap();
    for (bits, color, alpha) in [
        (8, false, false),
        (8, true, false),
        (10, true, false),
        (12, true, false),
        (8, false, true),
        (10, false, true),
        (12, false, true),
        (8, true, true),
    ] {
        let path = directory
            .path()
            .join(format!("normal-{bits}-{color}-{alpha}.heif"));
        heif(
            &path,
            bits,
            color,
            alpha,
            CompressionFormat::Hevc,
            ImageOrientation::Normal,
        );
        let decoded =
            images::decode(&path).unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        assert!(matches!(
            decoded.info,
            Inspection::Standard {
                width: 65,
                height: 17,
                ..
            }
        ));
        let pixels = decoded.image.as_rgba8().unwrap();
        assert!(pixels.pixels().any(|p| p.0[..3].iter().any(|&v| v > 0)));
        if color {
            assert!(pixels.pixels().any(|p| p[0] != p[1] || p[1] != p[2]));
        }
        if alpha {
            assert!((127..=129).contains(&pixels.get_pixel(20, 8)[3]));
        } else {
            assert!(pixels.pixels().all(|p| p[3] == 255));
        }
    }
}

#[test]
fn heic_container_orientation_is_applied_to_high_precision_pixels() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("rotated.heic");
    let samples = heif(
        &path,
        12,
        false,
        false,
        CompressionFormat::Hevc,
        ImageOrientation::Rotate90Cw,
    );
    let expected =
        DynamicImage::ImageLuma16(ImageBuffer::from_vec(65, 17, samples).unwrap()).rotate90();
    let decoded = images::decode(&path).unwrap();
    assert_eq!(decoded.image.as_luma16(), expected.as_luma16());
}

#[test]
fn premultiplied_heic_alpha_is_not_applied_twice() {
    use dng_png_viewer::view::{self, Size, View};

    let directory = tempdir().unwrap();
    let path = directory.path().join("premultiplied.heic");
    let mut source = Image::new(64, 16, ColorSpace::Monochrome).unwrap();
    for channel in [Channel::Y, Channel::Alpha] {
        source.create_plane(channel, 64, 16, 8).unwrap();
        let planes = source.planes_mut();
        let plane = if channel == Channel::Y {
            planes.y.unwrap()
        } else {
            planes.a.unwrap()
        };
        for y in 0..16 {
            for x in 0..64 {
                plane.data[y * plane.stride + x] = if channel == Channel::Y {
                    [0, 32, 64, 200][x % 4]
                } else {
                    [0, 128, 128, 255][x % 4]
                };
            }
        }
    }
    source.set_premultiplied_alpha(true);
    encode_heif(
        &path,
        &source,
        CompressionFormat::Hevc,
        true,
        ImageOrientation::Normal,
    );
    let decoded = images::decode(&path).unwrap();
    assert!(matches!(decoded.info, Inspection::Standard { .. }));
    let pixels = decoded.image.as_rgba8().unwrap();
    for (x, (value, alpha)) in [(0, 0), (64, 128), (128, 128), (200, 255)]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            pixels.get_pixel(x as u32, 0).0,
            [value, value, value, alpha]
        );
    }
    let frame = view::render(
        &decoded,
        &View::new(&decoded.info),
        Size {
            width: 64,
            height: 16,
        },
    )
    .unwrap();
    assert_eq!(
        &frame[..12],
        &[0, 0, 0, 32, 32, 32, 64, 64, 64, 200, 200, 200]
    );
}

#[test]
fn corrupt_raw_and_heif_images_report_errors() {
    let directory = tempdir().unwrap();
    let raw = directory.path().join("truncated.dng");
    Dng::gray(8, 8).write(&raw);
    let bytes = fs::read(&raw).unwrap();
    fs::write(&raw, &bytes[..bytes.len() / 2]).unwrap();
    assert!(images::decode(&raw).is_err());
    let heic = directory.path().join("broken.heic");
    fs::write(&heic, b"\0\0\0\x18ftypheic\0\0\0\0heicmif1").unwrap();
    assert!(images::decode(&heic).is_err());
    let crop = directory.path().join("bad-crop.dng");
    let mut source = Dng::gray(8, 8);
    source.crop = Some([7, 7, 8, 8]);
    source.write(&crop);
    assert!(images::decode(&crop).is_err());
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn new_format_non_utf8_paths() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let directory = tempdir().unwrap();
    let raw = directory
        .path()
        .join(OsString::from_vec(b"raw-\xff.dng".to_vec()));
    Dng::gray(8, 8).write(&raw);
    assert!(matches!(
        images::decode(&raw).unwrap().info,
        Inspection::Dr(_)
    ));
    let heic = directory.path().join("gray.heic");
    heif(
        &heic,
        10,
        false,
        false,
        CompressionFormat::Hevc,
        ImageOrientation::Normal,
    );
    let renamed = directory
        .path()
        .join(OsString::from_vec(b"gray-\xff.heic".to_vec()));
    fs::rename(&heic, &renamed).unwrap();
    assert!(matches!(
        images::decode(&renamed).unwrap().info,
        Inspection::Dr(_)
    ));
}
