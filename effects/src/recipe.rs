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
use super::film::{Aberration, Bloom, Drag, Fade, Grain, Leak, Streaks, Vignette};
use super::glitch::{ChannelSplit, PixelStretch, SliceShift};
use super::grade::{FalseColor, OnePoint, Tritone};
use super::jpeg::{Databend, GenerationLoss};
use super::pattern::{Change, Pattern, Time, split_words};
use super::print::{Dither, Duotone, Scanlines};
use super::prism::Prism;
use super::remap::Remap;
use super::sort::PixelSort;
use super::spin::Spin;
use super::warp::Liquid;

/// What an effect tells about itself, so that a recipe can name it,
/// read and write its parameters, and apply it.
pub(super) trait Spec: Copy {
    /// The name written in a recipe.
    const NAME: &'static str;
    /// What each parameter is, `(name, help)`, in the order they are
    /// written. A test checks that they are the parameters written, so that
    /// a parameter cannot be added without its help.
    const PARAMS: &'static [(&'static str, &'static str)];
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

            /// The help of the parameters of the effect called `name`.
            fn help(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
                $(
                    if name == <$spec as Spec>::NAME {
                        return Some(<$spec as Spec>::PARAMS);
                    }
                )*
                None
            }

            /// Applies the effect; `seed` feeds whatever it draws at random.
            pub fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
                match self {
                    $(Effect::$variant(params) => params.apply(image, seed),)*
                }
            }

            /// An effect from its name and its `name=value` parameters,
            /// read at moment `t` for the ranges.
            fn parse(name: &str, pairs: &[(&str, &str)], time: Time) -> Result<Effect, Error> {
                let mut params = Params::new(name, pairs, time);
                $(
                    if name == <$spec as Spec>::NAME {
                        let effect = Effect::$variant(<$spec as Spec>::parse(&mut params)?);
                        params.finish()?;
                        return Ok(effect);
                    }
                )*
                Err(unknown_effect(name))
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
    Streaks(Streaks),
    Bloom(Bloom),
    Liquid(Liquid),
    OnePoint(OnePoint),
    Tritone(Tritone),
    FalseColor(FalseColor),
    Spin(Spin),
    Remap(Remap),
    Prism(Prism),
}

/// One parameter of an effect, as the documentation shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamInfo {
    pub name: &'static str,
    /// The value it has when left out, as a recipe writes it.
    pub default: String,
    pub help: &'static str,
}

/// The parameters of the effect called `name`, in the order a recipe
/// writes them, with their defaults and what they do: what the README and
/// the help of the tools are made from.
pub fn describe(name: &str) -> Result<Vec<ParamInfo>, Error> {
    let help = Effect::help(name).ok_or_else(|| Error::UnknownEffect(name.to_owned()))?;
    let text = Effect::parse(name, &[], Time::default())?.to_string();
    let defaults: Vec<(&str, &str)> = text
        .split(' ')
        .skip(1)
        .filter_map(|word| word.split_once('='))
        .collect();
    Ok(help
        .iter()
        .map(|&(param, help)| ParamInfo {
            name: param,
            default: defaults
                .iter()
                .find(|(key, _)| *key == param)
                .map(|(_, value)| (*value).to_owned())
                .unwrap_or_default(),
            help,
        })
        .collect())
}

/// Effects applied in order, with the seed of their random draws: each
/// effect gets the seed plus its rank, so that changing the seed changes
/// every draw and the same recipe always gives the same picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub seed: u64,
    pub steps: Vec<Effect>,
}

/// What the steps of a recipe left behind, to be used again by a recipe
/// that starts the same way: the picture after each step, with the text of
/// the steps up to it and the seed. It holds for one source picture.
#[derive(Default)]
pub struct StepCache {
    steps: Vec<(String, RgbImage)>,
}

impl StepCache {
    /// How many steps are kept.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl Recipe {
    pub fn apply(&self, image: &RgbImage) -> Result<RgbImage, Error> {
        let mut current = image.clone();
        for (rank, effect) in self.steps.iter().enumerate() {
            current = effect.apply(&current, self.seed.wrapping_add(rank as u64))?;
        }
        Ok(current)
    }

