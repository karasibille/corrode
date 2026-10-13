//! A turning smear: the picture seen while the camera turns round a
//! point, each pixel dragged along an arc.

use std::fmt;

use image::RgbImage;

use super::Error;
use super::film::bilinear;
use super::parallel;
use super::recipe::{Params, Spec};
use super::units::Percent;

/// Parameters of the spin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spin {
    /// How far the smear turns, in degrees: the picture is seen at every
    /// angle from half of it one way to half of it the other.
    pub angle: u16,
    /// The centre of the turn, as shares of the width and the height.
    pub center: (Percent, Percent),
    /// How many positions along the arc are blended: a few give separate
    /// ghosts, many a smooth smear; 0 takes as many as each pixel needs,
    /// a position every few pixels along its arc.
    pub steps: u8,
    /// How much of the smear shows over the sharp picture, 0 to 100.
    pub mix: Percent,
}

impl Default for Spin {
    fn default() -> Spin {
        Spin {
            angle: 10,
            center: (Percent::new(50), Percent::new(50)),
            steps: 0,
            mix: Percent::new(100),
        }
    }
}

/// The most positions a pixel blends when they are left to the effect.
const MAX_AUTO_STEPS: u32 = 96;
/// The distance between two positions along an arc, in pixels, when they
/// are left to the effect.
const AUTO_SPACING: f32 = 4.0;

/// Blends the picture turned at several angles round the centre.
pub fn spin(image: &RgbImage, params: Spin) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if params.steps == 1 || params.angle == 0 || params.mix.value() == 0 {
        return image.clone();
    }
    let cx = params.center.0.fraction() * (width as f32 - 1.0);
    let cy = params.center.1.fraction() * (height as f32 - 1.0);
    let sweep = f32::from(params.angle).to_radians();
    let mix = params.mix.fraction();
    parallel::from_fn(width, height, |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let steps = match params.steps {
            0 => {
                // The arc of a pixel is as long as its radius times the sweep.
                let arc = (dx * dx + dy * dy).sqrt() * sweep;
                ((arc / AUTO_SPACING).ceil() as u32).clamp(2, MAX_AUTO_STEPS)
            }
            fixed => u32::from(fixed),
        };
        // From half the sweep one way, turning by the same small angle
        // each time, which is a rotation added to the one before.
        let delta = sweep / (steps - 1) as f32;
        let (step_sin, step_cos) = delta.sin_cos();
        let (mut sin, mut cos) = (-sweep / 2.0).sin_cos();
        let mut sum = [0.0f32; 3];
        for _ in 0..steps {
            let seen = bilinear(image, cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
            for (total, channel) in sum.iter_mut().zip(seen) {
                *total += channel;
            }
            (sin, cos) = (
                sin * step_cos + cos * step_sin,
                cos * step_cos - sin * step_sin,
            );
        }
        let original = image.get_pixel(x, y).0;
        let mut pixel = [0u8; 3];
        for channel in 0..3 {
            let smeared = sum[channel] / steps as f32;
            let value =
                f32::from(original[channel]) + (smeared - f32::from(original[channel])) * mix;
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
        image::Rgb(pixel)
    })
}

impl Spec for Spin {
    const NAME: &'static str = "spin";

    fn parse(params: &mut Params) -> Result<Spin, Error> {
        let d = Spin::default();
        Ok(Spin {
            angle: params.get("angle", d.angle)?,
            center: params.pair("center", d.center)?,
            steps: params.get("steps", d.steps)?,
            mix: params.get("mix", d.mix)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " angle={} center={},{} steps={} mix={}",
            self.angle, self.center.0, self.center.1, self.steps, self.mix
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(spin(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A white bar from the centre to the right edge, on black.
    fn spoke() -> RgbImage {
        RgbImage::from_fn(101, 101, |x, y| {
            if x >= 50 && y.abs_diff(50) <= 1 {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        })
    }

    #[test]
    fn no_angle_no_steps_or_no_mix_leave_the_picture() {
        let image = spoke();
        for params in [
            Spin {
                angle: 0,
                ..Spin::default()
            },
            Spin {
                steps: 1,
                ..Spin::default()
            },
            Spin {
                mix: Percent::new(0),
                ..Spin::default()
            },
        ] {
            assert_eq!(spin(&image, params), image);
        }
    }

    #[test]
    fn a_flat_picture_stays_the_same_and_the_centre_stays_put() {
        let flat = RgbImage::from_pixel(60, 40, Rgb([90, 120, 200]));
        assert_eq!(spin(&flat, Spin::default()), flat);
        let image = spoke();
        let turned = spin(
            &image,
            Spin {
                angle: 40,
                ..Spin::default()
            },
        );
        assert_eq!(turned.get_pixel(50, 50), image.get_pixel(50, 50));
    }

    #[test]
    fn the_far_end_of_a_spoke_smears_more_than_its_root() {
        let image = spoke();
        let turned = spin(
            &image,
            Spin {
                angle: 30,
                steps: 32,
                ..Spin::default()
            },
        );
        // A pixel above the bar, at the root and at the far end.
        let near = turned.get_pixel(55, 47).0[0];
        let far = turned.get_pixel(95, 40).0[0];
        assert!(
            far > near,
            "the arc is longer far from the centre: {near} {far}"
        );
        // And the bar itself has lost some of its light at the end.
        assert!(turned.get_pixel(95, 50).0[0] < 255);
    }

    #[test]
    fn the_centre_can_be_moved() {
        let image = spoke();
        let params = Spin {
            angle: 40,
            center: (Percent::new(0), Percent::new(50)),
            ..Spin::default()
        };
        let turned = spin(&image, params);
        // Round the left edge, the bar is a long radius and smears at once.
        assert!(turned.get_pixel(80, 50).0[0] < 255);
    }
}
