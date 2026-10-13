//! Pixel sorting, as Kim Asendorf's: the picture melts into streaks.

use std::fmt;

use image::{Rgb, RgbImage};
use rayon::prelude::*;

use super::Error;
use super::parallel;
use super::recipe::{Params, Spec};

/// Which way an effect runs over the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Along the rows, so that the streaks are horizontal.
    #[default]
    Horizontal,
    /// Along the columns.
    Vertical,
}

impl std::str::FromStr for Direction {
    type Err = String;

    fn from_str(text: &str) -> Result<Direction, String> {
        match text {
            "horizontal" | "h" | "x" => Ok(Direction::Horizontal),
            "vertical" | "v" | "y" => Ok(Direction::Vertical),
            other => Err(format!(
                "unknown direction '{other}' (horizontal or vertical)"
            )),
        }
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Direction::Horizontal => "horizontal",
            Direction::Vertical => "vertical",
        })
    }
}

/// Parameters of the pixel sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSort {
    pub direction: Direction,
    /// Brightness range, 0 to 255, of the pixels that are sorted: a run
    /// of neighbouring pixels within it is sorted, the others stay. The
    /// darkest shadows and the brightest highlights are usually left
    /// out, so that the streaks start and end on them.
    pub low: u8,
    pub high: u8,
    /// Whether the runs go from bright to dark instead of dark to bright.
    pub reverse: bool,
}

impl Default for PixelSort {
    fn default() -> PixelSort {
        PixelSort {
            direction: Direction::Horizontal,
            low: 40,
            high: 220,
            reverse: false,
        }
    }
}

/// Sorts the pixels by brightness along the rows or the columns, in
/// runs of neighbours whose brightness is within a range: the picture
/// melts into streaks between the shadows and the highlights that stop
/// them.
pub fn pixel_sort(image: &RgbImage, params: PixelSort) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    let (lines, length) = match params.direction {
        Direction::Horizontal => (height, width),
        Direction::Vertical => (width, height),
    };
    let at = |line: u32, i: u32| match params.direction {
        Direction::Horizontal => (i, line),
        Direction::Vertical => (line, i),
    };
    let (low, high) = (params.low.min(params.high), params.high.max(params.low));
    // Each line sorted on its own, lines in parallel.
    let sorted_lines: Vec<Vec<Rgb<u8>>> = (0..lines)
        .into_par_iter()
        .map(|line| {
            let mut pixels: Vec<Rgb<u8>> = (0..length)
                .map(|i| {
                    let (x, y) = at(line, i);
                    *image.get_pixel(x, y)
                })
                .collect();
            let mut i = 0;
            while i < pixels.len() {
                // A run of pixels within the range.
                let start = i;
                while i < pixels.len() && (low..=high).contains(&brightness(pixels[i])) {
                    i += 1;
                }
                if i - start > 1 {
                    let run = &mut pixels[start..i];
                    run.sort_unstable_by_key(|pixel| brightness(*pixel));
                    if params.reverse {
                        run.reverse();
                    }
                }
                // Past the pixel that ended the run.
                i += 1;
            }
            pixels
        })
        .collect();
    parallel::from_fn(width, height, |x, y| {
        let (line, i) = match params.direction {
            Direction::Horizontal => (y, x),
            Direction::Vertical => (x, y),
        };
        sorted_lines[line as usize][i as usize]
    })
}

/// Perceived brightness of a pixel, 0 to 255.
pub(super) fn brightness(pixel: Rgb<u8>) -> u8 {
    let [r, g, b] = pixel.0;
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000) as u8
}

impl Spec for PixelSort {
    const NAME: &'static str = "sort";

    fn parse(params: &mut Params) -> Result<PixelSort, Error> {
        let d = PixelSort::default();
        Ok(PixelSort {
            direction: params.get("direction", d.direction)?,
            low: params.get("low", d.low)?,
            high: params.get("high", d.high)?,
            reverse: params.get("reverse", d.reverse)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " direction={} low={} high={} reverse={}",
            self.direction, self.low, self.high, self.reverse
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(pixel_sort(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_melts_the_middle_tones_between_the_extremes() {
        // A row: dark, then a jumble of middle tones, then bright.
        let tones = [10u8, 120, 60, 200, 90, 150, 250, 80];
        let image = RgbImage::from_fn(8, 2, |x, _| Rgb([tones[x as usize]; 3]));
        let sorted = pixel_sort(
            &image,
            PixelSort {
                low: 40,
                high: 220,
                ..PixelSort::default()
            },
        );
        let row: Vec<u8> = (0..8).map(|x| sorted.get_pixel(x, 1)[0]).collect();
        assert_eq!(row, vec![10, 60, 90, 120, 150, 200, 250, 80]);

        let reversed = pixel_sort(
            &image,
            PixelSort {
                reverse: true,
                ..PixelSort::default()
            },
        );
        let row: Vec<u8> = (0..8).map(|x| reversed.get_pixel(x, 0)[0]).collect();
        assert_eq!(row, vec![10, 200, 150, 120, 90, 60, 250, 80]);

        // Vertically, nothing to sort here: every column is one tone.
        let vertical = pixel_sort(
            &image,
            PixelSort {
                direction: Direction::Vertical,
                ..PixelSort::default()
            },
        );
        assert_eq!(vertical, image);
    }

    #[test]
    fn directions_read_and_print() {
        assert_eq!("v".parse::<Direction>(), Ok(Direction::Vertical));
        assert_eq!(Direction::Horizontal.to_string(), "horizontal");
        assert!("diagonal".parse::<Direction>().is_err());
    }
}