    /// The same result as [`Recipe::apply`], starting from the last step
    /// `cache` holds that this recipe has in common with the one before it:
    /// when only the last effect changes, only the last effect is made
    /// again. The cache must only be used with the same `image`.
    pub fn apply_cached(&self, image: &RgbImage, cache: &mut StepCache) -> Result<RgbImage, Error> {
        // What a step leaves depends on the steps before it and the seed,
        // which each step takes with its rank added.
        let mut text = format!("seed={}", self.seed);
        let keys: Vec<String> = self
            .steps
            .iter()
            .map(|effect| {
                text = format!("{text} + {effect}");
                text.clone()
            })
            .collect();
        let kept = cache
            .steps
            .iter()
            .zip(&keys)
            .take_while(|((key, _), wanted)| key == *wanted)
            .count();
        cache.steps.truncate(kept);
        let mut current = match cache.steps.last() {
            Some((_, picture)) => picture.clone(),
            None => image.clone(),
        };
        for (rank, effect) in self.steps.iter().enumerate().skip(kept) {
            current = effect.apply(&current, self.seed.wrapping_add(rank as u64))?;
            cache.steps.push((keys[rank].clone(), current.clone()));
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
        Recipe::from_words_at_time(words, Time::new(t, 0.0))
    }

    /// A recipe from its words at a time: where the ranges stand, and how
    /// many cycles of its patterns (`"0 40 80"`) have gone by, as changed
    /// by the `fast`, `slow`, `rev` and `every` written before an effect.
    pub fn from_words_at_time<S: AsRef<str>>(words: &[S], time: Time) -> Result<Recipe, Error> {
        let mut seed = 1;
        let mut steps = Vec::new();
        // The effect being read: its name, its parameters and the time it
        // is read at, the changes written before it taken into account.
        let mut current: Option<Reading> = None;
        let mut changes: Vec<Change> = Vec::new();
        let mut at = 0;
        while at < words.len() {
            let word = words[at].as_ref();
            at += 1;
            if word == "+" || word.is_empty() {
                continue;
            }
            if !word.contains('=')
                && let Some((change, used)) = Change::read(words, at - 1).map_err(Error::Syntax)?
            {
                if let Some((name, pairs, time)) = current.take() {
                    steps.push(Effect::parse(name, &pairs, time)?);
                }
                changes.push(change);
                at += used - 1;
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
                (Some((name, value)), Some((_, pairs, _))) => pairs.push((name, value)),
                (Some((name, _)), None) => {
                    return Err(Error::UnknownParameter {
                        effect: "recipe".into(),
                        name: name.into(),
                    });
                }
                (None, _) => {
                    if let Some((name, pairs, time)) = current.take() {
                        steps.push(Effect::parse(name, &pairs, time)?);
                    }
                    let cycle = changes
                        .drain(..)
                        .fold(time.cycle, |cycle, change| change.apply(cycle));
                    current = Some((word, Vec::new(), Time { cycle, ..time }));
                }
            }
        }
        if let Some((name, pairs, time)) = current.take() {
            steps.push(Effect::parse(name, &pairs, time)?);
        }
        if steps.is_empty() {
            return Err(Error::Empty);
        }
        Ok(Recipe { seed, steps })
    }
}

/// An effect being read from the words of a recipe: its name, its
/// `name=value` parameters and the time it is read at.
type Reading<'a> = (&'a str, Vec<(&'a str, &'a str)>, Time);

/// The error for a word that is not the name of an effect: a number is
/// most likely a step of a pattern whose quotes the shell took away.
fn unknown_effect(name: &str) -> Error {
    if name.parse::<f64>().is_ok() {
        Error::Syntax(format!(
            "unknown effect '{name}': a pattern with spaces keeps its quotes, in a shell too (write 'rotate=\"0 40 80\"')"
        ))
    } else {
        Error::UnknownEffect(name.to_owned())
    }
}

impl FromStr for Recipe {
    type Err = Error;

    fn from_str(text: &str) -> Result<Recipe, Error> {
        Recipe::from_words(&split_words(text))
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
    time: Time,
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
    fn new(effect: &'a str, pairs: &'a [(&'a str, &'a str)], time: Time) -> Params<'a> {
        Params {
            effect,
            pairs,
            read: vec![false; pairs.len()],
            time,
        }
    }

