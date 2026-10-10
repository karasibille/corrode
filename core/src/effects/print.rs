//! The look of print and screens: pictures reduced to dots of ink, to
//! two inks, or seen through the lines of a tube.
//!
//! Sizes are in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide, as in the
//! glitch effects.

use std::fmt;

use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

use super::Error;
use super::parallel;
use super::recipe::{Params, Spec};
use super::sort::brightness;
use super::units::{Percent, Size};

/// A colour written `rrggbb`, as on the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub Rgb<u8>);

impl std::str::FromStr for Color {
    type Err = String;

    fn from_str(text: &str) -> Result<Color, String> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        match (hex.len(), channel(0), channel(2), channel(4)) {
            (6, Some(r), Some(g), Some(b)) => Ok(Color(Rgb([r, g, b]))),
            _ => Err(format!("'{text}' is not a colour (rrggbb)")),
        }
    }
}

impl std::fmt::Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [r, g, b] = self.0.0;
        write!(f, "{r:02x}{g:02x}{b:02x}")
    }
}

/// How a picture is reduced to black and white dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DitherMethod {
    /// Error diffusion (Floyd–Steinberg): dots scattered to keep the
    /// tones, the look of a fax or an old Mac.
    #[default]
    Diffusion,
    /// An ordered 8×8 matrix (Bayer): a regular pattern, the look of a
    /// newspaper or an early game.
    Bayer,
}

impl std::str::FromStr for DitherMethod {
    type Err = String;

    fn from_str(text: &str) -> Result<DitherMethod, String> {
        match text {
            "diffusion" | "fs" => Ok(DitherMethod::Diffusion),
            "bayer" | "ordered" => Ok(DitherMethod::Bayer),
            other => Err(format!("unknown method '{other}' (diffusion or bayer)")),
        }
    }
}

impl std::fmt::Display for DitherMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DitherMethod::Diffusion => "diffusion",
            DitherMethod::Bayer => "bayer",
        })
    }
}

/// Parameters of the dithering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dither {
    pub method: DitherMethod,
    /// Size of a dot, in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide:
    /// the picture is reduced by it before the dots are placed, and
    /// each dot is drawn that big.
    pub size: Size,
    /// Brightness, 0 to 255, that comes out mid-grey: lower brightens
    /// the result, higher darkens it.
    pub mid: u8,
}

impl Default for Dither {
    fn default() -> Dither {
        Dither {
            method: DitherMethod::Diffusion,
            size: Size::new(2),
            mid: 128,
        }
    }
}

/// Reduces the picture to black and white dots.
pub fn dither(image: &RgbImage, params: Dither) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let dot = params.size.on(width).max(1);
    let (small_width, small_height) = ((width / dot).max(1), (height / dot).max(1));
    let small = if dot > 1 {
        DynamicImage::ImageRgb8(image.clone())
            .resize_exact(small_width, small_height, FilterType::Triangle)
            .to_rgb8()
    } else {
        image.clone()
    };
    // Brightness with the mid-point moved to 128, as values 0 to 255.
    let shift = 128.0 - f32::from(params.mid);
    let mut light: Vec<f32> = small
        .pixels()
        .map(|&pixel| (f32::from(brightness(pixel)) + shift).clamp(0.0, 255.0))
        .collect();
    let (sw, sh) = (small_width as usize, small_height as usize);
    let mut white = vec![false; sw * sh];
    match params.method {
        DitherMethod::Bayer => {
            for y in 0..sh {
                for x in 0..sw {
                    let threshold = (f32::from(BAYER[y % 8][x % 8]) + 0.5) * 4.0;
                    white[y * sw + x] = light[y * sw + x] >= threshold;
                }
            }
        }
        DitherMethod::Diffusion => {
            for y in 0..sh {
                for x in 0..sw {
                    let old = light[y * sw + x];
                    let new = if old >= 128.0 { 255.0 } else { 0.0 };
                    white[y * sw + x] = new > 0.0;
                    let error = old - new;
                    let mut spread = |dx: isize, dy: usize, share: f32| {
                        let nx = x as isize + dx;
                        if nx >= 0 && (nx as usize) < sw && y + dy < sh {
                            light[(y + dy) * sw + nx as usize] += error * share;
                        }
                    };
                    spread(1, 0, 7.0 / 16.0);
                    spread(-1, 1, 3.0 / 16.0);
                    spread(0, 1, 5.0 / 16.0);
                    spread(1, 1, 1.0 / 16.0);
                }
            }
        }
    }
    let ink = |on: bool| {
        if on {
            Rgb([255, 255, 255])
        } else {
            Rgb([0, 0, 0])
        }
    };
    parallel::from_fn(width, height, |x, y| {
        let (sx, sy) = (
            ((x / dot) as usize).min(sw - 1),
            ((y / dot) as usize).min(sh - 1),
        );
        ink(white[sy * sw + sx])
    })
}

/// The 8×8 Bayer matrix, thresholds 0 to 63.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Parameters of the duotone: the inks the shadows and the lights are
/// printed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duotone {
    pub dark: Color,
    pub light: Color,
}

impl Default for Duotone {
    fn default() -> Duotone {
        Duotone {
            dark: Color(Rgb([0, 0, 0])),
            light: Color(Rgb([255, 79, 163])),
        }
    }
}

/// Prints the picture with two inks: its brightness goes from the dark
/// one to the light one.
pub fn duotone(image: &RgbImage, params: Duotone) -> RgbImage {
    let (dark, light) = (params.dark.0.0, params.light.0.0);
    let mut printed = image.clone();
    parallel::for_each_pixel(&mut printed, |_, _, pixel| {
        let t = f32::from(brightness(*pixel)) / 255.0;
        for channel in 0..3 {
            let value = f32::from(dark[channel])
                + (f32::from(light[channel]) - f32::from(dark[channel])) * t;
            pixel[channel] = value.round() as u8;
        }
    });
    printed
}

