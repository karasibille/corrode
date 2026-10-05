//! Pairing of the JPEG and RAW files written for the same shot.
//!
//! A camera set to JPEG+RAW writes two files with the same base name,
//! e.g. `DSCF1234.JPG` and `DSCF1234.RAF`. A [`Shot`] groups them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];

const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mos", "mrw", "nef", "nrw",
    "orf", "pef", "raf", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// Kind of image file, guessed from its extension (case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Raw,
}

impl Kind {
    /// Returns `None` for files that are neither JPEG nor RAW.
    pub fn of(path: &Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        if JPEG_EXTENSIONS.contains(&ext.as_str()) {
            Some(Kind::Jpeg)
        } else if RAW_EXTENSIONS.contains(&ext.as_str()) {
            Some(Kind::Raw)
        } else {
            None
        }
    }
}

/// The files of one shot. At least one of `jpeg` and `raw` is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    /// Base name shared by the files, without extension (`DSCF1234`).
    pub stem: OsString,
    pub jpeg: Option<PathBuf>,
    pub raw: Option<PathBuf>,
}

impl Shot {
    /// True when both the JPEG and the RAW are present.
    pub fn is_paired(&self) -> bool {
        self.jpeg.is_some() && self.raw.is_some()
    }
}

/// Groups JPEG and RAW paths by base name, sorted by base name.
///
/// The paths are expected to come from a single directory. Other files
/// (`.pp3`, `.xmp`, videos…) are ignored. If two files of the same kind
/// share a base name (`a.JPG` and `a.jpeg`), the smallest path is kept so
/// the result does not depend on the input order.
pub fn pair<I>(paths: I) -> Vec<Shot>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut shots: BTreeMap<OsString, Shot> = BTreeMap::new();

    for path in paths {
        let Some(kind) = Kind::of(&path) else {
            continue;
        };
        let Some(stem) = path.file_stem().map(|s| s.to_os_string()) else {
            continue;
        };

        let shot = shots.entry(stem.clone()).or_insert_with(|| Shot {
            stem,
            jpeg: None,
            raw: None,
        });
        let slot = match kind {
            Kind::Jpeg => &mut shot.jpeg,
            Kind::Raw => &mut shot.raw,
        };
        if slot.as_ref().is_none_or(|current| path < *current) {
            *slot = Some(path);
        }
    }

    shots.into_values().collect()
}

/// Lists the shots of a directory, without descending into subdirectories.
///
/// Hidden files are skipped: they are usually metadata left by other
/// systems (`._DSCF1234.JPG` written by macOS on removable drives).
pub fn scan_dir(dir: &Path) -> io::Result<Vec<Shot>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
        if !hidden && path.is_file() {
            paths.push(path);
        }
    }
    Ok(pair(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn shot(stem: &str, jpeg: Option<&str>, raw: Option<&str>) -> Shot {
        Shot {
            stem: stem.into(),
            jpeg: jpeg.map(PathBuf::from),
            raw: raw.map(PathBuf::from),
        }
    }

    #[test]
    fn pairs_files_with_the_same_base_name() {
        let shots = pair(paths(&["DSCF0001.JPG", "DSCF0001.RAF"]));
        assert_eq!(
            shots,
            [shot("DSCF0001", Some("DSCF0001.JPG"), Some("DSCF0001.RAF"))]
        );
        assert!(shots[0].is_paired());
    }

    #[test]
    fn extensions_are_case_insensitive() {
        let shots = pair(paths(&["a.jpeg", "a.Nef", "b.JpG", "b.cr3"]));
        assert_eq!(
            shots,
            [
                shot("a", Some("a.jpeg"), Some("a.Nef")),
                shot("b", Some("b.JpG"), Some("b.cr3")),
            ]
        );
    }

    #[test]
    fn keeps_unpaired_files() {
        let shots = pair(paths(&["only_jpeg.jpg", "only_raw.dng"]));
        assert_eq!(
            shots,
            [
                shot("only_jpeg", Some("only_jpeg.jpg"), None),
                shot("only_raw", None, Some("only_raw.dng")),
            ]
        );
        assert!(!shots[0].is_paired());
    }

    #[test]
    fn ignores_other_files() {
        let shots = pair(paths(&[
            "a.ARW.pp3",
            "a.xmp",
            "a.MOV",
            "notes.txt",
            "README",
            "a.ARW",
        ]));
        assert_eq!(shots, [shot("a", None, Some("a.ARW"))]);
    }

    #[test]
    fn sorts_by_base_name() {
        let shots = pair(paths(&["c.jpg", "a.jpg", "b.raf"]));
        let stems: Vec<_> = shots.iter().map(|s| s.stem.clone()).collect();
        assert_eq!(stems, ["a", "b", "c"]);
    }

    #[test]
    fn duplicate_kind_keeps_the_smallest_path_whatever_the_order() {
        let expected = [shot("a", Some("a.JPG"), Some("a.RAF"))];
        assert_eq!(pair(paths(&["a.jpeg", "a.JPG", "a.RAF"])), expected);
        assert_eq!(pair(paths(&["a.JPG", "a.RAF", "a.jpeg"])), expected);
    }

    #[test]
    fn scan_dir_lists_shots_of_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "IMG_0001.JPG",
            "IMG_0001.CR3",
            "IMG_0002.JPG",
            "._IMG_0002.JPG",
        ] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        // A directory named like a JPEG must not be taken for one.
        fs::create_dir(dir.path().join("IMG_0003.JPG")).unwrap();

        let shots = scan_dir(dir.path()).unwrap();

        let p = |name: &str| Some(dir.path().join(name));
        assert_eq!(
            shots,
            [
                Shot {
                    stem: "IMG_0001".into(),
                    jpeg: p("IMG_0001.JPG"),
                    raw: p("IMG_0001.CR3"),
                },
                Shot {
                    stem: "IMG_0002".into(),
                    jpeg: p("IMG_0002.JPG"),
                    raw: None,
                },
            ]
        );
    }

    #[test]
    fn scan_dir_fails_on_missing_directory() {
        assert!(scan_dir(Path::new("/nonexistent/corrode")).is_err());
    }
}
