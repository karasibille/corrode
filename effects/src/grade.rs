//! Colour grading: the colours of a picture moved on purpose. One colour
//! kept in a grey picture, the greys printed with three inks, the
//! channels swapped, the hues turned.

use std::fmt;
use std::str::FromStr;

use image::{Rgb, RgbImage};

use super::Error;
use super::colour::{Color, Hue, from_hsv, to_hsv};
use super::mask::SoftMap;
use super::parallel;
use super::recipe::{Params, Spec};
use super::sort::brightness;
use super::units::{Percent, Point};

/// 0 up to `from`, 1 from `to` on, smooth in between.
pub(super) fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Parameters of one point colour: the colour that stays, in a picture
/// that goes grey.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoint {
    /// The hue kept.
    pub hue: Hue,
    /// How far from it, in degrees either side, colours are kept whole.
    pub width: u16,
    /// Over how many more degrees the colour fades to grey.
    pub soft: u16,
    /// What is left of the colour of everything else, 0 to 100.
    pub rest: Percent,
    /// A point of the picture whose hue is the one kept, instead of
    /// `hue`; it is also used when the point itself is grey.
    pub at: Point,
    /// How soft the edge of the kept zone is, as a percent of the long
    /// side of the picture: the zone is judged on a blurred small copy,
    /// so that its edge is not a line and a speck of another colour
    /// inside it is kept with the rest. 0 judges every pixel by itself.
    pub smooth: f32,
}

impl Default for OnePoint {
    fn default() -> OnePoint {
        OnePoint {
            hue: Hue::new(0.0),
            width: 30,
            soft: 30,
            rest: Percent::new(0),
            at: Point::default(),
            smooth: 1.0,
        }
    }
}

/// The hue of the picture around a point, the mean colour of a small
/// square, or `None` where it is grey.
fn hue_at(image: &RgbImage, x: Percent, y: Percent) -> Option<Hue> {
    let (width, height) = (image.width(), image.height());
    let centre = |share: Percent, size: u32| (share.fraction() * (size - 1) as f32).round() as i64;
    let (cx, cy) = (centre(x, width), centre(y, height));
    let reach = (width.max(height) / 400).max(2) as i64;
    let mut sum = [0.0f32; 3];
    let mut count = 0.0;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let px = (cx + dx).clamp(0, i64::from(width) - 1) as u32;
            let py = (cy + dy).clamp(0, i64::from(height) - 1) as u32;
            for (total, channel) in sum.iter_mut().zip(image.get_pixel(px, py).0) {
                *total += f32::from(channel);
            }
            count += 1.0;
        }
    }
    Color(Rgb(sum.map(|total| byte(total / count)))).hue()
}

/// Keeps one colour, and takes the colour out of the rest: pixels whose
/// hue is within `width` degrees of the one kept stay as they are, the
/// others fade to grey over `soft` degrees, as far as `rest` leaves.
pub fn one_point(image: &RgbImage, params: OnePoint) -> RgbImage {
    let target = params
        .at
        .0
        .and_then(|(x, y)| hue_at(image, x, y))
        .unwrap_or(params.hue);
    let width = f32::from(params.width);
    let soft = f32::from(params.soft).max(1.0);
    let rest = params.rest.fraction();
    // How much of its colour a colour keeps, by its hue: all of it near the
    // hue kept, fading away from it, and none for a grey, whose hue means
    // nothing.
    let kept_of = |rgb: [f32; 3]| {
        let [hue, saturation, _] = to_hsv(rgb.map(|channel| channel / 255.0));
        let near = 1.0 - smoothstep(width, width + soft, Hue::new(hue).distance(target));
        near * smoothstep(0.04, 0.12, saturation)
    };
    // The zone is judged on a blurred small copy, not pixel by pixel.
    let zone = (params.smooth > 0.0)
        .then(|| SoftMap::build(image, params.smooth, |rgb| [kept_of(rgb), 0.0]));
    let mut result = image.clone();
    parallel::for_each_pixel(&mut result, |x, y, pixel| {
        let rgb = pixel.0.map(f32::from);
        let grey = f32::from(brightness(*pixel));
        let kept = match &zone {
            Some(zone) => zone.at(x, y)[0],
            None => kept_of(rgb),
        };
        for channel in 0..3 {
            let faded = grey + (rgb[channel] - grey) * rest;
            pixel[channel] = byte(faded + (rgb[channel] - faded) * kept);
        }
    });
    result
}

