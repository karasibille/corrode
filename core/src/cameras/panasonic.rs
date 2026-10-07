//! Panasonic cameras: the autofocus point in their maker notes.

use crate::formats::tiff::Tiff;

/// The autofocus point a Panasonic camera records in its maker notes,
/// as fractions of the width and height of the sensor. `exif` is EXIF
/// data as a TIFF structure.
pub fn af_point(exif: &[u8]) -> Option<(f32, f32)> {
    let tiff = Tiff::new(exif)?;
    let exif_ifd = tiff.entry(tiff.first_ifd()?, 0x8769)?; // Exif IFD pointer
    let maker_notes = tiff.entry(exif_ifd, 0x927c)?; // MakerNote
    // "Panasonic\0\0\0", then a directory whose offsets are relative to
    // the TIFF header, like the rest of the EXIF data.
    if exif.get(maker_notes..maker_notes + 12)? != b"Panasonic\0\0\0" {
        return None;
    }
    let point = tiff.entry(maker_notes + 12, 0x004d)?; // AFPointPosition, 2 RATIONAL
    let fraction = |at: usize| {
        let (numerator, denominator) = (tiff.u32(at)?, tiff.u32(at + 4)?);
        (denominator != 0).then(|| numerator as f32 / denominator as f32)
    };
    let (x, y) = (fraction(point)?, fraction(point + 8)?);
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some((x, y))
}

/// The autofocus point in the frame of the upright picture, given the EXIF
/// orientation of the file.
pub fn focus_point(exif: &[u8], orientation: u16) -> Option<(f32, f32)> {
    af_point(exif).map(|point| upright_point(point, orientation))
}

/// Turns an autofocus point recorded in the sensor's frame into the frame
/// of the upright picture. Established on photos of a Panasonic GX9 with
/// a plain subject: for shots held vertically, the point turns the other
/// way from the picture, as if seen from the back of the sensor.
fn upright_point((x, y): (f32, f32), orientation: u16) -> (f32, f32) {
    match orientation {
        3 => (1.0 - x, 1.0 - y),
        6 => (y, 1.0 - x),
        8 => (1.0 - y, x),
        _ => (x, y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::testing::{exif_orientation, exif_with_af_point};

    #[test]
    fn finds_the_af_point() {
        let exif = exif_with_af_point((90, 256), (143, 256));
        assert_eq!(af_point(&exif), Some((90.0 / 256.0, 143.0 / 256.0)));
        // Not Panasonic maker notes, or a point outside the frame.
        let mut other = exif.clone();
        other[44..53].copy_from_slice(b"Olympus\0\0");
        assert_eq!(af_point(&other), None);
        assert_eq!(af_point(&exif_with_af_point((300, 256), (1, 2))), None);
        assert_eq!(af_point(&exif_orientation(6)), None);
    }

    #[test]
    fn focus_points_follow_the_picture_upright() {
        let point = (0.35, 0.56);
        assert_eq!(upright_point(point, 1), (0.35, 0.56));
        assert_eq!(upright_point(point, 3), (0.65, 1.0 - 0.56));
        assert_eq!(upright_point(point, 6), (0.56, 0.65));
        assert_eq!(upright_point(point, 8), (1.0 - 0.56, 0.35));
        let exif = exif_with_af_point((128, 256), (64, 256));
        assert_eq!(focus_point(&exif, 1), Some((0.5, 0.25)));
    }
}
