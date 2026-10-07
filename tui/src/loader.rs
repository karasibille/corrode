//! Decoding in background threads, so that the interface never waits.
//!
//! The application says which jobs it wants, most urgent first, every time
//! the current shot changes; jobs no longer wanted are dropped before they
//! start. Results come back through a channel. A small cache shared by the
//! threads keeps the last previews and the shooting information, so that a
//! preview decoded to be shown is not decoded again to be assessed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use corrode_core::cache::Cache as MetaCache;
use corrode_core::exif::{Exif, Head};
use corrode_core::marks::Marks;
use corrode_core::pairing::Shot;
use corrode_core::picture::{self, Picture};
use corrode_core::{banding, rawtherapee, sharpness};

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
}

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
    Picture {
        job: Job,
        picture: Result<Arc<Picture>, String>,
        elapsed: Duration,
    },
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Job>,
    running: HashSet<Job>,
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
}

impl Loader {
    pub fn new(
        shots: Arc<Vec<Shot>>,
        threads: usize,
        results: Sender<Loaded>,
        meta: Arc<Mutex<MetaCache>>,
    ) -> Loader {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let cache = Arc::new(Mutex::new(Cache::default()));
        for _ in 0..threads {
            let (queue, shots, results) = (Arc::clone(&queue), Arc::clone(&shots), results.clone());
            let (cache, meta) = (Arc::clone(&cache), Arc::clone(&meta));
            thread::spawn(move || {
                while let Some(job) = next_job(&queue) {
                    let loaded = run(&shots, &cache, &meta, job);
                    queue.0.lock().unwrap().running.remove(&job);
                    if results.send(loaded).is_err() {
                        break;
                    }
                }
            });
        }
        Loader { queue }
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

/// Waits for the most urgent job, or `None` once the loader is dropped.
fn next_job(queue: &(Mutex<Queue>, Condvar)) -> Option<Job> {
    let (lock, wake) = queue;
    let mut queue = lock.lock().unwrap();
    loop {
        if queue.closed {
            return None;
        }
        if let Some(job) = queue.waiting.pop_front() {
            queue.running.insert(job);
            return Some(job);
        }
        queue = wake.wait(queue).unwrap();
    }
}

fn run(shots: &[Shot], cache: &Mutex<Cache>, meta: &Mutex<MetaCache>, job: Job) -> Loaded {
    let start = Instant::now();
    let shot = |index: usize| &shots[index];
    // The preview of a shot, from the cache or decoded and cached.
    let preview = |index: usize| -> Result<Arc<Picture>, String> {
        if let Some(picture) = cache.lock().unwrap().preview(index) {
            return Ok(picture);
        }
        let picture = picture::preview(shot(index))
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
            let cached = meta.lock().unwrap().get(shot(index));
            let (exif, thumbnail) = match cached {
                Some((exif, thumbnail)) => (Ok(exif), thumbnail),
                None => {
                    let head = Head::read(shot(index));
                    let exif = head
                        .as_ref()
                        .map_err(|err| err.to_string())
                        .and_then(|head| head.exif().map_err(|err| err.to_string()));
                    let thumbnail = head
                        .ok()
                        .and_then(|head| head.thumbnail())
                        .or_else(|| picture::thumbnail(shot(index)));
                    if let Ok(exif) = &exif {
                        meta.lock()
                            .unwrap()
                            .insert(shot(index), exif, thumbnail.as_ref());
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
                marks: rawtherapee::read_marks(shot(index)).map_err(|err| err.to_string()),
            }
        }
        Job::Preview(index) => loaded(job, preview(index)),
        Job::Full(index) => loaded(
            job,
            picture::full(shot(index))
                .map(Arc::new)
                .map_err(|err| err.to_string()),
        ),
        Job::Assess(index) => {
            let focus = match cache.lock().unwrap().exifs.get(&index) {
                Some(exif) => exif.focus_point,
                None => corrode_core::exif::read(shot(index))
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
