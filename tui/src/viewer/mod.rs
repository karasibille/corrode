//! The viewer: what is known of each shot, what is being loaded, and the
//! state of the screen. Keys and actions, drawing, the status lines and
//! the help each live in a submodule.

mod actions;
mod draw;
mod help;
mod lines;

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use corrode_core::bursts;
use corrode_core::cache::Cache;
use corrode_core::exif::Exif;
use corrode_core::marks::Marks;
use corrode_core::pairing::{self, Shot};
use corrode_core::picture::Picture;
use corrode_core::rawtherapee::Config;
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::FontSize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

use crate::app::{App, Command, Filter, Mode, Time, burst_around};
use crate::culling::{self, Assessment, Progress};
use crate::encoder::{Encoded, Encoder, Request};
use crate::jobs::{self, Job, Loaded, Registry, SharedRegistry, ShotId, Version};
use crate::loader::Loader;

/// Shots decoded ahead on each side of the current one.
const PRELOAD: usize = 2;
/// Previews kept in memory on each side, a bit more than preloaded so
/// that going back and forth does not decode again.
const KEEP: usize = 3;
/// Full pictures prepared on each side of the current shot while zoomed,
/// so that comparing a detail across a burst does not wait for each one:
/// about 60 MB each.
const KEEP_FULL: usize = 1;
/// Shots lately seen zoomed whose full pictures are kept too, so that
/// coming back to one is instant.
const RECENT_FULL: usize = 4;
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
    /// Sorted by stem, by position on screen; the registry has the same
    /// shots by id.
    shots: Vec<Shot>,
    /// The id of the shot at each position.
    ids: Vec<ShotId>,
    /// The position of each shot, by id.
    positions: Vec<usize>,
    /// The version of each shot's files, by id: results of jobs run on
    /// an older version are dropped.
    versions: Vec<Version>,
    /// The shots as the loader's threads see them.
    registry: SharedRegistry,
    states: Vec<ShotState>,
    /// When each shot was taken, as far as read: bursts come from it.
    times: Vec<Time>,
    /// What the files' heads told, kept between sessions.
    cache: Arc<Mutex<Cache>>,
    /// Whether the cache was saved since every shot was read.
    cache_saved: bool,
    filter: Filter,
    pub app: App,
    loader: Loader<Job>,
    /// What the loader's threads keep between jobs.
    loader_cache: Arc<Mutex<jobs::Cache>>,
    loaded: Receiver<Loaded>,
    encoder: Encoder,
    encoded: Receiver<Encoded>,
    /// Used for the thumbnails, which are small enough to encode here.
    picker: Picker,
    font: FontSize,
    protocol_name: String,
    thumbnail_size: Size,
    previews: HashMap<ShotId, Result<Arc<Picture>, String>>,
    /// The current shot's, and its neighbours' and the lately seen ones'
    /// while zoomed.
    fulls: HashMap<ShotId, Result<Arc<Picture>, String>>,
    /// Shots seen zoomed, the latest first, at most `RECENT_FULL`.
    recent: VecDeque<ShotId>,
    timings: HashMap<Job, Duration>,
    /// The shot and burst the jobs were last scheduled for, and whether
    /// it was zoomed.
    scheduled: Option<(usize, Range<usize>, bool)>,
    requested: Option<Request>,
    shown: Option<Encoded>,
    view: (u32, u32),
    /// RawTherapee's settings, needed to create sidecars; marks cannot be
    /// written without them.
    config: Result<Config, String>,
    /// Result of the last action, shown until the next key.
    message: Option<Result<String, String>>,
    /// Whether the next `m` sends the kept shots: the first asks, the
    /// second does it, any other key gives up.
    confirm_send: bool,
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
        let (loaded_tx, loaded) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let cache = Arc::new(Mutex::new(Cache::open(&dir)));
        let font = picker.font_size();
        // Thumbnails are 4:3, the shape of the sensor.
        let thumbnail_columns = (u32::from(THUMB_ROWS) * u32::from(font.height) * 4 / 3)
            .div_ceil(u32::from(font.width.max(1)));
        let registry = Arc::new(RwLock::new(Registry::new(shots.iter().cloned())));
        let (loader, loader_cache) = jobs::spawn(
            Arc::clone(&registry),
            thread_count(),
            loaded_tx,
            Arc::clone(&cache),
        );
        let count = u32::try_from(shots.len()).expect("fewer than four billion shots");
        Viewer {
            dir,
            ids: (0..count).map(ShotId).collect(),
            positions: (0..shots.len()).collect(),
            versions: vec![0; shots.len()],
            registry,
            states: shots.iter().map(|_| ShotState::default()).collect(),
            times: vec![Time::Unknown; shots.len()],
            filter: Filter::All,
            app: App::new(shots.len()),
            loader,
            loader_cache,
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
            fulls: HashMap::new(),
            recent: VecDeque::new(),
            timings: HashMap::new(),
            scheduled: None,
            requested: None,
            shown: None,
            view: (0, 0),
            config,
            message: None,
            confirm_send: false,
            help: false,
            help_scroll: 0,
        }
    }

    /// The id of the shot at a position on screen.
    fn id(&self, position: usize) -> ShotId {
        self.ids[position]
    }

    /// The position on screen of a shot.
    fn position(&self, id: ShotId) -> usize {
        self.positions[id.0 as usize]
    }

    /// Whether the full picture of a shot is kept: zoomed, the neighbours
    /// and the lately seen shots, for the next comparison.
    fn keeps_full(&self, id: ShotId) -> bool {
        matches!(self.app.mode, Mode::Zoom { .. })
            && (self.position(id).abs_diff(self.app.index) <= KEEP_FULL
                || self.recent.contains(&id))
    }

    fn full_picture(&self) -> Option<&Arc<Picture>> {
        match self.fulls.get(&self.id(self.app.index)) {
            Some(Ok(picture)) => Some(picture),
            _ => None,
        }
    }

    /// Gives a shot new files: moved ones keep what was loaded, written
    /// again ones (`reload`) drop it and are loaded afresh.
    fn replace_shot(&mut self, position: usize, shot: Shot, reload: bool) {
        let id = self.id(position);
        self.shots[position] = shot.clone();
        let version = self.registry.write().unwrap().replace(id, shot, reload);
        if reload {
            self.versions[id.0 as usize] = version;
            self.loader_cache.lock().unwrap().forget(id);
            self.states[position] = ShotState::default();
            self.times[position] = Time::Unknown;
            self.previews.remove(&id);
            self.fulls.remove(&id);
            self.recent.retain(|&i| i != id);
            self.scheduled = None;
            self.cache_saved = false;
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
        let zoomed = matches!(self.app.mode, Mode::Zoom { .. });
        let scheduled = Some((index, burst.clone(), zoomed));
        if self.scheduled == scheduled {
            return;
        }
        self.scheduled = scheduled;

        let id = self.id(index);
        let positions = &self.positions;
        self.previews
            .retain(|i, _| positions[i.0 as usize].abs_diff(index) <= KEEP);
        if zoomed {
            self.recent.retain(|&i| i != id);
            self.recent.push_front(id);
            self.recent.truncate(RECENT_FULL);
        } else {
            self.recent.clear();
        }
        let kept: Vec<ShotId> = self
            .fulls
            .keys()
            .copied()
            .filter(|&i| i == id || self.keeps_full(i))
            .collect();
        self.fulls.retain(|i, _| kept.contains(i));

        let last = self.shots.len() - 1;
        let neighbours = (1..=PRELOAD).flat_map(|distance| {
            [
                index.checked_add(distance).filter(|&i| i <= last),
                index.checked_sub(distance),
            ]
        });
        // Zoomed, the full picture is what is on screen: it comes first,
        // and the neighbours' full pictures are prepared for the next
        // comparison.
        let at = |position: usize| self.ids[position];
        let mut jobs = vec![Job::Head(id), Job::Preview(id)];
        if zoomed {
            jobs.insert(1, Job::Full(id));
            let near = (1..=KEEP_FULL).flat_map(|distance| {
                [
                    index.checked_add(distance).filter(|&i| i <= last),
                    index.checked_sub(distance),
                ]
            });
            jobs.extend(near.flatten().map(|i| Job::Full(at(i))));
        }
        jobs.extend(neighbours.flatten().map(|i| Job::Preview(at(i))));
        if !zoomed {
            jobs.push(Job::Full(id));
        }
        jobs.extend(burst.map(|i| Job::Assess(at(i))));
        let mut others: Vec<usize> = (0..self.shots.len()).filter(|&i| i != index).collect();
        others.sort_by_key(|&i| i.abs_diff(index));
        jobs.extend(others.into_iter().map(|i| Job::Head(at(i))));
        jobs.retain(|job| match *job {
            Job::Head(i) => self.times[self.position(i)] == Time::Unknown,
            Job::Preview(i) => !self.previews.contains_key(&i),
            Job::Full(i) => !self.fulls.contains_key(&i),
            Job::Assess(i) => self.states[self.position(i)].assessment.is_none(),
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

    /// Shows a shot just written next to the others, such as a debanded
    /// DNG, or shows it afresh when it was written again, and goes to it.
    fn add_shot(&mut self, shot: Shot) {
        let index = match self.shots.iter().position(|s| s.stem == shot.stem) {
            Some(index) => {
                self.replace_shot(index, shot, true);
                index
            }
            None => {
                let index = self.shots.partition_point(|s| s.stem < shot.stem);
                let id = self.registry.write().unwrap().add(shot.clone());
                self.shots.insert(index, shot);
                self.ids.insert(index, id);
                // The shots after it move down one position.
                for &moved in &self.ids[index + 1..] {
                    self.positions[moved.0 as usize] += 1;
                }
                self.positions.push(index);
                self.versions.push(0);
                self.states.insert(index, ShotState::default());
                self.times.insert(index, Time::Unknown);
                self.app.count += 1;
                self.scheduled = None;
                self.cache_saved = false;
                index
            }
        };
        self.requested = None;
        self.shown = None;
        self.app.apply(Command::GoTo(index), None, self.view);
    }

    /// Takes in what the background threads have loaded or encoded.
    pub fn receive(&mut self) {
        while let Ok(loaded) = self.loaded.try_recv() {
            // A job run on files written again since is stale: the shot
            // is asked for again.
            let stale = |id: ShotId, version: Version| self.versions[id.0 as usize] != version;
            match loaded {
                Loaded::Head {
                    id,
                    version,
                    exif,
                    thumbnail,
                    marks,
                } => {
                    if stale(id, version) {
                        continue;
                    }
                    let index = self.position(id);
                    let taken = exif.as_ref().ok().and_then(|exif| exif.taken_ms);
                    self.times[index] = taken.map_or(Time::Missing, Time::At);
                    let state = &mut self.states[index];
                    state.exif = Some(exif);
                    // Marks set meanwhile from the keyboard are more recent.
                    state.marks.get_or_insert(marks);
                    state.thumbnail = thumbnail;
                }
                Loaded::Assessment {
                    id,
                    version,
                    assessment,
                } => {
                    if stale(id, version) {
                        continue;
                    }
                    let index = self.position(id);
                    self.states[index].assessment = Some(assessment);
                }
                Loaded::Debanded { id, result } => {
                    let stem = self.shots[self.position(id)]
                        .stem
                        .to_string_lossy()
                        .into_owned();
                    match result {
                        Ok((dng, pattern)) => {
                            self.message = Some(Ok(format!(
                                "{stem}: bands every {:.0} rows removed (R {:.1}% G {:.1}% B {:.1}%) → {}",
                                pattern.period,
                                100.0 * pattern.amplitude(0),
                                100.0 * pattern.amplitude(1),
                                100.0 * pattern.amplitude(2),
                                dng.file_name().unwrap_or_default().to_string_lossy()
                            )));
                            if let Some(shot) = pairing::pair([dng]).pop() {
                                self.add_shot(shot);
                            }
                        }
                        Err(err) => self.message = Some(Err(format!("{stem}: {err}"))),
                    }
                }
                Loaded::Picture {
                    job,
                    version,
                    picture,
                    elapsed,
                } => {
                    if stale(job.id(), version) {
                        continue;
                    }
                    self.timings.insert(job, elapsed);
                    let current = self.id(self.app.index);
                    match job {
                        Job::Preview(i) if self.position(i).abs_diff(self.app.index) <= KEEP => {
                            self.previews.insert(i, picture);
                        }
                        Job::Full(i) if i == current || self.keeps_full(i) => {
                            self.fulls.insert(i, picture);
                        }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use corrode_core::marks::ColorLabel;
    use corrode_core::{pairing, rawtherapee, selection};
    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    use super::*;

    /// A shoot of three shots: one rated, one labelled, one rejected.
    fn shoot() -> tempfile::TempDir {
        let shoot = tempfile::tempdir().unwrap();
        let dir = shoot.path();
        for (name, pp3) in [
            ("A.RW2", "[General]\nRank=3\nColorLabel=0\nInTrash=false\n"),
            ("B.JPG", "[General]\nRank=0\nColorLabel=2\nInTrash=false\n"),
            ("C.JPG", "[General]\nRank=4\nColorLabel=0\nInTrash=true\n"),
        ] {
            fs::write(dir.join(name), b"image").unwrap();
            fs::write(dir.join(format!("{name}.pp3")), pp3).unwrap();
        }
        shoot
    }

    fn viewer(dir: &Path) -> Viewer {
        let shots = pairing::scan_dir(dir).unwrap();
        let mut viewer = Viewer::new(
            dir.to_path_buf(),
            shots,
            Picker::halfblocks(),
            Err("no RawTherapee".to_owned()),
        );
        // The marks, as the loader would have read them.
        for (state, shot) in viewer.states.iter_mut().zip(&viewer.shots) {
            state.marks = Some(rawtherapee::read_marks(shot).map_err(|err| err.to_string()));
        }
        viewer
    }

    fn press(viewer: &mut Viewer, c: char) {
        viewer.key(KeyEvent::from(KeyCode::Char(c)));
    }

    #[test]
    fn sending_asks_then_moves_the_kept_shots_whole() {
        let shoot = shoot();
        let dir = shoot.path();
        let mut viewer = viewer(dir);
        assert_eq!(viewer.states[1].marks().unwrap().color, ColorLabel::Yellow);

        press(&mut viewer, 'm');
        assert!(viewer.confirm_send);
        assert!(matches!(&viewer.message, Some(Ok(m)) if m.starts_with("move 2 kept shots")));
        assert!(dir.join("A.RW2").exists());

        press(&mut viewer, 'm');
        assert!(!viewer.confirm_send);
        assert!(
            matches!(&viewer.message, Some(Ok(m)) if m.starts_with("2 shots moved")),
            "{:?}",
            viewer.message
        );
        let selection = dir.join("selection");
        for name in ["A.RW2", "A.RW2.pp3", "B.JPG", "B.JPG.pp3"] {
            assert!(selection.join(name).exists(), "{name}");
            assert!(!dir.join(name).exists(), "{name}");
        }
        assert!(dir.join("C.JPG").exists());
        assert!(selection::is_sent(&viewer.shots[0], dir));
        assert!(!selection::is_sent(&viewer.shots[2], dir));

        // Nothing left to send; the sent shots keep their marks.
        press(&mut viewer, 'm');
        assert!(matches!(&viewer.message, Some(Err(m)) if m.contains("no kept shot")));
        assert_eq!(viewer.states[0].marks().unwrap().rank, 3);
    }

    #[test]
    fn an_added_shot_takes_its_place_and_keeps_the_others_ids() {
        let shoot = shoot();
        let dir = shoot.path();
        let mut viewer = viewer(dir);
        let ids_before = viewer.ids.clone();
        assert_eq!(ids_before, vec![ShotId(0), ShotId(1), ShotId(2)]);

        // A DNG written for A sorts right after it.
        fs::write(dir.join("A-deband.dng"), b"dng").unwrap();
        let shot = pairing::pair([dir.join("A-deband.dng")]).pop().unwrap();
        viewer.add_shot(shot.clone());
        assert_eq!(viewer.app.index, 1);
        assert_eq!(viewer.app.count, 4);
        assert_eq!(viewer.ids, vec![ShotId(0), ShotId(3), ShotId(1), ShotId(2)]);
        assert_eq!(viewer.position(ShotId(1)), 2);
        assert_eq!(viewer.position(ShotId(3)), 1);
        assert_eq!(viewer.shots[1].stem, "A-deband");
        assert_eq!(viewer.registry.read().unwrap().get(ShotId(3)).0, shot);
        assert_eq!(viewer.versions, vec![0, 0, 0, 0]);

        // Written again: same id, new version, loaded afresh.
        viewer.states[1].marks = Some(Ok(Marks::default()));
        viewer.add_shot(shot);
        assert_eq!(viewer.app.count, 4);
        assert_eq!(viewer.versions, vec![0, 0, 0, 1]);
        assert!(viewer.states[1].marks.is_none());
        assert_eq!(viewer.registry.read().unwrap().get(ShotId(3)).1, 1);
    }

    #[test]
    fn any_other_key_gives_up_sending() {
        let shoot = shoot();
        let mut viewer = viewer(shoot.path());
        press(&mut viewer, 'm');
        press(&mut viewer, 'l');
        assert!(!viewer.confirm_send);
        assert!(matches!(&viewer.message, Some(Err(m)) if m == "nothing sent"));
        assert!(shoot.path().join("A.RW2").exists());
        // The key that gave up was not acted on: still on the first shot.
        assert_eq!(viewer.app.index, 0);
    }
}
