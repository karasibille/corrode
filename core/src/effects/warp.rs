//! The picture seen through moving water: a field of smooth noise pushes
//! every pixel a little way, more here, less there.
//!
//! Sizes are in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide, as in the
//! glitch effects.

use std::fmt;

use image::{Rgb, RgbImage};

use super::Error;
use super::film::bilinear;
use super::parallel;
use super::random::Random;
use super::recipe::{Params, Spec};
use super::units::Size;

/// Parameters of the liquid distortion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liquid {
    /// How far a pixel can be pushed, in pixels for a picture
    /// [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
    pub amplitude: Size,
    /// Size of the waves, in the same pixels: the distance over which
    /// the push changes.
    pub scale: Size,
    /// Layers of finer waves added to the first, each half the size
    /// and half the strength: 1 is smooth, 3 is choppy.
    pub octaves: u32,
    /// How far the waves have drifted, in the same pixels: moving it
    /// over an animation makes the water flow.
    pub drift: Size,
}

impl Default for Liquid {
    fn default() -> Liquid {
        Liquid {
            amplitude: Size::new(40),
            scale: Size::new(150),
            octaves: 2,
            drift: Size::new(0),
        }
    }
}

/// Pushes every pixel by a field of smooth noise, in both directions:
/// the picture ripples. The seed shapes the waves.
pub fn liquid(image: &RgbImage, params: Liquid, seed: u64) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || params.scale.value() == 0 {
        return image.clone();
    }
    let amplitude = params.amplitude.on_f32(width);
    let wave = params.scale.on_f32(width);
    let drift = params.drift.on_f32(width);
    let octaves = params.octaves.clamp(1, 6);
    // One field for each direction, lattices large enough for the drift.
    let cells = ((width as f32 + drift) / wave).ceil() as usize + 2;
    let rows = (height as f32 / wave).ceil() as usize + 2;
    let field_x = Noise::new(seed, cells << octaves, rows << octaves);
    let field_y = Noise::new(seed.wrapping_add(0x5EED), cells << octaves, rows << octaves);
    parallel::from_fn(width, height, |x, y| {
        let (u, v) = ((x as f32 + drift) / wave, y as f32 / wave);
        let push = |field: &Noise| {
            let (mut sum, mut weight, mut total) = (0.0, 1.0, 0.0);
            for octave in 0..octaves {
                let k = (1 << octave) as f32;
                sum += (field.at(u * k, v * k) - 0.5) * 2.0 * weight;
                total += weight;
                weight /= 2.0;
            }
            sum / total * amplitude
        };
        let sample = bilinear(image, x as f32 + push(&field_x), y as f32 + push(&field_y));
        Rgb(sample.map(|v| v.round() as u8))
    })
}

/// Value noise: random values on a lattice, smoothly interpolated
/// between them.
struct Noise {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

impl Noise {
    fn new(seed: u64, width: usize, height: usize) -> Noise {
        let mut random = Random::new(seed);
        let values = (0..width * height)
            .map(|_| random.below(10_001) as f32 / 10_000.0)
            .collect();
        Noise {
            width,
            height,
            values,
        }
    }

    /// The noise at a point, 0 to 1, the lattice repeating beyond its
    /// edges.
    fn at(&self, u: f32, v: f32) -> f32 {
        let (u0, v0) = (u.floor(), v.floor());
        let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
        let (fu, fv) = (smooth(u - u0), smooth(v - v0));
        let value = |du: usize, dv: usize| {
            let i = (u0 as usize + du) % self.width;
            let j = (v0 as usize + dv) % self.height;
            self.values[j * self.width + i]
        };
        let top = value(0, 0) * (1.0 - fu) + value(1, 0) * fu;
        let bottom = value(0, 1) * (1.0 - fu) + value(1, 1) * fu;
        top * (1.0 - fv) + bottom * fv
    }
}

impl Spec for Liquid {
    const NAME: &'static str = "liquid";

    fn parse(params: &mut Params) -> Result<Liquid, Error> {
        let d = Liquid::default();
        Ok(Liquid {
            amplitude: params.get("amplitude", d.amplitude)?,
            scale: params.get("scale", d.scale)?,
            octaves: params.get("octaves", d.octaves)?,
            drift: params.get("drift", d.drift)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " amplitude={} scale={} octaves={} drift={}",
            self.amplitude, self.scale, self.octaves, self.drift
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(liquid(image, *self, seed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vertical stripes: a push sideways shows at once.
    fn stripes() -> RgbImage {
        RgbImage::from_fn(1000, 200, |x, _| {
            if (x / 20) % 2 == 0 {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        })
    }

    #[test]
    fn noise_is_smooth_and_within_range() {
        let noise = Noise::new(1, 8, 8);
        for i in 0..80 {
            let (u, v) = (i as f32 * 0.1, i as f32 * 0.07);
            let here = noise.at(u, v);
            assert!((0.0..=1.0).contains(&here));
            let near = noise.at(u + 0.01, v);
            assert!((here - near).abs() < 0.05, "{here} vs {near}");
        }
        // Repeats beyond the lattice.
        assert_eq!(noise.at(0.5, 0.5), noise.at(8.5, 8.5));
    }

    #[test]
    fn the_picture_ripples_the_same_way_for_a_seed() {
        let image = stripes();
        let params = Liquid {
            amplitude: Size::new(30),
            scale: Size::new(100),
            octaves: 2,
            drift: Size::new(0),
        };
        let rippled = liquid(&image, params, 3);
        assert_eq!((rippled.width(), rippled.height()), (1000, 200));
        let moved = rippled
            .pixels()
            .zip(image.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(moved > 20_000, "{moved}");
        assert_eq!(liquid(&image, params, 3), rippled);
        assert_ne!(liquid(&image, params, 4), rippled);
        assert_ne!(
            liquid(
                &image,
                Liquid {
                    drift: Size::new(50),
                    ..params
                },
                3
            ),
            rippled
        );
        let still = liquid(
            &image,
            Liquid {
                amplitude: Size::new(0),
                ..params
            },
            3,
        );
        assert_eq!(still, image);
    }
}
