//! What the culling viewer has done in the background: reading the head
//! of a shot's files, decoding its preview or its full picture, assessing
//! it, removing its light bands. Shots are named by a stable id, so that
//! the list on screen can change order or grow while jobs run.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use corrode_core::cache::Cache as MetaCache;
use corrode_core::debanding::{self, Pattern};
use corrode_core::exif::{Exif, Head};
use corrode_core::marks::Marks;
use corrode_core::pairing::Shot;
use corrode_core::picture::{self, Picture};
use corrode_core::{banding, rawtherapee, sharpness};
use image::DynamicImage;

use crate::culling::Assessment;
use corrode_ui::loader::Loader;

/// Names a shot for good, whatever its place on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShotId(pub u32);

/// A version of a shot's files: bumped when they are written again, so
/// that what was loaded from the old ones is told apart.
pub type Version = u32;

/// The shots the jobs run on, shared with the threads. A shot keeps its
/// id; replacing its files gives it a new version.
#[derive(Default)]
pub struct Registry {
    shots: Vec<(Shot, Version)>,
}

pub type SharedRegistry = Arc<RwLock<Registry>>;

impl Registry {
    pub fn new(shots: impl IntoIterator<Item = Shot>) -> Registry {
        Registry {
            shots: shots.into_iter().map(|shot| (shot, 0)).collect(),
        }
    }

    pub fn add(&mut self, shot: Shot) -> ShotId {
        self.shots.push((shot, 0));
        ShotId(u32::try_from(self.shots.len() - 1).expect("fewer than four billion shots"))
    }

    /// Replaces the files of a shot; `reload` bumps its version, for
    /// files written again rather than moved.
    pub fn replace(&mut self, id: ShotId, shot: Shot, reload: bool) -> Version {
        let entry = &mut self.shots[id.0 as usize];
        entry.0 = shot;
        if reload {
            entry.1 += 1;
        }
        entry.1
    }

    pub fn get(&self, id: ShotId) -> (Shot, Version) {
        self.shots[id.0 as usize].clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Job {
    /// What the head of the files tells: shooting information, including
    /// the time used to find bursts, thumbnail and marks.
    Head(ShotId),
    Preview(ShotId),
    Full(ShotId),
    /// Sharpness around the focus point, and light bands, from the preview.
    Assess(ShotId),
    /// Removes the light bands of the RAW into a DNG next to it.
    Deband(ShotId),
}

impl Job {
    pub fn id(self) -> ShotId {
        match self {
            Job::Head(id)
            | Job::Preview(id)
            | Job::Full(id)
            | Job::Assess(id)
            | Job::Deband(id) => id,
        }
    }
}

pub enum Loaded {
    Head {
        id: ShotId,
        version: Version,
        exif: Result<Exif, String>,
        thumbnail: Option<DynamicImage>,
        marks: Result<Marks, String>,
    },
    Assessment {
        id: ShotId,
        version: Version,
        assessment: Option<Assessment>,
    },
    Debanded {
        id: ShotId,
        /// The DNG written and what was removed, or why not.
        result: Result<(PathBuf, Box<Pattern>), String>,
    },
    Picture {
        job: Job,
        version: Version,
        picture: Result<Arc<Picture>, String>,
        elapsed: Duration,
    },
}

/// Previews kept for the assessment of a burst, and shooting information
/// kept for the focus point.
const CACHED_PREVIEWS: usize = 8;

/// What the threads keep between jobs, so that a preview decoded to be
/// shown is not decoded again to be assessed.
#[derive(Default)]
pub struct Cache {
    /// The last decoded previews, oldest first.
    previews: VecDeque<(ShotId, Arc<Picture>)>,
    exifs: HashMap<ShotId, Exif>,
}

impl Cache {
    fn preview(&self, id: ShotId) -> Option<Arc<Picture>> {
        self.previews
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, picture)| Arc::clone(picture))
    }

    fn keep_preview(&mut self, id: ShotId, picture: &Arc<Picture>) {
        self.previews.retain(|(i, _)| *i != id);
        if self.previews.len() >= CACHED_PREVIEWS {
            self.previews.pop_front();
        }
        self.previews.push_back((id, Arc::clone(picture)));
    }

