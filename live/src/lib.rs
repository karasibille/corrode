//! Working on a picture with effects live, in the terminal.
//!
//! The recipe is a text file watched while the picture is shown: each
//! time it is saved, it is applied to a reduced copy of the picture in a
//! background thread and the result replaces what is on screen. A recipe
//! that is not valid leaves the last good result and says what is wrong.
//! The ranges of a recipe (`a..b`) move on their own, about ten pictures
//! a second. `s` saves the picture on screen at full size next to the
//! source, `g` makes a GIF of the recipe.

mod motion;
mod recipe_file;
mod worker;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use corrode_core::pairing::{self, Shot};
use corrode_core::picture::{self, Picture};
use corrode_effects::{Animation, animate, write_gif};
use corrode_ui::encoder::{Encoded, Encoder, Request};
use corrode_ui::text::{centered, fit, keys_line};
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Stylize};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui_image::Image;
use ratatui_image::picker::Picker;

use motion::{PERIOD, has_ranges, moment, scaled};
use recipe_file::{Reading, Watcher, output_path, read, recipe_at};
use worker::{Done, Job, Worker};

/// The longer side of the copy of the picture the recipe is applied to
/// while working: a result in a fraction of a second.
const WORKING_SIDE: u32 = 1000;
/// The longer side of a GIF made with `g`, and its speed.
const GIF_SIDE: u32 = 800;
const GIF_FPS: u32 = 12;
/// What a recipe file holds when it is made.
const NEW_FILE: &str = "# A recipe: seed=N, then effects joined by +, each with its options.\n# Save to see it; `corrode effects` lists the effects.\nseed=7 fade + grain\n";

/// The picture reduced to the size the recipe is worked at.
fn working_image(image: &DynamicImage) -> RgbImage {
    if image.width().max(image.height()) > WORKING_SIDE {
        image
            .resize(WORKING_SIDE, WORKING_SIDE, FilterType::Triangle)
            .to_rgb8()
    } else {
        image.to_rgb8()
    }
}

/// What happened to the last recipe saved in the file.
enum Status {
    /// Nothing asked yet: the picture is shown as it is.
    Untouched,
    /// A recipe is being applied.
    Computing,
    /// The recipe applied, in this time.
    Applied(Duration),
    /// The recipe is wrong; the last good result stays on screen.
    Wrong(String),
}

/// A recipe as it was applied: what `s` saves.
struct Applied {
    words: Vec<String>,
    moment: f64,
    step: u64,
}

struct Live {
    source: PathBuf,
    shot: Shot,
    working: Arc<RgbImage>,
    /// What is on screen.
    current: Arc<Picture>,
    origin: picture::Origin,
    watcher: Watcher,
    worker: Worker,
    done: Receiver<Done>,
    /// The version of the recipe file, counted from 0.
    generation: u64,
    status: Status,
    /// The last recipe that applied, at the moment shown: what `s` and `g`
    /// use, whatever the file says now.
    applied: Option<Applied>,
    /// The words of the recipe in the file now, if it is valid.
    words: Option<Vec<String>>,
    jobs: u64,
    /// Whether the last job asked for is not done.
    pending: bool,
    /// Whether the recipe has ranges and they move.
    animating: bool,
    paused: bool,
    /// Whether the seed moves on at each frame.
    jitter: bool,
    period: Duration,
    started: Instant,
    /// How long the animation had run when it was paused.
    frozen: Duration,
    frames: u64,
    /// A save or a GIF is being made.
    busy: bool,
    finished_tx: Sender<Result<String, String>>,
    finished: Receiver<Result<String, String>>,
    message: Option<Result<String, String>>,
    encoder: Encoder,
    encoded: Receiver<Encoded>,
    requested: Option<Request>,
    shown: Option<Encoded>,
    quit: bool,
}

impl Live {
    fn new(source: PathBuf, shot: Shot, recipe: PathBuf, picker: Picker) -> Result<Live, String> {
        let preview = picture::preview(&shot).map_err(|err| err.to_string())?;
        let working = Arc::new(working_image(&preview.image));
        let origin = preview.origin;
        let (done_tx, done) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let (finished_tx, finished) = mpsc::channel();
        Ok(Live {
            source,
            shot,
            current: Arc::new(Picture {
                image: DynamicImage::ImageRgb8((*working).clone()),
                origin,
            }),
            origin,
            worker: Worker::new(Arc::clone(&working), done_tx),
            working,
            watcher: Watcher::new(recipe),
            done,
            generation: 0,
            status: Status::Untouched,
            applied: None,
            words: None,
            jobs: 0,
            pending: false,
            animating: false,
            paused: false,
            jitter: false,
            period: PERIOD,
            started: Instant::now(),
            frozen: Duration::ZERO,
            frames: 0,
            busy: false,
            finished_tx,
            finished,
            message: None,
            encoder: Encoder::new(picker, encoded_tx),
            encoded,
            requested: None,
            shown: None,
            quit: false,
        })
    }

