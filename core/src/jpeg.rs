//! Reading the metadata segments of a JPEG file without decoding it.
//!
//! Cameras store the EXIF data in an APP1 segment and, in the Multi-Picture
//! Format (CIPA DC-007), an index of extra images in an APP2 segment. A
//! Panasonic GX9 appends a 1440×1080 preview this way, which decodes about
//! nine times faster than the full 20 Mpx image. The EXIF data itself holds
//! a 160×120 thumbnail, within the first 64 KB of the file.
//!
//! Functions taking the head of a file give the place of an image in the
//! whole file, so that only its bytes need to be read from the disk.

use std::ops::Range;

/// Iterates over the `(marker, payload offset, payload)` of the segments
/// before the image data. Stops at the first malformed segment.
fn segments(data: &[u8]) -> impl Iterator<Item = (u8, usize, &[u8])> {
    let mut pos = if data.starts_with(&[0xff, 0xd8]) {
        2
    } else {
        data.len()
    };
    std::iter::from_fn(move || {
        // Markers may be preceded by any number of 0xff fill bytes.
        while data.get(pos..pos + 2) == Some(&[0xff, 0xff]) {
            pos += 1;
        }
        let &[0xff, marker] = data.get(pos..pos + 2)? else {
            return None;
        };
        // Start of scan or end of image: the metadata is over.
        if marker == 0xda || marker == 0xd9 {
            return None;
        }
        let length = usize::from(u16::from_be_bytes(
            data.get(pos + 2..pos + 4)?.try_into().ok()?,
        ));
        let payload = data.get(pos + 4..pos + 2 + length)?;
        let segment = (marker, pos + 4, payload);
        pos += 2 + length;
        Some(segment)
    })
}

/// The EXIF data of the file, as a TIFF structure (without the `Exif\0\0`
/// prefix of the APP1 segment).
pub fn exif(data: &[u8]) -> Option<&[u8]> {
    segments(data).find_map(|(marker, _, payload)| match marker {
        0xe1 => payload.strip_prefix(b"Exif\0\0"),
        _ => None,
    })
}

/// The largest extra image listed in the Multi-Picture Format index, as a
/// slice of `data`: usually a preview of the main image.
pub fn mpf_preview(data: &[u8]) -> Option<&[u8]> {
    let image = data.get(mpf_preview_range(data)?)?;
    image.starts_with(&[0xff, 0xd8]).then_some(image)
}

/// Where the largest extra image of the Multi-Picture Format index lies in
/// the file, from its head: the index comes first, the images last.
pub fn mpf_preview_range(head: &[u8]) -> Option<Range<usize>> {
    let (offset, payload) = segments(head).find_map(|(marker, offset, payload)| {
        (marker == 0xe2 && payload.starts_with(b"MPF\0")).then_some((offset, payload))
    })?;
    // Offsets in the index are relative to the TIFF header after "MPF\0".
    let base = offset + 4;
    let tiff = Tiff::new(&payload[4..])?;

    // Tag 0xb002, MP Entry: 16 bytes per image, the main one first.
    let (count, value) = tiff.entry_with_count(tiff.u32(4)?, 0xb002)?;

    (1..count / 16)
        .filter_map(|index| {
            let record = value + 16 * index;
            let size = tiff.u32(record + 4)?;
            let start = base.checked_add(tiff.u32(record + 8)?)?;
            Some(start..start.checked_add(size)?)
        })
        .filter(|range| !range.is_empty())
        .max_by_key(|range| range.len())
}

/// The thumbnail referenced by EXIF data (a TIFF structure, as returned by
/// [`exif`]): a small JPEG stored in the second image directory.
pub fn exif_thumbnail(exif: &[u8]) -> Option<&[u8]> {
    let tiff = Tiff::new(exif)?;
    let thumbnail_ifd = tiff.next_ifd(tiff.u32(4)?)?;
    let start = tiff.entry(thumbnail_ifd, 0x0201)?; // JPEGInterchangeFormat
    let length = tiff.entry(thumbnail_ifd, 0x0202)?; // JPEGInterchangeFormatLength
    let image = exif.get(start..start.checked_add(length)?)?;
    image.starts_with(&[0xff, 0xd8]).then_some(image)
}

/// The JPEG preview embedded in a Panasonic RW2 file, as far as `data`
/// goes: reading the head of the file is enough for its EXIF data.
pub fn rw2_preview(data: &[u8]) -> Option<&[u8]> {
    let range = rw2_preview_range(data)?;
    let image = data.get(range.start..range.end.min(data.len()))?;
    image.starts_with(&[0xff, 0xd8]).then_some(image)
}

/// Where the JPEG preview of a Panasonic RW2 file lies, from its head.
pub fn rw2_preview_range(head: &[u8]) -> Option<Range<usize>> {
    let tiff = Tiff::new(head).filter(|tiff| tiff.panasonic)?;
    let (count, start) = tiff.entry_with_count(tiff.u32(4)?, 0x002e)?; // JpgFromRaw
    Some(start..start.checked_add(count)?)
}

