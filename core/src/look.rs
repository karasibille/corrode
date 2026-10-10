//! The look of a shot: what its RawTherapee profile does to the picture,
//! apart from what belongs to the shot itself (marks, crop, rotation…)
//! or to its scene (white balance). Applying the look of a reference to
//! other shots copies those settings and leaves the rest of each shot's
//! profile as it was.

use crate::pp3::Profile;

/// Sections tied to one shot, never copied: its marks, its framing, its
/// lens and its raw file settings. `RAW Bayer` and `RAW X-Trans`, which
/// choose how the picture is developed, are part of the look.
const SHOT_SECTIONS: [&str; 10] = [
    "General",
    "Version",
    "Crop",
    "Resize",
    "Rotation",
    "Perspective",
    "Coarse",
    "LensProfile",
    "Film Negative",
    "RAW",
];
/// The metadata written into the exported file belongs to the shot too.
const METADATA: &str = "MetaData";
/// Tied to the light of the scene, copied only between shots of the same
/// scene.
const WHITE_BALANCE: &str = "White Balance";
const EXPOSURE: &str = "Exposure";
const COMPENSATION: &str = "Compensation";
/// The most a brightness correction between two scenes may be, in stops:
/// beyond, the two scenes are not alike enough to be matched by exposure.
pub const MAX_SHIFT_EV: f32 = 2.0;

/// What to do with the exposure compensation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Exposure {
    /// Each shot keeps its own.
    Keep,
    /// The reference's, for shots of the same scene.
    Copy,
    /// The reference's plus a number of stops, to bring another scene to
    /// the same brightness; see [`shift_ev`].
    Shift(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Copy the white balance too: for shots lit like the reference.
    pub with_scene: bool,
    pub exposure: Exposure,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            with_scene: false,
            exposure: Exposure::Keep,
        }
    }
}

/// How many stops to add to the compensation of a reference so that a
/// scene of mean luminance `target` ends as bright as the reference's
/// scene, of mean luminance `reference`, did. Luminances are linear and
/// come from pictures before the edit. Limited to [`MAX_SHIFT_EV`]; 0 when
/// a luminance is not positive.
pub fn shift_ev(reference: f32, target: f32) -> f32 {
    if reference <= 0.0 || target <= 0.0 {
        return 0.0;
    }
    (reference / target)
        .log2()
        .clamp(-MAX_SHIFT_EV, MAX_SHIFT_EV)
}

/// The profile of a shot with the look of `reference` applied: every
/// section but the shot's own (and the scene's, unless asked) is taken
/// from the reference, so what the reference leaves at its defaults is at
/// its defaults for the shot too.
pub fn apply(target: &Profile, reference: &Profile, options: &Options) -> Profile {
    let mut result = target.clone();
    let mut names: Vec<&str> = reference.section_names();
    for name in target.section_names() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for name in names {
        let shot_own = SHOT_SECTIONS.contains(&name) || name == METADATA;
        if shot_own || (name == WHITE_BALANCE && !options.with_scene) {
            continue;
        }
        result.replace_section(name, reference);
    }
    set_compensation(&mut result, target, reference, options.exposure);
    result
}

/// Puts the exposure compensation the options ask for, which the copy of
/// the `[Exposure]` section has just overwritten with the reference's.
fn set_compensation(result: &mut Profile, target: &Profile, reference: &Profile, how: Exposure) {
    let number = |profile: &Profile| {
        profile
            .get(EXPOSURE, COMPENSATION)
            .and_then(|value| value.parse::<f32>().ok())
    };
    let value = match how {
        Exposure::Copy => return,
        Exposure::Keep => number(target).unwrap_or(0.0),
        Exposure::Shift(ev) => number(reference).unwrap_or(0.0) + ev,
    };
    // Hundredths of a stop are as fine as anyone can see.
    let value = (value * 100.0).round() / 100.0;
    result.set_in(EXPOSURE, COMPENSATION, &value.to_string());
}

/// One setting that differs between two profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub section: String,
    pub key: String,
    /// `None` when the key was not there.
    pub from: Option<String>,
    pub to: Option<String>,
}

