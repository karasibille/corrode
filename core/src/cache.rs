//! A cache of what the head of each file tells, so that a directory
//! already seen opens at once instead of reading every file again, which
//! takes seconds to half a minute on a spinning disk.
//!
//! One file per photo directory, in `$XDG_CACHE_HOME/corrode` (usually
//! `~/.cache/corrode`), holds the shooting information and the thumbnail
//! of each shot: about 4.5 KB per shot. An entry is reused as long as the
//! file it came from has the same size and modification time. Marks are
//! never cached: RawTherapee may change the sidecars at any time.
//!
//! A missing or unreadable cache is an empty one; it is only a shortcut.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};

use crate::exif::Exif;
use crate::pairing::Shot;

/// Bumped whenever the format changes: an older cache is then ignored.
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    /// Size and modification time of the file the entry came from.
    size: u64,
    modified: (i64, u32),
    exif: Exif,
    /// The thumbnail, upright, as JPEG data.
    thumbnail: Option<Vec<u8>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Contents {
    version: u32,
    /// Keyed by the name of the file read for the shot.
    entries: HashMap<String, Entry>,
}

/// The cache of one photo directory.
#[derive(Debug)]
pub struct Cache {
    /// Where it is saved; `None` when no cache directory can be found.
    path: Option<PathBuf>,
    contents: Contents,
    dirty: bool,
}

/// The directory of the caches: `$XDG_CACHE_HOME/corrode`, else
/// `~/.cache/corrode`.
pub fn cache_dir() -> Option<PathBuf> {
    let base = match env::var_os("XDG_CACHE_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?).join(".cache"),
    };
    Some(base.join("corrode"))
}

/// The cache file of a photo directory: its absolute path, made into a
/// file name.
fn cache_file(cache_dir: &Path, photo_dir: &Path) -> PathBuf {
    let absolute = photo_dir
        .canonicalize()
        .unwrap_or_else(|_| photo_dir.to_path_buf());
    let text = absolute.to_string_lossy();
    let mut name: String = text
        .chars()
        .map(|c| if c == '/' || c == '\\' { '%' } else { c })
        .collect();
    // File names are limited to 255 bytes; a hash keeps long paths apart.
    if name.len() > 200 {
        let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        let cut = name.char_indices().nth(150).map_or(name.len(), |(i, _)| i);
        name = format!("{}%{hash:016x}", &name[..cut]);
    }
    cache_dir.join(format!("{name}.corrode"))
}

/// The file whose head is read for a shot: the RAW, else the JPEG.
fn key_file(shot: &Shot) -> &Path {
    shot.raw
        .as_deref()
        .or(shot.jpeg.as_deref())
        .expect("a shot has a JPEG or a RAW")
}

/// Size and modification time of a file, what tells whether it changed.
fn stamp(path: &Path) -> io::Result<(u64, (i64, u32))> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let modified = match modified.duration_since(UNIX_EPOCH) {
        Ok(since) => (
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            since.subsec_nanos(),
        ),
        Err(before) => (
            -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
            before.duration().subsec_nanos(),
        ),
    };
    Ok((metadata.len(), modified))
}

