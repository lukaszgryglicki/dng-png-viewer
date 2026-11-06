use dng_png_viewer::images::{self, Inspection, Source};
use jxl_sys as jxl;
use std::{fs, mem::MaybeUninit, ptr};
use tempfile::tempdir;

mod common;

#[cfg(target_os = "freebsd")]
#[link(name = "c++")]
unsafe extern "C" {}

struct Fixture {
    width: u32,
    height: u32,
    bits: u32,
    channels: u32,
    pixels: Vec<u16>,
    orientation: jxl::JxlOrientation,
    associated_alpha: bool,
    animation: bool,
    floating: bool,
    lossless: bool,
}

impl Fixture {
    fn gray(bits: u32) -> Self {
        let maximum = (1u32 << bits) - 1;
        Self {
            width: 5,
            height: 1,
            bits,
            channels: 1,
            pixels: [0, 1, maximum / 2, maximum - 1, maximum]
                .map(|value| value as u16)
                .to_vec(),
            orientation: jxl::JxlOrientation::JXL_ORIENT_IDENTITY,
            associated_alpha: false,
            animation: false,
            floating: false,
            lossless: true,
        }
    }

    fn encode(&self, container: bool) -> Vec<u8> {
        self.encode_with_exif(container, None, false)
    }

    fn encode_with_exif(&self, container: bool, exif: Option<&[u8]>, compressed: bool) -> Vec<u8> {
        assert_eq!(
            self.pixels.len(),
            (self.width * self.height * self.channels) as usize
        );
        unsafe {
            let encoder = jxl::JxlEncoderCreate(ptr::null());
            assert!(!encoder.is_null());
            let check = |status| {
                assert_eq!(
                    status,
                    jxl::JxlEncoderStatus::JXL_ENC_SUCCESS,
                    "{:?}",
                    jxl::JxlEncoderGetError(encoder)
                );
            };
            check(jxl::JxlEncoderUseContainer(encoder, i32::from(container)));
            if exif.is_some() {
                check(jxl::JxlEncoderUseBoxes(encoder));
            }
            let mut info = MaybeUninit::uninit();
            jxl::JxlEncoderInitBasicInfo(info.as_mut_ptr());
            let mut info = info.assume_init();
            info.xsize = self.width;
            info.ysize = self.height;
            info.bits_per_sample = if self.floating { 32 } else { self.bits };
            info.exponent_bits_per_sample = if self.floating { 8 } else { 0 };
            info.num_color_channels = if self.channels <= 2 { 1 } else { 3 };
            info.num_extra_channels = self.channels - info.num_color_channels;
            if info.num_extra_channels != 0 {
                info.alpha_bits = info.bits_per_sample;
                info.alpha_exponent_bits = info.exponent_bits_per_sample;
                info.alpha_premultiplied = i32::from(self.associated_alpha);
            }
            info.uses_original_profile = i32::from(self.lossless);
            info.orientation = self.orientation;
            info.have_animation = i32::from(self.animation);
            check(jxl::JxlEncoderSetBasicInfo(encoder, &info));
            let mut color = MaybeUninit::uninit();
            jxl::JxlColorEncodingSetToSRGB(
                color.as_mut_ptr(),
                i32::from(info.num_color_channels == 1),
            );
            check(jxl::JxlEncoderSetColorEncoding(encoder, color.as_ptr()));
            let settings = jxl::JxlEncoderFrameSettingsCreate(encoder, ptr::null());
            assert!(!settings.is_null());
            check(jxl::JxlEncoderFrameSettingsSetOption(
                settings,
                jxl::JxlEncoderFrameSettingId::JXL_ENC_FRAME_SETTING_EFFORT,
                1,
            ));
            check(jxl::JxlEncoderSetFrameLossless(
                settings,
                i32::from(self.lossless),
            ));
            if !self.lossless {
                check(jxl::JxlEncoderSetFrameDistance(settings, 1.0));
            }
            if !self.floating {
                check(jxl::JxlEncoderSetFrameBitDepth(
                    settings,
                    &jxl::JxlBitDepth {
                        type_: jxl::JxlBitDepthType::JXL_BIT_DEPTH_FROM_CODESTREAM,
                        ..Default::default()
                    },
                ));
            }
            if self.animation {
                let mut header = MaybeUninit::uninit();
                jxl::JxlEncoderInitFrameHeader(header.as_mut_ptr());
                let mut header = header.assume_init();
                header.duration = 100;
                check(jxl::JxlEncoderSetFrameHeader(settings, &header));
            }
            let format = jxl::JxlPixelFormat {
                num_channels: self.channels,
                data_type: if self.floating {
                    jxl::JxlDataType::JXL_TYPE_FLOAT
                } else if self.bits <= 8 {
                    jxl::JxlDataType::JXL_TYPE_UINT8
                } else {
                    jxl::JxlDataType::JXL_TYPE_UINT16
                },
                endianness: jxl::JxlEndianness::JXL_NATIVE_ENDIAN,
                align: 0,
            };
            for frame in 0..=usize::from(self.animation) {
                let mut pixels = self.pixels.clone();
                if frame != 0 {
                    pixels.reverse();
                }
                let floats: Vec<_> = if self.floating {
                    pixels.iter().map(|&v| f32::from(v) / 65535.0).collect()
                } else {
                    Vec::new()
                };
                let bytes: Vec<_> = if self.bits <= 8 {
                    pixels.iter().map(|&v| v as u8).collect()
                } else {
                    Vec::new()
                };
                let (pointer, size) = if self.floating {
                    (
                        floats.as_ptr().cast(),
                        std::mem::size_of_val(floats.as_slice()),
                    )
                } else if self.bits <= 8 {
                    (bytes.as_ptr().cast(), bytes.len())
                } else {
                    (
                        pixels.as_ptr().cast(),
                        std::mem::size_of_val(pixels.as_slice()),
                    )
                };
                check(jxl::JxlEncoderAddImageFrame(
                    settings, &format, pointer, size,
                ));
            }
            if let Some(exif) = exif {
                check(jxl::JxlEncoderAddBox(
                    encoder,
                    b"Exif".as_ptr().cast(),
                    exif.as_ptr(),
                    exif.len(),
                    i32::from(compressed),
                ));
            }
            jxl::JxlEncoderCloseInput(encoder);
            let mut output = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let mut next = buffer.as_mut_ptr();
                let mut available = buffer.len();
                let status = jxl::JxlEncoderProcessOutput(encoder, &mut next, &mut available);
                output.extend_from_slice(&buffer[..buffer.len() - available]);
                if status == jxl::JxlEncoderStatus::JXL_ENC_SUCCESS {
                    break;
                }
                assert_eq!(status, jxl::JxlEncoderStatus::JXL_ENC_NEED_MORE_OUTPUT);
                assert!(available < buffer.len());
            }
            jxl::JxlEncoderDestroy(encoder);
            output
        }
    }
}

