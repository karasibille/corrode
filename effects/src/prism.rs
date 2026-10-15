//! Light split into the colours of a spectrum, as through a prism or a
//! lens flare: a band whose hue changes across it, straight or round a
//! point, uneven and screened over the picture.

use std::fmt;
use std::str::FromStr;

use image::RgbImage;

use super::Error;
use super::colour::from_hsv;
use super::parallel;
use super::recipe::{Params, Spec};
use super::units::{Percent, Size, long_side};
use super::warp::Noise;

/// Which way the spectrum runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrismKind {
    /// Straight bands, the spectrum running along `angle`.
    #[default]
    Linear,
    /// Rings round `center`, the spectrum running outwards.
    Radial,
}

impl FromStr for PrismKind {
    type Err = String;

    fn from_str(text: &str) -> Result<PrismKind, String> {
        match text {
            "linear" => Ok(PrismKind::Linear),
            "radial" => Ok(PrismKind::Radial),
            other => Err(format!("unknown kind '{other}' (linear or radial)")),
        }
    }
}

impl fmt::Display for PrismKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PrismKind::Linear => "linear",
            PrismKind::Radial => "radial",
        })
    }
}

/// Parameters of the prism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prism {
    pub kind: PrismKind,
    /// Direction the spectrum runs along for straight bands, in degrees,
    /// 0 from the left to the right and 90 downwards.
    pub angle: u16,
    /// The centre of the rings, as shares of the width and the height.
    pub center: (Percent, Percent),
    /// Where the spectrum starts, 0 to 100 of the width, from the left
    /// edge (or the corner, along `angle`) or from the centre.
    pub position: Percent,
    /// How wide the whole spectrum is, 0 to 100 of the width.
    pub size: Percent,
    /// The hue it starts with, in degrees: 0 red, 120 green, 240 blue.
    pub hue: u16,
    /// How many degrees of hue it goes through; negative goes the other
    /// way round the wheel.
    pub range: i16,
    /// How strong the light is, 0 to 100.
    pub strength: Percent,
    /// How much of the core is turned towards white, 0 to 100.
    pub white: Percent,
    /// How uneven it is: the band bends and its light changes from place
    /// to place, 0 to 100.
    pub wobble: Percent,
}

impl Default for Prism {
    fn default() -> Prism {
        Prism {
            kind: PrismKind::Linear,
            angle: 0,
            center: (Percent::new(50), Percent::new(100)),
            position: Percent::new(30),
            size: Percent::new(40),
            hue: 220,
            range: -200,
            strength: Percent::new(80),
            white: Percent::new(35),
            wobble: Percent::new(0),
        }
    }
}

/// Size, for a picture 1000 pixels wide, of the patches the band bends
/// and fades along.
const PRISM_PATCH: u32 = 300;

/// Lays a band of spectrum over the picture. The seed draws how it bends.
pub fn prism(image: &RgbImage, params: Prism, seed: u64) -> RgbImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    if width == 0.0 || height == 0.0 || params.size.value() == 0 {
        return image.clone();
    }
    let (sin, cos) = f32::from(params.angle).to_radians().sin_cos();
    // Where a pixel stands along the direction, from 0 at the corner the
    // bands start from, in widths of the picture.
    let corners = [0.0, width * cos, height * sin, width * cos + height * sin];
    let start = corners.iter().copied().fold(f32::MAX, f32::min);
    let (cx, cy) = (
        params.center.0.fraction() * width,
        params.center.1.fraction() * height,
    );
    let wobble = params.wobble.fraction();
    let cell = Size::new(PRISM_PATCH).on_f32(long_side(image)).max(1.0);
    let lattice = |seed: u64| {
        Noise::new(
            seed,
            (width / cell).ceil() as usize + 2,
            (height / cell).ceil() as usize + 2,
        )
    };
    let (bend, fade) = (
        lattice(seed.wrapping_add(0xB3ED)),
        lattice(seed.wrapping_add(0xFA11)),
    );
    let first = params.position.fraction();
    let span = params.size.fraction();
    let strength = params.strength.fraction();
    let white = params.white.fraction();
    let mut lit = image.clone();
    parallel::for_each_pixel(&mut lit, |x, y, pixel| {
        let (px, py) = (x as f32, y as f32);
        let along = match params.kind {
            PrismKind::Linear => (px * cos + py * sin - start) / width,
            PrismKind::Radial => ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() / width,
        };
        let (u, v) = (px / cell, py / cell);
        let moved = (bend.at(u, v) - 0.5) * 2.0 * wobble * span * 0.8;
        let t = (along - first - moved) / span;
        if !(0.0..1.0).contains(&t) {
            return;
        }
        // Brightest in the middle of the band, soft at both ends.
        let bump = (t * std::f32::consts::PI).sin().powf(0.6);
        let light = bump * (1.0 + (fade.at(u, v) - 0.5) * 2.0 * wobble).max(0.0);
        let amount = (light * strength).clamp(0.0, 1.0);
        let hue = f32::from(params.hue) + t * f32::from(params.range);
        let [r, g, b] = from_hsv([hue, 0.85 * (1.0 - white * bump * bump), 1.0]);
        let color = [r, g, b].map(|channel| channel * 255.0);
        for channel in 0..3 {
            let below = f32::from(pixel[channel]);
            let value = 255.0 - (255.0 - below) * (255.0 - color[channel] * amount) / 255.0;
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
    });
    lit
}