impl Cache {
    /// Opens the cache of a photo directory, empty if there is none yet or
    /// it cannot be read.
    pub fn open(photo_dir: &Path) -> Cache {
        let path = cache_dir().map(|dir| cache_file(&dir, photo_dir));
        let contents = path
            .as_deref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|data| postcard::from_bytes::<Contents>(&data).ok())
            .filter(|contents| contents.version == VERSION)
            .unwrap_or_default();
        Cache {
            path,
            contents,
            dirty: false,
        }
    }

    /// A cache that is never saved, for tests and tools.
    pub fn in_memory() -> Cache {
        Cache {
            path: None,
            contents: Contents::default(),
            dirty: false,
        }
    }

    pub fn len(&self) -> usize {
        self.contents.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contents.entries.is_empty()
    }

    /// What is cached for a shot, if its file has not changed since.
    pub fn get(&self, shot: &Shot) -> Option<(Exif, Option<DynamicImage>)> {
        let file = key_file(shot);
        let entry = self.contents.entries.get(file.file_name()?.to_str()?)?;
        let (size, modified) = stamp(file).ok()?;
        if entry.size != size || entry.modified != modified {
            return None;
        }
        let thumbnail = entry
            .thumbnail
            .as_deref()
            .and_then(|data| image::load_from_memory_with_format(data, ImageFormat::Jpeg).ok());
        Some((entry.exif.clone(), thumbnail))
    }

    /// Remembers what the head of a shot's file told.
    pub fn insert(&mut self, shot: &Shot, exif: &Exif, thumbnail: Option<&DynamicImage>) {
        let file = key_file(shot);
        let (Some(name), Ok((size, modified))) =
            (file.file_name().and_then(|name| name.to_str()), stamp(file))
        else {
            return;
        };
        let thumbnail = thumbnail.and_then(|image| {
            let mut data = Cursor::new(Vec::new());
            JpegEncoder::new_with_quality(&mut data, 90)
                .encode_image(&image.to_rgb8())
                .ok()?;
            Some(data.into_inner())
        });
        let entry = Entry {
            size,
            modified,
            exif: exif.clone(),
            thumbnail,
        };
        if self.contents.entries.get(name) != Some(&entry) {
            self.contents.entries.insert(name.to_owned(), entry);
            self.dirty = true;
        }
    }

    /// Writes the cache if it changed, through a temporary file so that a
    /// crash never leaves a truncated cache.
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        self.contents.version = VERSION;
        let data = postcard::to_allocvec(&self.contents).map_err(io::Error::other)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let mut file = tempfile::Builder::new()
            .prefix(".corrode-")
            .tempfile_in(dir)?;
        file.write_all(&data)?;
        file.persist(path).map_err(|err| err.error)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::Duration;

    use image::{GrayImage, Luma};

    use super::*;

    fn shot(dir: &Path, name: &str) -> Shot {
        Shot {
            stem: name.into(),
            jpeg: None,
            raw: Some(dir.join(format!("{name}.RW2"))),
        }
    }

    fn exif(iso: u32) -> Exif {
        Exif {
            iso: Some(iso),
            taken_ms: Some(1_596_441_815_000),
            lens: Some("Leica DG Summilux 15mm".into()),
            ..Exif::default()
        }
    }

    /// A cache saved under a temporary cache directory.
    fn cache_in(cache_dir: &Path, photo_dir: &Path) -> Cache {
        Cache {
            path: Some(cache_file(cache_dir, photo_dir)),
            contents: Contents::default(),
            dirty: false,
        }
    }

    #[test]
    fn remembers_exif_and_thumbnail_across_a_save() {
        let photos = tempfile::tempdir().unwrap();
        let caches = tempfile::tempdir().unwrap();
        let shot = shot(photos.path(), "P1011259");
        fs::write(shot.raw.as_ref().unwrap(), b"raw data").unwrap();
        let thumbnail = DynamicImage::ImageLuma8(GrayImage::from_pixel(16, 12, Luma([200])));

        let mut cache = cache_in(caches.path(), photos.path());
        assert!(cache.get(&shot).is_none());
        cache.insert(&shot, &exif(1600), Some(&thumbnail));
        cache.save().unwrap();

        let path = cache_file(caches.path(), photos.path());
        assert!(path.is_file());
        let reloaded = Cache {
            contents: postcard::from_bytes(&fs::read(&path).unwrap()).unwrap(),
            ..cache_in(caches.path(), photos.path())
        };
        let (found, image) = reloaded.get(&shot).unwrap();
        assert_eq!(found, exif(1600));
        let image = image.unwrap();
        assert_eq!((image.width(), image.height()), (16, 12));
        assert!(image.to_luma8().get_pixel(8, 6)[0].abs_diff(200) < 4);
    }

    #[test]
    fn a_changed_file_is_not_served_from_the_cache() {
        let photos = tempfile::tempdir().unwrap();
        let shot = shot(photos.path(), "P1011259");
        let raw = shot.raw.as_ref().unwrap();
        fs::write(raw, b"raw data").unwrap();
        let mut cache = Cache::in_memory();
        cache.insert(&shot, &exif(200), None);
        assert!(cache.get(&shot).is_some());

        fs::write(raw, b"other raw data").unwrap(); // size changes
        assert!(cache.get(&shot).is_none());

        cache.insert(&shot, &exif(400), None);
        thread::sleep(Duration::from_millis(20));
        fs::write(raw, b"other raw data").unwrap(); // only the time changes
        assert!(cache.get(&shot).is_none());
        fs::remove_file(raw).unwrap();
        assert!(cache.get(&shot).is_none());
    }

    #[test]
    fn a_missing_or_corrupt_cache_is_empty() {
        let caches = tempfile::tempdir().unwrap();
        let photos = tempfile::tempdir().unwrap();
        let path = cache_file(caches.path(), photos.path());
        fs::create_dir_all(caches.path()).unwrap();
        fs::write(&path, b"not a cache").unwrap();
        let data = fs::read(&path).unwrap();
        assert!(postcard::from_bytes::<Contents>(&data).is_err());
        let cache = cache_in(caches.path(), photos.path());
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_files_are_named_after_the_directory() {
        let caches = Path::new("/tmp/caches");
        let file = cache_file(caches, Path::new("/nonexistent/photos/2026 concert"));
        assert_eq!(
            file,
            Path::new("/tmp/caches/%nonexistent%photos%2026 concert.corrode")
        );
        let long = format!("/nonexistent/{}", "x".repeat(300));
        let name = cache_file(caches, Path::new(&long));
        assert!(name.file_name().unwrap().len() < 255);
        assert_ne!(name, cache_file(caches, Path::new(&format!("{long}y"))));
    }

    #[test]
    fn saving_an_unchanged_cache_writes_nothing() {
        let caches = tempfile::tempdir().unwrap();
        let photos = tempfile::tempdir().unwrap();
        let mut cache = cache_in(caches.path(), photos.path());
        cache.save().unwrap();
        assert!(!cache_file(caches.path(), photos.path()).exists());
    }
}
