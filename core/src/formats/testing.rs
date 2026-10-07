//! Files and structures built for tests: JPEG data, EXIF data with one
//! tag, a thumbnail or Panasonic maker notes, and a Multi-Picture Format
//! index.

/// A tiny but complete JPEG, as the image crate writes it.
pub(crate) fn encode(width: u32, height: u32, exif: Option<Vec<u8>>) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    let mut data = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut data, 90);
    if let Some(exif) = exif {
        encoder.set_exif_metadata(exif).unwrap();
    }
    let pixels = vec![128; (3 * width * height) as usize];
    encoder
        .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
        .unwrap();
    data
}

/// EXIF data holding only an Orientation tag (big-endian TIFF layout).
pub(crate) fn exif_orientation(value: u8) -> Vec<u8> {
    let mut exif = b"MM\0\x2a\0\0\0\x08".to_vec(); // header, first IFD at 8
    exif.extend([0, 1]); // one entry
    exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, value, 0, 0]); // Orientation, SHORT
    exif.extend([0, 0, 0, 0]); // no next IFD
    exif
}

/// Inserts an MPF index right after the SOI of `main` and appends
/// `preview`, as cameras do. `shift` moves the preview's offset.
pub(crate) fn with_mpf_preview(main: &[u8], preview: &[u8], shift: i64) -> Vec<u8> {
    // Little-endian TIFF: header, an IFD with the MP Entry tag only,
    // then the two 16-byte records it points to (at offset 26).
    let segment_length = 2 + 4 + 26 + 32;
    let base = 2 + 4 + 4; // SOI, APP2 marker and length, "MPF\0"
    let main_length = main.len() + segment_length + 2;
    let preview_offset = (main_length - base) as i64 + shift;

    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend([0x02, 0xb0, 7, 0]); // tag 0xb002, UNDEFINED
    tiff.extend(32u32.to_le_bytes());
    tiff.extend(26u32.to_le_bytes());
    tiff.extend(0u32.to_le_bytes()); // no next IFD
    for (attribute, size, offset) in [
        (0x2003_0000u32, main_length as u32, 0u32),
        (0x0001_0001, preview.len() as u32, preview_offset as u32),
    ] {
        tiff.extend(attribute.to_le_bytes());
        tiff.extend(size.to_le_bytes());
        tiff.extend(offset.to_le_bytes());
        tiff.extend([0; 4]);
    }

    let mut data = vec![0xff, 0xd8, 0xff, 0xe2];
    data.extend((segment_length as u16).to_be_bytes());
    data.extend(b"MPF\0");
    data.extend(tiff);
    data.extend(&main[2..]);
    data.extend(preview);
    data
}

/// Little-endian EXIF data whose second directory points to a thumbnail
/// stored right after it.
pub(crate) fn exif_with_thumbnail(thumbnail: &[u8]) -> Vec<u8> {
    // Header (8), empty first IFD at 8 (2 + 4), second IFD at 14 with
    // two entries (2 + 24 + 4 = 30), thumbnail at 44.
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(0u16.to_le_bytes());
    tiff.extend(14u32.to_le_bytes());
    tiff.extend(2u16.to_le_bytes());
    for (tag, value) in [(0x0201u16, 44u32), (0x0202, thumbnail.len() as u32)] {
        tiff.extend(tag.to_le_bytes());
        tiff.extend(4u16.to_le_bytes()); // LONG
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(value.to_le_bytes());
    }
    tiff.extend(0u32.to_le_bytes());
    assert_eq!(tiff.len(), 44);
    tiff.extend(thumbnail);
    tiff
}

/// Little-endian EXIF data with Panasonic maker notes holding an
/// autofocus point.
pub(crate) fn exif_with_af_point(x: (u32, u32), y: (u32, u32)) -> Vec<u8> {
    fn entry(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
        [
            &tag.to_le_bytes()[..],
            &kind.to_le_bytes(),
            &count.to_le_bytes(),
            &value.to_le_bytes(),
        ]
        .concat()
    }
    // Header (8), IFD0 at 8 with the Exif pointer (2 + 12 + 4 = 18),
    // Exif IFD at 26 with the maker notes (18), maker notes at 44:
    // "Panasonic\0\0\0" (12) then a directory at 56 with the AF point
    // (18), whose two rationals are at 74.
    let mut tiff = b"II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(entry(0x8769, 4, 1, 26));
    tiff.extend(0u32.to_le_bytes());
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(entry(0x927c, 7, 46, 44));
    tiff.extend(0u32.to_le_bytes());
    tiff.extend(b"Panasonic\0\0\0");
    tiff.extend(1u16.to_le_bytes());
    tiff.extend(entry(0x004d, 5, 2, 74));
    tiff.extend(0u32.to_le_bytes());
    assert_eq!(tiff.len(), 74);
    for value in [x.0, x.1, y.0, y.1] {
        tiff.extend(value.to_le_bytes());
    }
    tiff
}
