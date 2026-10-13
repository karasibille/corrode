//! The failures of a screen or a stream: slices of the picture pushed
//! aside, lines that stretch, colour channels that come apart.
//!
//! Sizes are given in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) pixels
//! wide and scale with the picture, so that a recipe looks the same on
//! a preview and on the full-size picture.

use std::fmt;

use image::{Rgb, RgbImage};

use super::Error;
use super::parallel;
use super::random::Random;
use super::recipe::{Params, Spec, pair};
use super::sort::Direction;
use super::units::{Size, scale_of};

fn scaled_offset(offset: i32, scale: f32) -> i32 {
    (offset as f32 * scale).round() as i32
}

/// Parameters of the slice shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceShift {
    /// How many horizontal slices are pushed aside.
    pub slices: u32,
    /// How far a slice can be pushed, either way, in pixels for a
    /// picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide. What goes out on one side comes
    /// back on the other.
    pub shift: Size,
    /// Height of a slice, drawn between these two, in the same pixels.
    pub height: (Size, Size),
    /// Whether the red, green and blue of a slice are pushed by
    /// different amounts, so that they come apart.
    pub split: bool,
    /// Whether some slices, one in three, get their colours inverted.
    pub invert: bool,
}

impl Default for SliceShift {
    fn default() -> SliceShift {
        SliceShift {
            slices: 12,
            shift: Size::new(120),
            height: (Size::new(8), Size::new(120)),
            split: false,
            invert: false,
        }
    }
}

/// Pushes horizontal slices of the picture aside, as a broken stream
/// does. The seed chooses where the slices are and how far they go.
pub fn slice_shift(image: &RgbImage, params: SliceShift, seed: u64) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let max_shift = params.shift.on(width);
    let heights = (params.height.0.on(width), params.height.1.on(width));
    let mut random = Random::new(seed);
    let mut shifted = image.clone();
    for _ in 0..params.slices {
        let slice_height = random.between(heights.0, heights.1).max(1);
        let top = random.below(u64::from(height)) as u32;
        let bottom = top.saturating_add(slice_height).min(height);
        let shift = random.around(max_shift);
        let shifts = if params.split {
            [shift, random.around(max_shift), random.around(max_shift)]
        } else {
            [shift; 3]
        };
        let invert = params.invert && random.once_in(3);
        parallel::for_each_pixel_in_rows(&mut shifted, top, bottom, |x, y, pixel| {
            for (channel, &dx) in shifts.iter().enumerate() {
                let source = (i64::from(x) - i64::from(dx)).rem_euclid(i64::from(width)) as u32;
                pixel[channel] = image.get_pixel(source, y)[channel];
            }
            if invert {
                *pixel = Rgb(pixel.0.map(|v| 255 - v));
            }
        });
    }
    shifted
}

/// Parameters of the pixel stretch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelStretch {
    /// How many lines are stretched.
    pub bands: u32,
    /// How far a line is stretched, drawn between these two, in pixels
    /// for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
    pub length: (Size, Size),
    /// Rows stretched downwards, or columns stretched rightwards.
    pub direction: Direction,
}

impl Default for PixelStretch {
    fn default() -> PixelStretch {
        PixelStretch {
            bands: 6,
            length: (Size::new(20), Size::new(200)),
            direction: Direction::Horizontal,
        }
    }
}

/// Repeats a few lines of the picture over the ones that follow, so that
/// they run like wet paint. The seed chooses the lines and how far.
pub fn pixel_stretch(image: &RgbImage, params: PixelStretch, seed: u64) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let (lines, length) = match params.direction {
        Direction::Horizontal => (height, width),
        Direction::Vertical => (width, height),
    };
    let at = |line: u32, i: u32| match params.direction {
        Direction::Horizontal => (i, line),
        Direction::Vertical => (line, i),
    };
    let lengths = (params.length.0.on(width), params.length.1.on(width));
    let mut random = Random::new(seed);
    let mut stretched = image.clone();
    for _ in 0..params.bands {
        let line = random.below(u64::from(lines)) as u32;
        let run = random.between(lengths.0, lengths.1);
        let end = line.saturating_add(run).min(lines);
        for target in line + 1..end {
            for i in 0..length {
                let (sx, sy) = at(line, i);
                let (tx, ty) = at(target, i);
                stretched.put_pixel(tx, ty, *image.get_pixel(sx, sy));
            }
        }
    }
    stretched
}

/// Parameters of the channel split: how far each channel is moved, as
/// `(dx, dy)`, in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelSplit {
    pub red: (i32, i32),
    pub green: (i32, i32),
    pub blue: (i32, i32),
}

impl Default for ChannelSplit {
    fn default() -> ChannelSplit {
        ChannelSplit {
            red: (-6, 0),
            green: (0, 0),
            blue: (6, 0),
        }
    }
}

/// Moves the red, green and blue of the picture apart, each its own
/// way, as a misaligned print or a bad cable does. The edges fill what
/// is left uncovered.
pub fn channel_split(image: &RgbImage, params: ChannelSplit) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let scale = scale_of(width);
    let offsets = [params.red, params.green, params.blue]
        .map(|(dx, dy)| (scaled_offset(dx, scale), scaled_offset(dy, scale)));
    let source = |coordinate: u32, delta: i32, size: u32| {
        (i64::from(coordinate) - i64::from(delta)).clamp(0, i64::from(size) - 1) as u32
    };
    parallel::from_fn(width, height, |x, y| {
        let mut pixel = Rgb([0; 3]);
        for (channel, &(dx, dy)) in offsets.iter().enumerate() {
            pixel[channel] = image.get_pixel(source(x, dx, width), source(y, dy, height))[channel];
        }
        pixel
    })
}