    /// What a parameter's text gives now: itself, or for a pattern the
    /// step of the cycle (`None` for a rest, which is the default). Every
    /// step of a pattern must be a value `valid` accepts, so that a
    /// mistake in the third step is found now, not when it comes round.
    fn step(
        &self,
        name: &str,
        value: &str,
        valid: &dyn Fn(&str) -> bool,
    ) -> Result<Option<String>, Error> {
        if !Pattern::is_pattern(value) {
            return Ok(Some(value.to_owned()));
        }
        let pattern: Pattern = value
            .parse()
            .map_err(|why| Error::Syntax(format!("{}: {name}: {why}", self.effect)))?;
        if let Some(atom) = pattern.atoms().into_iter().find(|atom| !valid(atom)) {
            return Err(self.bad(name, atom));
        }
        Ok(pattern.at(self.time.cycle).map(str::to_owned))
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
        let moment = self.time.moment;
        let Some(value) = self.text(name) else {
            return Ok(default);
        };
        let valid =
            |text: &str| at_moment(text, moment).is_some_and(|now| now.parse::<T>().is_ok());
        match self.step(name, value, &valid)? {
            Some(text) => at_moment(&text, moment)
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
        let moment = self.time.moment;
        let Some(value) = self.text(name) else {
            return Ok(default);
        };
        let read = |text: &str| -> Option<(T, T)> {
            let (a, b) = text.split_once(',')?;
            let a = at_moment(a, moment)?.parse().ok()?;
            let b = at_moment(b, moment)?.parse().ok()?;
            Some((a, b))
        };
        match self.step(name, value, &|text| read(text).is_some())? {
            Some(text) => read(&text).ok_or_else(|| self.bad(name, value)),
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
    use crate::Size;

    #[test]
    fn a_cached_recipe_gives_what_apply_gives_and_keeps_its_steps() {
        let image = RgbImage::from_fn(60, 40, |x, y| Rgb([(x * 4) as u8, (y * 6) as u8, 90]));
        let apply = |text: &str, cache: &mut StepCache| {
            let recipe: Recipe = text.parse().unwrap();
            let cached = recipe.apply_cached(&image, cache).unwrap();
            assert!(cached == recipe.apply(&image).unwrap(), "{text}");
        };
        let mut cache = StepCache::default();
        assert!(cache.is_empty());
        apply("seed=3 grain + fade lift=10 + vignette", &mut cache);
        assert_eq!(cache.len(), 3);
        // Only the last step changes, then a middle one, then the seed:
        // each time the result is what a fresh application gives.
        apply(
            "seed=3 grain + fade lift=10 + vignette strength=80",
            &mut cache,
        );
        apply(
            "seed=3 grain + fade lift=30 + vignette strength=80",
            &mut cache,
        );
        apply(
            "seed=4 grain + fade lift=30 + vignette strength=80",
            &mut cache,
        );
        apply("seed=4 grain", &mut cache);
        assert_eq!(cache.len(), 1);
        apply("seed=4 grain + fade + vignette + grain", &mut cache);
        assert_eq!(cache.len(), 4);
    }

    /// The shift of the first step of a recipe read at a cycle.
    fn shift_at(text: &str, cycle: f64) -> u32 {
        let words = split_words(text);
        match Recipe::from_words_at_time(&words, Time::new(0.0, cycle))
            .unwrap()
            .steps[0]
        {
            Effect::SliceShift(slice) => slice.shift.value(),
            ref other => panic!("not a slice: {other:?}"),
        }
    }

    #[test]
    fn a_pattern_gives_a_step_of_the_cycle_to_a_parameter() {
        let recipe = "seed=1 slice shift=\"10 20 30\"";
        assert_eq!(
            [0.0, 0.4, 0.7, 1.0, 1.5].map(|cycle| shift_at(recipe, cycle)),
            [10, 20, 30, 10, 20]
        );
        // A rest is the default; steps can be ranges at the moment.
        let default = SliceShift::default().shift.value();
        assert_eq!(shift_at("slice shift=\"~ 5\"", 0.1), default);
        assert_eq!(shift_at("slice shift=\"~ 5\"", 0.6), 5);
        let words = split_words("slice shift=\"0..100\"");
        let at = |moment| match Recipe::from_words_at_time(&words, Time::new(moment, 0.0))
            .unwrap()
            .steps[0]
        {
            Effect::SliceShift(slice) => slice.shift.value(),
            _ => unreachable!(),
        };
        assert_eq!((at(0.0), at(0.5), at(1.0)), (0, 50, 100));
    }

    #[test]
    fn a_pattern_gives_colours_booleans_and_pairs_too() {
        let words = split_words("seed=1 slice split=\"true false\" + spin center=\"10,20 80,90\"");
        let at = |cycle| Recipe::from_words_at_time(&words, Time::new(0.0, cycle)).unwrap();
        assert!(matches!(at(0.0).steps[0], Effect::SliceShift(slice) if slice.split));
        assert!(matches!(at(0.6).steps[0], Effect::SliceShift(slice) if !slice.split));
        assert!(matches!(at(0.6).steps[1], Effect::Spin(spin)
            if (spin.center.0.value(), spin.center.1.value()) == (80, 90)));
        let leak = split_words("leak color=\"#ff0000 blue\"");
        assert!(Recipe::from_words(&leak).is_ok());
    }

    #[test]
    fn every_step_of_a_pattern_is_checked_when_it_is_read() {
        // The third step of an alternation is wrong, and a cycle would
        // not reach it for a while.
        let words = split_words("slice shift=\"<10 20 oops>\"");
        let err = Recipe::from_words(&words).unwrap_err().to_string();
        assert!(err.contains("shift") && err.contains("oops"), "{err}");
        let err = Recipe::from_words(&split_words("slice shift=\"10 [20\""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("shift") && err.contains("missing"), "{err}");
    }

    #[test]
    fn changes_before_an_effect_move_its_cycle_only() {
        let fast = "seed=1 fast 2 slice shift=\"10 20 30\"";
        assert_eq!(shift_at(fast, 0.2), 20, "twice as fast: cycle 0.4");
        let slow = "slow 3 slice shift=\"10 20 30\"";
        assert_eq!(shift_at(slow, 1.5), 20, "a third as fast: cycle 0.5");
        let rev = "rev slice shift=\"10 20 30\"";
        assert_eq!(
            [0.1, 0.5, 0.9].map(|cycle| shift_at(rev, cycle)),
            [30, 20, 10]
        );
        let every = "every 2 rev slice shift=\"10 20 30\"";
        assert_eq!(shift_at(every, 0.1), 30, "cycle 0 is reversed");
        assert_eq!(shift_at(every, 1.1), 10, "cycle 1 is not");
        assert_eq!(shift_at(every, 2.1), 30, "cycle 2 is");
        // The change belongs to the next effect only.
        let two = split_words("seed=1 rev slice shift=\"10 20 30\" + slice shift=\"10 20 30\"");
        let recipe = Recipe::from_words_at_time(&two, Time::new(0.0, 0.1)).unwrap();
        let shifts: Vec<u32> = recipe
            .steps
            .iter()
            .map(|step| match step {
                Effect::SliceShift(slice) => slice.shift.value(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(shifts, [30, 10]);
        // Several changes stack, in the order written.
        assert_eq!(shift_at("fast 3 rev slice shift=\"10 20 30\"", 0.0), 30);
    }

    #[test]
    fn a_shell_that_split_a_pattern_gets_a_hint_and_names_are_free() {
        let err = Recipe::from_words(&["slice", "shift=10", "20", "30"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("quotes"), "{err}");
        assert!(Recipe::from_words(&["fast"]).is_err());
        for word in ["fast", "slow", "rev", "every"] {
            assert!(!NAMES.contains(&word), "{word} is an effect");
        }
    }

    #[test]
    fn a_recipe_without_patterns_reads_as_it_always_did() {
        let text = "seed=7 sort low=40 + slice slices=3 split=true + bend hits=2";
        let plain: Recipe = text.parse().unwrap();
        let timed = Recipe::from_words_at_time(&split_words(text), Time::new(0.0, 5.7)).unwrap();
        assert_eq!(plain, timed);
    }

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
        assert_eq!(NAMES.len(), 24);
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
    fn every_effect_documents_exactly_the_parameters_it_writes() {
        for name in NAMES {
            let written = name.parse::<Recipe>().unwrap().to_string();
            let keys: Vec<&str> = written
                .split(' ')
                .filter_map(|word| word.split_once('=').map(|(key, _)| key))
                .filter(|key| *key != "seed")
                .collect();
            let documented: Vec<&str> = describe(name).unwrap().iter().map(|p| p.name).collect();
            assert_eq!(keys, documented, "{name}");
            for param in describe(name).unwrap() {
                assert!(!param.default.is_empty(), "{name}.{}", param.name);
                assert!(
                    param.help.len() > 8,
                    "{name}.{} has no real help",
                    param.name
                );
            }
        }
        assert!(matches!(describe("nothing"), Err(Error::UnknownEffect(_))));
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