    /// Forgets a shot whose files were written again.
    pub fn forget(&mut self, id: ShotId) {
        self.previews.retain(|(i, _)| *i != id);
        self.exifs.remove(&id);
    }
}

/// Starts the loader of the culling viewer, and gives the cache its
/// threads share, so that a shot can be forgotten when its files change.
pub fn spawn(
    registry: SharedRegistry,
    threads: usize,
    results: Sender<Loaded>,
    meta: Arc<Mutex<MetaCache>>,
) -> (Loader<Job>, Arc<Mutex<Cache>>) {
    let cache = Arc::new(Mutex::new(Cache::default()));
    let shared = Arc::clone(&cache);
    let loader = Loader::new(threads, results, move |job: Job| {
        let (shot, version) = registry.read().unwrap().get(job.id());
        run(&shot, version, &shared, &meta, job)
    });
    (loader, cache)
}

/// Runs a job on its shot.
fn run(
    shot: &Shot,
    version: Version,
    cache: &Mutex<Cache>,
    meta: &Mutex<MetaCache>,
    job: Job,
) -> Loaded {
    let start = Instant::now();
    let id = job.id();
    // The preview of the shot, from the cache or decoded and cached.
    let preview = || -> Result<Arc<Picture>, String> {
        if let Some(picture) = cache.lock().unwrap().preview(id) {
            return Ok(picture);
        }
        let picture = picture::preview(shot)
            .map(Arc::new)
            .map_err(|err| err.to_string())?;
        cache.lock().unwrap().keep_preview(id, &picture);
        Ok(picture)
    };
    let loaded = |picture| Loaded::Picture {
        job,
        version,
        picture,
        elapsed: start.elapsed(),
    };
    match job {
        Job::Head(_) => {
            // The cache spares reading the file when it has not changed.
            let cached = meta.lock().unwrap().get(shot);
            let (exif, thumbnail) = match cached {
                Some((exif, thumbnail)) => (Ok(exif), thumbnail),
                None => {
                    let head = Head::read(shot);
                    let exif = head
                        .as_ref()
                        .map_err(|err| err.to_string())
                        .and_then(|head| head.exif().map_err(|err| err.to_string()));
                    let thumbnail = head
                        .ok()
                        .and_then(|head| head.thumbnail())
                        .or_else(|| picture::thumbnail(shot));
                    if let Ok(exif) = &exif {
                        meta.lock().unwrap().insert(shot, exif, thumbnail.as_ref());
                    }
                    (exif, thumbnail)
                }
            };
            if let Ok(exif) = &exif {
                cache.lock().unwrap().exifs.insert(id, exif.clone());
            }
            Loaded::Head {
                id,
                version,
                exif,
                thumbnail,
                marks: rawtherapee::read_marks(shot).map_err(|err| err.to_string()),
            }
        }
        Job::Preview(_) => loaded(preview()),
        Job::Full(_) => loaded(
            picture::full(shot)
                .map(Arc::new)
                .map_err(|err| err.to_string()),
        ),
        Job::Deband(_) => {
            let result = match shot.raw.as_deref() {
                None => Err("this shot has no RAW file".to_owned()),
                Some(raw) => {
                    let dng = debanding::output_path(raw);
                    debanding::to_dng(raw, &dng)
                        .map_err(|err| err.to_string())
                        .and_then(|pattern| {
                            rawtherapee::copy_sidecar(raw, &dng)
                                .map_err(|err| format!("{}: {err}", dng.display()))?;
                            Ok((dng, Box::new(pattern)))
                        })
                }
            };
            Loaded::Debanded { id, result }
        }
        Job::Assess(_) => {
            let focus = match cache.lock().unwrap().exifs.get(&id) {
                Some(exif) => exif.focus_point,
                None => corrode_core::exif::read(shot)
                    .ok()
                    .and_then(|exif| exif.focus_point),
            };
            let assessment = preview().ok().map(|preview| Assessment {
                sharpness: sharpness::score(&preview.image, focus),
                banded: banding::analyze(&preview.image).is_some_and(|bands| bands.is_banded()),
            });
            Loaded::Assessment {
                id,
                version,
                assessment,
            }
        }
    }
}
