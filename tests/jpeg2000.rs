use dng_png_viewer::images::{self, Inspection, Source};
use image::DynamicImage;
use openjpeg_sys as jp2;
use std::{ffi::CString, fs, mem::MaybeUninit, path::Path, slice};
use tempfile::tempdir;

struct Fixture {
    width: u32,
    height: u32,
    bits: u32,
    signed: bool,
    color: jp2::COLOR_SPACE,
    alpha: bool,
    planes: Vec<Vec<i32>>,
}

impl Fixture {
    fn gray(bits: u32, signed: bool) -> Self {
        let maximum = (1i32 << bits) - 1;
        let offset = if signed { 1 << (bits - 1) } else { 0 };
        Self {
            width: 5,
            height: 1,
            bits,
            signed,
            color: jp2::COLOR_SPACE::OPJ_CLRSPC_GRAY,
            alpha: false,
            planes: vec![
                [0, 1, maximum / 2, maximum - 1, maximum]
                    .map(|value| value - offset)
                    .to_vec(),
            ],
        }
    }

    fn write(&self, path: &Path, raw: bool) {
        let mut components: Vec<_> = self
            .planes
            .iter()
            .map(|plane| {
                assert_eq!(plane.len(), (self.width * self.height) as usize);
                jp2::opj_image_cmptparm_t {
                    dx: 1,
                    dy: 1,
                    w: self.width,
                    h: self.height,
                    x0: 0,
                    y0: 0,
                    prec: self.bits,
                    bpp: self.bits,
                    sgnd: u32::from(self.signed),
                }
            })
            .collect();
        let path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        unsafe {
            let image =
                jp2::opj_image_create(components.len() as u32, components.as_mut_ptr(), self.color);
            assert!(!image.is_null());
            (*image).x1 = self.width;
            (*image).y1 = self.height;
            for (index, source) in self.planes.iter().enumerate() {
                let component = &mut *(*image).comps.add(index);
                component.alpha = u16::from(self.alpha && index == self.planes.len() - 1);
                slice::from_raw_parts_mut(component.data, source.len()).copy_from_slice(source);
            }
            let codec = jp2::opj_create_compress(if raw {
                jp2::CODEC_FORMAT::OPJ_CODEC_J2K
            } else {
                jp2::CODEC_FORMAT::OPJ_CODEC_JP2
            });
            assert!(!codec.is_null());
            let mut parameters = MaybeUninit::uninit();
            jp2::opj_set_default_encoder_parameters(parameters.as_mut_ptr());
            let mut parameters = parameters.assume_init();
            parameters.cod_format = i32::from(!raw);
            parameters.cp_disto_alloc = 1;
            parameters.tcp_numlayers = 1;
            parameters.tcp_rates[0] = 0.0;
            parameters.numresolution = (self.width.min(self.height).ilog2() + 1) as i32;
            let stream = jp2::opj_stream_create_default_file_stream(path.as_ptr(), 0);
            assert!(!stream.is_null());
            let success = jp2::opj_setup_encoder(codec, &mut parameters, image) != 0
                && jp2::opj_start_compress(codec, image, stream) != 0
                && jp2::opj_encode(codec, stream) != 0
                && jp2::opj_end_compress(codec, stream) != 0;
            jp2::opj_stream_destroy(stream);
            jp2::opj_destroy_codec(codec);
            jp2::opj_image_destroy(image);
            assert!(success, "encoding JPEG2000 fixture");
        }
    }
}

