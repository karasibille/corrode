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
}
