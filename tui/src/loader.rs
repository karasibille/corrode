//! Decoding in background threads, so that the interface never waits.
//!
//! The application says which jobs it wants, most urgent first, every time
//! the current shot changes; jobs no longer wanted are dropped before they
//! start. Results come back through a channel. A small cache shared by the
//! threads keeps the last previews and the shooting information, so that a
//! preview decoded to be shown is not decoded again to be assessed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use corrode_core::cache::Cache as MetaCache;
use corrode_core::debanding::{self, Pattern};
use corrode_core::exif::{Exif, Head};
use corrode_core::marks::Marks;
use corrode_core::pairing::Shot;
use corrode_core::picture::{self, Picture};
use corrode_core::{banding, rawtherapee, sharpness};
use std::path::PathBuf;

use crate::culling::Assessment;
use image::DynamicImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Job {
    /// What the head of the files tells: shooting information, including
    /// the time used to find bursts, thumbnail and marks.
    Head(usize),
    Preview(usize),
    Full(usize),
    /// Sharpness around the focus point, and light bands, from the preview.
    Assess(usize),
    /// Removes the light bands of the RAW into a DNG next to it.
    Deband(usize),
}

impl Job {
    fn index_mut(&mut self) -> &mut usize {
        match self {
            Job::Head(i) | Job::Preview(i) | Job::Full(i) | Job::Assess(i) | Job::Deband(i) => i,
        }
    }

    fn index(self) -> usize {
        let mut job = self;
        *job.index_mut()
    }
}

/// Counts the changes of the list of shots: a result of a job started
/// before a change speaks of the old indices.
pub type Epoch = u64;

pub enum Loaded {
    Head {
        index: usize,
        exif: Result<Exif, String>,
        thumbnail: Option<DynamicImage>,
        marks: Result<Marks, String>,
    },
    Assessment {
        index: usize,
        assessment: Option<Assessment>,
    },
    Debanded {
        /// The RAW corrected: the list may have changed since.
        raw: PathBuf,
        /// The DNG written and what was removed, or why not.
        result: Result<(PathBuf, Box<Pattern>), String>,
    },
    Picture {
        job: Job,
        picture: Result<Arc<Picture>, String>,
        elapsed: Duration,
    },
}

#[derive(Default)]
struct Queue {
    /// Jobs asked for explicitly, run first and never dropped.
    pinned: VecDeque<Job>,
    waiting: VecDeque<Job>,
    running: HashSet<Job>,
    epoch: Epoch,
    closed: bool,
}

/// Previews kept for the assessment of a burst, and shooting information
/// kept for the focus point.
const CACHED_PREVIEWS: usize = 8;

#[derive(Default)]
struct Cache {
    /// The last decoded previews, oldest first.
    previews: VecDeque<(usize, Arc<Picture>)>,
    exifs: HashMap<usize, Exif>,
}

impl Cache {
    fn preview(&self, index: usize) -> Option<Arc<Picture>> {
        self.previews
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, picture)| Arc::clone(picture))
    }

    fn keep_preview(&mut self, index: usize, picture: &Arc<Picture>) {
        self.previews.retain(|(i, _)| *i != index);
        if self.previews.len() >= CACHED_PREVIEWS {
            self.previews.pop_front();
        }
        self.previews.push_back((index, Arc::clone(picture)));
    }
}

pub struct Loader {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    shots: Arc<RwLock<Vec<Shot>>>,
    cache: Arc<Mutex<Cache>>,
}

impl Loader {
    pub fn new(
        shots: Vec<Shot>,
        threads: usize,
        results: Sender<(Epoch, Loaded)>,
        meta: Arc<Mutex<MetaCache>>,
    ) -> Loader {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let shots = Arc::new(RwLock::new(shots));
        let cache = Arc::new(Mutex::new(Cache::default()));
        for _ in 0..threads {
            let (queue, shots, results) = (Arc::clone(&queue), Arc::clone(&shots), results.clone());
            let (cache, meta) = (Arc::clone(&cache), Arc::clone(&meta));
            thread::spawn(move || {
                while let Some((job, epoch)) = next_job(&queue) {
                    let shot = shots.read().unwrap()[job.index()].clone();
                    let loaded = run(&shot, &cache, &meta, job);
                    queue.0.lock().unwrap().running.remove(&job);
                    if results.send((epoch, loaded)).is_err() {
                        break;
                    }
                }
            });
        }
        Loader {
            queue,
            shots,
            cache,
        }
    }