    /// Reads the file if it changed, and asks for the recipe it holds.
    fn watch(&mut self) {
        let Some(text) = self.watcher.poll() else {
            return;
        };
        match read(&text) {
            Reading::Empty => {}
            Reading::Invalid(why) => self.status = Status::Wrong(why),
            Reading::Valid(_, words) => {
                self.generation += 1;
                self.status = Status::Computing;
                self.animating = has_ranges(&words);
                self.paused = false;
                self.started = Instant::now();
                self.frames = 0;
                self.words = Some(words);
                self.submit(0.0, 0);
            }
        }
    }

    /// Asks for the recipe of the file at a moment of its ranges.
    fn submit(&mut self, moment: f64, step: u64) {
        let Some(words) = self.words.clone() else {
            return;
        };
        self.jobs += 1;
        self.pending = true;
        self.worker.submit(Job {
            id: self.jobs,
            generation: self.generation,
            words,
            moment,
            step,
        });
    }

    /// How long the animation has run.
    fn elapsed(&self) -> Duration {
        if self.paused {
            self.frozen
        } else {
            self.started.elapsed()
        }
    }

    /// While the ranges move, asks for the next picture as soon as the
    /// last one is done, which is as fast as the effects allow.
    fn animate(&mut self) {
        if !self.animating || self.paused || self.pending {
            return;
        }
        self.frames += 1;
        let step = if self.jitter { self.frames } else { 0 };
        self.submit(moment(self.elapsed(), self.period), step);
    }

    fn pause(&mut self) {
        if !self.animating {
            return;
        }
        if self.paused {
            // Goes on from where it stopped.
            self.started = Instant::now() - self.frozen;
        } else {
            self.frozen = self.started.elapsed();
        }
        self.paused = !self.paused;
    }

    /// Changes the speed, the moment staying where it is.
    fn faster(&mut self, factor: f64) {
        let at = self.elapsed().as_secs_f64() / self.period.as_secs_f64();
        self.period = scaled(self.period, factor);
        let elapsed = Duration::from_secs_f64(at * self.period.as_secs_f64());
        self.started = Instant::now() - elapsed;
        self.frozen = elapsed;
    }