#[test]
fn jpegxl_exposure_reads_trailing_and_compressed_exif_without_changing_the_first_frame() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("exposure.jxl");
    let tiff = common::exposure_exif(50000, (23, 10), (22, 1));
    let mut exif = 8192u32.to_be_bytes().to_vec();
    exif.resize(8196, 0);
    exif.extend_from_slice(&tiff);
    for compressed in [false, true] {
        for animation in [false, true] {
            let mut fixture = Fixture::gray(12);
            fixture.animation = animation;
            fs::write(
                &path,
                fixture.encode_with_exif(true, Some(&exif), compressed),
            )
            .unwrap();
            let loaded = images::decode(&path).unwrap();
            assert_eq!(loaded.exposure.to_string(), "ISO 50K | 2.3s | f22");
            assert_eq!(loaded.image.as_luma16().unwrap().as_raw(), &fixture.pixels);
        }
    }
}

#[test]
fn grayscale_depths_preserve_native_dr_samples_and_normalize_low_bits() {
    let tmp = tempdir().unwrap();
    for container in [false, true] {
        for bits in 1..=16 {
            let fixture = Fixture::gray(bits);
            let path = tmp.path().join(format!("{bits}-{container}.jxl"));
            fs::write(&path, fixture.encode(container)).unwrap();
            let loaded = images::decode(&path).unwrap();
            if bits > 8 {
                let Inspection::Dr(info) = loaded.info else {
                    panic!("high-bit grayscale must support DR scrolling");
                };
                assert_eq!(info.max_shift, bits - 8);
                assert_eq!(loaded.image.as_luma16().unwrap().as_raw(), &fixture.pixels);
            } else {
                assert!(matches!(loaded.info, Inspection::Standard { .. }));
                let maximum = (1u32 << bits) - 1;
                let expected: Vec<_> = fixture
                    .pixels
                    .iter()
                    .flat_map(|&code| {
                        let value = ((u32::from(code) * 255 + maximum / 2) / maximum) as u8;
                        [value, value, value, 255]
                    })
                    .collect();
                assert_eq!(loaded.image.as_rgba8().unwrap().as_raw(), &expected);
            }
        }
    }
}

