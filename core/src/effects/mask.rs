//! Soft maps for the effects that act on some colours only: how much of
//! an effect each pixel gets, worked out on a small copy of the picture
//! and blurred, so that the edge of a zone is soft and the specks of
//! another colour inside a zone, the sparkle of water or the noise of a
//! sky, do not stay as they were.

use image::RgbImage;
use image::imageops::{self, FilterType};

/// The long side of the small copy the map is worked out on.
const MAP_SIDE: u32 = 384;

/// Two numbers for each point of a small copy of a picture, read at any
/// pixel of the picture by interpolation.
pub(super) struct SoftMap {
    width: u32,
    height: u32,
    /// Pixels of the picture for a cell of the map.
    scale: f32,
    cells: Vec<[f32; 2]>,
}

impl SoftMap {
    /// The map of a picture: `weight` gives the two numbers of a colour,
    /// each channel 0 to 255, and they are blurred over `smooth` percent
    /// of the long side of the picture, whatever its size.
    pub(super) fn build(
        image: &RgbImage,
        smooth: f32,
        weight: impl Fn([f32; 3]) -> [f32; 2],
    ) -> SoftMap {
        let (width, height) = image.dimensions();
        let long = width.max(height).max(1);
        let reduction = if long > MAP_SIDE {
            MAP_SIDE as f32 / long as f32
        } else {
            1.0
        };
        let size = |side: u32| ((side as f32 * reduction).round() as u32).max(1);
        let (small_width, small_height) = (size(width), size(height));
        // A reduction takes the mean colour of an area, which is steadier
        // than the colour of a pixel.
        let small = if reduction < 1.0 {
            imageops::resize(image, small_width, small_height, FilterType::Triangle)
        } else {
            image.clone()
        };
        let mut cells: Vec<[f32; 2]> = small
            .pixels()
            .map(|pixel| weight(pixel.0.map(f32::from)))
            .collect();
        let sigma = smooth / 100.0 * small_width.max(small_height) as f32;
        blur(&mut cells, small_width, small_height, sigma);
        SoftMap {
            width: small_width,
            height: small_height,
            scale: 1.0 / reduction,
            cells,
        }
    }

    /// The two numbers at a pixel of the picture.
    pub(super) fn at(&self, x: u32, y: u32) -> [f32; 2] {
        let fx = ((x as f32 + 0.5) / self.scale - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) / self.scale - 0.5).max(0.0);
        let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let cell = |x: u32, y: u32| {
            self.cells[(y.min(self.height - 1) * self.width + x.min(self.width - 1)) as usize]
        };
        let (a, b, c, d) = (
            cell(x0, y0),
            cell(x0 + 1, y0),
            cell(x0, y0 + 1),
            cell(x0 + 1, y0 + 1),
        );
        let mut out = [0.0; 2];
        for k in 0..2 {
            let top = a[k] * (1.0 - tx) + b[k] * tx;
            let bottom = c[k] * (1.0 - tx) + d[k] * tx;
            out[k] = top * (1.0 - ty) + bottom * ty;
        }
        out
    }
}

/// A gaussian blur of `sigma` cells, rows then columns, the edges extended.
fn blur(cells: &mut [[f32; 2]], width: u32, height: u32, sigma: f32) {
    if sigma < 0.3 {
        return;
    }
    let reach = (sigma * 3.0).ceil() as i32;
    let mut kernel: Vec<f32> = (-reach..=reach)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|weight| *weight /= total);
    let (w, h) = (width as i32, height as i32);
    let pass = |from: &[[f32; 2]], along_rows: bool| -> Vec<[f32; 2]> {
        let mut out = vec![[0.0; 2]; from.len()];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 2];
                for (k, weight) in kernel.iter().enumerate() {
                    let offset = k as i32 - reach;
                    let (sx, sy) = if along_rows {
                        ((x + offset).clamp(0, w - 1), y)
                    } else {
                        (x, (y + offset).clamp(0, h - 1))
                    };
                    let cell = from[(sy * w + sx) as usize];
                    sum[0] += cell[0] * weight;
                    sum[1] += cell[1] * weight;
                }
                out[(y * w + x) as usize] = sum;
            }
        }
        out
    };
    let rows = pass(cells, true);
    let both = pass(&rows, false);
    cells.copy_from_slice(&both);
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// 1 for a red pixel, 0 for others.
    fn redness([r, g, b]: [f32; 3]) -> [f32; 2] {
        [if r > g + b { 1.0 } else { 0.0 }, 0.0]
    }

    #[test]
    fn a_flat_zone_stays_whole_and_an_edge_softens() {
        // Left half red, right half blue, 200 wide.
        let image = RgbImage::from_fn(200, 20, |x, _| {
            Rgb(if x < 100 {
                [220, 20, 20]
            } else {
                [20, 20, 220]
            })
        });
        let map = SoftMap::build(&image, 5.0, redness);
        assert!(map.at(10, 10)[0] > 0.99);
        assert!(map.at(190, 10)[0] < 0.01);
        let edge = map.at(100, 10)[0];
        assert!(edge > 0.3 && edge < 0.7, "{edge}");
        assert!(map.at(85, 10)[0] > map.at(100, 10)[0], "softer than a step");
        assert!(map.at(85, 10)[0] < 1.0 - 1e-3);
    }

    #[test]
    fn a_speck_inside_a_zone_is_filled_and_without_smoothing_it_stays() {
        let mut image = RgbImage::from_pixel(300, 300, Rgb([220, 20, 20]));
        for (x, y) in [(150, 150), (151, 150), (150, 151), (151, 151)] {
            image.put_pixel(x, y, Rgb([20, 20, 220]));
        }
        let smooth = SoftMap::build(&image, 3.0, redness);
        assert!(smooth.at(150, 150)[0] > 0.9, "{}", smooth.at(150, 150)[0]);
        let sharp = SoftMap::build(&image, 0.0, redness);
        assert!(sharp.at(150, 150)[0] < 0.5);
    }

    #[test]
    fn the_map_reads_at_every_pixel_of_a_large_picture() {
        let image = RgbImage::from_pixel(2000, 1000, Rgb([220, 20, 20]));
        let map = SoftMap::build(&image, 1.0, redness);
        for (x, y) in [(0, 0), (1999, 0), (0, 999), (1999, 999), (1000, 500)] {
            assert!(map.at(x, y)[0] > 0.99, "{x},{y}");
        }
    }
}
