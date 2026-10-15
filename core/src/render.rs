//! The RAW of a shot developed by RawTherapee, with the shot's own sidecar.
//!
//! The development in [`picture`](crate::picture) is quick and approximate:
//! it has the exposure of the camera's preview but not the look of the
//! shot as its sidecar sets it, the demosaic of RawTherapee, its handling
//! of highlights and of noise. This one asks RawTherapee itself, through
//! `rawtherapee-cli`, which takes seconds (16 s for a sidecar with noise
//! reduction), so the result is kept in the cache directory, named after the
//! RAW and the sidecar and what they last changed, and only made again when
//! one of them changes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use image::ImageReader;

use crate::cache::cache_dir;
use crate::pairing::Shot;
use crate::picture::{Origin, Picture};
use crate::pp3;

/// The quality of the JPEG RawTherapee writes, at its best subsampling.
const QUALITY: &str = "-j95";
const SUBSAMPLING: &str = "-js3";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the shot has no RAW file")]
    NoRaw,
    #[error("no cache directory (set XDG_CACHE_HOME or HOME)")]
    NoCache,
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("rawtherapee-cli could not be run: {0}")]
    Run(io::Error),
    #[error("rawtherapee-cli failed ({0})")]
    Failed(String),
    #[error("{}: {source}", path.display())]
    Decode {
        path: PathBuf,
        source: image::ImageError,
    },
}

/// Whether `rawtherapee-cli` can be run, found once.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("rawtherapee-cli")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    })
}

/// What a file looks like when it last changed, for a name that changes
/// with it: its size and its date, in nanoseconds.
fn stamp(path: &Path) -> u64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as u64);
    meta.len().rotate_left(32) ^ modified
}

/// Where the render of a shot is kept: named after the RAW, with a number
/// that changes when the RAW or its sidecar does.
fn cache_path(dir: &Path, raw: &Path, sidecar: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let text = raw.to_string_lossy();
    for byte in text
        .bytes()
        .chain(stamp(raw).to_le_bytes())
        .chain(stamp(sidecar).to_le_bytes())
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let stem = raw
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shot".to_owned());
    dir.join(format!("{stem}-{hash:016x}.jpg"))
}

/// The render of a shot by `rawtherapee-cli`, from the cache or made now,
/// as a picture turned upright (RawTherapee does that itself).
pub fn render(shot: &Shot) -> Result<Picture, Error> {
    let dir = cache_dir().ok_or(Error::NoCache)?.join("render");
    render_with(Path::new("rawtherapee-cli"), shot, &dir)
}