#[test]
fn all_65536_native_codes_survive_without_rescaling() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("all-codes.jxl");
    let mut fixture = Fixture::gray(16);
    fixture.width = 256;
    fixture.height = 256;
    fixture.pixels = (0..=u16::MAX).collect();
    fs::write(&path, fixture.encode(true)).unwrap();
    let loaded = images::decode(&path).unwrap();
    assert_eq!(loaded.image.as_luma16().unwrap().as_raw(), &fixture.pixels);
}

#[test]
#[ignore = "60-megapixel allocation-limit regression"]
fn large_sixteen_bit_grayscale_decodes_with_default_limits() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("large-gray16.jxl");
    let mut fixture = Fixture::gray(16);
    fixture.width = 9528;
    fixture.height = 6328;
    let mut state = 0x12345678u32;
    fixture.pixels = (0..fixture.width * fixture.height)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((state >> 18) << 2) as u16
        })
        .collect();
    fixture.pixels[0] = 1;
    *fixture.pixels.last_mut().unwrap() = u16::MAX;
    for lossless in [true, false] {
        fixture.lossless = lossless;
        fs::write(&path, fixture.encode(true)).unwrap();
        let loaded = images::decode(&path).unwrap_or_else(|error| {
            panic!(
                "decoding {}-byte large fixture: {error:#}",
                fs::metadata(&path).unwrap().len()
            )
        });
        let pixels = loaded.image.as_luma16().unwrap();
        assert_eq!(pixels.dimensions(), (fixture.width, fixture.height));
        if lossless {
            assert!(pixels.as_raw() == &fixture.pixels);
        }
    }
}

#[test]
fn rgb_and_alpha_are_standard_display_with_full_range() {
    let tmp = tempdir().unwrap();
    for bits in [8, 12, 16] {
        for channels in 2..=4 {
            let maximum = ((1u32 << bits) - 1) as u16;
            let mut fixture = Fixture::gray(bits);
            fixture.width = 1;
            fixture.channels = channels;
            fixture.pixels = match channels {
                2 => vec![maximum, maximum / 2 + 1],
                3 => vec![maximum, 0, maximum / 2 + 1],
                _ => vec![maximum, 0, maximum / 2 + 1, maximum / 2 + 1],
            };
            let path = tmp.path().join(format!("{bits}-{channels}.jxl"));
            fs::write(&path, fixture.encode(true)).unwrap();
            let loaded = images::decode(&path).unwrap();
            assert!(matches!(loaded.info, Inspection::Standard { .. }));
            let expected = match channels {
                2 => [255, 255, 255, 128],
                3 => [255, 0, 128, 255],
                _ => [255, 0, 128, 128],
            };
            assert_eq!(loaded.image.as_rgba8().unwrap().as_raw(), &expected);
        }
    }
}

#[test]
fn associated_alpha_is_unpremultiplied_before_display_quantization() {
    let tmp = tempdir().unwrap();
    for channels in [2, 4] {
        let mut fixture = Fixture::gray(16);
        fixture.width = 2;
        fixture.channels = channels;
        fixture.associated_alpha = true;
        fixture.pixels = if channels == 2 {
            vec![0, 0, 16384, 32768]
        } else {
            vec![0, 0, 0, 0, 32768, 0, 16384, 32768]
        };
        let path = tmp.path().join(format!("associated-{channels}.jxl"));
        fs::write(&path, fixture.encode(true)).unwrap();
        let loaded = images::decode(&path).unwrap();
        assert!(matches!(loaded.info, Inspection::Standard { .. }));
        let expected = if channels == 2 {
            [0, 0, 0, 0, 128, 128, 128, 128]
        } else {
            [0, 0, 0, 0, 255, 0, 128, 128]
        };
        assert_eq!(loaded.image.as_rgba8().unwrap().as_raw(), &expected);
    }
}

