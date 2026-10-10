//! Applying a preset to a whole selection. The shots of a selection come
//! in groups, the subfolders `similarity` sorted them into, plus the shots
//! left alone; the preset is a profile made on one shot, and it does not
//! suit every group as it is: the light of each group asks for its own
//! exposure, and the photographer may want other settings changed too.
//!
//! The adjustments are settings, `Section.Key=value`, set after the preset
//! is applied. Some are worked out (the exposure compensation), the others
//! are written by hand in a file with a `[group]` header for each group.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::look::{self, Exposure, Options};
use crate::pairing::{self, Shot};
use crate::pp3::{self, Profile};
use crate::rawtherapee::{self, Config};

const EXPOSURE: &str = "Exposure";
const COMPENSATION: &str = "Compensation";

/// Shots that are given the same adjustments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The name of the subfolder, or `@` and the stem of a shot left alone.
    pub name: String,
    pub shots: Vec<Shot>,
}

/// The groups of a selection folder: each subfolder with shots in it, by
/// name, then each shot at the top level on its own, whose light is its own.
pub fn groups(dir: &Path) -> io::Result<Vec<Group>> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
        if path.is_dir() && !hidden {
            folders.push(path);
        }
    }
    folders.sort();
    let mut groups = Vec::new();
    for folder in folders {
        let shots = pairing::scan_dir(&folder)?;
        if !shots.is_empty() {
            let name = folder.file_name().unwrap_or_default().to_string_lossy();
            groups.push(Group {
                name: name.into_owned(),
                shots,
            });
        }
    }
    for shot in pairing::scan_dir(dir)? {
        groups.push(Group {
            name: format!("@{}", shot.stem.to_string_lossy()),
            shots: vec![shot],
        });
    }
    Ok(groups)
}

/// One setting to put into the profiles of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub section: String,
    pub key: String,
    pub value: String,
}

impl Setting {
    pub fn new(section: &str, key: &str, value: &str) -> Setting {
        Setting {
            section: section.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}={}", self.section, self.key, self.value)
    }
}

/// What can be worked out for a group: its exposure compensation, which
/// is the preset's moved by the difference of light between the shot the
/// preset was made on (`reference`) and the group, both linear mean
/// luminances. Without the two, nothing: the preset's own stays.
pub fn automatic(preset: &Profile, reference: Option<f32>, group: Option<f32>) -> Vec<Setting> {
    let (Some(reference), Some(group)) = (reference, group) else {
        return Vec::new();
    };
    let own: f32 = preset
        .get(EXPOSURE, COMPENSATION)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    // Hundredths of a stop are as fine as anyone can see.
    let value = ((own + look::shift_ev(reference, group)) * 100.0).round() / 100.0;
    vec![Setting::new(EXPOSURE, COMPENSATION, &value.to_string())]
}

/// Puts the settings of `manual` over the ones of `automatic`: the same
/// setting is the manual one, the others are all kept.
pub fn merge(automatic: Vec<Setting>, manual: &[Setting]) -> Vec<Setting> {
    let mut merged = automatic;
    for setting in manual {
        match merged
            .iter_mut()
            .find(|s| s.section == setting.section && s.key == setting.key)
        {
            Some(found) => found.value = setting.value.clone(),
            None => merged.push(setting.clone()),
        }
    }
    merged
}

/// The profile of a shot with the preset applied and then the settings of
/// its group. The marks, the framing and the white balance of the shot
/// stay its own, unless a setting says otherwise.
pub fn adapt(target: &Profile, preset: &Profile, settings: &[Setting]) -> Profile {
    let options = Options {
        with_scene: false,
        exposure: Exposure::Copy,
    };
    let mut profile = look::apply(target, preset, &options);
    for setting in settings {
        profile.set_in(&setting.section, &setting.key, &setting.value);
    }
    profile
}

/// The adjustments written by hand, by group.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Adjustments {
    groups: BTreeMap<String, Vec<Setting>>,
}