impl Spec for OnePoint {
    const NAME: &'static str = "onepoint";

    fn parse(params: &mut Params) -> Result<OnePoint, Error> {
        let d = OnePoint::default();
        Ok(OnePoint {
            hue: params.get("hue", d.hue)?,
            width: params.get("width", d.width)?,
            soft: params.get("soft", d.soft)?,
            rest: params.get("rest", d.rest)?,
            at: params.get("at", d.at)?,
            smooth: params.get::<f32>("smooth", d.smooth)?.clamp(0.0, 10.0),
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " hue={} width={} soft={} rest={} at={} smooth={}",
            self.hue, self.width, self.soft, self.rest, self.at, self.smooth
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(one_point(image, *self))
    }
}

/// Parameters of the tritone: the three inks the shadows, the mid tones
/// and the lights are printed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tritone {
    pub dark: Color,
    pub mid: Color,
    pub light: Color,
    /// How much of the print shows over the picture, 0 to 100.
    pub mix: Percent,
}

impl Default for Tritone {
    fn default() -> Tritone {
        Tritone {
            dark: Color(Rgb([20, 5, 43])),
            mid: Color(Rgb([224, 32, 79])),
            light: Color(Rgb([255, 217, 160])),
            mix: Percent::new(100),
        }
    }
}

/// Prints the picture with three inks: its brightness goes from the dark
/// one through the middle one to the light one.
pub fn tritone(image: &RgbImage, params: Tritone) -> RgbImage {
    let ink = |color: Color| color.0.0.map(f32::from);
    let (dark, mid, light) = (ink(params.dark), ink(params.mid), ink(params.light));
    let mix = params.mix.fraction();
    let mut printed = image.clone();
    parallel::for_each_pixel(&mut printed, |_, _, pixel| {
        let t = f32::from(brightness(*pixel)) / 255.0;
        let (from, to, along) = if t < 0.5 {
            (dark, mid, t * 2.0)
        } else {
            (mid, light, (t - 0.5) * 2.0)
        };
        for channel in 0..3 {
            let ink = from[channel] + (to[channel] - from[channel]) * along;
            let original = f32::from(pixel[channel]);
            pixel[channel] = byte(original + (ink - original) * mix);
        }
    });
    printed
}

impl Spec for Tritone {
    const NAME: &'static str = "tritone";

    fn parse(params: &mut Params) -> Result<Tritone, Error> {
        let d = Tritone::default();
        Ok(Tritone {
            dark: params.get("dark", d.dark)?,
            mid: params.get("mid", d.mid)?,
            light: params.get("light", d.light)?,
            mix: params.get("mix", d.mix)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " dark={} mid={} light={} mix={}",
            self.dark, self.mid, self.light, self.mix
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(tritone(image, *self))
    }
}

/// Which channel each channel is taken from, written as the three
/// letters `r`, `g` and `b` in some order: `bgr` swaps red and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelOrder([usize; 3]);

impl Default for ChannelOrder {
    fn default() -> ChannelOrder {
        // Red and blue swapped, the look of infrared film.
        ChannelOrder([2, 1, 0])
    }
}

impl FromStr for ChannelOrder {
    type Err = String;

    fn from_str(text: &str) -> Result<ChannelOrder, String> {
        let letters: Vec<char> = text.to_lowercase().chars().collect();
        let index = |c: char| "rgb".find(c);
        let order = match letters.as_slice() {
            [a, b, c] => index(*a)
                .zip(index(*b))
                .zip(index(*c))
                .map(|((a, b), c)| [a, b, c]),
            _ => None,
        };
        match order {
            Some([a, b, c]) if a != b && b != c && a != c => Ok(ChannelOrder([a, b, c])),
            _ => Err(format!(
                "'{text}' is not an order of the channels (rgb, bgr, gbr…)"
            )),
        }
    }
}

impl fmt::Display for ChannelOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|&i| write!(f, "{}", ["r", "g", "b"][i]))
    }
}

/// Some of the three channels, written as their letters (`rb`), or
/// `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Channels([bool; 3]);

impl FromStr for Channels {
    type Err = String;

    fn from_str(text: &str) -> Result<Channels, String> {
        let lower = text.to_lowercase();
        if lower == "none" {
            return Ok(Channels::default());
        }
        let mut set = [false; 3];
        for c in lower.chars() {
            match "rgb".find(c) {
                Some(i) => set[i] = true,
                None => {
                    return Err(format!(
                        "'{text}' is not some channels (r, g, b, rb… or none)"
                    ));
                }
            }
        }
        if lower.is_empty() {
            return Err(format!(
                "'{text}' is not some channels (r, g, b, rb… or none)"
            ));
        }
        Ok(Channels(set))
    }
}