/// A TIFF structure, little- or big-endian, or a Panasonic RW2 file,
/// which is a little-endian TIFF with another magic number.
struct Tiff<'a> {
    data: &'a [u8],
    big_endian: bool,
    panasonic: bool,
}

impl<'a> Tiff<'a> {
    fn new(data: &'a [u8]) -> Option<Tiff<'a>> {
        let (big_endian, panasonic) = match data.get(..4)? {
            b"II*\0" => (false, false),
            b"MM\0*" => (true, false),
            b"IIU\0" => (false, true),
            _ => return None,
        };
        Some(Tiff {
            data,
            big_endian,
            panasonic,
        })
    }

    /// The `(count, value or offset)` of a tag in the directory at `ifd`.
    fn entry_with_count(&self, ifd: usize, tag: u16) -> Option<(usize, usize)> {
        (0..self.u16(ifd)?).find_map(|index| {
            let entry = ifd + 2 + 12 * usize::from(index);
            (self.u16(entry)? == tag).then(|| Some((self.u32(entry + 4)?, self.u32(entry + 8)?)))?
        })
    }

    /// The value of a single LONG tag, or the offset of a longer one.
    fn entry(&self, ifd: usize, tag: u16) -> Option<usize> {
        self.entry_with_count(ifd, tag).map(|(_, value)| value)
    }

    /// The directory following the one at `ifd`, if any.
    fn next_ifd(&self, ifd: usize) -> Option<usize> {
        let next = self.u32(ifd + 2 + 12 * usize::from(self.u16(ifd)?))?;
        (next != 0).then_some(next)
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let bytes = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    fn u32(&self, at: usize) -> Option<usize> {
        let bytes = self.data.get(at..at + 4)?.try_into().ok()?;
        let value = if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        };
        usize::try_from(value).ok()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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

    #[test]
    fn finds_the_exif_data() {
        let data = encode(16, 8, Some(exif_orientation(6)));
        assert_eq!(exif(&data), Some(&exif_orientation(6)[..]));
        assert_eq!(exif(&encode(16, 8, None)), None);
    }

    #[test]
    fn finds_the_mpf_preview() {
        let preview = encode(32, 24, None);
        let data = with_mpf_preview(&encode(64, 48, None), &preview, 0);
        assert_eq!(mpf_preview(&data), Some(&preview[..]));
    }

    #[test]
    fn locates_the_mpf_preview_from_the_head_alone() {
        let preview = encode(32, 24, None);
        let data = with_mpf_preview(&encode(64, 48, None), &preview, 0);
        let range = mpf_preview_range(&data[..200]).unwrap();
        assert_eq!(range, data.len() - preview.len()..data.len());
    }

    #[test]
    fn ignores_missing_or_broken_mpf_data() {
        assert_eq!(mpf_preview(&encode(64, 48, None)), None);
        let preview = encode(32, 24, None);
        let main = encode(64, 48, None);
        // Offset pointing past the end, or not at the start of a JPEG.
        assert_eq!(mpf_preview(&with_mpf_preview(&main, &preview, 1000)), None);
        assert_eq!(mpf_preview(&with_mpf_preview(&main, &preview, 1)), None);
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

    #[test]
    fn finds_the_exif_thumbnail() {
        let thumbnail = encode(16, 12, None);
        let exif = exif_with_thumbnail(&thumbnail);
        assert_eq!(exif_thumbnail(&exif), Some(&thumbnail[..]));
        // Truncated data, or no second directory.
        assert_eq!(exif_thumbnail(&exif[..exif.len() - 1]), None);
        assert_eq!(exif_thumbnail(&exif_orientation(6)), None);
    }

    #[test]
    fn finds_the_preview_of_a_rw2_head() {
        let preview = encode(32, 24, None);
        // RW2 header, IFD at 8 with the JpgFromRaw tag, preview at 26.
        let mut rw2 = b"IIU\0".to_vec();
        rw2.extend(8u32.to_le_bytes());
        rw2.extend(1u16.to_le_bytes());
        rw2.extend(0x002eu16.to_le_bytes());
        rw2.extend(7u16.to_le_bytes()); // UNDEFINED
        rw2.extend((preview.len() as u32).to_le_bytes());
        rw2.extend(26u32.to_le_bytes());
        rw2.extend(0u32.to_le_bytes());
        rw2.extend(&preview);

        assert_eq!(rw2_preview(&rw2), Some(&preview[..]));
        // A head that stops inside the preview still gives its start.
        assert_eq!(rw2_preview(&rw2[..40]), Some(&preview[..14]));
        // Only RW2 files: a plain TIFF with the same tag is not one.
        rw2[2] = b'*';
        assert_eq!(rw2_preview(&rw2), None);
    }

    #[test]
    fn rejects_data_that_is_not_a_jpeg() {
        for data in [&b""[..], b"\xff", b"not a jpeg", b"\xff\xd8\xff\xe1\xff"] {
            assert_eq!(exif(data), None);
            assert_eq!(mpf_preview(data), None);
        }
    }
}
