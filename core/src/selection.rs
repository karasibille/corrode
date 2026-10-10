//! Sending the shots kept from a shoot to its selection folder, a
//! subfolder next to them, so that RawTherapee opens the selection alone
//! and the rest of the shoot stays where it is.
//!
//! A shot moves whole: its JPEG, its RAW and their sidecars, renamed
//! within the same file system, so nothing is copied and nothing is
//! lost half way: if one file cannot move, the ones already moved come
//! back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::pairing::Shot;
use crate::pp3;

/// The name of the selection folder, under the shoot's directory.
pub const FOLDER: &str = "selection";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{} already exists", path.display())]
    Exists { path: PathBuf },
}

/// The selection folder of a shoot's directory.
pub fn folder(dir: &Path) -> PathBuf {
    dir.join(FOLDER)
}

/// Whether a shot has been sent: its files are not in `dir` any more.
pub fn is_sent(shot: &Shot, dir: &Path) -> bool {
    let file = shot.raw.as_deref().or(shot.jpeg.as_deref());
    file.and_then(Path::parent) != Some(dir)
}

/// Moves a shot, with its sidecars, into `into`, which is created if
/// needed, and gives the shot at its new place. Nothing is overwritten:
/// a file already there is an error, and nothing moves.
pub fn send(shot: &Shot, into: &Path) -> Result<Shot, Error> {
    let io = |path: &Path, source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    fs::create_dir_all(into).map_err(|err| io(into, err))?;
    // Every file of the shot, images first: a sidecar follows its image.
    let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    for image in [&shot.jpeg, &shot.raw].into_iter().flatten() {
        let name = image.file_name().ok_or_else(|| Error::Io {
            path: image.clone(),
            source: io::Error::other("no file name"),
        })?;
        moves.push((image.clone(), into.join(name)));
        let sidecar = pp3::sidecar_path(image);
        if sidecar.is_file() {
            let target = into.join(sidecar.file_name().expect("a sidecar has a name"));
            moves.push((sidecar, target));
        }
    }
    if let Some((_, target)) = moves.iter().find(|(_, target)| target.exists()) {
        return Err(Error::Exists {
            path: target.clone(),
        });
    }
    let mut done: Vec<&(PathBuf, PathBuf)> = Vec::new();
    for step in &moves {
        if let Err(err) = fs::rename(&step.0, &step.1) {
            // Bring back what moved, so that the shot stays whole.
            for (from, to) in done.into_iter().rev() {
                let _ = fs::rename(to, from);
            }
            return Err(io(&step.0, err));
        }
        done.push(step);
    }
    let moved = |image: &Option<PathBuf>| {
        image
            .as_ref()
            .map(|path| into.join(path.file_name().expect("checked above")))
    };
    Ok(Shot {
        stem: shot.stem.clone(),
        jpeg: moved(&shot.jpeg),
        raw: moved(&shot.raw),
    })
}

/// Shots sent into one folder by `sort_into_groups`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sorted {
    pub folder: PathBuf,
    /// The shots at their new place.
    pub shots: Vec<Shot>,
}

/// What `sort_into_groups` did: the folders filled, and the error that
/// stopped it, if any.
#[derive(Debug)]
pub struct Sorting {
    pub sorted: Vec<Sorted>,
    pub error: Option<Error>,
}

/// Shots to sort together, and what to call their folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The start of the folder's name, `bleu` for instance.
    pub label: String,
    /// Indices into the shots given to `sort_into_groups`.
    pub members: Vec<usize>,
}

