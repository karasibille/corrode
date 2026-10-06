//! Detection of the light bands LED stage lighting leaves on photos taken
//! with an electronic shutter.
//!
//! The sensor is read row after row while the LEDs flicker, so each row
//! catches them at a different moment: fine stripes with a regular
//! spacing appear along the sensor rows, parallel to the long side of the
//! picture whatever its orientation. They show on flat areas (floor,
//! background, haze), not on brightly lit subjects.
//!
//! The picture is cut into overlapping blocks. In each block, the
//! brightness profile across the stripes is searched for a clear periodic
//! peak. Scene details may give a peak in one block, but stripes give the
//! same period in many blocks; a picture is banded when enough blocks
//! agree. Periods around 4 and 8 pixels, and of 16 pixels, are ignored:
//! JPEG compression leaves a grid there.

use std::f64::consts::PI;
use std::sync::OnceLock;

use image::DynamicImage;

/// Side of the blocks, in pixels of the picture.
const BLOCK: usize = 256;
/// Shortest and longest period searched, in pixels, in tenths.
const PERIODS: std::ops::RangeInclusive<usize> = 60..=640;
/// Periods whose frequency is this close to the 4 and 8 px JPEG grid, in
/// cycles per block, are skipped: a block cannot tell them apart from it.
const GRID_MARGIN: f64 = 2.5;
/// Periods this close to 16 px are skipped, in pixels: chroma subsampling
/// can leave a weaker grid there, and a wide margin would hide real
/// stripes of 14 to 19 px.
const WIDE_GRID_MARGIN: f64 = 0.3;
/// How far above the median power a block's peak must stand.
const BLOCK_PEAK: f64 = 10.0;
/// Two blocks agree when their periods differ by less than this share.
const AGREEMENT: f64 = 0.04;
/// A picture is banded when this share of its blocks agree on a period…
const MIN_COVERAGE: f32 = 0.25;
/// …and the strongest of these blocks stands this far above its median.
const MIN_PEAK: f32 = 200.0;

/// What the analysis of a picture found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bands {
    /// The spacing of the stripes most blocks agree on, in pixels.
    pub period: f32,
    /// The share of the blocks that found this period.
    pub coverage: f32,
    /// How far the strongest of these blocks' peak stands above its median.
    pub peak: f32,
}

impl Bands {
    /// Whether the stripes are clear enough to report the picture banded.
    pub fn is_banded(&self) -> bool {
        self.coverage >= MIN_COVERAGE && self.peak >= MIN_PEAK
    }
}

/// The candidate periods, with their sine and cosine over a block.
struct Tables {
    periods: Vec<f64>,
    cos: Vec<Vec<f64>>,
    sin: Vec<Vec<f64>>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let periods: Vec<f64> = PERIODS
            .map(|tenths| tenths as f64 / 10.0)
            .filter(|&period| {
                let cycles = |period: f64| BLOCK as f64 / period;
                [4.0, 8.0]
                    .iter()
                    .all(|&grid| (cycles(period) - cycles(grid)).abs() > GRID_MARGIN)
                    && (period - 16.0).abs() > WIDE_GRID_MARGIN
            })
            .collect();
        let wave = |f: fn(f64) -> f64| -> Vec<Vec<f64>> {
            periods
                .iter()
                .map(|period| {
                    (0..BLOCK)
                        .map(|i| f(2.0 * PI * i as f64 / period))
                        .collect()
                })
                .collect()
        };
        Tables {
            cos: wave(f64::cos),
            sin: wave(f64::sin),
            periods,
        }
    })
}

