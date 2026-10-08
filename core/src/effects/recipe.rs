//! A recipe: effects applied in order, with a seed, written as text so
//! that it can be given on a command line, kept next to a result and
//! made again.
//!
//! The text is `seed=<n>` then the effects, each its name followed by
//! `name=value` parameters; `+` may separate effects for the eye:
//!
//! ```text
//! seed=7 sort low=40 high=220 + slice slices=12 shift=120 split=true
//! ```
//!
//! Parameters left out take their defaults; printing a recipe writes
//! them all.

use std::fmt;
use std::str::FromStr;

use image::RgbImage;

use super::Error;
use super::glitch::{
    ChannelSplit, PixelStretch, SliceShift, channel_split, pixel_stretch, slice_shift,
};
use super::jpeg::{Databend, GenerationLoss, databend, generation_loss};
use super::sort::{PixelSort, pixel_sort};

/// One effect with its parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    GenerationLoss(GenerationLoss),
    Databend(Databend),
    PixelSort(PixelSort),
    SliceShift(SliceShift),
    PixelStretch(PixelStretch),
    ChannelSplit(ChannelSplit),
}

/// The names of the effects, as written in a recipe.
pub const NAMES: [&str; 6] = ["loss", "bend", "sort", "slice", "stretch", "split"];

impl Effect {
    pub fn name(&self) -> &'static str {
        match self {
            Effect::GenerationLoss(_) => "loss",
            Effect::Databend(_) => "bend",
            Effect::PixelSort(_) => "sort",
            Effect::SliceShift(_) => "slice",
            Effect::PixelStretch(_) => "stretch",
            Effect::ChannelSplit(_) => "split",
        }
    }

    /// Applies the effect; `seed` feeds whatever it draws at random.
    pub fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(match *self {
            Effect::GenerationLoss(params) => generation_loss(image, params),
            Effect::Databend(params) => databend(image, params, seed)?,
            Effect::PixelSort(params) => pixel_sort(image, params),
            Effect::SliceShift(params) => slice_shift(image, params, seed),
            Effect::PixelStretch(params) => pixel_stretch(image, params, seed),
            Effect::ChannelSplit(params) => channel_split(image, params),
        })
    }

    /// An effect from its name and its `name=value` parameters.
    fn parse(name: &str, pairs: &[(&str, &str)]) -> Result<Effect, Error> {
        let mut params = Params::new(name, pairs);
        let effect = match name {
            "loss" => {
                let d = GenerationLoss::default();
                Effect::GenerationLoss(GenerationLoss {
                    generations: params.get("generations", d.generations)?,
                    quality: params.get("quality", d.quality)?,
                    shift: params.pair("shift", d.shift)?,
                })
            }
            "bend" => {
                let d = Databend::default();
                Effect::Databend(Databend {
                    quality: params.get("quality", d.quality)?,
                    hits: params.get("hits", d.hits)?,
                })
            }
            "sort" => {
                let d = PixelSort::default();
                Effect::PixelSort(PixelSort {
                    direction: params.get("direction", d.direction)?,
                    low: params.get("low", d.low)?,
                    high: params.get("high", d.high)?,
                    reverse: params.get("reverse", d.reverse)?,
                })
            }
            "slice" => {
                let d = SliceShift::default();
                Effect::SliceShift(SliceShift {
                    slices: params.get("slices", d.slices)?,
                    shift: params.get("shift", d.shift)?,
                    height: params.pair("height", d.height)?,
                    split: params.get("split", d.split)?,
                    invert: params.get("invert", d.invert)?,
                })
            }
            "stretch" => {
                let d = PixelStretch::default();
                Effect::PixelStretch(PixelStretch {
                    bands: params.get("bands", d.bands)?,
                    length: params.pair("length", d.length)?,
                    direction: params.get("direction", d.direction)?,
                })
            }
            "split" => {
                let d = ChannelSplit::default();
                Effect::ChannelSplit(ChannelSplit {
                    red: params.pair("red", d.red)?,
                    green: params.pair("green", d.green)?,
                    blue: params.pair("blue", d.blue)?,
                })
            }
            other => return Err(Error::UnknownEffect(other.to_owned())),
        };
        params.finish()?;
        Ok(effect)
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pair = |(a, b): (i32, i32)| format!("{a},{b}");
        let upair = |(a, b): (u32, u32)| format!("{a},{b}");
        match *self {
            Effect::GenerationLoss(p) => write!(
                f,
                "loss generations={} quality={} shift={}",
                p.generations,
                p.quality,
                pair(p.shift)
            ),
            Effect::Databend(p) => write!(f, "bend quality={} hits={}", p.quality, p.hits),
            Effect::PixelSort(p) => write!(
                f,
                "sort direction={} low={} high={} reverse={}",
                p.direction, p.low, p.high, p.reverse
            ),
            Effect::SliceShift(p) => write!(
                f,
                "slice slices={} shift={} height={} split={} invert={}",
                p.slices,
                p.shift,
                upair(p.height),
                p.split,
                p.invert
            ),
            Effect::PixelStretch(p) => write!(
                f,
                "stretch bands={} length={} direction={}",
                p.bands,
                upair(p.length),
                p.direction
            ),
            Effect::ChannelSplit(p) => write!(
                f,
                "split red={} green={} blue={}",
                pair(p.red),
                pair(p.green),
                pair(p.blue)
            ),
        }
    }
}

