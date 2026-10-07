//! Decoding in background threads, so that the interface never waits.
//!
//! The application says which jobs it wants, most urgent first, every time
//! the current shot changes; jobs no longer wanted are dropped before they
//! start. Results come back through a channel.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use corrode_core::exif::{self, Exif};
use corrode_core::pairing::Shot;
use corrode_core::picture::{self, Picture};
use corrode_core::pp3::Marks;
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

pub struct Loader {
    queue: Arc<(Mutex<Queue>, Condvar)>,
}

impl Loader {
    pub fn new(shots: Arc<Vec<Shot>>, threads: usize, results: Sender<Loaded>) -> Loader {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        for _ in 0..threads {
            let (queue, shots, results) = (Arc::clone(&queue), Arc::clone(&shots), results.clone());
            thread::spawn(move || {
                while let Some(job) = next_job(&queue) {
                    let loaded = run(&shots, job);
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

fn run(shots: &[Shot], job: Job) -> Loaded {
    let start = Instant::now();
    let picture = |index: usize, decode: fn(&Shot) -> Result<Picture, picture::Error>| {
        let picture = decode(&shots[index])
            .map(Arc::new)
            .map_err(|err| err.to_string());
        Loaded::Picture {
            job,
            picture,
            elapsed: start.elapsed(),
        }
    };
    match job {
        Job::Head(index) => Loaded::Head {
            index,
            exif: exif::read(&shots[index]).map_err(|err| err.to_string()),
            thumbnail: picture::thumbnail(&shots[index]),
            marks: rawtherapee::read_marks(&shots[index]).map_err(|err| err.to_string()),
        },
        Job::Preview(index) => picture(index, picture::preview),
        Job::Full(index) => picture(index, picture::full),
        Job::Assess(index) => {
            let shot = &shots[index];
            let focus = exif::read(shot).ok().and_then(|exif| exif.focus_point);
            let assessment = picture::preview(shot).ok().map(|preview| Assessment {
                sharpness: sharpness::score(&preview.image, focus),
                banded: banding::analyze(&preview.image).is_some_and(|bands| bands.is_banded()),
            });
            Loaded::Assessment { index, assessment }
        }
    }
}