impl fmt::Display for Channels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == [false; 3] {
            return f.write_str("none");
        }
        for (set, letter) in self.0.iter().zip(["r", "g", "b"]) {
            if *set {
                f.write_str(letter)?;
            }
        }
        Ok(())
    }
}

/// A hue, or no hue at all: `none` or a hue as `Hue` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MaybeHue(pub Option<Hue>);

impl FromStr for MaybeHue {
    type Err = String;

    fn from_str(text: &str) -> Result<MaybeHue, String> {
        if text.trim().eq_ignore_ascii_case("none") {
            return Ok(MaybeHue(None));
        }
        text.parse().map(|hue| MaybeHue(Some(hue)))
    }
}

impl fmt::Display for MaybeHue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(hue) => write!(f, "{hue}"),
            None => f.write_str("none"),
        }
    }
}

/// Parameters of the false colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FalseColor {
    /// Where each channel comes from.
    pub order: ChannelOrder,
    /// Channels turned negative after the swap.
    pub invert: Channels,
    /// The hues turned by this many degrees, -180 to 180.
    pub rotate: i16,
    /// How much colour there is, in percent: 100 leaves it, 0 takes it
    /// all out, 200 doubles it. A pale colour, a grey sky, has little to
    /// turn until it is given more.
    pub saturation: u16,
    /// Only the colours of this hue are moved, the others left as they
    /// are; `none` moves them all.
    pub only: MaybeHue,
    /// How far from `only`, in degrees either side, colours are moved
    /// whole, and over how many more degrees the move fades out.
    pub width: u16,
    pub soft: u16,
    /// How soft the edge of the range is, as a percent of the long side
    /// of the picture: the zone of a hue is worked out on a small copy
    /// and blurred by this much, so that its edge is not a line and a
    /// speck of another colour inside it is moved with the rest. 0 judges
    /// every pixel by itself.
    pub smooth: f32,
    /// How much of the result shows over the picture, 0 to 100.
    pub mix: Percent,
}

impl Default for FalseColor {
    fn default() -> FalseColor {
        FalseColor {
            order: ChannelOrder::default(),
            invert: Channels::default(),
            rotate: 0,
            saturation: 100,
            only: MaybeHue::default(),
            width: 30,
            soft: 30,
            smooth: 1.0,
            mix: Percent::new(100),
        }
    }
}

/// Moves the colours without touching the light: the channels swapped,
/// some turned negative, the hues turned around the colour wheel, the
/// colour made stronger or weaker.
pub fn false_color(image: &RgbImage, params: FalseColor) -> RgbImage {
    let mix = params.mix.fraction();
    let rotate = f32::from(params.rotate);
    let saturation = f32::from(params.saturation) / 100.0;
    let (width, soft) = (f32::from(params.width), f32::from(params.soft).max(1.0));
    // How much of the move a colour gets, by its hue: whole near the hue
    // asked for, fading away from it, and none for a grey, which has no
    // hue to be near.
    let share_of = |only: Hue, rgb: [f32; 3]| {
        let [hue, strength, _] = to_hsv(rgb.map(|c| c / 255.0));
        let near = 1.0 - smoothstep(width, width + soft, Hue::new(hue).distance(only));
        near * smoothstep(0.02, 0.06, strength)
    };
    // The share is judged on a blurred small copy, not pixel by pixel.
    let zone = params
        .only
        .0
        .filter(|_| params.smooth > 0.0)
        .map(|only| SoftMap::build(image, params.smooth, |rgb| [share_of(only, rgb), 0.0]));
    let mut result = image.clone();
    parallel::for_each_pixel(&mut result, |x, y, pixel| {
        let original = pixel.0.map(f32::from);
        let share = match (params.only.0, &zone) {
            (None, _) => 1.0,
            (Some(_), Some(zone)) => zone.at(x, y)[0],
            (Some(only), None) => share_of(only, original),
        };
        let mix = mix * share;
        let mut moved = [0.0f32; 3];
        for channel in 0..3 {
            let value = original[params.order.0[channel]];
            moved[channel] = if params.invert.0[channel] {
                255.0 - value
            } else {
                value
            };
        }
        if rotate != 0.0 || saturation != 1.0 {
            let [hue, strength, value] = to_hsv(moved.map(|c| c / 255.0));
            let strength = (strength * saturation).min(1.0);
            moved = from_hsv([hue + rotate, strength, value]).map(|c| c * 255.0);
        }
        for channel in 0..3 {
            pixel[channel] = byte(original[channel] + (moved[channel] - original[channel]) * mix);
        }
    });
    result
}

