//! Running jobs in background threads, so that the interface never
//! waits. Any kind of job: the loader takes the function that runs one
//! and the channel its results go through.
//!
//! The application says which jobs it wants, most urgent first, every
//! time its needs change; jobs no longer wanted are dropped before they
//! start. A job pushed explicitly runs first and is never dropped.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct Queue<J> {
    /// Jobs asked for explicitly, run first and never dropped.
    pinned: VecDeque<J>,
    waiting: VecDeque<J>,
    running: HashSet<J>,
    closed: bool,
}

impl<J> Default for Queue<J> {
    fn default() -> Queue<J> {
        Queue {
            pinned: VecDeque::new(),
            waiting: VecDeque::new(),
            running: HashSet::new(),
            closed: false,
        }
    }
}

pub struct Loader<J> {
    queue: Arc<(Mutex<Queue<J>>, Condvar)>,
}

impl<J: Copy + Eq + Hash + Send + 'static> Loader<J> {
    /// Starts `threads` threads that run the jobs with `run` and send
    /// what comes out to `results`. They stop when the loader is dropped
    /// or the results are no longer received.
    pub fn new<R: Send + 'static>(
        threads: usize,
        results: Sender<R>,
        run: impl Fn(J) -> R + Send + Sync + 'static,
    ) -> Loader<J> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let run = Arc::new(run);
        for _ in 0..threads {
            let (queue, results, run) = (Arc::clone(&queue), results.clone(), Arc::clone(&run));
            thread::spawn(move || {
                while let Some(job) = next_job(&queue) {
                    let result = run(job);
                    queue.0.lock().unwrap().running.remove(&job);
                    if results.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        Loader { queue }
    }

    /// Adds a job that must run whatever is asked for next, such as a
    /// correction the user started.
    pub fn push(&self, job: J) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        if !queue.running.contains(&job) && !queue.pinned.contains(&job) {
            queue.pinned.push_back(job);
        }
        wake.notify_all();
    }

    /// Replaces the waiting jobs by these ones, in this order. Jobs
    /// already running are not started twice.
    pub fn want(&self, jobs: impl IntoIterator<Item = J>) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        let jobs: VecDeque<J> = jobs
            .into_iter()
            .filter(|job| !queue.running.contains(job))
            .collect();
        queue.waiting = jobs;
        wake.notify_all();
    }
}

impl<J> Drop for Loader<J> {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        lock.lock().unwrap().closed = true;
        wake.notify_all();
    }
}

/// Waits for the most urgent job, or `None` once the loader is dropped.
fn next_job<J: Copy + Eq + Hash>(queue: &(Mutex<Queue<J>>, Condvar)) -> Option<J> {
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
            return Some(job);
        }
        queue = wake.wait(queue).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn jobs_run_in_order_and_results_come_back() {
        let (tx, rx) = mpsc::channel();
        let loader = Loader::new(1, tx, |job: u32| job * 10);
        loader.want([1, 2, 3]);
        let mut results: Vec<u32> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        results.sort_unstable();
        assert_eq!(results, vec![10, 20, 30]);
    }

    #[test]
    fn a_pushed_job_runs_before_the_wanted_ones() {
        let (tx, rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate = Mutex::new(gate_rx);
        // The first job waits at the gate, so that the queue fills up.
        let loader = Loader::new(1, tx, move |job: u32| {
            if job == 0 {
                let _ = gate.lock().unwrap().recv();
            }
            job
        });
        loader.want([0, 1, 2]);
        thread::sleep(Duration::from_millis(50));
        loader.push(9);
        gate_tx.send(()).unwrap();
        let results: Vec<u32> = (0..4)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(results, vec![0, 9, 1, 2]);
    }

    #[test]
    fn wanting_again_drops_the_jobs_not_started() {
        let (tx, rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate = Mutex::new(gate_rx);
        let loader = Loader::new(1, tx, move |job: u32| {
            if job == 0 {
                let _ = gate.lock().unwrap().recv();
            }
            job
        });
        loader.want([0, 1, 2]);
        thread::sleep(Duration::from_millis(50));
        loader.want([5]);
        gate_tx.send(()).unwrap();
        let results: Vec<u32> = (0..2)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(results, vec![0, 5]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