/// The same with the program to run and the directory to keep it in.
pub fn render_with(program: &Path, shot: &Shot, dir: &Path) -> Result<Picture, Error> {
    let raw = shot.raw.as_deref().ok_or(Error::NoRaw)?;
    let sidecar = pp3::sidecar_path(raw);
    let out = cache_path(dir, raw, &sidecar);
    if !out.is_file() {
        fs::create_dir_all(dir).map_err(|source| Error::Io {
            path: dir.to_owned(),
            source,
        })?;
        // Written under another name and renamed, so that a render cut
        // short is never taken for a whole one.
        let partial = out.with_extension("partial.jpg");
        let mut command = Command::new(program);
        command
            .arg("-o")
            .arg(&partial)
            .args([QUALITY, SUBSAMPLING, "-Y", "-q"]);
        if sidecar.is_file() {
            command.arg("-p").arg(&sidecar);
        }
        let output = command
            .arg("-c")
            .arg(raw)
            .stdin(Stdio::null())
            .output()
            .map_err(Error::Run)?;
        if !output.status.success() || !partial.is_file() {
            let _ = fs::remove_file(&partial);
            let said = String::from_utf8_lossy(&output.stderr);
            let said = said.lines().last().unwrap_or("no message").trim();
            return Err(Error::Failed(format!("{}: {said}", output.status)));
        }
        fs::rename(&partial, &out).map_err(|source| Error::Io {
            path: out.clone(),
            source,
        })?;
    }
    let image = ImageReader::open(&out)
        .map_err(|source| Error::Io {
            path: out.clone(),
            source,
        })?
        .decode()
        .map_err(|source| Error::Decode {
            path: out.clone(),
            source,
        })?;
    Ok(Picture {
        image,
        origin: Origin::Rendered,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use image::{DynamicImage, RgbImage};

    use super::*;

    /// A program that stands for `rawtherapee-cli`: it copies `sample` to
    /// the file given after `-o`, and counts its runs in `log`.
    fn fake_program(dir: &Path, sample: &Path, log: &Path) -> PathBuf {
        let program = dir.join("fake-rawtherapee-cli");
        fs::write(
            &program,
            format!(
                "#!/bin/sh\necho run >> '{}'\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -o ]; then cp '{}' \"$2\"; fi\n  shift\ndone\n",
                log.display(),
                sample.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    fn shot_in(dir: &Path) -> Shot {
        let raw = dir.join("P1000001.RW2");
        fs::write(&raw, b"raw").unwrap();
        Shot {
            stem: "P1000001".into(),
            jpeg: None,
            raw: Some(raw),
        }
    }

    #[test]
    fn a_render_is_made_once_kept_and_made_again_when_the_sidecar_changes() {
        let work = tempfile::tempdir().unwrap();
        let sample = work.path().join("sample.jpg");
        DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 24, image::Rgb([10, 200, 30])))
            .save(&sample)
            .unwrap();
        let log = work.path().join("runs.log");
        let program = fake_program(work.path(), &sample, &log);
        let shot = shot_in(work.path());
        let cache = work.path().join("render");
        let runs = || fs::read_to_string(&log).map_or(0, |text| text.lines().count());

        let first = render_with(&program, &shot, &cache).unwrap();
        assert_eq!(first.origin, Origin::Rendered);
        assert_eq!((first.image.width(), first.image.height()), (32, 24));
        assert_eq!(runs(), 1);
        // The second time it comes from the cache: no run.
        render_with(&program, &shot, &cache).unwrap();
        assert_eq!(runs(), 1);
        // A sidecar written changes the name: it is made again.
        let sidecar = pp3::sidecar_path(shot.raw.as_deref().unwrap());
        fs::write(&sidecar, "[General]\nRank=3\n").unwrap();
        render_with(&program, &shot, &cache).unwrap();
        assert_eq!(runs(), 2);
        assert!(!cache.join("P1000001.partial.jpg").exists());
    }

    #[test]
    fn a_program_that_fails_leaves_nothing_and_says_why() {
        let work = tempfile::tempdir().unwrap();
        let program = work.path().join("fails");
        fs::write(&program, "#!/bin/sh\necho 'no profile' >&2\nexit 3\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let shot = shot_in(work.path());
        let cache = work.path().join("render");
        let err = render_with(&program, &shot, &cache)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no profile") && err.contains("3"), "{err}");
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        let jpeg_only = Shot {
            stem: "x".into(),
            jpeg: Some("x.jpg".into()),
            raw: None,
        };
        assert!(matches!(
            render_with(&program, &jpeg_only, &cache),
            Err(Error::NoRaw)
        ));
    }

    #[test]
    fn the_name_follows_the_raw_and_its_sidecar() {
        let work = tempfile::tempdir().unwrap();
        let raw = work.path().join("a.RW2");
        let sidecar = work.path().join("a.RW2.pp3");
        fs::write(&raw, b"1").unwrap();
        let alone = cache_path(work.path(), &raw, &sidecar);
        assert_eq!(alone, cache_path(work.path(), &raw, &sidecar));
        fs::write(&sidecar, b"x").unwrap();
        assert_ne!(alone, cache_path(work.path(), &raw, &sidecar));
        assert!(alone.to_string_lossy().contains("a-"));
    }
}