/// Effects applied in order, with the seed of their random draws: each
/// effect gets the seed plus its rank, so that changing the seed changes
/// every draw and the same recipe always gives the same picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub seed: u64,
    pub steps: Vec<Effect>,
}

impl Recipe {
    pub fn apply(&self, image: &RgbImage) -> Result<RgbImage, Error> {
        let mut current = image.clone();
        for (rank, effect) in self.steps.iter().enumerate() {
            current = effect.apply(&current, self.seed.wrapping_add(rank as u64))?;
        }
        Ok(current)
    }

    /// A recipe from its words, as split on spaces.
    pub fn from_words<S: AsRef<str>>(words: &[S]) -> Result<Recipe, Error> {
        let mut seed = 1;
        let mut steps = Vec::new();
        let mut current: Option<(&str, Vec<(&str, &str)>)> = None;
        for word in words {
            let word = word.as_ref();
            if word == "+" || word.is_empty() {
                continue;
            }
            match (word.split_once('='), &mut current) {
                (Some(("seed", value)), None) => {
                    seed = value.parse().map_err(|_| Error::BadValue {
                        effect: "recipe".into(),
                        name: "seed".into(),
                        value: value.into(),
                    })?;
                }
                (Some((name, value)), Some((_, pairs))) => pairs.push((name, value)),
                (Some((name, _)), None) => {
                    return Err(Error::UnknownParameter {
                        effect: "recipe".into(),
                        name: name.into(),
                    });
                }
                (None, _) => {
                    if let Some((name, pairs)) = current.take() {
                        steps.push(Effect::parse(name, &pairs)?);
                    }
                    current = Some((word, Vec::new()));
                }
            }
        }
        if let Some((name, pairs)) = current.take() {
            steps.push(Effect::parse(name, &pairs)?);
        }
        if steps.is_empty() {
            return Err(Error::Empty);
        }
        Ok(Recipe { seed, steps })
    }
}

impl FromStr for Recipe {
    type Err = Error;

    fn from_str(text: &str) -> Result<Recipe, Error> {
        Recipe::from_words(&text.split_whitespace().collect::<Vec<_>>())
    }
}

impl fmt::Display for Recipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "seed={}", self.seed)?;
        for (rank, effect) in self.steps.iter().enumerate() {
            write!(f, " {}{effect}", if rank > 0 { "+ " } else { "" })?;
        }
        Ok(())
    }
}