impl Adjustments {
    /// Reads `[group]` headers followed by `Section.Key=value` lines;
    /// blank lines and `#` comments are free. The error names the line.
    pub fn parse(text: &str) -> Result<Adjustments, String> {
        let mut adjustments = Adjustments::default();
        let mut group: Option<String> = None;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            let error = |what: &str| format!("line {}: {what}", index + 1);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                adjustments.groups.entry(name.to_owned()).or_default();
                group = Some(name.to_owned());
                continue;
            }
            let Some(name) = &group else {
                return Err(error("a setting before any [group]"));
            };
            let (left, value) = line
                .split_once('=')
                .ok_or_else(|| error("expected Section.Key=value"))?;
            let (section, key) = left
                .rsplit_once('.')
                .ok_or_else(|| error("expected Section.Key=value"))?;
            adjustments
                .groups
                .entry(name.clone())
                .or_default()
                .push(Setting::new(section.trim(), key.trim(), value.trim()));
        }
        Ok(adjustments)
    }

    /// The settings of a group; none if it has no `[group]`.
    pub fn settings(&self, group: &str) -> &[Setting] {
        self.groups.get(group).map_or(&[], Vec::as_slice)
    }

    pub fn set(&mut self, group: &str, settings: Vec<Setting>) {
        self.groups.insert(group.to_owned(), settings);
    }
}

impl fmt::Display for Adjustments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "# Settings put after the preset, group by group: edit a value, or delete a line\n# to leave it to the preset. Lines are Section.Key=value."
        )?;
        for (name, settings) in &self.groups {
            writeln!(f, "\n[{name}]")?;
            for setting in settings {
                writeln!(f, "{setting}")?;
            }
        }
        Ok(())
    }
}

