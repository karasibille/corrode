//! Several colours moved at once, each its own way: the sky towards
//! violet, the grass towards red.

use std::fmt;
use std::str::FromStr;

use image::RgbImage;

use super::Error;
use super::colour::{Hue, from_hsv, to_hsv};
use super::grade::{byte, smoothstep};
use super::mask::SoftMap;
use super::parallel;
use super::recipe::{Params, Spec};
use super::units::Percent;

/// How many hues can be moved in one effect.
pub const MAX_MOVES: usize = 8;

/// Hues and where they go, written `blue:violet,green:red`: the colours
/// near the first of a pair move as far as it takes it to the second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HueMap([Option<(Hue, Hue)>; MAX_MOVES]);

impl HueMap {
    pub fn pairs(&self) -> impl Iterator<Item = (Hue, Hue)> + '_ {
        self.0.iter().flatten().copied()
    }
}

impl FromStr for HueMap {
    type Err = String;

    fn from_str(text: &str) -> Result<HueMap, String> {
        let mut map = HueMap::default();
        if text.trim().eq_ignore_ascii_case("none") {
            return Ok(map);
        }
        let moves: Vec<&str> = text.split(',').collect();
        if moves.len() > MAX_MOVES {
            return Err(format!("{} hues to move, at most {MAX_MOVES}", moves.len()));
        }
        for (slot, text) in map.0.iter_mut().zip(moves) {
            let (from, to) = text
                .split_once(':')
                .ok_or_else(|| format!("'{text}' is not from:to (blue:violet)"))?;
            *slot = Some((from.trim().parse()?, to.trim().parse()?));
        }
        Ok(map)
    }
}

impl fmt::Display for HueMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut pairs = self.pairs().peekable();
        if pairs.peek().is_none() {
            return f.write_str("none");
        }
        for (i, (from, to)) in pairs.enumerate() {
            let comma = if i == 0 { "" } else { "," };
            write!(f, "{comma}{from}:{to}")?;
        }
        Ok(())
    }
}

/// Parameters of the remap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Remap {
    /// The hues to move and where each goes.
    pub map: HueMap,
    /// How far from a hue, in degrees either side, colours move whole,
    /// and over how many more degrees the move fades out.
    pub width: u16,
    pub soft: u16,
    /// How soft the edge of a zone is, as a percent of the long side of
    /// the picture: the zones are worked out on a small copy and blurred
    /// by this much, so that an edge is not a line and a speck of another
    /// colour inside a zone is moved with the rest. 0 judges every pixel
    /// by itself.
    pub smooth: f32,
    /// How much colour the moved ones get, in percent: 100 leaves it.
    pub saturation: u16,
    /// How much of the result shows over the picture, 0 to 100.
    pub mix: Percent,
}

impl Default for Remap {
    fn default() -> Remap {
        Remap {
            map: HueMap::default(),
            width: 25,
            soft: 25,
            smooth: 1.0,
            saturation: 100,
            mix: Percent::new(100),
        }
    }
}