/// Sorts shots into subfolders of `dir`, one per batch of at least two
/// shots, named after its label and the stem of its first shot
/// (`bleu-_1117041`); shots alone stay where they are. A folder that
/// already exists is never reused, a number is added to the name. Each
/// shot moves whole, as with `send`; if one fails, the sorting stops
/// there: what was sorted before stays sorted, and is given with the
/// error.
pub fn sort_into_groups(dir: &Path, shots: &[Shot], batches: &[Batch]) -> Sorting {
    let mut sorted = Vec::new();
    for batch in batches.iter().filter(|batch| batch.members.len() >= 2) {
        let first = batch.members.iter().min().expect("at least two members");
        let stem = shots[*first].stem.to_string_lossy();
        let name = if batch.label.is_empty() {
            stem.into_owned()
        } else {
            format!("{}-{stem}", batch.label)
        };
        let mut folder = dir.join(&name);
        for number in 2.. {
            if !folder.exists() {
                break;
            }
            folder = dir.join(format!("{name}-{number}"));
        }
        let mut moved = Vec::new();
        for &index in &batch.members {
            match send(&shots[index], &folder) {
                Ok(shot) => moved.push(shot),
                Err(error) => {
                    if !moved.is_empty() {
                        sorted.push(Sorted {
                            folder,
                            shots: moved,
                        });
                    }
                    return Sorting {
                        sorted,
                        error: Some(error),
                    };
                }
            }
        }
        sorted.push(Sorted {
            folder,
            shots: moved,
        });
    }
    Sorting {
        sorted,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(dir: &Path, with_jpeg: bool) -> Shot {
        Shot {
            stem: "P1011259".into(),
            jpeg: with_jpeg.then(|| dir.join("P1011259.JPG")),
            raw: Some(dir.join("P1011259.RW2")),
        }
    }

    #[test]
    fn a_shot_moves_whole_with_its_sidecars() {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        let shot = shot(dir, true);
        fs::write(dir.join("P1011259.JPG"), b"jpeg").unwrap();
        fs::write(dir.join("P1011259.RW2"), b"raw").unwrap();
        fs::write(dir.join("P1011259.RW2.pp3"), b"[General]\nRank=3\n").unwrap();
        fs::write(dir.join("P1011260.RW2"), b"other").unwrap();

        let sent = send(&shot, &folder(dir)).unwrap();
        let selection = dir.join("selection");
        assert_eq!(sent.jpeg, Some(selection.join("P1011259.JPG")));
        assert_eq!(sent.raw, Some(selection.join("P1011259.RW2")));
        assert_eq!(
            fs::read(selection.join("P1011259.RW2.pp3")).unwrap(),
            b"[General]\nRank=3\n"
        );
        assert!(!dir.join("P1011259.JPG").exists());
        assert!(!dir.join("P1011259.RW2.pp3").exists());
        assert!(dir.join("P1011260.RW2").exists());
        assert!(is_sent(&sent, dir));
        assert!(!is_sent(&shot, dir));
    }

    #[test]
    fn nothing_is_overwritten_and_nothing_moves_on_a_clash() {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        let shot = shot(dir, true);
        fs::write(dir.join("P1011259.JPG"), b"jpeg").unwrap();
        fs::write(dir.join("P1011259.RW2"), b"raw").unwrap();
        fs::create_dir_all(dir.join("selection")).unwrap();
        fs::write(dir.join("selection/P1011259.RW2"), b"older").unwrap();

        let err = send(&shot, &folder(dir)).unwrap_err();
        assert!(matches!(err, Error::Exists { .. }), "{err}");
        assert!(dir.join("P1011259.JPG").exists());
        assert!(dir.join("P1011259.RW2").exists());
        assert_eq!(
            fs::read(dir.join("selection/P1011259.RW2")).unwrap(),
            b"older"
        );
    }

    #[test]
    fn a_failed_move_brings_the_moved_files_back() {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        // The RAW is missing: its move fails after the JPEG moved.
        let shot = shot(dir, true);
        fs::write(dir.join("P1011259.JPG"), b"jpeg").unwrap();

        let err = send(&shot, &folder(dir)).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err}");
        assert!(dir.join("P1011259.JPG").exists());
        assert!(!dir.join("selection/P1011259.JPG").exists());
    }

    fn batch(label: &str, members: &[usize]) -> Batch {
        Batch {
            label: label.into(),
            members: members.to_vec(),
        }
    }

    #[test]
    fn groups_go_to_named_folders_and_lone_shots_stay() {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        let shots: Vec<Shot> = ["A", "B", "C", "D"]
            .iter()
            .map(|stem| {
                let raw = dir.join(format!("{stem}.RW2"));
                fs::write(&raw, b"raw").unwrap();
                fs::write(dir.join(format!("{stem}.RW2.pp3")), b"[General]\n").unwrap();
                Shot {
                    stem: (*stem).into(),
                    jpeg: None,
                    raw: Some(raw),
                }
            })
            .collect();
        // A folder of the same name is there already: it is not reused.
        fs::create_dir(dir.join("bleu-A")).unwrap();

        let sorted = sort_into_groups(
            dir,
            &shots,
            &[batch("bleu", &[0, 2]), batch("rose", &[1]), batch("", &[3])],
        );
        assert!(sorted.error.is_none());
        let sorted = sorted.sorted;
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].folder, dir.join("bleu-A-2"));
        assert!(dir.join("bleu-A-2/A.RW2").exists());
        assert!(dir.join("bleu-A-2/C.RW2.pp3").exists());
        assert!(dir.join("bleu-A").read_dir().unwrap().next().is_none());
        assert!(dir.join("B.RW2").exists());
        assert!(dir.join("D.RW2").exists());

        // The shots are gone from where they were: nothing more can move.
        let again = sort_into_groups(dir, &shots, &[batch("bleu", &[0, 2])]);
        assert!(matches!(again.error, Some(Error::Io { .. })));
        assert!(again.sorted.is_empty());
    }

    #[test]
    fn a_sorted_group_gives_the_shots_at_their_new_place() {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        let shots: Vec<Shot> = ["A", "B"]
            .iter()
            .map(|stem| {
                let raw = dir.join(format!("{stem}.RW2"));
                fs::write(&raw, b"raw").unwrap();
                Shot {
                    stem: (*stem).into(),
                    jpeg: None,
                    raw: Some(raw),
                }
            })
            .collect();

        let sorted = sort_into_groups(dir, &shots, &[batch("orange", &[1, 0])]).sorted;
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].folder, dir.join("orange-A"));
        assert_eq!(
            sorted[0].shots[1].raw,
            Some(dir.join("orange-A").join("A.RW2"))
        );
    }
}