/// The profile of a shot, the sidecar it has or else the default profile
/// RawTherapee would give it, and where it is written.
pub fn profile_of(shot: &Shot, config: Option<&Config>) -> Result<(Profile, PathBuf), String> {
    let sidecar = rawtherapee::sidecar(shot);
    match Profile::load(&sidecar.path) {
        Ok(profile) => Ok((profile, sidecar.path)),
        Err(pp3::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => {
            let config = config.ok_or("no sidecar and RawTherapee's settings are not readable")?;
            let profile = config
                .default_profile(sidecar.kind)
                .map_err(|err| err.to_string())?;
            Ok((profile, sidecar.path))
        }
        Err(err) => Err(format!("{}: {err}", sidecar.path.display())),
    }
}

/// Writes a profile, after a copy of the file as it was next to it
/// (`.bak`), kept if it exists: the sidecars are hours of hand work.
pub fn save_with_backup(profile: &Profile, path: &Path) -> io::Result<()> {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".bak");
    let backup = PathBuf::from(backup);
    if path.exists() && !backup.exists() {
        fs::copy(path, &backup)?;
    }
    profile.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn groups_are_the_folders_then_the_shots_left_alone() {
        let selection = tempfile::tempdir().unwrap();
        let dir = selection.path();
        touch(&dir.join("rose-B/B.RW2"));
        touch(&dir.join("rose-B/C.RW2"));
        touch(&dir.join("bleu-A/A.RW2"));
        touch(&dir.join("bleu-A/A.RW2.pp3"));
        touch(&dir.join("empty/note.txt"));
        touch(&dir.join(".hidden/H.RW2"));
        touch(&dir.join("Z.RW2"));
        touch(&dir.join("D.RW2"));

        let found = groups(dir).unwrap();
        let names: Vec<_> = found.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["bleu-A", "rose-B", "@D", "@Z"]);
        assert_eq!(found[1].shots.len(), 2);
        assert_eq!(found[2].shots.len(), 1);
    }

    #[test]
    fn the_exposure_follows_the_light_of_the_group() {
        let preset = Profile::parse("[Exposure]\nCompensation=-0.2\n").unwrap();
        // A group twice as dark as the shot of the preset: one stop more.
        let settings = automatic(&preset, Some(0.4), Some(0.2));
        assert_eq!(settings, [Setting::new("Exposure", "Compensation", "0.8")]);
        // As bright: the preset's own.
        let settings = automatic(&preset, Some(0.4), Some(0.4));
        assert_eq!(settings[0].value, "-0.2");
        // Nothing known, nothing to adjust.
        assert!(automatic(&preset, None, Some(0.2)).is_empty());
        assert!(automatic(&preset, Some(0.4), None).is_empty());
    }

    #[test]
    fn manual_settings_win_over_the_automatic_ones() {
        let automatic = vec![Setting::new("Exposure", "Compensation", "0.8")];
        let manual = [
            Setting::new("Exposure", "Compensation", "0.3"),
            Setting::new("Vibrance", "Pastels", "-40"),
        ];
        let merged = merge(automatic, &manual);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].value, "0.3");
        assert_eq!(merged[1].to_string(), "Vibrance.Pastels=-40");
    }

    #[test]
    fn adjustments_are_read_and_written_back() {
        let text = "# a comment\n\n[bleu-A]\nExposure.Compensation=0.3\nWhite Balance.Temperature=4000\n\n[@Z]\nShadows & Highlights.Shadows=60\n";
        let adjustments = Adjustments::parse(text).unwrap();
        assert_eq!(
            adjustments.settings("bleu-A"),
            [
                Setting::new("Exposure", "Compensation", "0.3"),
                Setting::new("White Balance", "Temperature", "4000"),
            ]
        );
        assert_eq!(
            adjustments.settings("@Z")[0].section,
            "Shadows & Highlights"
        );
        assert!(adjustments.settings("rose-B").is_empty());
        assert_eq!(
            Adjustments::parse(&adjustments.to_string()).unwrap(),
            adjustments
        );

        for (bad, line) in [
            ("Exposure.Compensation=1\n", "line 1"),
            ("[g]\nnothing\n", "line 2"),
            ("[g]\nNoDot=1\n", "line 2"),
        ] {
            let err = Adjustments::parse(bad).unwrap_err();
            assert!(err.starts_with(line), "{err}");
        }
    }

    #[test]
    fn a_shot_gets_the_preset_then_the_settings_of_its_group() {
        let preset = Profile::parse(
            "[Exposure]\nCompensation=-0.2\nContrast=10\n\n[Vibrance]\nPastels=-27\n",
        )
        .unwrap();
        let target = Profile::parse("[General]\nRank=4\n\n[Exposure]\nContrast=0\n\n[Crop]\nX=5\n\n[White Balance]\nTemperature=3000\n").unwrap();
        let settings = [
            Setting::new("Exposure", "Compensation", "0.8"),
            Setting::new("Vibrance", "Pastels", "-40"),
        ];
        let result = adapt(&target, &preset, &settings);
        assert_eq!(result.get("Exposure", "Contrast"), Some("10"));
        assert_eq!(result.get("Exposure", "Compensation"), Some("0.8"));
        assert_eq!(result.get("Vibrance", "Pastels"), Some("-40"));
        assert_eq!(result.get("General", "Rank"), Some("4"));
        assert_eq!(result.get("Crop", "X"), Some("5"));
        assert_eq!(result.get("White Balance", "Temperature"), Some("3000"));
    }

    #[test]
    fn a_backup_is_made_once_before_the_first_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.RW2.pp3");
        fs::write(&path, "[General]\nRank=3\n").unwrap();
        let backup = dir.path().join("A.RW2.pp3.bak");

        save_with_backup(&Profile::parse("[General]\nRank=1\n").unwrap(), &path).unwrap();
        save_with_backup(&Profile::parse("[General]\nRank=2\n").unwrap(), &path).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "[General]\nRank=3\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[General]\nRank=2\n");

        // No file yet: nothing to save a copy of.
        let new = dir.path().join("B.RW2.pp3");
        save_with_backup(&Profile::default(), &new).unwrap();
        assert!(!dir.path().join("B.RW2.pp3.bak").exists());
    }
}
