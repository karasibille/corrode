//! Reading a TIFF structure: EXIF data is one, and so is a RW2 file.

/// A TIFF structure, little- or big-endian, or a Panasonic RW2 file,
/// which is a little-endian TIFF with another magic number.
pub(crate) struct Tiff<'a> {
    data: &'a [u8],
    big_endian: bool,
    pub(crate) rw2: bool,
}

impl<'a> Tiff<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Option<Tiff<'a>> {
        let (big_endian, rw2) = match data.get(..4)? {
            b"II*\0" => (false, false),
            b"MM\0*" => (true, false),
            b"IIU\0" => (false, true),
            _ => return None,
        };
        Some(Tiff {
            data,
            big_endian,
            rw2,
        })
    }

    /// The first image directory.
    pub(crate) fn first_ifd(&self) -> Option<usize> {
        self.u32(4)
    }

    /// The `(count, value or offset)` of a tag in the directory at `ifd`.
    pub(crate) fn entry_with_count(&self, ifd: usize, tag: u16) -> Option<(usize, usize)> {
        (0..self.u16(ifd)?).find_map(|index| {
            let entry = ifd + 2 + 12 * usize::from(index);
            (self.u16(entry)? == tag).then(|| Some((self.u32(entry + 4)?, self.u32(entry + 8)?)))?
        })
    }

    /// The value of a single LONG tag, or the offset of a longer one.
    pub(crate) fn entry(&self, ifd: usize, tag: u16) -> Option<usize> {
        self.entry_with_count(ifd, tag).map(|(_, value)| value)
    }

    /// The directory following the one at `ifd`, if any.
    pub(crate) fn next_ifd(&self, ifd: usize) -> Option<usize> {
        let next = self.u32(ifd + 2 + 12 * usize::from(self.u16(ifd)?))?;
        (next != 0).then_some(next)
    }

    pub(crate) fn u16(&self, at: usize) -> Option<u16> {
        let bytes = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    pub(crate) fn u32(&self, at: usize) -> Option<usize> {
        let bytes = self.data.get(at..at + 4)?.try_into().ok()?;
        let value = if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        };
        usize::try_from(value).ok()
    }
}

/// The thumbnail referenced by EXIF data (a TIFF structure, as returned by
/// [`super::jpeg::exif`]): a small JPEG stored in the second image
/// directory.
pub fn exif_thumbnail(exif: &[u8]) -> Option<&[u8]> {
    let tiff = Tiff::new(exif)?;
    let thumbnail_ifd = tiff.next_ifd(tiff.first_ifd()?)?;
    let start = tiff.entry(thumbnail_ifd, 0x0201)?; // JPEGInterchangeFormat
    let length = tiff.entry(thumbnail_ifd, 0x0202)?; // JPEGInterchangeFormatLength
    let image = exif.get(start..start.checked_add(length)?)?;
    image.starts_with(&[0xff, 0xd8]).then_some(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::testing::{encode, exif_orientation, exif_with_thumbnail};

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
    fn reads_both_byte_orders() {
        let little = b"II*\0\x08\0\0\0\x01\0";
        let big = b"MM\0*\0\0\0\x08\0\x01";
        assert_eq!(Tiff::new(little).unwrap().first_ifd(), Some(8));
        assert_eq!(Tiff::new(big).unwrap().u16(8), Some(1));
        assert!(Tiff::new(b"not a tiff").is_none());
    }
}
