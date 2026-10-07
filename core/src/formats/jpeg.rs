//! Reading the metadata segments of a JPEG file without decoding it.
//!
//! Cameras store the EXIF data in an APP1 segment and, in the Multi-Picture
//! Format (CIPA DC-007), an index of extra images in an APP2 segment. A
//! Panasonic GX9 appends a 1440×1080 preview this way, which decodes about
//! nine times faster than the full 20 Mpx image.
//!
//! Functions taking the head of a file give the place of an image in the
//! whole file, so that only its bytes need to be read from the disk.

use std::ops::Range;

use super::tiff::Tiff;

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
    let (count, value) = tiff.entry_with_count(tiff.first_ifd()?, 0xb002)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::testing::{encode, exif_orientation, with_mpf_preview};

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

    #[test]
    fn rejects_data_that_is_not_a_jpeg() {
        for data in [&b""[..], b"\xff", b"not a jpeg", b"\xff\xd8\xff\xe1\xff"] {
            assert_eq!(exif(data), None);
            assert_eq!(mpf_preview(data), None);
        }
    }
}
