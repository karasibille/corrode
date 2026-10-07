//! The viewer: what is known of each shot, what is being loaded, and the
//! state of the screen. Keys and actions, drawing, the status lines and
//! the help each live in a submodule.

mod actions;
mod draw;
mod help;
mod lines;

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corrode_core::bursts;
use corrode_core::cache::Cache;
use corrode_core::exif::Exif;
use corrode_core::marks::Marks;
use corrode_core::pairing::Shot;
use corrode_core::picture::Picture;
use corrode_core::rawtherapee::Config;
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::FontSize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use crate::app::{App, Filter, Time, burst_around};
use crate::culling::{self, Assessment, Progress};
use crate::encoder::{Encoded, Encoder, Request};
use crate::loader::{Job, Loaded, Loader};

/// Shots decoded ahead on each side of the current one.
const PRELOAD: usize = 2;
/// Previews kept in memory on each side, a bit more than preloaded so
/// that going back and forth does not decode again.
const KEEP: usize = 3;
/// Height of the burst thumbnails, in rows; a line of marks goes below.
const THUMB_ROWS: u16 = 5;

/// What is known of one shot, filled in as its files are read.
#[derive(Default)]
struct ShotState {
    /// Shooting information, once the head of the files is read.
    exif: Option<Result<Exif, String>>,
    marks: Option<Result<Marks, String>>,
    thumbnail: Option<DynamicImage>,
    /// The thumbnail ready for the terminal, made when first shown.
    thumbnail_protocol: Option<Protocol>,
    /// Sharpness and light bands, `Some(None)` if they cannot be measured.
    assessment: Option<Option<Assessment>>,
}

impl ShotState {
    fn marks(&self) -> Option<&Marks> {
        self.marks.as_ref()?.as_ref().ok()
    }

    fn assessment(&self) -> Option<Assessment> {
        self.assessment.flatten()
    }
}

pub struct Viewer {
    dir: PathBuf,
    shots: Arc<Vec<Shot>>,
    states: Vec<ShotState>,
    /// When each shot was taken, as far as read: bursts come from it.
    times: Vec<Time>,
    /// What the files' heads told, kept between sessions.
    cache: Arc<Mutex<Cache>>,
    /// Whether the cache was saved since every shot was read.
    cache_saved: bool,
    filter: Filter,
    pub app: App,
    loader: Loader,
    loaded: Receiver<Loaded>,
    encoder: Encoder,
    encoded: Receiver<Encoded>,
    /// Used for the thumbnails, which are small enough to encode here.
    picker: Picker,
    font: FontSize,
    protocol_name: String,
    thumbnail_size: Size,
    previews: HashMap<usize, Result<Arc<Picture>, String>>,
    full: Option<(usize, Result<Arc<Picture>, String>)>,
    timings: HashMap<Job, Duration>,
    /// The shot and burst the jobs were last scheduled for.
    scheduled: Option<(usize, Range<usize>)>,
    requested: Option<Request>,
    shown: Option<Encoded>,
    view: (u32, u32),
    /// RawTherapee's settings, needed to create sidecars; marks cannot be
    /// written without them.
    config: Result<Config, String>,
    /// Result of the last action, shown until the next key.
    message: Option<Result<String, String>>,
    /// Whether the full help is shown instead of the shots.
    help: bool,
    /// How many lines the help is scrolled down.
    help_scroll: u16,
}

impl Viewer {
    pub fn new(
        dir: PathBuf,
        shots: Vec<Shot>,
        picker: Picker,
        config: Result<Config, String>,
    ) -> Viewer {
        let shots = Arc::new(shots);
        let (loaded_tx, loaded) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let cache = Arc::new(Mutex::new(Cache::open(&dir)));
        let font = picker.font_size();
        // Thumbnails are 4:3, the shape of the sensor.
        let thumbnail_columns = (u32::from(THUMB_ROWS) * u32::from(font.height) * 4 / 3)
            .div_ceil(u32::from(font.width.max(1)));
        Viewer {
            dir,
            states: shots.iter().map(|_| ShotState::default()).collect(),
            times: vec![Time::Unknown; shots.len()],
            filter: Filter::All,
            app: App::new(shots.len()),
            loader: Loader::new(
                Arc::clone(&shots),
                thread_count(),
                loaded_tx,
                Arc::clone(&cache),
            ),
            cache,
            cache_saved: false,
            loaded,
            font,
            protocol_name: format!("{:?}", picker.protocol_type()),
            encoder: Encoder::new(picker.clone(), encoded_tx),
            encoded,
            picker,
            shots,
            thumbnail_size: Size::new(
                u16::try_from(thumbnail_columns).unwrap_or(u16::MAX).max(4),
                THUMB_ROWS,
            ),
            previews: HashMap::new(),
            full: None,
            timings: HashMap::new(),
            scheduled: None,
            requested: None,
            shown: None,
            view: (0, 0),
            config,
            message: None,
            help: false,
            help_scroll: 0,
        }
    }

    fn full_picture(&self) -> Option<&Arc<Picture>> {
        match &self.full {
            Some((index, Ok(picture))) if *index == self.app.index => Some(picture),
            _ => None,
        }
    }