#[test]
fn float_grayscale_is_standard_and_lossy_integer_grayscale_keeps_dr() {
    let tmp = tempdir().unwrap();
    let mut fixture = Fixture::gray(16);
    fixture.floating = true;
    let path = tmp.path().join("float.jxl");
    fs::write(&path, fixture.encode(true)).unwrap();
    let loaded = images::decode(&path).unwrap();
    assert!(matches!(loaded.info, Inspection::Standard { .. }));
    assert_eq!(loaded.image.as_rgba8().unwrap().get_pixel(4, 0).0, [255; 4]);

    let mut fixture = Fixture::gray(12);
    fixture.lossless = false;
    let path = tmp.path().join("lossy.jxl");
    fs::write(&path, fixture.encode(true)).unwrap();
    let loaded = images::decode(&path).unwrap();
    assert!(matches!(loaded.info, Inspection::Dr(_)));
    assert!(
        loaded
            .image
            .as_luma16()
            .unwrap()
            .iter()
            .all(|&code| code <= 4095)
    );
}

#[test]
fn codestream_orientation_and_first_animation_frame_are_applied_once() {
    let tmp = tempdir().unwrap();
    for animation in [false, true] {
        let mut fixture = Fixture::gray(16);
        fixture.width = 2;
        fixture.height = 3;
        fixture.pixels = vec![0, 1000, 2000, 3000, 4000, 5000];
        fixture.orientation = jxl::JxlOrientation::JXL_ORIENT_ROTATE_90_CW;
        fixture.animation = animation;
        let path = tmp.path().join(format!("oriented-{animation}.jxl"));
        fs::write(&path, fixture.encode(true)).unwrap();
        let loaded = images::decode(&path).unwrap();
        assert_eq!((loaded.image.width(), loaded.image.height()), (3, 2));
        assert_eq!(
            loaded.image.as_luma16().unwrap().as_raw(),
            &[4000, 2000, 0, 5000, 3000, 1000]
        );
    }
}

#[test]
fn discovery_and_content_detection_accept_jxl_with_or_without_its_extension() {
    let tmp = tempdir().unwrap();
    let bytes = Fixture::gray(12).encode(true);
    for name in ["photo.JXL", "wrong.png", "extensionless"] {
        let path = tmp.path().join(name);
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            images::decode(&path).unwrap().info,
            Inspection::Dr(_)
        ));
    }
    let discovered = images::discover(&[Source::Directory(tmp.path().to_owned())]).unwrap();
    assert_eq!(discovered.len(), 2);
    assert!(discovered.contains(&tmp.path().join("photo.JXL").canonicalize().unwrap()));
    let glob = images::discover(&[Source::File(tmp.path().join("*.JXL"))]).unwrap();
    assert_eq!(glob, [tmp.path().join("photo.JXL").canonicalize().unwrap()]);
}

#[test]
fn truncated_images_and_oversized_headers_are_rejected() {
    let tmp = tempdir().unwrap();
    let bytes = Fixture::gray(12).encode(true);
    let path = tmp.path().join("broken.jxl");
    for length in [0, 1, 12, bytes.len() / 2, bytes.len() - 1] {
        fs::write(&path, &bytes[..length]).unwrap();
        assert!(images::decode(&path).is_err(), "accepted {length} bytes");
    }
    for (dimension, message) in [(10000, "allocation limit"), (65536, "too large")] {
        let mut bytes = vec![0xff, 0x0a];
        let mut position = 16;
        // Full-size square header, default metadata and default opsin matrix.
        for (value, count) in [
            (0u32, 1),
            (3, 2),
            (dimension - 1, 30),
            (1, 3),
            (1, 1),
            (1, 1),
        ] {
            for bit in 0..count {
                if position / 8 == bytes.len() {
                    bytes.push(0);
                }
                bytes[position / 8] |= (((value >> bit) & 1) as u8) << (position % 8);
                position += 1;
            }
        }
        fs::write(&path, bytes).unwrap();
        let error = images::decode(&path).expect_err("oversized image was accepted");
        assert!(format!("{error:#}").contains(message), "{error:#}");
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_paths_are_supported() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let tmp = tempdir().unwrap();
    let path = tmp
        .path()
        .join(OsString::from_vec(b"photo-\xff.jxl".to_vec()));
    fs::write(&path, Fixture::gray(12).encode(true)).unwrap();
    assert!(matches!(
        images::decode(&path).unwrap().info,
        Inspection::Dr(_)
    ));
}
