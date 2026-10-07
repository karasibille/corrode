//! The user's RawTherapee setup: where its settings live, which default
//! profile it applies to a new image, and which sidecar holds a shot's marks.

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crate::marks::Marks;
use crate::pairing::{Kind, Shot};
use crate::pp3::{self, Profile};

/// Where distribution packages install RawTherapee's bundled profiles,
/// written `${G}` in its settings.
pub const GLOBAL_PROFILES: &str = "/usr/share/rawtherapee/profiles";

/// RawTherapee's built-in neutral profile, which has no file.
const NEUTRAL: &str = "Neutral";
/// Rule-based profile selection, which corrode does not evaluate.
const DYNAMIC: &str = "Dynamic";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No `HOME` to locate RawTherapee's settings from.
    #[error("cannot locate RawTherapee's settings: HOME is not set")]
    NoConfigDir,
    /// RawTherapee's options file could not be read or parsed.
    #[error("{}: {source}", path.display())]
    Options { path: PathBuf, source: pp3::Error },
    /// A setting corrode needs is missing from the options file.
    #[error("RawTherapee setting {0} is missing")]
    MissingSetting(&'static str),
    /// The default profile named in the options was not found.
    #[error("default profile not found: {0}")]
    ProfileNotFound(String),
    /// The default profile is chosen by rules (`Dynamic`), which corrode
    /// cannot reproduce: creating a sidecar would risk the wrong rendering.
    #[error("dynamic default profiles are not supported")]
    DynamicProfile,
    /// A sidecar or profile could not be read or written.
    #[error("{}: {source}", path.display())]
    Pp3 { path: PathBuf, source: pp3::Error },
}

/// RawTherapee's settings directory: `$RT_SETTINGS`, else
/// `$XDG_CONFIG_HOME/RawTherapee`, else `~/.config/RawTherapee`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("RT_SETTINGS").filter(|dir| !dir.is_empty()) {
        return Some(dir.into());
    }
    let config = match env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?).join(".config"),
    };
    Some(config.join("RawTherapee"))
}

/// The part of RawTherapee's settings corrode relies on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    raw_default: String,
    img_default: String,
    user_profiles: PathBuf,
    global_profiles: PathBuf,
}

impl Config {
    /// Reads the user's RawTherapee settings.
    pub fn load() -> Result<Config, Error> {
        let dir = config_dir().ok_or(Error::NoConfigDir)?;
        Config::load_from(&dir, Path::new(GLOBAL_PROFILES))
    }

    /// Reads the settings of the given directory, `global_profiles` being
    /// where the bundled profiles are installed.
    pub fn load_from(config_dir: &Path, global_profiles: &Path) -> Result<Config, Error> {
        let path = config_dir.join("options");
        let options = Profile::load(&path).map_err(|source| Error::Options { path, source })?;
        let setting = |key| {
            options
                .get("Profiles", key)
                .map(str::to_owned)
                .ok_or(Error::MissingSetting(key))
        };
        // A relative directory is relative to the settings directory;
        // joining an absolute one replaces it.
        let user_profiles =
            config_dir.join(options.get("Profiles", "Directory").unwrap_or("profiles"));

        Ok(Config {
            raw_default: setting("RawDefault")?,
            img_default: setting("ImgDefault")?,
            user_profiles,
            global_profiles: global_profiles.to_owned(),
        })
    }

    /// The profile RawTherapee applies to an image of this kind that has
    /// no sidecar yet.
    pub fn default_profile(&self, kind: Kind) -> Result<Profile, Error> {
        let name = match kind {
            Kind::Raw => &self.raw_default,
            Kind::Jpeg => &self.img_default,
        };
        match name.as_str() {
            NEUTRAL => return Ok(Profile::default()),
            DYNAMIC => return Err(Error::DynamicProfile),
            _ => {}
        }
        let path = self
            .profile_path(name)
            .ok_or_else(|| Error::ProfileNotFound(name.clone()))?;
        Profile::load(&path).map_err(|source| Error::Pp3 { path, source })
    }