    /// The burst of the current shot, and whether its ends are known.
    fn burst(&self) -> (Range<usize>, bool) {
        burst_around(&self.times, self.app.index, bursts::MAX_GAP_MS)
    }

    /// The sharpest shot of the current burst without light bands, if any.
    fn sharpest(&self) -> Option<usize> {
        culling::sharpest(self.burst().0, |i| self.states[i].assessment())
    }

    /// How far culling has gone, from the marks read so far.
    fn progress(&self) -> Progress {
        Progress::count(
            self.states
                .iter()
                .map(|state| state.marks.as_ref().map(|marks| marks.as_ref().ok())),
        )
    }

    /// Asks for the current shot first, then its neighbours, then the
    /// full picture for the zoom, then the quality of its burst, then the
    /// head of every other file, the closest first, so that bursts take
    /// shape around the current shot. Runs again when the shot changes, or
    /// when its burst grows as files are read.
    pub fn schedule(&mut self) {
        let index = self.app.index;
        let (burst, _) = self.burst();
        let scheduled = Some((index, burst.clone()));
        if self.scheduled == scheduled {
            return;
        }
        self.scheduled = scheduled;

        self.previews.retain(|&i, _| i.abs_diff(index) <= KEEP);
        if self.full.as_ref().is_some_and(|(i, _)| *i != index) {
            self.full = None;
        }

        let last = self.shots.len() - 1;
        let neighbours = (1..=PRELOAD).flat_map(|distance| {
            [
                index.checked_add(distance).filter(|&i| i <= last),
                index.checked_sub(distance),
            ]
        });
        let mut jobs = vec![Job::Head(index), Job::Preview(index)];
        jobs.extend(neighbours.flatten().map(Job::Preview));
        jobs.push(Job::Full(index));
        jobs.extend(burst.map(Job::Assess));
        let mut others: Vec<usize> = (0..self.shots.len()).filter(|&i| i != index).collect();
        others.sort_by_key(|&i| i.abs_diff(index));
        jobs.extend(others.into_iter().map(Job::Head));
        jobs.retain(|job| match *job {
            Job::Head(i) => self.times[i] == Time::Unknown,
            Job::Preview(i) => !self.previews.contains_key(&i),
            Job::Full(i) => self.full.as_ref().is_none_or(|(full, _)| *full != i),
            Job::Assess(i) => self.states[i].assessment.is_none(),
            // Only started by the user, through the loader's pinned jobs.
            Job::Deband(_) => false,
        });
        self.loader.want(jobs);
    }

    /// Saves what was read of the files for the next session. Errors are
    /// ignored: the cache is only a shortcut.
    pub fn save_cache(&mut self) {
        let _ = self.cache.lock().unwrap().save();
        self.cache_saved = true;
    }

    /// Takes in what the background threads have loaded or encoded.
    pub fn receive(&mut self) {
        while let Ok(loaded) = self.loaded.try_recv() {
            match loaded {
                Loaded::Head {
                    index,
                    exif,
                    thumbnail,
                    marks,
                } => {
                    let taken = exif.as_ref().ok().and_then(|exif| exif.taken_ms);
                    self.times[index] = taken.map_or(Time::Missing, Time::At);
                    let state = &mut self.states[index];
                    state.exif = Some(exif);
                    // Marks set meanwhile from the keyboard are more recent.
                    state.marks.get_or_insert(marks);
                    state.thumbnail = thumbnail;
                }
                Loaded::Assessment { index, assessment } => {
                    self.states[index].assessment = Some(assessment);
                }
                Loaded::Debanded { index, result } => {
                    let stem = self.shots[index].stem.to_string_lossy();
                    self.message = Some(match result {
                        Ok((dng, pattern)) => Ok(format!(
                            "{stem}: bands every {:.0} rows removed (R {:.1}% G {:.1}% B {:.1}%), written to {}; restart to see it",
                            pattern.period,
                            100.0 * pattern.amplitude(0),
                            100.0 * pattern.amplitude(1),
                            100.0 * pattern.amplitude(2),
                            dng.file_name().unwrap_or_default().to_string_lossy()
                        )),
                        Err(err) => Err(format!("{stem}: {err}")),
                    });
                }
                Loaded::Picture {
                    job,
                    picture,
                    elapsed,
                } => {
                    self.timings.insert(job, elapsed);
                    match job {
                        Job::Preview(i) if i.abs_diff(self.app.index) <= KEEP => {
                            self.previews.insert(i, picture);
                        }
                        Job::Full(i) if i == self.app.index => self.full = Some((i, picture)),
                        _ => {}
                    }
                }
            }
        }
        if !self.cache_saved && self.times.iter().all(|time| *time != Time::Unknown) {
            self.save_cache();
        }
        while let Ok(encoded) = self.encoded.try_recv() {
            // Ignore results for a request that was replaced meanwhile.
            if self.requested.as_ref() == Some(&encoded.request) {
                self.shown = Some(encoded);
            }
        }
    }
}

/// Decoding threads: enough to prepare the neighbours while the current
/// shot is decoded, leaving a core to the interface.
fn thread_count() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4))
}