/// Moves every colour by the way of the hue it is near, all judged by the
/// colours the picture had, so that a move never feeds another. Where two
/// hues overlap their moves are averaged, by how near each is. Greys have
/// no hue and stay as they are.
pub fn remap(image: &RgbImage, params: Remap) -> RgbImage {
    let moves: Vec<(Hue, f32)> = params
        .map
        .pairs()
        .map(|(from, to)| (from, from.towards(to)))
        .collect();
    let mut result = image.clone();
    if moves.is_empty() {
        return result;
    }
    let (width, soft) = (f32::from(params.width), f32::from(params.soft).max(1.0));
    let saturation = f32::from(params.saturation) / 100.0;
    let mix = params.mix.fraction();
    // For a colour: how near it is to the hues, summed, and that nearness
    // times the turn each asks for; greys, which have no hue, count for
    // nothing.
    let pull = |rgb: [f32; 3]| {
        let [hue, strength, _] = to_hsv(rgb.map(|c| c / 255.0));
        let gate = smoothstep(0.02, 0.06, strength);
        let (mut near, mut turn) = (0.0f32, 0.0f32);
        for (from, gap) in &moves {
            let here = 1.0 - smoothstep(width, width + soft, Hue::new(hue).distance(*from));
            near += here;
            turn += here * gap;
        }
        [near * gate, turn * gate]
    };
    // The pull is judged on a blurred small copy, not pixel by pixel; the
    // turn is averaged by how near each hue is, so that a blur does not
    // mix a zone sent one way with a zone sent the other.
    let zones = (params.smooth > 0.0).then(|| SoftMap::build(image, params.smooth, pull));
    parallel::for_each_pixel(&mut result, |x, y, pixel| {
        let original = pixel.0.map(f32::from);
        let [near, turn] = match &zones {
            Some(zones) => zones.at(x, y),
            None => pull(original),
        };
        let amount = near.min(1.0) * mix;
        if amount <= 1e-4 || near <= 1e-4 {
            return;
        }
        let [hue, strength, value] = to_hsv(original.map(|c| c / 255.0));
        let moved = from_hsv([hue + turn / near, (strength * saturation).min(1.0), value])
            .map(|c| c * 255.0);
        for channel in 0..3 {
            pixel[channel] =
                byte(original[channel] + (moved[channel] - original[channel]) * amount);
        }
    });
    result
}

impl Spec for Remap {
    const NAME: &'static str = "remap";