/// The `name=value` parameters of one effect, each read once; what is
/// left unread is a mistake.
struct Params<'a> {
    effect: &'a str,
    pairs: &'a [(&'a str, &'a str)],
    read: Vec<bool>,
}

impl<'a> Params<'a> {
    fn new(effect: &'a str, pairs: &'a [(&'a str, &'a str)]) -> Params<'a> {
        Params {
            effect,
            pairs,
            read: vec![false; pairs.len()],
        }
    }

    /// The last value given for a parameter, or the default.
    fn text(&mut self, name: &str) -> Option<&'a str> {
        let mut found = None;
        for (i, (key, value)) in self.pairs.iter().enumerate() {
            if *key == name {
                self.read[i] = true;
                found = Some(*value);
            }
        }
        found
    }

    fn get<T: FromStr>(&mut self, name: &str, default: T) -> Result<T, Error> {
        match self.text(name) {
            Some(value) => value.parse().map_err(|_| self.bad(name, value)),
            None => Ok(default),
        }
    }

    /// A parameter written `a,b`.
    fn pair<T: FromStr + Copy>(&mut self, name: &str, default: (T, T)) -> Result<(T, T), Error> {
        match self.text(name) {
            Some(value) => value
                .split_once(',')
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                .ok_or_else(|| self.bad(name, value)),
            None => Ok(default),
        }
    }

    fn bad(&self, name: &str, value: &str) -> Error {
        Error::BadValue {
            effect: self.effect.to_owned(),
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    fn finish(self) -> Result<(), Error> {
        match self.read.iter().position(|read| !read) {
            Some(i) => Err(Error::UnknownParameter {
                effect: self.effect.to_owned(),
                name: self.pairs[i].0.to_owned(),
            }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    #[test]
    fn a_recipe_reads_and_prints_the_same() {
        let recipe: Recipe = "seed=7 sort low=40 + slice slices=3 split=true bend hits=2"
            .parse()
            .unwrap();
        assert_eq!(recipe.seed, 7);
        assert_eq!(recipe.steps.len(), 3);
        assert_eq!(
            recipe.steps[1],
            Effect::SliceShift(SliceShift {
                slices: 3,
                split: true,
                ..SliceShift::default()
            })
        );
        let text = recipe.to_string();
        assert_eq!(
            text,
            "seed=7 sort direction=horizontal low=40 high=220 reverse=false \
             + slice slices=3 shift=120 height=8,120 split=true invert=false \
             + bend quality=75 hits=2"
        );
        assert_eq!(text.parse::<Recipe>().unwrap(), recipe);
        for name in NAMES {
            assert_eq!(name.parse::<Recipe>().unwrap().steps[0].name(), name);
        }
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(
            "melt".parse::<Recipe>(),
            Err(Error::UnknownEffect("melt".into()))
        );
        assert_eq!(
            "sort depth=3".parse::<Recipe>(),
            Err(Error::UnknownParameter {
                effect: "sort".into(),
                name: "depth".into()
            })
        );
        assert_eq!(
            "loss shift=big".parse::<Recipe>(),
            Err(Error::BadValue {
                effect: "loss".into(),
                name: "shift".into(),
                value: "big".into()
            })
        );
        assert_eq!("seed=3".parse::<Recipe>(), Err(Error::Empty));
        assert!("low=3 sort".parse::<Recipe>().is_err());
    }

    #[test]
    fn a_recipe_applies_its_steps_in_order_with_the_seed() {
        let image = RgbImage::from_fn(64, 64, |x, y| Rgb([(x * 4) as u8, (y * 4) as u8, 100]));
        let recipe: Recipe = "seed=2 split red=-3,0 + slice slices=4 shift=20"
            .parse()
            .unwrap();
        let result = recipe.apply(&image).unwrap();
        assert_eq!((result.width(), result.height()), (64, 64));
        assert_ne!(result, image);
        assert_eq!(recipe.apply(&image).unwrap(), result);
        let other = Recipe { seed: 3, ..recipe };
        assert_ne!(other.apply(&image).unwrap(), result);
    }
}