/// Parameters of the scanlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scanlines {
    /// Rows from one line to the next, in pixels for a picture
    /// [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
    pub period: Size,
    /// Rows a line is thick, in the same pixels.
    pub thickness: Size,
    /// How dark the lines are, 0 to 100: 100 is black.
    pub strength: Percent,
}

impl Default for Scanlines {
    fn default() -> Scanlines {
        Scanlines {
            period: Size::new(4),
            thickness: Size::new(1),
            strength: Percent::new(60),
        }
    }
}

/// Darkens rows at a regular period, as the lines of a tube screen.
pub fn scanlines(image: &RgbImage, params: Scanlines) -> RgbImage {
    let width = image.width();
    let period = params.period.on(width).max(1);
    let thickness = params.thickness.on(width).clamp(1, period);
    let keep = 1.0 - params.strength.fraction();
    let mut lined = image.clone();
    parallel::for_each_pixel(&mut lined, |_, y, pixel| {
        if y % period < thickness {
            *pixel = Rgb(pixel.0.map(|v| (f32::from(v) * keep).round() as u8));
        }
    });
    lined
}

impl Spec for Dither {
    const NAME: &'static str = "dither";

    fn parse(params: &mut Params) -> Result<Dither, Error> {
        let d = Dither::default();
        Ok(Dither {
            method: params.get("method", d.method)?,
            size: params.get("size", d.size)?,
            mid: params.get("mid", d.mid)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " method={} size={} mid={}",
            self.method, self.size, self.mid
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(dither(image, *self))
    }
}

impl Spec for Duotone {
    const NAME: &'static str = "duotone";

    fn parse(params: &mut Params) -> Result<Duotone, Error> {
        let d = Duotone::default();
        Ok(Duotone {
            dark: params.get("dark", d.dark)?,
            light: params.get("light", d.light)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, " dark={} light={}", self.dark, self.light)
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(duotone(image, *self))
    }
}

impl Spec for Scanlines {
    const NAME: &'static str = "scanlines";

    fn parse(params: &mut Params) -> Result<Scanlines, Error> {
        let d = Scanlines::default();
        Ok(Scanlines {
            period: params.get("period", d.period)?,
            thickness: params.get("thickness", d.thickness)?,
            strength: params.get("strength", d.strength)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " period={} thickness={} strength={}",
            self.period, self.thickness, self.strength
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(scanlines(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(level: u8, width: u32, height: u32) -> RgbImage {
        RgbImage::from_pixel(width, height, Rgb([level; 3]))
    }

    fn white_share(image: &RgbImage) -> f64 {
        let white = image.pixels().filter(|p| p[0] == 255).count();
        white as f64 / (image.width() * image.height()) as f64
    }

    #[test]
    fn dots_keep_the_tone_of_a_grey() {
        for method in [DitherMethod::Diffusion, DitherMethod::Bayer] {
            let dotted = dither(
                &grey(64, 1000, 64),
                Dither {
                    method,
                    size: Size::new(1),
                    mid: 128,
                },
            );
            let share = white_share(&dotted);
            assert!((0.2..0.3).contains(&share), "{method}: {share}");
            assert!(dotted.pixels().all(|p| p[0] == 0 || p[0] == 255));
        }
        // Moving the mid-point brightens or darkens.
        let brighter = dither(
            &grey(64, 1000, 64),
            Dither {
                mid: 64,
                size: Size::new(1),
                ..Dither::default()
            },
        );
        assert!((0.45..0.55).contains(&white_share(&brighter)));
    }

    #[test]
    fn dots_grow_with_the_size() {
        let dotted = dither(
            &grey(128, 1000, 40),
            Dither {
                size: Size::new(5),
                ..Dither::default()
            },
        );
        assert_eq!((dotted.width(), dotted.height()), (1000, 40));
        // Every 5×5 block is one dot.
        for by in 0..8 {
            for bx in 0..200 {
                let first = dotted.get_pixel(bx * 5, by * 5);
                for y in 0..5 {
                    for x in 0..5 {
                        assert_eq!(dotted.get_pixel(bx * 5 + x, by * 5 + y), first);
                    }
                }
            }
        }
    }

    #[test]
    fn two_inks_span_the_tones() {
        let params = Duotone {
            dark: "102030".parse().unwrap(),
            light: "#ff4fa3".parse().unwrap(),
        };
        assert_eq!(
            duotone(&grey(0, 2, 2), params).get_pixel(0, 0),
            &Rgb([16, 32, 48])
        );
        assert_eq!(
            duotone(&grey(255, 2, 2), params).get_pixel(0, 0),
            &Rgb([255, 79, 163])
        );
        let mid = duotone(&grey(128, 2, 2), params).get_pixel(0, 0).0;
        assert!(mid[0] > 120 && mid[0] < 145, "{mid:?}");
        assert_eq!(params.light.to_string(), "ff4fa3");
        assert!("ff4f".parse::<Color>().is_err());
        assert!("gg0000".parse::<Color>().is_err());
    }

    #[test]
    fn lines_darken_rows_at_their_period() {
        let lined = scanlines(
            &grey(200, 1000, 12),
            Scanlines {
                period: Size::new(4),
                thickness: Size::new(1),
                strength: Percent::new(50),
            },
        );
        for y in 0..12 {
            let expected = if y % 4 == 0 { 100 } else { 200 };
            assert_eq!(lined.get_pixel(0, y)[0], expected, "row {y}");
        }
    }
}
