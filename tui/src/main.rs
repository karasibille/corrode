//! `corrode`: browse the shots of a directory in the terminal.
//!
//! This first version is a spike: it checks that browsing and 100% zoom
//! are fluid enough with real shoots before building the culling features.

mod app;
mod encoder;
mod loader;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use corrode_core::exif::Exif;
use corrode_core::pairing::{self, Shot};
use corrode_core::picture::Picture;
use corrode_core::pp3::{self, Marks};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui_image::picker::Picker;
use ratatui_image::{FontSize, Image};

use app::{App, Command, Mode, zoom_crop};
use encoder::{Encoded, Encoder, Request};
use loader::{Job, Loaded, Loader};

/// Shots decoded ahead on each side of the current one.
const PRELOAD: usize = 2;
/// Previews kept in memory on each side, a bit more than preloaded so
/// that going back and forth does not decode again.
const KEEP: usize = 3;

type Info = (Result<Marks, String>, Result<Exif, String>);

struct Viewer {
    shots: Arc<Vec<Shot>>,
    app: App,
    loader: Loader,
    loaded: Receiver<Loaded>,
    encoder: Encoder,
    encoded: Receiver<Encoded>,
    font: FontSize,
    protocol_name: String,
    previews: HashMap<usize, Result<Arc<Picture>, String>>,
    full: Option<(usize, Result<Arc<Picture>, String>)>,
    infos: HashMap<usize, Info>,
    timings: HashMap<Job, Duration>,
    scheduled: Option<usize>,
    requested: Option<Request>,
    shown: Option<Encoded>,
    view: (u32, u32),
}

impl Viewer {
    fn new(shots: Vec<Shot>, picker: Picker) -> Viewer {
        let shots = Arc::new(shots);
        let (loaded_tx, loaded) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let threads = thread_count();
        Viewer {
            app: App::new(shots.len()),
            loader: Loader::new(Arc::clone(&shots), threads, loaded_tx),
            loaded,
            font: picker.font_size(),
            protocol_name: format!("{:?}", picker.protocol_type()),
            encoder: Encoder::new(picker, encoded_tx),
            encoded,
            shots,
            previews: HashMap::new(),
            full: None,
            infos: HashMap::new(),
            timings: HashMap::new(),
            scheduled: None,
            requested: None,
            shown: None,
            view: (0, 0),
        }
    }

    fn full_picture(&self) -> Option<&Arc<Picture>> {
        match &self.full {
            Some((index, Ok(picture))) if *index == self.app.index => Some(picture),
            _ => None,
        }
    }

    /// Asks for the current shot first, then its neighbours, then the
    /// full picture for the zoom, and forgets what is too far away.
    fn schedule(&mut self) {
        let index = self.app.index;
        if self.scheduled == Some(index) {
            return;
        }
        self.scheduled = Some(index);

        self.previews.retain(|&i, _| i.abs_diff(index) <= KEEP);
        if self.full.as_ref().is_some_and(|(i, _)| *i != index) {
            self.full = None;
        }

        let last = self.shots.len().saturating_sub(1);
        let mut jobs = vec![Job::Info(index), Job::Preview(index)];
        for distance in 1..=PRELOAD {
            jobs.extend(
                index
                    .checked_add(distance)
                    .filter(|&i| i <= last)
                    .map(Job::Preview),
            );
            jobs.extend(index.checked_sub(distance).map(Job::Preview));
        }
        jobs.push(Job::Full(index));
        jobs.retain(|job| match *job {
            Job::Info(i) => !self.infos.contains_key(&i),
            Job::Preview(i) => !self.previews.contains_key(&i),
            Job::Full(i) => self.full.as_ref().is_none_or(|(full, _)| *full != i),
        });
        self.loader.want(jobs);
    }

