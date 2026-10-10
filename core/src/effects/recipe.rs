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
//! them all. For an animation, a number may be a range, `shift=20..160`:
//! the recipe is then read at a moment between 0 and 1, where the value
//! stands between the two ends.
//!
//! Each effect describes itself through [`Spec`]: its name, how its
//! parameters are read and written, and how it applies. The list of
//! effects is the one place that names them all.

use std::fmt;
use std::str::FromStr;

use image::RgbImage;

use super::Error;
use super::film::{Aberration, Bloom, Drag, Fade, Grain, Leak, Vignette};
use super::glitch::{ChannelSplit, PixelStretch, SliceShift};
use super::jpeg::{Databend, GenerationLoss};
use super::print::{Dither, Duotone, Scanlines};
use super::sort::PixelSort;
use super::warp::Liquid;

/// What an effect tells about itself, so that a recipe can name it,
/// read and write its parameters, and apply it.
pub(super) trait Spec: Copy {
    /// The name written in a recipe.
    const NAME: &'static str;
    /// The parameters from their `name=value` text, the rest at their
    /// defaults.
    fn parse(params: &mut Params) -> Result<Self, Error>;
    /// The parameters as `name=value` text, each with a leading space.
    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
    /// Applies the effect; `seed` feeds whatever it draws at random.
    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error>;
}

/// Declares the effects of a recipe: the enum of them, their names and
/// the dispatch of everything a recipe does with one.
macro_rules! effects {
    ($($variant:ident($spec:ty)),* $(,)?) => {
        /// One effect with its parameters.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum Effect {
            $($variant($spec),)*
        }

        /// The names of the effects, as written in a recipe.
        pub const NAMES: &[&str] = &[$(<$spec as Spec>::NAME,)*];

        impl Effect {
            pub fn name(&self) -> &'static str {
                match self {
                    $(Effect::$variant(_) => <$spec as Spec>::NAME,)*
                }
            }

            /// Applies the effect; `seed` feeds whatever it draws at random.
            pub fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
                match self {
                    $(Effect::$variant(params) => params.apply(image, seed),)*
                }
            }

            /// An effect from its name and its `name=value` parameters,
            /// read at moment `t` for the ranges.
            fn parse(name: &str, pairs: &[(&str, &str)], t: f64) -> Result<Effect, Error> {
                let mut params = Params::new(name, pairs, t);
                $(
                    if name == <$spec as Spec>::NAME {
                        let effect = Effect::$variant(<$spec as Spec>::parse(&mut params)?);
                        params.finish()?;
                        return Ok(effect);
                    }
                )*
                Err(Error::UnknownEffect(name.to_owned()))
            }
        }

        impl fmt::Display for Effect {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $(Effect::$variant(params) => {
                        f.write_str(<$spec as Spec>::NAME)?;
                        params.write(f)
                    })*
                }
            }
        }
    };
}

effects! {
    GenerationLoss(GenerationLoss),
    Databend(Databend),
    PixelSort(PixelSort),
    SliceShift(SliceShift),
    PixelStretch(PixelStretch),
    ChannelSplit(ChannelSplit),
    Dither(Dither),
    Duotone(Duotone),
    Scanlines(Scanlines),
    Drag(Drag),
    Aberration(Aberration),
    Grain(Grain),
    Fade(Fade),
    Vignette(Vignette),
    Leak(Leak),
    Bloom(Bloom),
    Liquid(Liquid),
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

    /// A recipe from its words, as split on spaces; a range takes its
    /// first value.
    pub fn from_words<S: AsRef<str>>(words: &[S]) -> Result<Recipe, Error> {
        Recipe::from_words_at(words, 0.0)
    }

    /// A recipe from its words at moment `t`, 0 to 1, of an animation:
    /// a parameter given as a range `a..b` stands at `t` between `a`
    /// and `b`.
    pub fn from_words_at<S: AsRef<str>>(words: &[S], t: f64) -> Result<Recipe, Error> {
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
                        steps.push(Effect::parse(name, &pairs, t)?);
                    }
                    current = Some((word, Vec::new()));
                }
            }
        }
        if let Some((name, pairs)) = current.take() {
            steps.push(Effect::parse(name, &pairs, t)?);
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
/// left unread is a mistake. Ranges are read at moment `t`.
pub(super) struct Params<'a> {
    effect: &'a str,
    pairs: &'a [(&'a str, &'a str)],
    read: Vec<bool>,
    t: f64,
}