#[test]
fn grayscale_depths_preserve_native_dr_samples_and_normalize_low_bits() {
    let tmp = tempdir().unwrap();
    for signed in [false, true] {
        for bits in 1..=16 {
            let fixture = Fixture::gray(bits, signed);
            let path = tmp.path().join(format!("{bits}-{signed}.jp2"));
            fixture.write(&path, false);
            let loaded = images::decode(&path).unwrap();
            let maximum = (1u32 << bits) - 1;
            let codes = [0, 1, maximum / 2, maximum - 1, maximum];
            if bits > 8 {
                let Inspection::Dr(info) = loaded.info else {
                    panic!("high-bit grayscale must support DR scrolling");
                };
                assert_eq!(info.max_shift, bits - 8);
                assert_eq!(
                    loaded.image.as_luma16().unwrap().as_raw(),
                    &codes.map(|value| value as u16)
                );
            } else {
                assert!(matches!(loaded.info, Inspection::Standard { .. }));
                let expected: Vec<_> = codes
                    .iter()
                    .flat_map(|&code| {
                        let value = ((code * 255 + maximum / 2) / maximum) as u8;
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
    let path = tmp.path().join("all-codes.jp2");
    let mut fixture = Fixture::gray(16, false);
    fixture.width = 256;
    fixture.height = 256;
    fixture.planes = vec![(0..=65535).collect()];
    fixture.write(&path, false);
    let loaded = images::decode(&path).unwrap();
    assert_eq!(
        loaded.image.as_luma16().unwrap().as_raw(),
        &(0..=65535).collect::<Vec<u16>>()
    );
}

#[test]
fn rgb_and_alpha_are_standard_display_with_full_range() {
    let tmp = tempdir().unwrap();
    for bits in [8, 12, 16] {
        for channels in 2..=4 {
            let maximum = (1i32 << bits) - 1;
            let mut fixture = Fixture::gray(bits, false);
            fixture.width = 1;
            fixture.alpha = channels != 3;
            fixture.color = if channels == 2 {
                jp2::COLOR_SPACE::OPJ_CLRSPC_GRAY
            } else {
                jp2::COLOR_SPACE::OPJ_CLRSPC_SRGB
            };
            fixture.planes = if channels == 2 {
                vec![vec![maximum], vec![maximum / 2 + 1]]
            } else {
                let mut planes = vec![vec![maximum], vec![0], vec![maximum / 2 + 1]];
                if channels == 4 {
                    planes.push(vec![maximum / 2 + 1]);
                }
                planes
            };
            let path = tmp.path().join(format!("{bits}-{channels}.jp2"));
            fixture.write(&path, false);
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
fn magic_detection_raw_codestreams_and_directory_discovery() {
    let tmp = tempdir().unwrap();
    let fixture = Fixture::gray(12, false);
    for extension in ["jp2", "JP2", "j2k", "j2c", "jpc", "data"] {
        let path = tmp.path().join(format!("image.{extension}"));
        fixture.write(&path, extension != "jp2" && extension != "JP2");
        assert!(matches!(
            images::decode(&path).unwrap().image,
            DynamicImage::ImageLuma16(_)
        ));
    }
    let paths = images::discover(&[Source::Directory(tmp.path().to_owned())]).unwrap();
    assert_eq!(paths.len(), 5);
    assert!(paths.iter().all(|path| images::supported_extension(path)));
}

#[test]
fn oversized_headers_unsupported_precision_and_corrupt_files_fail() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("image.j2k");
    for dimension in [50000u32, 90000] {
        Fixture::gray(12, false).write(&path, true);
        let mut bytes = fs::read(&path).unwrap();
        for offset in [8, 12, 24, 28] {
            bytes[offset..offset + 4].copy_from_slice(&dimension.to_be_bytes());
        }
        fs::write(&path, bytes).unwrap();
        let error = images::decode(&path).unwrap_err().to_string();
        assert!(
            error.contains("too large") || error.contains("allocation limit"),
            "{error}"
        );
    }
    Fixture::gray(17, false).write(&path, true);
    assert!(
        images::decode(&path)
            .unwrap_err()
            .to_string()
            .contains("precision")
    );
    fs::write(&path, jpeg2k::format::J2K_CODESTREAM_MAGIC).unwrap();
    assert!(images::decode(&path).is_err());
}

#[test]
fn jp2_exif_orientation_rotates_native_samples() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("rotated.jp2");
    let fixture = Fixture::gray(12, false);
    fixture.write(&path, false);
    let exif = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(&(24 + exif.len() as u32).to_be_bytes());
    bytes.extend_from_slice(b"uuidJpgTiffExif->JP2");
    bytes.extend_from_slice(exif);
    fs::write(&path, bytes).unwrap();
    let loaded = images::decode(&path).unwrap();
    assert_eq!((loaded.image.width(), loaded.image.height()), (1, 5));
    assert_eq!(
        loaded.image.as_luma16().unwrap().as_raw(),
        &[0, 1, 2047, 4094, 4095]
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_supported() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let tmp = tempdir().unwrap();
    let path = tmp
        .path()
        .join(OsString::from_vec(b"image-\xff.jp2".to_vec()));
    Fixture::gray(12, false).write(&path, false);
    assert!(matches!(
        images::decode(&path).unwrap().info,
        Inspection::Dr(_)
    ));
}