    fn receive(&mut self) {
        while let Ok(loaded) = self.loaded.try_recv() {
            match loaded {
                Loaded::Info { index, marks, exif } => {
                    self.infos.insert(index, (marks, exif));
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
        while let Ok(encoded) = self.encoded.try_recv() {
            // Ignore results for a request that was replaced meanwhile.
            if self.requested.as_ref() == Some(&encoded.request) {
                self.shown = Some(encoded);
            }
        }
    }

    fn key(&mut self, key: KeyEvent) {
        let zoomed = matches!(self.app.mode, Mode::Zoom { .. });
        let command = match key.code {
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Esc if zoomed => Command::ToggleZoom,
            KeyCode::Esc => Command::Quit,
            KeyCode::Char('z') | KeyCode::Enter => Command::ToggleZoom,
            KeyCode::Left | KeyCode::Char('h') if zoomed => Command::Pan { dx: -1, dy: 0 },
            KeyCode::Right | KeyCode::Char('l') if zoomed => Command::Pan { dx: 1, dy: 0 },
            KeyCode::Up | KeyCode::Char('k') if zoomed => Command::Pan { dx: 0, dy: -1 },
            KeyCode::Down | KeyCode::Char('j') if zoomed => Command::Pan { dx: 0, dy: 1 },
            KeyCode::Right | KeyCode::Char('l' | ' ' | 'n') | KeyCode::PageDown => Command::Next,
            KeyCode::Left | KeyCode::Char('h' | 'p') | KeyCode::Backspace | KeyCode::PageUp => {
                Command::Previous
            }
            KeyCode::Home | KeyCode::Char('g') => Command::First,
            KeyCode::End | KeyCode::Char('G') => Command::Last,
            _ => return,
        };
        let full_size = self
            .full_picture()
            .map(|p| (p.image.width(), p.image.height()));
        self.app.apply(command, full_size, self.view);
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [image_area, status_area, info_area, help_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.view = (
            u32::from(image_area.width) * u32::from(self.font.width),
            u32::from(image_area.height) * u32::from(self.font.height),
        );

        let index = self.app.index;
        let wanted = match self.app.mode {
            Mode::Fit => self.previews.get(&index).map(|p| (p.clone(), None)),
            Mode::Zoom { x, y } => self.full_picture().map(|picture| {
                let size = (picture.image.width(), picture.image.height());
                (
                    Ok(Arc::clone(picture)),
                    Some(zoom_crop((x, y), size, self.view)),
                )
            }),
        };
        let message = match wanted {
            None => Some("loading…".to_owned()),
            Some((Err(err), _)) => Some(err),
            Some((Ok(picture), crop)) => {
                let request = Request {
                    picture,
                    crop,
                    area: Size::new(image_area.width, image_area.height),
                };
                if self.requested.as_ref() != Some(&request) {
                    self.encoder.request(request.clone());
                    self.requested = Some(request);
                }
                None
            }
        };

        match (&self.shown, message) {
            (_, Some(message)) => {
                frame.render_widget(Paragraph::new(message).centered(), image_area)
            }
            (Some(shown), None) if Some(&shown.request) == self.requested.as_ref() => {
                match &shown.protocol {
                    Ok(protocol) => frame
                        .render_widget(Image::new(protocol), centered(image_area, protocol.size())),
                    Err(err) => {
                        frame.render_widget(Paragraph::new(err.as_str()).centered(), image_area)
                    }
                }
            }
            _ => frame.render_widget(Paragraph::new("drawing…").centered(), image_area),
        }

        frame.render_widget(
            Paragraph::new(self.status_line()).style(Style::new().reversed()),
            status_area,
        );
        frame.render_widget(Paragraph::new(self.info_line()), info_area);
        let help = "←/→ browse · z zoom 100% · arrows/hjkl move when zoomed · Home/End · q quit";
        frame.render_widget(Paragraph::new(help).fg(Color::DarkGray), help_area);
    }

    fn status_line(&self) -> String {
        let index = self.app.index;
        let shot = &self.shots[index];
        let files = [&shot.jpeg, &shot.raw]
            .into_iter()
            .flatten()
            .filter_map(|path| path.extension())
            .map(|ext| ext.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("+");
        let marks = match self.infos.get(&index) {
            Some((Ok(marks), _)) => describe(marks),
            Some((Err(_), _)) => "marks: unreadable sidecar".to_owned(),
            None => String::new(),
        };
        let (picture, job) = match self.app.mode {
            Mode::Fit => (self.previews.get(&index), Job::Preview(index)),
            Mode::Zoom { .. } => (self.full.as_ref().map(|(_, p)| p), Job::Full(index)),
        };
        let origin = match picture {
            Some(Ok(picture)) => format!("{:?}", picture.origin),
            _ => String::new(),
        };
        let ms = |duration: Option<&Duration>| {
            duration.map_or("–".to_owned(), |d| format!("{} ms", d.as_millis()))
        };
        let encode = self.shown.as_ref().map(|shown| &shown.elapsed);
        let zoom = match self.app.mode {
            Mode::Fit => "fit",
            Mode::Zoom { .. } => "100%",
        };
        format!(
            " {}/{}  {}  {files}  {marks}  {zoom} {origin}  decode {} · draw {} · {}",
            index + 1,
            self.shots.len(),
            shot.stem.to_string_lossy(),
            ms(self.timings.get(&job)),
            ms(encode),
            self.protocol_name,
        )
    }

    fn info_line(&self) -> Line<'static> {
        match self.infos.get(&self.app.index) {
            Some((_, Ok(exif))) => {
                let parts = [
                    exif.taken.clone(),
                    exif.camera.clone(),
                    exif.lens.clone(),
                    Some(exif.to_string()).filter(|s| !s.is_empty()),
                ];
                Line::from(format!(
                    " {}",
                    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
                ))
            }
            Some((_, Err(err))) => Line::from(format!(" exif: {err}")).fg(Color::Red),
            None => Line::default(),
        }
    }
}

fn describe(marks: &Marks) -> String {
    let stars = "★".repeat(marks.rank.into()) + &"☆".repeat((pp3::MAX_RANK - marks.rank).into());
    let trash = if marks.in_trash { " rejected" } else { "" };
    format!("{stars} {:?}{trash}", marks.color)
}

/// A rectangle of the given size centred in `area`.
fn centered(area: Rect, size: Size) -> Rect {
    let width = size.width.min(area.width);
    let height = size.height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// Decoding threads: enough to prepare the neighbours while the current
/// shot is decoded, leaving a core to the interface.
fn thread_count() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4))
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let shots = pairing::scan_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if shots.is_empty() {
        return Err(format!("{}: no JPEG or RAW file", dir.display()).into());
    }

    let mut terminal = ratatui::init();
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut viewer = Viewer::new(shots, picker);

    let result = (|| -> Result<(), Box<dyn Error>> {
        while !viewer.app.quit {
            viewer.schedule();
            viewer.receive();
            terminal.draw(|frame| viewer.draw(frame))?;
            if event::poll(Duration::from_millis(20))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                viewer.key(key);
            }
        }
        Ok(())
    })();

    ratatui::restore();
    result
}
