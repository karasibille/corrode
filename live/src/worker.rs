//! The thread that applies the recipe to the picture being worked on, so
//! that the screen never waits for an effect.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use corrode_effects::StepCache;
use image::RgbImage;

use crate::recipe_file::recipe_at;

/// A recipe to apply, at a moment of its ranges.
pub struct Job {
    /// Which job it is, to know when the one asked for last is done.
    pub id: u64,
    /// Which version of the recipe file it is for: a result is only shown
    /// if the file was not saved again since.
    pub generation: u64,
    pub words: Vec<String>,
    /// Where the ranges stand, 0 to 1.
    pub moment: f64,
    /// How far the seed has moved on.
    pub step: u64,
}

/// The outcome of a job, with what it was for.
pub struct Done {
    pub id: u64,
    pub generation: u64,
    pub words: Vec<String>,
    pub moment: f64,
    pub step: u64,
    pub result: Result<RgbImage, String>,
    pub elapsed: Duration,
}

pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    /// Starts the thread, which applies the recipes it is given to
    /// `source` and sends what comes out to `results`. It stops when the
    /// worker is dropped or the results are no longer received.
    pub fn new(source: Arc<RgbImage>, results: Sender<Done>) -> Worker {
        let (jobs, received) = mpsc::channel();
        thread::spawn(move || work(&source, &received, &results));
        Worker { jobs }
    }

    pub fn submit(&self, job: Job) {
        // The thread only stops when the worker is dropped.
        let _ = self.jobs.send(job);
    }
}

fn work(source: &RgbImage, jobs: &Receiver<Job>, results: &Sender<Done>) {
    // The steps of the last recipe: the next one starts from the ones it
    // shares with it, so that moving a range of the last effect makes the
    // last effect only.
    let mut cache = StepCache::default();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest recipe matters: the others are stale.
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let start = Instant::now();
        let result = recipe_at(&job.words, job.moment, job.step).and_then(|recipe| {
            recipe
                .apply_cached(source, &mut cache)
                .map_err(|err| err.to_string())
        });
        let done = Done {
            id: job.id,
            generation: job.generation,
            words: job.words,
            moment: job.moment,
            step: job.step,
            result,
            elapsed: start.elapsed(),
        };
        if results.send(done).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use image::Rgb;

    use super::*;

    fn job(id: u64, words: &[&str], moment: f64) -> Job {
        Job {
            id,
            generation: 3,
            words: words.iter().map(|word| (*word).to_owned()).collect(),
            moment,
            step: 0,
        }
    }

    #[test]
    fn a_recipe_comes_back_applied_with_what_it_was_for() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(Arc::new(RgbImage::from_pixel(20, 10, Rgb([100; 3]))), tx);
        worker.submit(job(7, &["seed=1", "fade", "lift=0..40"], 1.0));
        let done = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((done.id, done.generation, done.moment), (7, 3, 1.0));
        let image = done.result.unwrap();
        assert_eq!((image.width(), image.height()), (20, 10));
        assert_ne!(image.get_pixel(0, 0), &Rgb([100; 3]), "the fade shows");
    }

    #[test]
    fn a_wrong_recipe_comes_back_as_an_error_and_the_next_one_still_works() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(Arc::new(RgbImage::from_pixel(8, 8, Rgb([50; 3]))), tx);
        worker.submit(job(1, &["seed=1", "fade", "lift=oops"], 0.0));
        let wrong = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(wrong.result.unwrap_err().contains("lift"));
        worker.submit(job(2, &["seed=1", "grain"], 0.0));
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .unwrap()
                .result
                .is_ok()
        );
    }
}