    /// Finds a profile file as RawTherapee does: `${U}/` and `${G}/` select
    /// the user or the bundled profiles, a bare name is looked up in both,
    /// user profiles first. Names are given without the `.pp3` extension.
    fn profile_path(&self, name: &str) -> Option<PathBuf> {
        let find = |dir: &Path, name: &str| {
            let path = dir.join(format!("{name}.pp3"));
            path.is_file().then_some(path)
        };
        if let Some(name) = name.strip_prefix("${U}/") {
            return find(&self.user_profiles, name);
        }
        if let Some(name) = name.strip_prefix("${G}/") {
            return find(&self.global_profiles, name);
        }
        find(&self.user_profiles, name).or_else(|| find(&self.global_profiles, name))
    }

    /// Writes the marks of a shot. A missing sidecar is created from the
    /// default profile of its image, so RawTherapee renders the photo as it
    /// would have without corrode.
    pub fn write_marks(&self, shot: &Shot, marks: &Marks) -> Result<(), Error> {
        let Sidecar { path, kind } = sidecar(shot);
        let base = if path.exists() {
            Profile::default()
        } else {
            self.default_profile(kind)?
        };
        pp3::write_marks(&path, marks, &base).map_err(|source| Error::Pp3 { path, source })
    }
}

/// Opens a file in RawTherapee's editor, or a directory in its file
/// browser, without waiting for it to close. Its output is discarded, so
/// that it does not draw over a terminal interface.
pub fn open(path: &Path) -> io::Result<()> {
    let mut child = Command::new("rawtherapee")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Collect its exit status when it closes, so that no zombie is left.
    thread::spawn(move || child.wait());
    Ok(())
}

/// The file of a shot to open in RawTherapee: its RAW, else its JPEG.
pub fn file_to_open(shot: &Shot) -> &Path {
    shot.raw
        .as_deref()
        .or(shot.jpeg.as_deref())
        .expect("a shot has a JPEG or a RAW")
}

/// The sidecar holding a shot's marks, and the kind of image it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub path: PathBuf,
    pub kind: Kind,
}

/// Picks the sidecar of a shot: the RAW's if it exists, else the JPEG's if
/// it exists, else the one to create, on the RAW when there is one.
pub fn sidecar(shot: &Shot) -> Sidecar {
    let candidates: Vec<Sidecar> = [(&shot.raw, Kind::Raw), (&shot.jpeg, Kind::Jpeg)]
        .into_iter()
        .filter_map(|(image, kind)| {
            Some(Sidecar {
                path: pp3::sidecar_path(image.as_deref()?),
                kind,
            })
        })
        .collect();
    let existing = candidates.iter().position(|sidecar| sidecar.path.exists());
    candidates
        .into_iter()
        .nth(existing.unwrap_or(0))
        .expect("a shot has a JPEG or a RAW")
}