/// A number, or `a..b` taken at `t` between `a` and `b`: whole numbers
/// give a whole number, so that `20..160` suits a parameter in pixels.
fn at_moment(value: &str, t: f64) -> Option<String> {
    let Some((a, b)) = value.split_once("..") else {
        return Some(value.to_owned());
    };
    let t = t.clamp(0.0, 1.0);
    if let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) {
        let between = a as f64 + (b - a) as f64 * t;
        return Some(between.round().to_string());
    }
    let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    Some((a + (b - a) * t).to_string())
}

impl<'a> Params<'a> {
    fn new(effect: &'a str, pairs: &'a [(&'a str, &'a str)], t: f64) -> Params<'a> {
        Params {
            effect,
            pairs,
            read: vec![false; pairs.len()],
            t,
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

    pub(super) fn get<T: FromStr>(&mut self, name: &str, default: T) -> Result<T, Error> {
        match self.text(name) {
            Some(value) => at_moment(value, self.t)
                .and_then(|now| now.parse().ok())
                .ok_or_else(|| self.bad(name, value)),
            None => Ok(default),
        }
    }

    /// A parameter written `a,b`, each side maybe a range.
    pub(super) fn pair<T: FromStr + Copy>(
        &mut self,
        name: &str,
        default: (T, T),
    ) -> Result<(T, T), Error> {
        match self.text(name) {
            Some(value) => value
                .split_once(',')
                .and_then(|(a, b)| {
                    let a = at_moment(a, self.t)?.parse().ok()?;
                    let b = at_moment(b, self.t)?.parse().ok()?;
                    Some((a, b))
                })
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

/// Writes a pair `a,b`.
pub(super) fn pair<T: fmt::Display>((a, b): (T, T)) -> String {
    format!("{a},{b}")
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;
    use crate::effects::Size;

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
            assert_eq!(name.parse::<Recipe>().unwrap().steps[0].name(), *name);
        }
        assert_eq!(NAMES.len(), 17);
    }

    #[test]
    fn every_effect_prints_all_its_parameters_and_reads_them_back() {
        for name in NAMES {
            let recipe = name.parse::<Recipe>().unwrap();
            let text = recipe.to_string();
            assert_eq!(text.parse::<Recipe>().unwrap(), recipe, "{text}");
            assert!(text.matches('=').count() >= 2, "{text}");
        }
    }

    #[test]
    fn ranges_stand_at_a_moment_of_the_animation() {
        let words = ["slice", "shift=20..160", "height=4..8,10", "slices=3"];
        let at = |t| match Recipe::from_words_at(&words, t).unwrap().steps[0] {
            Effect::SliceShift(p) => (p.shift, p.height, p.slices),
            _ => panic!(),
        };
        let size = Size::new;
        assert_eq!(at(0.0), (size(20), (size(4), size(10)), 3));
        assert_eq!(at(0.5), (size(90), (size(6), size(10)), 3));
        assert_eq!(at(1.0), (size(160), (size(8), size(10)), 3));
        assert_eq!(
            Recipe::from_words(&words).unwrap().steps[0],
            Recipe::from_words_at(&words, 0.0).unwrap().steps[0]
        );
        assert!(Recipe::from_words_at(&["slice", "shift=a..b"], 0.5).is_err());
        assert_eq!(at_moment("0.5..1.5", 0.5), Some("1".to_owned()));
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
        assert_eq!(
            "vignette strength=150".parse::<Recipe>(),
            Err(Error::BadValue {
                effect: "vignette".into(),
                name: "strength".into(),
                value: "150".into()
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