    /// The epoch of the list of shots: results from an older one are stale.
    pub fn epoch(&self) -> Epoch {
        self.queue.0.lock().unwrap().epoch
    }

    /// Adds a shot at `index`, such as a DNG just written: the jobs
    /// waiting are dropped, the pinned ones follow their shot, and the
    /// results of the running ones will be stale.
    pub fn insert(&self, index: usize, shot: Shot) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        self.shots.write().unwrap().insert(index, shot);
        for job in &mut queue.pinned {
            let i = job.index_mut();
            if *i >= index {
                *i += 1;
            }
        }
        self.change(&mut queue);
        wake.notify_all();
    }

    /// Replaces the shot at `index` by a new version of its files, such as
    /// a DNG written again: what was loaded of it is forgotten.
    pub fn replace(&self, index: usize, shot: Shot) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        self.shots.write().unwrap()[index] = shot;
        self.change(&mut queue);
        wake.notify_all();
    }

    fn change(&self, queue: &mut Queue) {
        queue.waiting.clear();
        queue.epoch += 1;
        *self.cache.lock().unwrap() = Cache::default();
    }

    /// Adds a job that must run whatever the viewer asks for next, such as
    /// a correction the user started.
    pub fn push(&self, job: Job) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        if !queue.running.contains(&job) && !queue.pinned.contains(&job) {
            queue.pinned.push_back(job);
        }
        wake.notify_all();
    }

    /// Replaces the waiting jobs by these ones, in this order. Jobs already
    /// running are not started twice.
    pub fn want(&self, jobs: impl IntoIterator<Item = Job>) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        let jobs: VecDeque<Job> = jobs
            .into_iter()
            .filter(|job| !queue.running.contains(job))
            .collect();
        queue.waiting = jobs;
        wake.notify_all();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().closed = true;
        wake.notify_all();
    }
}

/// Waits for the most urgent job, with the epoch it was started in, or
/// `None` once the loader is dropped.
fn next_job(queue: &(Mutex<Queue>, Condvar)) -> Option<(Job, Epoch)> {
    let (lock, wake) = queue;
    let mut queue = lock.lock().unwrap();
    loop {
        if queue.closed {
            return None;
        }
        if let Some(job) = queue
            .pinned
            .pop_front()
            .or_else(|| queue.waiting.pop_front())
        {
            queue.running.insert(job);
            return Some((job, queue.epoch));
        }
        queue = wake.wait(queue).unwrap();
    }
}

/// Runs a job on its shot.
fn run(shot: &Shot, cache: &Mutex<Cache>, meta: &Mutex<MetaCache>, job: Job) -> Loaded {
    let start = Instant::now();
    // The preview of the shot, from the cache or decoded and cached.
    let preview = |index: usize| -> Result<Arc<Picture>, String> {
        if let Some(picture) = cache.lock().unwrap().preview(index) {
            return Ok(picture);
        }
        let picture = picture::preview(shot)
            .map(Arc::new)
            .map_err(|err| err.to_string())?;
        cache.lock().unwrap().keep_preview(index, &picture);
        Ok(picture)
    };
    let loaded = |job, picture| Loaded::Picture {
        job,
        picture,
        elapsed: start.elapsed(),
    };
    match job {
        Job::Head(index) => {
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
                cache.lock().unwrap().exifs.insert(index, exif.clone());
            }
            Loaded::Head {
                index,
                exif,
                thumbnail,
                marks: rawtherapee::read_marks(shot).map_err(|err| err.to_string()),
            }
        }
        Job::Preview(index) => loaded(job, preview(index)),
        Job::Full(_) => loaded(
            job,
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
            Loaded::Debanded {
                raw: shot.raw.clone().unwrap_or_default(),
                result,
            }
        }
        Job::Assess(index) => {
            let focus = match cache.lock().unwrap().exifs.get(&index) {
                Some(exif) => exif.focus_point,
                None => corrode_core::exif::read(shot)
                    .ok()
                    .and_then(|exif| exif.focus_point),
            };
            let assessment = preview(index).ok().map(|preview| Assessment {
                sharpness: sharpness::score(&preview.image, focus),
                banded: banding::analyze(&preview.image).is_some_and(|bands| bands.is_banded()),
            });
            Loaded::Assessment { index, assessment }
        }
    }
}