/// The marks of a shot; a shot without sidecar is unmarked.
pub fn read_marks(shot: &Shot) -> Result<Marks, Error> {
    let Sidecar { path, .. } = sidecar(shot);
    let marks = match Profile::load(&path) {
        Ok(profile) => profile.marks(),
        Err(pp3::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(Marks::default()),
        Err(err) => Err(err),
    };
    marks.map_err(|source| Error::Pp3 { path, source })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::marks::ColorLabel;

    const RAW_DEFAULT: &str = "[Exposure]\nHistogramMatching=true\n";
    const USER_PROFILE: &str = "[Sharpening]\nEnabled=true\n";

    /// A settings directory and a bundled profiles directory in a temporary
    /// directory, with the given `[Profiles]` settings.
    struct Setup {
        root: tempfile::TempDir,
    }

    impl Setup {
        fn new(profiles_settings: &str) -> Setup {
            let root = tempfile::tempdir().unwrap();
            let config = root.path().join("config");
            let global = root.path().join("global");
            fs::create_dir_all(config.join("profiles")).unwrap();
            fs::create_dir_all(global.join("Non-raw")).unwrap();
            fs::write(
                config.join("options"),
                format!("[General]\nVersion=5.11\n\n[Profiles]\n{profiles_settings}"),
            )
            .unwrap();
            fs::write(global.join("Auto-Matched Curve - ISO Low.pp3"), RAW_DEFAULT).unwrap();
            fs::write(
                global.join("Non-raw/Brighten.pp3"),
                "[Exposure]\nBrightness=10\n",
            )
            .unwrap();
            fs::write(global.join("Shared.pp3"), "[Global]\nA=1\n").unwrap();
            fs::write(config.join("profiles/Mine.pp3"), USER_PROFILE).unwrap();
            fs::write(config.join("profiles/Shared.pp3"), "[User]\nA=1\n").unwrap();
            Setup { root }
        }

        fn config(&self) -> Result<Config, Error> {
            Config::load_from(
                &self.root.path().join("config"),
                &self.root.path().join("global"),
            )
        }

        fn default_profile(&self, kind: Kind) -> Result<String, Error> {
            Ok(self.config()?.default_profile(kind)?.to_string())
        }
    }

    fn real_like(raw: &str, img: &str) -> Setup {
        Setup::new(&format!(
            "Directory=profiles\nRawDefault={raw}\nImgDefault={img}\n"
        ))
    }

    #[test]
    fn resolves_bundled_and_user_profiles() {
        let setup = real_like("${G}/Auto-Matched Curve - ISO Low", "${U}/Mine");
        assert_eq!(setup.default_profile(Kind::Raw).unwrap(), RAW_DEFAULT);
        assert_eq!(setup.default_profile(Kind::Jpeg).unwrap(), USER_PROFILE);

        let setup = real_like("${G}/Non-raw/Brighten", "Neutral");
        assert!(
            setup
                .default_profile(Kind::Raw)
                .unwrap()
                .contains("Brightness=10")
        );
    }

    #[test]
    fn bare_names_are_looked_up_in_user_profiles_first() {
        let setup = real_like("Shared", "Auto-Matched Curve - ISO Low");
        assert!(setup.default_profile(Kind::Raw).unwrap().contains("[User]"));
        assert_eq!(setup.default_profile(Kind::Jpeg).unwrap(), RAW_DEFAULT);
    }

    #[test]
    fn neutral_is_the_empty_profile() {
        let setup = real_like("Neutral", "Neutral");
        assert_eq!(setup.default_profile(Kind::Jpeg).unwrap(), "");
    }

    #[test]
    fn reports_dynamic_missing_and_unknown_profiles() {
        let setup = real_like("Dynamic", "${G}/Nope");
        assert!(matches!(
            setup.default_profile(Kind::Raw),
            Err(Error::DynamicProfile)
        ));
        assert!(matches!(
            setup.default_profile(Kind::Jpeg),
            Err(Error::ProfileNotFound(name)) if name == "${G}/Nope"
        ));

        let setup = Setup::new("RawDefault=Neutral\n");
        assert!(matches!(
            setup.config(),
            Err(Error::MissingSetting("ImgDefault"))
        ));

        let missing = Config::load_from(Path::new("/nonexistent/corrode"), Path::new("/"));
        assert!(matches!(missing, Err(Error::Options { .. })));
    }

    #[test]
    fn user_profiles_directory_may_be_absolute() {
        let setup = Setup::new("");
        let elsewhere = setup.root.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("Far.pp3"), "[Far]\nA=1\n").unwrap();
        fs::write(
            setup.root.path().join("config/options"),
            format!(
                "[Profiles]\nDirectory={}\nRawDefault=${{U}}/Far\nImgDefault=Neutral\n",
                elsewhere.display()
            ),
        )
        .unwrap();
        assert!(setup.default_profile(Kind::Raw).unwrap().contains("[Far]"));
    }

    fn shot(dir: &Path, jpeg: Option<&str>, raw: Option<&str>) -> Shot {
        Shot {
            stem: "P1011259".into(),
            jpeg: jpeg.map(|name| dir.join(name)),
            raw: raw.map(|name| dir.join(name)),
        }
    }

    #[test]
    fn sidecar_prefers_an_existing_raw_then_jpeg_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let pair = shot(dir, Some("P1011259.JPG"), Some("P1011259.RW2"));
        let raw = Sidecar {
            path: dir.join("P1011259.RW2.pp3"),
            kind: Kind::Raw,
        };
        let jpeg = Sidecar {
            path: dir.join("P1011259.JPG.pp3"),
            kind: Kind::Jpeg,
        };

        assert_eq!(sidecar(&pair), raw, "none exists: create on the RAW");
        fs::write(&jpeg.path, "").unwrap();
        assert_eq!(sidecar(&pair), jpeg, "only the JPEG's exists");
        fs::write(&raw.path, "").unwrap();
        assert_eq!(sidecar(&pair), raw, "both exist");

        let jpeg_only = shot(dir, Some("P1011259.JPG"), None);
        assert_eq!(sidecar(&jpeg_only), jpeg);
    }

    #[test]
    fn write_marks_creates_the_sidecar_from_the_default_profile_of_its_image() {
        let setup = real_like("${G}/Auto-Matched Curve - ISO Low", "${U}/Mine");
        let config = setup.config().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let marks = Marks {
            rank: 3,
            color: ColorLabel::Red,
            in_trash: false,
        };

        let pair = shot(dir.path(), Some("P1011259.JPG"), Some("P1011259.RW2"));
        config.write_marks(&pair, &marks).unwrap();
        let text = fs::read_to_string(dir.path().join("P1011259.RW2.pp3")).unwrap();
        assert!(text.starts_with(RAW_DEFAULT));
        assert_eq!(read_marks(&pair).unwrap(), marks);

        let jpeg_only = shot(dir.path(), Some("P1011260.JPG"), None);
        config.write_marks(&jpeg_only, &marks).unwrap();
        let text = fs::read_to_string(dir.path().join("P1011260.JPG.pp3")).unwrap();
        assert!(text.starts_with(USER_PROFILE));
    }

    #[test]
    fn write_marks_does_not_need_the_default_profile_for_an_existing_sidecar() {
        let setup = real_like("Dynamic", "Dynamic");
        let config = setup.config().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let pair = shot(dir.path(), None, Some("P1011259.RW2"));
        fs::write(
            dir.path().join("P1011259.RW2.pp3"),
            "[Exposure]\nAuto=false\n",
        )
        .unwrap();

        config.write_marks(&pair, &Marks::default()).unwrap();
        let unmarked = shot(dir.path(), None, Some("P1011260.RW2"));
        assert!(matches!(
            config.write_marks(&unmarked, &Marks::default()),
            Err(Error::DynamicProfile)
        ));
    }

    #[test]
    fn the_raw_is_opened_rather_than_the_jpeg() {
        let dir = Path::new("/photos");
        assert_eq!(
            file_to_open(&shot(dir, Some("P1011259.JPG"), Some("P1011259.RW2"))),
            dir.join("P1011259.RW2")
        );
        assert_eq!(
            file_to_open(&shot(dir, Some("P1011259.JPG"), None)),
            dir.join("P1011259.JPG")
        );
    }

    #[test]
    fn read_marks_of_a_shot_without_sidecar_is_unmarked() {
        let dir = tempfile::tempdir().unwrap();
        let pair = shot(dir.path(), Some("P1011259.JPG"), Some("P1011259.RW2"));
        assert_eq!(read_marks(&pair).unwrap(), Marks::default());

        fs::write(dir.path().join("P1011259.RW2.pp3"), [0x8b, 0x00]).unwrap();
        assert!(matches!(
            read_marks(&pair),
            Err(Error::Pp3 {
                source: pp3::Error::NotText,
                ..
            })
        ));
    }
}
