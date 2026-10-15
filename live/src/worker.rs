//! The thread that applies the recipe to the picture being worked on, so
//! that the screen never waits for an effect.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use corrode_effects::Recipe;
use image::RgbImage;

/// A recipe to apply.
pub struct Job {
    /// Which request it is: a result is only shown if no newer one was
    /// asked for since.
    pub generation: u64,
    pub recipe: Box<Recipe>,
    /// The words of the recipe, which a save reads again.
    pub words: Vec<String>,
}

/// The outcome of a job.
pub struct Done {
    pub generation: u64,
    pub words: Vec<String>,
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
    while let Ok(mut job) = jobs.recv() {
        // Only the latest recipe matters: the others are stale.
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let start = Instant::now();
        let result = job.recipe.apply(source).map_err(|err| err.to_string());
        let done = Done {
            generation: job.generation,
            words: job.words,
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

    #[test]
    fn a_recipe_comes_back_applied_with_its_generation() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::new(Arc::new(RgbImage::from_pixel(20, 10, Rgb([100; 3]))), tx);
        let words: Vec<String> = ["seed=1", "fade"].map(str::to_owned).to_vec();
        worker.submit(Job {
            generation: 3,
            recipe: Box::new(Recipe::from_words(&words).unwrap()),
            words: words.clone(),
        });
        let done = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((done.generation, done.words), (3, words));
        let image = done.result.unwrap();
        assert_eq!((image.width(), image.height()), (20, 10));
        assert_ne!(image.get_pixel(0, 0), &Rgb([100; 3]), "the fade shows");
    }
}