    fn parse(params: &mut Params) -> Result<Remap, Error> {
        let d = Remap::default();
        Ok(Remap {
            map: params.get("map", d.map)?,
            width: params.get("width", d.width)?,
            soft: params.get("soft", d.soft)?,
            smooth: params.get::<f32>("smooth", d.smooth)?.clamp(0.0, 10.0),
            saturation: params.get::<u16>("saturation", d.saturation)?.min(400),
            mix: params.get("mix", d.mix)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " map={} width={} soft={} smooth={} saturation={} mix={}",
            self.map, self.width, self.soft, self.smooth, self.saturation, self.mix
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(remap(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;
    use crate::effects::Color;

    fn map(text: &str) -> HueMap {
        text.parse().unwrap()
    }

    fn hue_of(pixel: Rgb<u8>) -> Hue {
        Color(pixel).hue().expect("a colour")
    }

    /// Left a blue sky, right green grass.
    fn sky_and_grass() -> RgbImage {
        RgbImage::from_fn(8, 4, |x, _| {
            Rgb(if x < 4 { [60, 100, 220] } else { [70, 190, 50] })
        })
    }

    #[test]
    fn a_map_is_read_and_written_back() {
        let moves = map("blue:violet, green:red");
        assert_eq!(moves.pairs().count(), 2);
        assert_eq!(moves.to_string().parse::<HueMap>().unwrap(), moves);
        assert_eq!(map("none").pairs().count(), 0);
        assert_eq!(HueMap::default().to_string(), "none");
        let nine = ["red:blue"; MAX_MOVES + 1].join(",");
        for bad in ["", "blue", "blue:", ":red", "blue:pale", nine.as_str()] {
            assert!(bad.parse::<HueMap>().is_err(), "{bad}");
        }
    }

    #[test]
    fn each_hue_goes_where_it_is_sent() {
        let image = sky_and_grass();
        let params = Remap {
            map: map("220deg:280deg,110deg:0deg"),
            ..Remap::default()
        };
        let moved = remap(&image, params);
        let sky = hue_of(*moved.get_pixel(0, 0));
        let grass = hue_of(*moved.get_pixel(7, 0));
        assert!(sky.distance(Hue::new(280.0)) < 15.0, "{sky}");
        assert!(grass.distance(Hue::new(0.0)) < 15.0, "{grass}");
    }

    #[test]
    fn a_move_never_feeds_another() {
        // Blue goes to green and green goes to red: the blue must end
        // green, not red, as it would if the moves were done one by one.
        let image = sky_and_grass();
        let params = Remap {
            map: map("220deg:120deg,110deg:0deg"),
            ..Remap::default()
        };
        let moved = remap(&image, params);
        let sky = hue_of(*moved.get_pixel(0, 0));
        assert!(sky.distance(Hue::new(120.0)) < 15.0, "{sky}");
        let grass = hue_of(*moved.get_pixel(7, 0));
        assert!(grass.distance(Hue::new(0.0)) < 15.0, "{grass}");
    }

    #[test]
    fn smoothing_softens_an_edge_and_takes_a_speck_with_its_zone() {
        // Blue left, green right, with a green speck in the blue.
        let mut image = RgbImage::from_fn(300, 100, |x, _| {
            Rgb(if x < 150 {
                [60, 100, 220]
            } else {
                [70, 190, 50]
            })
        });
        for (x, y) in [(70, 50), (71, 50), (70, 51), (71, 51)] {
            image.put_pixel(x, y, Rgb([70, 190, 50]));
        }
        let moves = |smooth: f32| Remap {
            map: map("220deg:280deg"),
            smooth,
            ..Remap::default()
        };
        let sharp = remap(&image, moves(0.0));
        let soft = remap(&image, moves(3.0));
        // The green speck stays green when judged alone, and turns with the
        // sky when smoothed.
        assert_eq!(sharp.get_pixel(70, 50), image.get_pixel(70, 50));
        assert_ne!(soft.get_pixel(70, 50), image.get_pixel(70, 50));
        // The green zone is left alone, far from the edge.
        assert_eq!(soft.get_pixel(290, 50), image.get_pixel(290, 50));
        // At the edge the move fades instead of stopping at a line: the
        // pixels just left of it are less moved than far from it.
        let moved =
            |x: u32| i32::from(soft.get_pixel(x, 50).0[0]) - i32::from(image.get_pixel(x, 50).0[0]);
        assert!(
            moved(145).abs() < moved(20).abs(),
            "{} {}",
            moved(145),
            moved(20)
        );
    }

    #[test]
    fn colours_far_from_every_hue_greys_and_an_empty_map_stay() {
        let image = RgbImage::from_fn(8, 4, |x, _| {
            Rgb(if x < 4 {
                [220, 30, 30]
            } else {
                [128, 128, 128]
            })
        });
        let params = Remap {
            map: map("blue:violet"),
            ..Remap::default()
        };
        assert_eq!(remap(&image, params), image);
        assert_eq!(remap(&image, Remap::default()), image);
    }

    #[test]
    fn overlapping_hues_are_averaged_not_stacked() {
        // A hue right between two hues sent to opposite sides stays put.
        let image = RgbImage::from_pixel(4, 4, Color::from_hue(Hue::new(120.0)).0);
        let params = Remap {
            map: map("100deg:60deg,140deg:180deg"),
            width: 30,
            soft: 30,
            ..Remap::default()
        };
        let moved = remap(&image, params);
        let hue = hue_of(*moved.get_pixel(0, 0));
        assert!(hue.distance(Hue::new(120.0)) < 5.0, "{hue}");
    }

    #[test]
    fn saturation_and_mix_apply_to_the_moved_colours_only() {
        let image = sky_and_grass();
        let base = Remap {
            map: map("220deg:280deg"),
            ..Remap::default()
        };
        let strong = remap(
            &image,
            Remap {
                saturation: 200,
                ..base
            },
        );
        let spread = |image: &RgbImage| {
            let [r, g, b] = image.get_pixel(0, 0).0;
            r.max(g).max(b) - r.min(g).min(b)
        };
        assert!(spread(&strong) > spread(&remap(&image, base)));
        assert_eq!(strong.get_pixel(7, 0), image.get_pixel(7, 0));
        assert_eq!(
            remap(
                &image,
                Remap {
                    mix: Percent::new(0),
                    ..base
                }
            ),
            image
        );
    }
}