    /// Takes what the background threads finished.
    fn receive(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            if done.id == self.jobs {
                self.pending = false;
            }
            if done.generation != self.generation {
                continue; // The file was saved again since.
            }
            match done.result {
                Ok(image) => {
                    self.current = Arc::new(Picture {
                        image: DynamicImage::ImageRgb8(image),
                        origin: self.origin,
                    });
                    self.applied = Some(Applied {
                        words: done.words,
                        moment: done.moment,
                        step: done.step,
                    });
                    self.status = Status::Applied(done.elapsed);
                }
                Err(why) => self.status = Status::Wrong(why),
            }
        }
        while let Ok(finished) = self.finished.try_recv() {
            self.busy = false;
            self.message = Some(finished);
        }
        while let Ok(encoded) = self.encoded.try_recv() {
            self.shown = Some(encoded);
        }
    }

    fn key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char(' ') => self.pause(),
            KeyCode::Char('j') => self.jitter = !self.jitter,
            KeyCode::Char('[') => self.faster(1.25),
            KeyCode::Char(']') => self.faster(0.8),
            KeyCode::Char('s') => self.save(),
            KeyCode::Char('g') => self.gif(),
            _ => {}
        }
    }

    /// The recipe to save, if a save can start.
    fn start_saving(&mut self) -> Option<Applied> {
        if self.busy {
            self.message = Some(Err("already saving".to_owned()));
            return None;
        }
        let Some(applied) = self.applied.as_ref() else {
            self.message = Some(Err("no recipe applied yet".to_owned()));
            return None;
        };
        let applied = Applied {
            words: applied.words.clone(),
            moment: applied.moment,
            step: applied.step,
        };
        self.busy = true;
        self.message = Some(Ok("saving…".to_owned()));
        Some(applied)
    }

    /// Applies the recipe, as it is on screen, to the full-size picture
    /// and saves it next to the source, in a thread: a RAW takes a while
    /// to develop.
    fn save(&mut self) {
        let Some(applied) = self.start_saving() else {
            return;
        };
        let (shot, tx) = (self.shot.clone(), self.finished_tx.clone());
        let path = output_path(&self.source, "jpg", Path::exists);
        let _ = thread::spawn(move || {
            let made = (|| -> Result<String, String> {
                let recipe = recipe_at(&applied.words, applied.moment, applied.step)?;
                let full = picture::full(&shot)
                    .map_err(|err| err.to_string())?
                    .image
                    .to_rgb8();
                let result = recipe.apply(&full).map_err(|err| err.to_string())?;
                result
                    .save(&path)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                Ok(format!("saved {}", path.display()))
            })();
            let _ = tx.send(made);
        });
    }

    /// Makes a GIF of the last good recipe, the ranges in it moving.
    fn gif(&mut self) {
        let Some(Applied { words, .. }) = self.start_saving() else {
            return;
        };
        let jitter = self.jitter;
        let (working, tx) = (Arc::clone(&self.working), self.finished_tx.clone());
        let path = output_path(&self.source, "gif", Path::exists);
        let _ = thread::spawn(move || {
            let made = (|| -> Result<String, String> {
                let small = DynamicImage::ImageRgb8((*working).clone());
                let small = if small.width().max(small.height()) > GIF_SIDE {
                    small
                        .resize(GIF_SIDE, GIF_SIDE, FilterType::Triangle)
                        .to_rgb8()
                } else {
                    small.to_rgb8()
                };
                let animation = Animation {
                    jitter,
                    ..Animation::default()
                };
                let frames = animate(&small, &words, animation).map_err(|err| err.to_string())?;
                write_gif(frames, GIF_FPS, &path)?;
                Ok(format!("saved {}", path.display()))
            })();
            let _ = tx.send(made);
        });
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [picture_area, status_area, keys_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_picture(frame, picture_area);
        frame.render_widget(
            Paragraph::new(self.status_line(status_area.width)),
            status_area,
        );
        frame.render_widget(
            Paragraph::new(keys_line(&self.keys(), keys_area.width)),
            keys_area,
        );
    }

    /// The picture, which stays as it was while the next one is made.
    fn draw_picture(&mut self, frame: &mut Frame, area: Rect) {
        let request = Request {
            picture: Arc::clone(&self.current),
            crop: None,
            scale: 1,
            area: Size::new(area.width, area.height),
        };
        if self.requested.as_ref() != Some(&request) {
            self.encoder.request(request.clone());
            self.requested = Some(request);
        }
        match &self.shown {
            Some(shown) => match &shown.protocol {
                Ok(protocol) => {
                    frame.render_widget(Image::new(protocol), centered(area, protocol.size()));
                }
                Err(err) => frame.render_widget(Paragraph::new(err.as_str()).centered(), area),
            },
            None => frame.render_widget(Paragraph::new("drawing…").centered(), area),
        }
    }

    /// The keys that do something now: the ones for the animation only
    /// when the recipe has ranges.
    fn keys(&self) -> Vec<(&'static str, &'static str)> {
        let mut keys = Vec::new();
        if self.animating {
            keys.extend([
                ("space", "pause"),
                ("[ ]", "speed"),
                (
                    "j",
                    if self.jitter {
                        "seed still"
                    } else {
                        "seed moves"
                    },
                ),
            ]);
        }
        keys.extend([("s", "save full size"), ("g", "gif"), ("q", "quit")]);
        keys
    }

    fn status_line(&self, width: u16) -> ratatui::text::Line<'static> {
        let name = self
            .watcher
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let state = match &self.status {
            Status::Untouched => Span::from("save the recipe to see it"),
            Status::Computing => Span::from("computing…").yellow(),
            Status::Applied(elapsed) => {
                let time = format!("{:.0} ms", elapsed.as_secs_f64() * 1000.0);
                match (&self.applied, self.animating) {
                    (Some(applied), true) => Span::from(format!(
                        "{} at {:.0} %, {time} a picture",
                        if self.paused { "paused" } else { "playing" },
                        applied.moment * 100.0,
                    ))
                    .green(),
                    _ => Span::from(format!("applied in {time}")).green(),
                }
            }
            Status::Wrong(why) => {
                let kept = if self.applied.is_some() {
                    " (the last good recipe stays)"
                } else {
                    ""
                };
                Span::from(format!("error: {why}{kept}")).fg(Color::Red)
            }
        };
        let message = match &self.message {
            Some(Ok(text)) => Span::from(text.clone()).cyan(),
            Some(Err(text)) => Span::from(text.clone()).fg(Color::Red),
            None => Span::default(),
        };
        fit(
            &[
                (1, vec![state]),
                (2, vec![message]),
                (3, vec![Span::from(name).bold()]),
            ],
            usize::from(width),
        )
    }
}

/// Works live on a picture with the recipe of a file, until the user
/// quits. The file is made, with an example, if it does not exist.
pub fn run(image: PathBuf, recipe: PathBuf) -> Result<(), Box<dyn Error>> {
    let shot = pairing::shots_of(std::slice::from_ref(&image))
        .map_err(|err| format!("{}: {err}", image.display()))?
        .into_iter()
        .next()
        .ok_or("no shot")?;
    if !recipe.exists() {
        fs::write(&recipe, NEW_FILE).map_err(|err| format!("{}: {err}", recipe.display()))?;
    }
    // Everything that can fail on the files comes before the terminal is
    // taken over.
    let mut terminal = ratatui::init();
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut live = match Live::new(image, shot, recipe, picker) {
        Ok(live) => live,
        Err(err) => {
            ratatui::restore();
            return Err(err.into());
        }
    };

    let result = (|| -> Result<(), Box<dyn Error>> {
        while !live.quit {
            live.watch();
            live.receive();
            live.animate();
            terminal.draw(|frame| live.draw(frame))?;
            if event::poll(Duration::from_millis(20))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                live.key(key.code, key.modifiers);
            }
        }
        Ok(())
    })();

    ratatui::restore();
    result
}