impl Spec for FalseColor {
    const NAME: &'static str = "falsecolor";

    fn parse(params: &mut Params) -> Result<FalseColor, Error> {
        let d = FalseColor::default();
        let rotate: i16 = params.get("rotate", d.rotate)?;
        Ok(FalseColor {
            order: params.get("order", d.order)?,
            invert: params.get("invert", d.invert)?,
            rotate: rotate.clamp(-180, 180),
            saturation: params.get::<u16>("saturation", d.saturation)?.min(400),
            only: params.get("only", d.only)?,
            width: params.get("width", d.width)?,
            soft: params.get("soft", d.soft)?,
            smooth: params.get::<f32>("smooth", d.smooth)?.clamp(0.0, 10.0),
            mix: params.get("mix", d.mix)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " order={} invert={} rotate={} saturation={} only={} width={} soft={} smooth={} mix={}",
            self.order,
            self.invert,
            self.rotate,
            self.saturation,
            self.only,
            self.width,
            self.soft,
            self.smooth,
            self.mix
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(false_color(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(rgb: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(8, 8, Rgb(rgb))
    }

    /// Left half red, right half blue.
    fn red_and_blue() -> RgbImage {
        RgbImage::from_fn(8, 4, |x, _| {
            Rgb(if x < 4 { [220, 30, 30] } else { [30, 30, 220] })
        })
    }

    #[test]
    fn one_point_keeps_the_hue_and_greys_the_rest() {
        let result = one_point(&red_and_blue(), OnePoint::default());
        assert_eq!(result.get_pixel(0, 0).0, [220, 30, 30], "red is kept");
        let [r, g, b] = result.get_pixel(7, 0).0;
        assert!(
            r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1,
            "blue goes grey: {r} {g} {b}"
        );
    }

    #[test]
    fn a_speck_inside_the_kept_zone_is_kept_with_it_when_smoothed() {
        // A red zone with a small blue speck: alone, the speck goes grey;
        // smoothed, it is kept with the zone it is in.
        let mut image = RgbImage::from_pixel(300, 300, Rgb([220, 30, 30]));
        for (x, y) in [(150, 150), (151, 150), (150, 151), (151, 151)] {
            image.put_pixel(x, y, Rgb([30, 30, 220]));
        }
        let keep = |smooth: f32| OnePoint {
            smooth,
            ..OnePoint::default()
        };
        let sharp = one_point(&image, keep(0.0));
        let soft = one_point(&image, keep(3.0));
        assert_ne!(sharp.get_pixel(150, 150), image.get_pixel(150, 150));
        let [r, _, b] = soft.get_pixel(150, 150).0;
        assert!(b > 200 && r < 40, "kept as blue: {r} {b}");
        // The zone itself is kept either way.
        assert_eq!(soft.get_pixel(10, 10), image.get_pixel(10, 10));
    }

    #[test]
    fn what_is_left_of_the_rest_is_asked_for() {
        let params = OnePoint {
            rest: Percent::new(100),
            ..OnePoint::default()
        };
        assert_eq!(one_point(&red_and_blue(), params), red_and_blue());
        let half = OnePoint {
            rest: Percent::new(50),
            ..OnePoint::default()
        };
        let [r, _, b] = one_point(&red_and_blue(), half).get_pixel(7, 0).0;
        assert!(b > 30 && b < 220 && r > 30, "{r} {b}");
    }

    #[test]
    fn the_hue_can_be_taken_from_a_point_of_the_picture() {
        let params = OnePoint {
            hue: Hue::new(0.0),
            at: Point(Some((Percent::new(90), Percent::new(50)))),
            ..OnePoint::default()
        };
        let result = one_point(&red_and_blue(), params);
        assert_eq!(result.get_pixel(7, 0).0, [30, 30, 220], "blue is kept");
        assert_ne!(result.get_pixel(0, 0).0, [220, 30, 30], "red goes grey");
    }

    #[test]
    fn a_grey_point_falls_back_to_the_hue_given() {
        let image = RgbImage::from_fn(8, 4, |x, _| {
            Rgb(if x < 4 {
                [220, 30, 30]
            } else {
                [128, 128, 128]
            })
        });
        let params = OnePoint {
            at: Point(Some((Percent::new(90), Percent::new(50)))),
            ..OnePoint::default()
        };
        assert_eq!(one_point(&image, params).get_pixel(0, 0).0, [220, 30, 30]);
    }

    #[test]
    fn the_edge_of_the_hue_fades_instead_of_cutting() {
        // A kept red with width 30 and soft 30: hue 0 is whole, hue 45 is
        // half way to grey, hue 90 is grey.
        let red = |rgb: [u8; 3]| one_point(&flat(rgb), OnePoint::default()).get_pixel(0, 0).0[0];
        assert_eq!(red([200, 0, 0]), 200);
        let half = red([200, 150, 0]);
        let grey = ((299 * 200 + 587 * 150) / 1000) as u8;
        assert!(half > grey && half < 200, "{half} between {grey} and 200");
        let far = red([100, 200, 0]);
        assert_eq!(
            far,
            ((299 * 100 + 587 * 200) / 1000) as u8,
            "far hues are grey"
        );
    }

    #[test]
    fn the_tritone_runs_through_its_three_inks() {
        let params = Tritone::default();
        let print = |v: u8| tritone(&flat([v, v, v]), params).get_pixel(0, 0).0;
        assert_eq!(print(0), [20, 5, 43]);
        assert_eq!(print(255), [255, 217, 160]);
        let mid = print(128);
        assert!(
            mid[0].abs_diff(224) < 8 && mid[1].abs_diff(32) < 8,
            "{mid:?}"
        );
        assert_eq!(
            tritone(
                &flat([90, 40, 10]),
                Tritone {
                    mix: Percent::new(0),
                    ..params
                }
            ),
            flat([90, 40, 10])
        );
    }

    #[test]
    fn false_colours_swap_invert_and_turn() {
        let pixel =
            |params: FalseColor| false_color(&flat([200, 100, 50]), params).get_pixel(0, 0).0;
        let neutral = FalseColor {
            order: "rgb".parse().unwrap(),
            ..FalseColor::default()
        };
        assert_eq!(pixel(neutral), [200, 100, 50]);
        assert_eq!(
            pixel(FalseColor::default()),
            [50, 100, 200],
            "red and blue swapped"
        );
        let inverted = FalseColor {
            invert: "g".parse().unwrap(),
            ..neutral
        };
        assert_eq!(pixel(inverted), [200, 155, 50]);
        let turned = FalseColor {
            rotate: 120,
            ..neutral
        };
        let [r, g, b] = pixel(turned);
        assert!(
            g > r && g > b,
            "red-orange turned 120 degrees is green: {r} {g} {b}"
        );
        let none = FalseColor {
            mix: Percent::new(0),
            ..FalseColor::default()
        };
        assert_eq!(pixel(none), [200, 100, 50]);
    }

    #[test]
    fn saturation_gives_a_pale_colour_something_to_turn() {
        // A pale blue-grey, like an overcast sky.
        let pale = flat([150, 160, 175]);
        let spread = |image: &RgbImage| {
            let [r, g, b] = image.get_pixel(0, 0).0;
            r.max(g).max(b) - r.min(g).min(b)
        };
        let params = |saturation| FalseColor {
            order: "rgb".parse().unwrap(),
            saturation,
            ..FalseColor::default()
        };
        assert_eq!(
            false_color(&pale, params(100)),
            pale,
            "100 leaves the colour"
        );
        assert!(spread(&false_color(&pale, params(200))) > 2 * spread(&pale) - 2);
        assert_eq!(
            spread(&false_color(&pale, params(0))),
            0,
            "0 takes it all out"
        );
        // Turning then shows: the pale blue goes towards violet once it is stronger.
        let violet = |saturation| {
            let image = false_color(
                &pale,
                FalseColor {
                    rotate: 45,
                    ..params(saturation)
                },
            );
            let [r, g, b] = image.get_pixel(0, 0).0;
            i32::from(r.min(b)) - i32::from(g)
        };
        assert!(
            violet(300) > violet(100) + 10,
            "{} {}",
            violet(300),
            violet(100)
        );
    }

    #[test]
    fn only_the_hue_asked_for_is_moved() {
        // Left a blue sky, right a green field.
        let image = RgbImage::from_fn(8, 4, |x, _| {
            Rgb(if x < 4 { [90, 120, 200] } else { [60, 180, 50] })
        });
        let violet = FalseColor {
            order: "rgb".parse().unwrap(),
            rotate: 60,
            only: "blue".parse().unwrap(),
            ..FalseColor::default()
        };
        let moved = false_color(&image, violet);
        assert_ne!(
            moved.get_pixel(0, 0),
            image.get_pixel(0, 0),
            "the blue moves"
        );
        assert_eq!(
            moved.get_pixel(7, 0),
            image.get_pixel(7, 0),
            "the green stays"
        );
        // Without a hue, both move.
        let all = false_color(
            &image,
            FalseColor {
                only: MaybeHue(None),
                ..violet
            },
        );
        assert_ne!(all.get_pixel(7, 0), image.get_pixel(7, 0));
    }

    #[test]
    fn a_speck_inside_the_range_is_moved_with_the_zone_when_smoothed() {
        // A blue sky with a small red speck in the middle.
        let mut sky = RgbImage::from_pixel(300, 300, Rgb([90, 120, 200]));
        for (x, y) in [(150, 150), (151, 150), (150, 151), (151, 151)] {
            sky.put_pixel(x, y, Rgb([220, 30, 30]));
        }
        let turn = |smooth: f32| FalseColor {
            order: "rgb".parse().unwrap(),
            rotate: 90,
            only: "blue".parse().unwrap(),
            smooth,
            ..FalseColor::default()
        };
        // Without smoothing the speck is its own colour, out of range, and
        // stays red; smoothed, it is taken for sky and its hue turns too.
        let sharp = false_color(&sky, turn(0.0));
        let soft = false_color(&sky, turn(3.0));
        assert_eq!(sharp.get_pixel(150, 150), sky.get_pixel(150, 150));
        assert_ne!(soft.get_pixel(150, 150), sky.get_pixel(150, 150));
        // A whole zone is turned the same either way.
        assert_eq!(soft.get_pixel(10, 10), sharp.get_pixel(10, 10));
    }

    #[test]
    fn the_edge_of_the_hue_range_fades_and_greys_stay() {
        let turn = FalseColor {
            order: "rgb".parse().unwrap(),
            rotate: 90,
            only: "red".parse().unwrap(),
            width: 10,
            soft: 60,
            ..FalseColor::default()
        };
        let full = |rgb: [u8; 3]| false_color(&flat(rgb), turn).get_pixel(0, 0).0;
        let reds_hue = [220, 30, 30];
        assert_ne!(full(reds_hue), reds_hue);
        // A hue 40 degrees from red is half way through the fade: between
        // the original and the whole move.
        let orange = [220, 150, 30];
        let moved = false_color(
            &flat(orange),
            FalseColor {
                only: MaybeHue(None),
                ..turn
            },
        )
        .get_pixel(0, 0)
        .0;
        let part = full(orange);
        assert!(
            part != orange && part != moved,
            "{orange:?} {part:?} {moved:?}"
        );
        assert_eq!(full([128, 128, 128]), [128, 128, 128], "a grey has no hue");
        // A hue far from red stays whole.
        assert_eq!(full([30, 30, 220]), [30, 30, 220]);
    }

    #[test]
    fn a_hue_or_none_is_read_and_written_back() {
        assert_eq!("none".parse::<MaybeHue>().unwrap(), MaybeHue(None));
        assert_eq!("NONE".parse::<MaybeHue>().unwrap(), MaybeHue(None));
        let blue: MaybeHue = "blue".parse().unwrap();
        assert!(blue.0.is_some());
        assert_eq!(blue.to_string().parse::<MaybeHue>().unwrap(), blue);
        assert_eq!(MaybeHue(None).to_string(), "none");
        assert!("pale".parse::<MaybeHue>().is_err());
    }

    #[test]
    fn orders_and_channels_are_read_and_written_back() {
        assert_eq!("BGR".parse::<ChannelOrder>().unwrap().to_string(), "bgr");
        for bad in ["rr", "rgbb", "rgg", "xyz", ""] {
            assert!(bad.parse::<ChannelOrder>().is_err(), "{bad}");
        }
        assert_eq!("rb".parse::<Channels>().unwrap().to_string(), "rb");
        assert_eq!("none".parse::<Channels>().unwrap(), Channels::default());
        assert_eq!(Channels::default().to_string(), "none");
        for bad in ["", "x", "rgbx"] {
            assert!(bad.parse::<Channels>().is_err(), "{bad}");
        }
    }
}