/// What differs between two profiles, section by section, in file order.
pub fn changes(before: &Profile, after: &Profile) -> Vec<Change> {
    let mut sections: Vec<&str> = before.section_names();
    for name in after.section_names() {
        if !sections.contains(&name) {
            sections.push(name);
        }
    }
    let mut found = Vec::new();
    for section in sections {
        let (old, new) = (
            before.section_entries(section),
            after.section_entries(section),
        );
        let mut keys: Vec<&str> = old.iter().map(|(key, _)| *key).collect();
        for (key, _) in &new {
            if !keys.contains(key) {
                keys.push(key);
            }
        }
        let value = |entries: &[(&str, &str)], key: &str| {
            entries
                .iter()
                .rfind(|(k, _)| *k == key)
                .map(|(_, value)| (*value).to_owned())
        };
        for key in keys {
            let (from, to) = (value(&old, key), value(&new, key));
            if from != to {
                found.push(Change {
                    section: section.to_owned(),
                    key: key.to_owned(),
                    from,
                    to,
                });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Profile {
        Profile::parse(text).unwrap()
    }

    const TARGET: &str = "\
[General]
Rank=4
ColorLabel=2

[Exposure]
Compensation=0.5
Contrast=0

[White Balance]
Setting=Custom
Temperature=3000

[Crop]
Enabled=true
X=5

[Dehaze]
Enabled=true
Strength=40
";
    const REFERENCE: &str = "\
[General]
Rank=0
ColorLabel=0

[Exposure]
Compensation=-0.2
Contrast=10
Saturation=10

[White Balance]
Setting=Custom
Temperature=4400

[Crop]
Enabled=false

[Vibrance]
Enabled=true
Pastels=-27
";

    #[test]
    fn the_look_is_copied_and_the_shot_stays_itself() {
        let (target, reference) = (parse(TARGET), parse(REFERENCE));
        let result = apply(&target, &reference, &Options::default());

        // The marks, framing and light of the shot are its own.
        assert_eq!(result.marks().unwrap(), target.marks().unwrap());
        assert_eq!(
            result.section_entries("Crop"),
            target.section_entries("Crop")
        );
        assert_eq!(result.get("White Balance", "Temperature"), Some("3000"));
        // The look comes from the reference, with its leftovers gone.
        assert_eq!(result.get("Exposure", "Contrast"), Some("10"));
        assert_eq!(result.get("Exposure", "Saturation"), Some("10"));
        assert_eq!(result.get("Vibrance", "Pastels"), Some("-27"));
        assert_eq!(result.get("Dehaze", "Strength"), None);
        assert!(!result.section_names().contains(&"Dehaze"));
    }

    #[test]
    fn exposure_is_kept_copied_or_shifted() {
        let (target, reference) = (parse(TARGET), parse(REFERENCE));
        let compensation = |exposure| {
            let options = Options {
                exposure,
                ..Options::default()
            };
            apply(&target, &reference, &options)
                .get("Exposure", "Compensation")
                .map(str::to_owned)
        };
        assert_eq!(compensation(Exposure::Keep).as_deref(), Some("0.5"));
        assert_eq!(compensation(Exposure::Copy).as_deref(), Some("-0.2"));
        assert_eq!(compensation(Exposure::Shift(0.75)).as_deref(), Some("0.55"));
    }

    #[test]
    fn the_white_balance_follows_only_within_a_scene() {
        let (target, reference) = (parse(TARGET), parse(REFERENCE));
        let options = Options {
            with_scene: true,
            ..Options::default()
        };
        let result = apply(&target, &reference, &options);
        assert_eq!(result.get("White Balance", "Temperature"), Some("4400"));
    }

    #[test]
    fn applying_the_same_look_twice_changes_nothing() {
        let (target, reference) = (parse(TARGET), parse(REFERENCE));
        let options = Options::default();
        let once = apply(&target, &reference, &options);
        let twice = apply(&once, &reference, &options);
        assert_eq!(once, twice);
        assert!(changes(&once, &twice).is_empty());
    }

    #[test]
    fn changes_say_what_was_set_added_and_removed() {
        let (target, reference) = (parse(TARGET), parse(REFERENCE));
        let result = apply(&target, &reference, &Options::default());
        let found = changes(&target, &result);
        let change = |section: &str, key: &str| {
            found
                .iter()
                .find(|c| c.section == section && c.key == key)
                .unwrap_or_else(|| panic!("no change of {section}.{key}"))
        };
        assert_eq!(change("Exposure", "Contrast").from.as_deref(), Some("0"));
        assert_eq!(change("Exposure", "Contrast").to.as_deref(), Some("10"));
        assert_eq!(change("Exposure", "Saturation").from, None);
        assert_eq!(change("Dehaze", "Strength").to, None);
        assert!(!found.iter().any(|c| c.section == "General"));
    }

    #[test]
    fn a_brighter_reference_scene_asks_for_less_light_and_it_is_limited() {
        assert_eq!(shift_ev(0.2, 0.2), 0.0);
        assert!((shift_ev(0.4, 0.2) - 1.0).abs() < 1e-6);
        assert!((shift_ev(0.1, 0.2) + 1.0).abs() < 1e-6);
        assert_eq!(shift_ev(0.9, 0.01), MAX_SHIFT_EV);
        assert_eq!(shift_ev(0.01, 0.9), -MAX_SHIFT_EV);
        assert_eq!(shift_ev(0.0, 0.2), 0.0);
    }
}