impl Spec for SliceShift {
    const NAME: &'static str = "slice";

    fn parse(params: &mut Params) -> Result<SliceShift, Error> {
        let d = SliceShift::default();
        Ok(SliceShift {
            slices: params.get("slices", d.slices)?,
            shift: params.get("shift", d.shift)?,
            height: params.pair("height", d.height)?,
            split: params.get("split", d.split)?,
            invert: params.get("invert", d.invert)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " slices={} shift={} height={} split={} invert={}",
            self.slices,
            self.shift,
            pair(self.height),
            self.split,
            self.invert
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(slice_shift(image, *self, seed))
    }
}

impl Spec for PixelStretch {
    const NAME: &'static str = "stretch";

    fn parse(params: &mut Params) -> Result<PixelStretch, Error> {
        let d = PixelStretch::default();
        Ok(PixelStretch {
            bands: params.get("bands", d.bands)?,
            length: params.pair("length", d.length)?,
            direction: params.get("direction", d.direction)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " bands={} length={} direction={}",
            self.bands,
            pair(self.length),
            self.direction
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(pixel_stretch(image, *self, seed))
    }
}

impl Spec for ChannelSplit {
    const NAME: &'static str = "split";

    fn parse(params: &mut Params) -> Result<ChannelSplit, Error> {
        let d = ChannelSplit::default();
        Ok(ChannelSplit {
            red: params.pair("red", d.red)?,
            green: params.pair("green", d.green)?,
            blue: params.pair("blue", d.blue)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " red={} green={} blue={}",
            pair(self.red),
            pair(self.green),
            pair(self.blue)
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(channel_split(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient, at the reference width so that sizes are what they
    /// say: a pixel tells where it came from.
    fn ramp() -> RgbImage {
        RgbImage::from_fn(1000, 40, |x, y| Rgb([(x / 4) as u8, (y * 6) as u8, 128]))
    }

    #[test]
    fn slices_move_and_wrap_around_following_the_seed() {
        let image = ramp();
        let params = SliceShift {
            slices: 3,
            shift: Size::new(30),
            height: (Size::new(4), Size::new(10)),
            split: false,
            invert: false,
        };
        let shifted = slice_shift(&image, params, 1);
        assert_eq!((shifted.width(), shifted.height()), (1000, 40));
        assert_ne!(shifted, image);
        assert_eq!(slice_shift(&image, params, 1), shifted);
        assert_ne!(slice_shift(&image, params, 2), shifted);
        // Every row is still a permutation of the ramp: nothing is lost.
        for y in 0..40 {
            let mut reds: Vec<u8> = (0..1000).map(|x| shifted.get_pixel(x, y)[0]).collect();
            reds.sort_unstable();
            assert_eq!(reds, (0..1000).map(|x| (x / 4) as u8).collect::<Vec<_>>());
        }
        assert_eq!(
            slice_shift(
                &image,
                SliceShift {
                    slices: 0,
                    ..params
                },
                1
            ),
            image
        );
    }

    #[test]
    fn a_stretched_line_repeats_over_the_next_ones() {
        let image = ramp();
        let params = PixelStretch {
            bands: 1,
            length: (Size::new(10), Size::new(10)),
            direction: Direction::Horizontal,
        };
        let stretched = pixel_stretch(&image, params, 5);
        // Find the stretched row: the first one equal to the one above.
        let line = (1..40)
            .find(|&y| {
                (0..1000).all(|x| stretched.get_pixel(x, y) == stretched.get_pixel(x, y - 1))
            })
            .expect("a stretched row")
            - 1;
        for y in line..(line + 10).min(40) {
            for x in 0..1000 {
                assert_eq!(stretched.get_pixel(x, y), image.get_pixel(x, line));
            }
        }
        if line + 10 < 40 {
            assert_eq!(
                stretched.get_pixel(0, line + 10),
                image.get_pixel(0, line + 10)
            );
        }
    }

    #[test]
    fn channels_come_apart_by_their_own_offsets() {
        let image = ramp();
        let split = channel_split(
            &image,
            ChannelSplit {
                red: (-5, 0),
                green: (0, 3),
                blue: (0, 0),
            },
        );
        // Red comes from 5 pixels to the right, green from 3 rows above.
        assert_eq!(split.get_pixel(500, 20)[0], image.get_pixel(505, 20)[0]);
        assert_eq!(split.get_pixel(500, 20)[1], image.get_pixel(500, 17)[1]);
        assert_eq!(split.get_pixel(500, 20)[2], image.get_pixel(500, 20)[2]);
        // At the edge, the last pixel fills in.
        assert_eq!(split.get_pixel(999, 20)[0], image.get_pixel(999, 20)[0]);
    }

    #[test]
    fn sizes_scale_with_the_picture() {
        // Twice the reference width: offsets double.
        let wide = RgbImage::from_fn(2000, 10, |x, _| Rgb([(x / 8) as u8, 0, 0]));
        let split = channel_split(
            &wide,
            ChannelSplit {
                red: (-10, 0),
                green: (0, 0),
                blue: (0, 0),
            },
        );
        assert_eq!(split.get_pixel(1000, 5)[0], wide.get_pixel(1020, 5)[0]);
    }
}
