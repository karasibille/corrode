//! A sharpness score, to suggest the best frame of a burst.
//!
//! It measures the energy of fine edges, the Laplacian of the luminance:
//! a frame blurred by motion or missed focus has less of it. Pictures are
//! scaled to the same width first, so that JPEG and RAW previews compare.
//! The score is the mean of the sharpest tenth of the picture's tiles, so
//! that a sharp subject on a dark, blurred stage is what counts.
//!
//! When the camera recorded where it focused, only a square around that
//! point is measured: on a stage, the floor or the audience in front can
//! be sharp while the subject is not, and the whole picture would then
//! rank a missed frame first.
//!
//! Scores only compare frames of the same scene with the same settings,
//! such as a burst: noise at high ISO also reads as fine detail.

use image::DynamicImage;
use image::imageops::FilterType;

/// Width the picture is scaled to before measuring.
const WIDTH: u32 = 1024;
/// Side of the tiles, in pixels of the scaled picture.
const TILE: usize = 32;
/// Side of the square measured around the focus point, as a fraction of
/// the longer side of the picture: wide enough for a person on a stage,
/// and for the uncertainty of the point.
const FOCUS_AREA: f32 = 0.35;

/// The sharpness of a picture, around `focus` (fractions of the width and
/// height, as `Exif::focus_point`) when known; higher is sharper.
pub fn score(image: &DynamicImage, focus: Option<(f32, f32)>) -> f32 {
    let image = if image.width() > WIDTH {
        image.resize(WIDTH, u32::MAX, FilterType::Triangle)
    } else {
        image.clone()
    };
    let image = match focus {
        Some(point) => around(&image, point),
        None => image,
    };
    let gray = image.to_luma8();
    let (width, height) = (gray.width() as usize, gray.height() as usize);
    if width < 3 || height < 3 {
        return 0.0;
    }
    let pixels = gray.as_raw();
    let at = |x: usize, y: usize| i32::from(pixels[y * width + x]);

    let columns = width.div_ceil(TILE);
    let rows = height.div_ceil(TILE);
    let mut energy = vec![0f64; columns * rows];
    let mut counts = vec![0u32; columns * rows];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let laplacian =
                4 * at(x, y) - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1);
            let tile = (y / TILE) * columns + x / TILE;
            energy[tile] += f64::from(laplacian * laplacian);
            counts[tile] += 1;
        }
    }

    let mut tiles: Vec<f64> = energy
        .iter()
        .zip(&counts)
        .filter(|&(_, &count)| count > 0)
        .map(|(&energy, &count)| energy / f64::from(count))
        .collect();
    tiles.sort_by(|a, b| b.total_cmp(a));
    let sharpest = &tiles[..tiles.len().div_ceil(10)];
    (sharpest.iter().sum::<f64>() / sharpest.len() as f64).sqrt() as f32
}

/// The square of `FOCUS_AREA` centred on a point, moved to stay inside
/// the picture.
fn around(image: &DynamicImage, (x, y): (f32, f32)) -> DynamicImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let side = (FOCUS_AREA * width.max(height)).min(width).min(height);
    let left = (x * width - side / 2.0).clamp(0.0, width - side);
    let top = (y * height - side / 2.0).clamp(0.0, height - side);
    image.crop_imm(left as u32, top as u32, side as u32, side as u32)
}

#[cfg(test)]
mod tests {
    use image::{GrayImage, Luma};

    use super::*;

    /// A textured picture: a pseudo-random pattern, the same every time.
    fn texture(width: u32, height: u32) -> DynamicImage {
        let mut state = 12345u32;
        DynamicImage::ImageLuma8(GrayImage::from_fn(width, height, |_, _| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            Luma([(state >> 16) as u8])
        }))
    }

    #[test]
    fn blurring_lowers_the_score() {
        let sharp = texture(640, 480);
        let scores: Vec<f32> = [0.0, 1.0, 2.0, 4.0]
            .iter()
            .map(|&sigma| {
                if sigma == 0.0 {
                    score(&sharp, None)
                } else {
                    score(&sharp.blur(sigma), None)
                }
            })
            .collect();
        assert!(
            scores.windows(2).all(|pair| pair[0] > pair[1]),
            "{scores:?}"
        );
    }

    #[test]
    fn a_flat_picture_has_no_sharpness() {
        let flat = DynamicImage::ImageLuma8(GrayImage::from_pixel(200, 100, Luma([90])));
        assert_eq!(score(&flat, None), 0.0);
        assert_eq!(
            score(&DynamicImage::ImageLuma8(GrayImage::new(2, 2)), None),
            0.0
        );
    }

    #[test]
    fn a_small_sharp_subject_counts_more_than_a_blurred_background() {
        // A sharp patch on a flat stage, against the same patch blurred.
        let patch = texture(96, 96);
        let mut sharp = GrayImage::from_pixel(1024, 768, Luma([20]));
        let mut soft = sharp.clone();
        image::imageops::overlay(&mut sharp, &patch.to_luma8(), 400, 300);
        image::imageops::overlay(&mut soft, &patch.blur(3.0).to_luma8(), 400, 300);
        let (sharp, soft) = (
            score(&DynamicImage::ImageLuma8(sharp), None),
            score(&DynamicImage::ImageLuma8(soft), None),
        );
        // The subject covers about 1% of the picture, yet sets the score.
        assert!(sharp > 3.0 * soft, "sharp {sharp}, soft {soft}");
    }

    #[test]
    fn only_the_area_around_the_focus_point_counts() {
        // Sharp texture on the left half, blurred on the right half.
        let sharp = texture(800, 600).to_luma8();
        let mut picture = texture(800, 600).blur(3.0).to_luma8();
        image::imageops::replace(
            &mut picture,
            &image::imageops::crop_imm(&sharp, 0, 0, 400, 600).to_image(),
            0,
            0,
        );
        let picture = DynamicImage::ImageLuma8(picture);
        let left = score(&picture, Some((0.2, 0.5)));
        let right = score(&picture, Some((0.8, 0.5)));
        assert!(left > 3.0 * right, "left {left}, right {right}");
        // A point at the edge still measures a full square.
        assert!(score(&picture, Some((1.0, 1.0))) > 0.0);
    }

    #[test]
    fn large_pictures_are_scaled_to_compare_with_smaller_ones() {
        let large = texture(2048, 1536).blur(2.0);
        let small = large.resize(1024, 768, FilterType::Triangle);
        let (large, small) = (score(&large, None), score(&small, None));
        assert!(
            (large - small).abs() < 0.01 * small,
            "large {large}, small {small}"
        );
    }
}
