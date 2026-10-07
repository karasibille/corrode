//! Panasonic RW2 files: a TIFF structure with its own magic number, which
//! embeds a JPEG preview at its start, EXIF data included.

use std::ops::Range;

use super::tiff::Tiff;

/// The JPEG preview embedded in a RW2 file, as far as `data` goes: the
/// head of the file is enough for the preview's EXIF data.
pub fn preview(data: &[u8]) -> Option<&[u8]> {
    let range = preview_range(data)?;
    let image = data.get(range.start..range.end.min(data.len()))?;
    image.starts_with(&[0xff, 0xd8]).then_some(image)
}

/// Where the JPEG preview of a RW2 file lies, from its head.
pub fn preview_range(head: &[u8]) -> Option<Range<usize>> {
    let tiff = Tiff::new(head).filter(|tiff| tiff.rw2)?;
    let (count, start) = tiff.entry_with_count(tiff.first_ifd()?, 0x002e)?; // JpgFromRaw
    Some(start..start.checked_add(count)?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::formats::testing::encode;

    /// A RW2 head: the header, a directory with the JpgFromRaw tag, and
    /// `preview` at offset 26.
    pub(crate) fn with_preview(preview: &[u8]) -> Vec<u8> {
        let mut rw2 = b"IIU\0".to_vec();
        rw2.extend(8u32.to_le_bytes());
        rw2.extend(1u16.to_le_bytes());
        rw2.extend(0x002eu16.to_le_bytes());
        rw2.extend(7u16.to_le_bytes()); // UNDEFINED
        rw2.extend((preview.len() as u32).to_le_bytes());
        rw2.extend(26u32.to_le_bytes());
        rw2.extend(0u32.to_le_bytes());
        rw2.extend(preview);
        rw2
    }

    #[test]
    fn finds_the_preview_of_a_rw2_head() {
        let embedded = encode(32, 24, None);
        let mut rw2 = with_preview(&embedded);
        assert_eq!(preview(&rw2), Some(&embedded[..]));
        // A head that stops inside the preview still gives its start.
        assert_eq!(preview(&rw2[..40]), Some(&embedded[..14]));
        // Only RW2 files: a plain TIFF with the same tag is not one.
        rw2[2] = b'*';
        assert_eq!(preview(&rw2), None);
    }
}