/// Analyses a picture, upright as displayed. `None` when it is too small
/// to hold a block.
pub fn analyze(image: &DynamicImage) -> Option<Bands> {
    let gray = image.to_luma8();
    let (width, height) = (gray.width() as usize, gray.height() as usize);
    // Stripes run along the long side; brightness varies along the short one.
    let across_x = width < height;
    let (across, along) = if across_x {
        (width, height)
    } else {
        (height, width)
    };
    if across < BLOCK || along < BLOCK {
        return None;
    }
    let pixels = gray.as_raw();
    let value = |u: usize, v: usize| {
        let (x, y) = if across_x { (u, v) } else { (v, u) };
        f64::from(pixels[y * width + x])
    };

    let tables = tables();
    let hann: Vec<f64> = (0..BLOCK)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / BLOCK as f64).cos())
        .collect();
    let step = BLOCK / 2;
    let mut peaks = Vec::new();
    let mut blocks = 0;
    for v0 in (0..=along - BLOCK).step_by(step) {
        for u0 in (0..=across - BLOCK).step_by(step) {
            blocks += 1;
            // Brightness profile across the stripes, in log units so that
            // a stripe weighs the same in dark and bright areas.
            let profile: Vec<f64> = (u0..u0 + BLOCK)
                .map(|u| {
                    let sum: f64 = (v0..v0 + BLOCK).map(|v| value(u, v)).sum();
                    (sum / BLOCK as f64 + 4.0).ln()
                })
                .collect();
            // Remove the scene's slow changes, keeping periods up to 64 px.
            let signal: Vec<f64> = (0..BLOCK)
                .map(|i| {
                    let (a, b) = (i.saturating_sub(8), (i + 9).min(BLOCK));
                    let local = profile[a..b].iter().sum::<f64>() / (b - a) as f64;
                    (profile[i] - local) * hann[i]
                })
                .collect();
            let power: Vec<f64> = (0..tables.periods.len())
                .map(|p| {
                    let re: f64 = signal.iter().zip(&tables.cos[p]).map(|(s, c)| s * c).sum();
                    let im: f64 = signal.iter().zip(&tables.sin[p]).map(|(s, c)| s * c).sum();
                    re * re + im * im
                })
                .collect();
            let mut sorted = power.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let median = sorted[sorted.len() / 2];
            let (best, &peak) = power
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .expect("periods are not empty");
            if median > 0.0 && peak / median > BLOCK_PEAK {
                peaks.push((tables.periods[best], peak / median));
            }
        }
    }

    // The period the most blocks agree on.
    let mut found = Bands {
        period: 0.0,
        coverage: 0.0,
        peak: 0.0,
    };
    let mut most = 0;
    for &(period, _) in &peaks {
        let agreeing = peaks
            .iter()
            .filter(|(other, _)| (other - period).abs() / period < AGREEMENT);
        let (count, strongest) = agreeing.fold((0, 0f64), |(n, max), &(_, r)| (n + 1, max.max(r)));
        if count > most {
            most = count;
            found = Bands {
                period: period as f32,
                coverage: count as f32 / blocks as f32,
                peak: strongest as f32,
            };
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use image::{GrayImage, Luma};

    use super::*;

    /// A textured picture, the same every time, with optional stripes:
    /// brightness multiplied by `1 + depth·sin`, varying along x.
    fn picture(width: u32, height: u32, stripes: Option<(f64, f64)>) -> DynamicImage {
        let mut state = 99u32;
        DynamicImage::ImageLuma8(GrayImage::from_fn(width, height, |x, y| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = f64::from((state >> 16) % 24) - 12.0;
            // A smooth "scene": a gradient and a large blob.
            let scene = 90.0
                + 0.03 * f64::from(y)
                + 40.0 * (-(f64::from(x) - 300.0).powi(2) / 20000.0).exp();
            let gain = stripes.map_or(1.0, |(period, depth)| {
                1.0 + depth * (2.0 * PI * f64::from(x) / period).sin()
            });
            Luma([(scene * gain + noise).clamp(0.0, 255.0) as u8])
        }))
    }

    #[test]
    fn finds_fine_regular_stripes() {
        let bands = analyze(&picture(1080, 1440, Some((12.6, 0.02)))).unwrap();
        assert!(bands.is_banded(), "{bands:?}");
        assert!((bands.period - 12.6).abs() < 0.3, "{bands:?}");
        assert!(bands.coverage > 0.8, "{bands:?}");
    }

    #[test]
    fn stripes_follow_the_long_side_in_landscape_too() {
        let striped = picture(1080, 1440, Some((20.0, 0.02)));
        let landscape = striped.rotate90();
        let bands = analyze(&landscape).unwrap();
        assert!(bands.is_banded(), "{bands:?}");
        assert!((bands.period - 20.0).abs() < 0.4, "{bands:?}");
    }

    #[test]
    fn a_picture_without_stripes_is_not_banded() {
        let bands = analyze(&picture(1080, 1440, None)).unwrap();
        assert!(!bands.is_banded(), "{bands:?}");
    }

    #[test]
    fn the_jpeg_grid_period_is_ignored() {
        let bands = analyze(&picture(1080, 1440, Some((8.0, 0.02)))).unwrap();
        assert!(!bands.is_banded(), "{bands:?}");
    }

    #[test]
    fn small_pictures_are_not_analysed() {
        assert!(analyze(&picture(200, 300, None)).is_none());
    }
}