impl Spec for Prism {
    const NAME: &'static str = "prism";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        (
            "kind",
            "`linear` (straight bands) or `radial` (rings round a centre)",
        ),
        (
            "angle",
            "direction the spectrum runs along for straight bands, in degrees, 0 to the right, 90 down",
        ),
        (
            "center",
            "`x,y` centre of the rings, in percent of the width and height",
        ),
        (
            "position",
            "where the spectrum starts, 0 to 100 of the width",
        ),
        (
            "size",
            "how wide the whole spectrum is, 0 to 100 of the width",
        ),
        (
            "hue",
            "hue it starts with, in degrees: 0 red, 120 green, 240 blue",
        ),
        (
            "range",
            "degrees of hue it goes through, negative the other way round",
        ),
        ("strength", "how strong the light is, 0 to 100"),
        (
            "white",
            "how much of the core turns towards white, 0 to 100",
        ),
        (
            "wobble",
            "how uneven it is: bending and fading from place to place, 0 to 100",
        ),
    ];

    fn parse(params: &mut Params) -> Result<Prism, Error> {
        let d = Prism::default();
        Ok(Prism {
            kind: params.get("kind", d.kind)?,
            angle: params.get("angle", d.angle)?,
            center: params.pair("center", d.center)?,
            position: params.get("position", d.position)?,
            size: params.get("size", d.size)?,
            hue: params.get("hue", d.hue)?,
            range: params.get("range", d.range)?,
            strength: params.get("strength", d.strength)?,
            white: params.get("white", d.white)?,
            wobble: params.get("wobble", d.wobble)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " kind={} angle={} center={},{} position={} size={} hue={} range={} strength={} white={} wobble={}",
            self.kind,
            self.angle,
            self.center.0,
            self.center.1,
            self.position,
            self.size,
            self.hue,
            self.range,
            self.strength,
            self.white,
            self.wobble
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(prism(image, *self, seed))
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    fn black() -> RgbImage {
        RgbImage::from_pixel(300, 100, Rgb([0; 3]))
    }

    #[test]
    fn a_band_runs_through_the_hues_and_leaves_the_rest() {
        let params = Prism {
            position: Percent::new(20),
            size: Percent::new(40),
            wobble: Percent::new(0),
            white: Percent::new(0),
            strength: Percent::new(100),
            hue: 0,
            range: 240,
            ..Prism::default()
        };
        let lit = prism(&black(), params, 1);
        // Nothing before the band (0-20 %) nor after it (60 % on).
        assert_eq!(lit.get_pixel(30, 50), &Rgb([0; 3]));
        assert_eq!(lit.get_pixel(250, 50), &Rgb([0; 3]));
        // Red-ish early in the band, blue-ish late in it.
        let (early, late) = (lit.get_pixel(70, 50), lit.get_pixel(170, 50));
        assert!(early[0] > early[2], "{early:?}");
        assert!(late[2] > late[0], "{late:?}");
        // Straight bands without wobble: the same from top to bottom.
        assert_eq!(lit.get_pixel(120, 0), lit.get_pixel(120, 99));
        assert_eq!(prism(&black(), params, 1), lit);
    }

    #[test]
    fn rings_have_the_same_light_all_round_the_centre() {
        let params = Prism {
            kind: PrismKind::Radial,
            center: (Percent::new(50), Percent::new(50)),
            position: Percent::new(5),
            size: Percent::new(30),
            wobble: Percent::new(0),
            ..Prism::default()
        };
        let lit = prism(&black(), params, 1);
        assert_eq!(lit.get_pixel(150 + 60, 50), lit.get_pixel(150 - 60, 50));
        assert_ne!(lit.get_pixel(150 + 60, 50), &Rgb([0; 3]));
    }
}
