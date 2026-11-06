use rawler::formats::tiff::{DirectoryWriter, Rational, TiffWriter};
use std::io::Cursor;

pub fn exposure_exif(iso: u32, exposure: (u32, u32), aperture: (u32, u32)) -> Vec<u8> {
    let mut exif = DirectoryWriter::new();
    exif.add_tag(34855u16, iso.min(u32::from(u16::MAX)) as u16);
    if iso >= u32::from(u16::MAX) {
        exif.add_tag(34867u16, iso);
    }
    exif.add_tag(
        33434u16,
        Rational {
            n: exposure.0,
            d: exposure.1,
        },
    );
    exif.add_tag(
        33437u16,
        Rational {
            n: aperture.0,
            d: aperture.1,
        },
    );
    exif_bytes(exif)
}

pub fn exif_bytes(exif: DirectoryWriter) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    let mut writer = TiffWriter::new(&mut bytes).unwrap();
    let offset = exif.build(&mut writer).unwrap();
    let mut root = DirectoryWriter::new();
    root.add_tag(34665u16, offset);
    writer.build(root).unwrap();
    bytes.into_inner()
}
