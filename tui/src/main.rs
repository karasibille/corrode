//! `corrode`: cull the shots of a directory in the terminal.
//!
//! Shots are shown one at a time, with a strip of the burst they belong
//! to. Marks are written to RawTherapee sidecars as soon as they are set.

mod app;
mod encoder;
mod loader;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use corrode_core::bursts;
use corrode_core::exif::Exif;
use corrode_core::pairing::{self, Shot};
use corrode_core::picture::Picture;
use corrode_core::pp3::{self, ColorLabel, Marks};
use corrode_core::rawtherapee::{self, Config};
use image::DynamicImage;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Image, Resize};

use app::{App, Command, MarkChange, Mode, Time, burst_around, zoom_crop};
use encoder::{Encoded, Encoder, Request};
use loader::{Job, Loaded, Loader};

/// Shots decoded ahead on each side of the current one.
const PRELOAD: usize = 2;
/// Previews kept in memory on each side, a bit more than preloaded so
/// that going back and forth does not decode again.
const KEEP: usize = 3;
/// Height of the burst thumbnails, in rows; a line of marks goes below.
const THUMB_ROWS: u16 = 5;

struct Viewer {
    shots: Arc<Vec<Shot>>,
    app: App,
    loader: Loader,
    loaded: Receiver<Loaded>,
    encoder: Encoder,
    encoded: Receiver<Encoded>,
    /// Used for the thumbnails, which are small enough to encode here.
    picker: Picker,
    font: FontSize,
    protocol_name: String,
    previews: HashMap<usize, Result<Arc<Picture>, String>>,
    full: Option<(usize, Result<Arc<Picture>, String>)>,
    times: Vec<Time>,
    exifs: HashMap<usize, Result<Exif, String>>,
    marks: HashMap<usize, Result<Marks, String>>,
    thumbnails: HashMap<usize, Option<DynamicImage>>,
    thumbnail_protocols: HashMap<usize, Protocol>,
    thumbnail_size: Size,
    timings: HashMap<Job, Duration>,
    scheduled: Option<usize>,
    requested: Option<Request>,
    shown: Option<Encoded>,
    view: (u32, u32),
    /// RawTherapee's settings, needed to create sidecars; marks cannot be
    /// written without them.
    config: Result<Config, String>,
    /// Result of the last action, shown until the next key.
    message: Option<Result<String, String>>,
}

impl Viewer {
    fn new(shots: Vec<Shot>, picker: Picker, config: Result<Config, String>) -> Viewer {
        let shots = Arc::new(shots);
        let (loaded_tx, loaded) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let font = picker.font_size();
        // Thumbnails are 4:3, the shape of the sensor.
        let thumbnail_columns = (u32::from(THUMB_ROWS) * u32::from(font.height) * 4 / 3)
            .div_ceil(u32::from(font.width.max(1)));
        Viewer {
            app: App::new(shots.len()),
            loader: Loader::new(Arc::clone(&shots), thread_count(), loaded_tx),
            loaded,
            font,
            protocol_name: format!("{:?}", picker.protocol_type()),
            encoder: Encoder::new(picker.clone(), encoded_tx),
            encoded,
            picker,
            times: vec![Time::Unknown; shots.len()],
            shots,
            previews: HashMap::new(),
            full: None,
            exifs: HashMap::new(),
            marks: HashMap::new(),
            thumbnails: HashMap::new(),
            thumbnail_protocols: HashMap::new(),
            thumbnail_size: Size::new(
                u16::try_from(thumbnail_columns).unwrap_or(u16::MAX).max(4),
                THUMB_ROWS,
            ),
            timings: HashMap::new(),
            scheduled: None,
            requested: None,
            shown: None,
            view: (0, 0),
            config,
            message: None,
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

    /// Asks for the current shot first, then its neighbours, then the
    /// full picture for the zoom, then the head of every other file, the
    /// closest first, so that bursts take shape around the current shot.
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
        let mut others: Vec<usize> = (0..self.shots.len()).filter(|&i| i != index).collect();
        others.sort_by_key(|&i| i.abs_diff(index));
        jobs.extend(others.into_iter().map(Job::Head));
        jobs.retain(|job| match *job {
            Job::Head(i) => self.times[i] == Time::Unknown,
            Job::Preview(i) => !self.previews.contains_key(&i),
            Job::Full(i) => self.full.as_ref().is_none_or(|(full, _)| *full != i),
        });
        self.loader.want(jobs);
    }

    fn receive(&mut self) {
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
                    self.exifs.insert(index, exif);
                    // Marks set meanwhile from the keyboard are more recent.
                    self.marks.entry(index).or_insert(marks);
                    self.thumbnails.insert(index, thumbnail);
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
        self.message = None;
        let zoomed = matches!(self.app.mode, Mode::Zoom { .. });
        let command = match key.code {
            KeyCode::Char(c) if rank_key(c).is_some() => {
                Command::Mark(MarkChange::Rank(rank_key(c).unwrap_or_default()))
            }
            KeyCode::Char('r') => Command::Mark(MarkChange::ToggleColor(ColorLabel::Red)),
            KeyCode::Char('y') => Command::Mark(MarkChange::ToggleColor(ColorLabel::Yellow)),
            KeyCode::Char('g') => Command::Mark(MarkChange::ToggleColor(ColorLabel::Green)),
            KeyCode::Char('b') => Command::Mark(MarkChange::ToggleColor(ColorLabel::Blue)),
            KeyCode::Char('p') => Command::Mark(MarkChange::ToggleColor(ColorLabel::Purple)),
            KeyCode::Char('x') | KeyCode::Delete => Command::Mark(MarkChange::ToggleTrash),
            KeyCode::Char('k') => return self.keep_in_burst(),
            KeyCode::Char('q') => Command::Quit,
            KeyCode::Esc if zoomed => Command::ToggleZoom,
            KeyCode::Esc => Command::Quit,
            KeyCode::Char('z') | KeyCode::Enter => Command::ToggleZoom,
            KeyCode::Left if zoomed => Command::Pan { dx: -1, dy: 0 },
            KeyCode::Right if zoomed => Command::Pan { dx: 1, dy: 0 },
            KeyCode::Up if zoomed => Command::Pan { dx: 0, dy: -1 },
            KeyCode::Down if zoomed => Command::Pan { dx: 0, dy: 1 },
            KeyCode::Right | KeyCode::Char('l' | ' ') | KeyCode::PageDown => Command::Next,
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace | KeyCode::PageUp => {
                Command::Previous
            }
            KeyCode::Down | KeyCode::Char(']') => Command::GoTo(self.next_burst()),
            KeyCode::Up | KeyCode::Char('[') => Command::GoTo(self.previous_burst()),
            KeyCode::Home => Command::First,
            KeyCode::End => Command::Last,
            _ => return,
        };
        if let Command::Mark(change) = command {
            self.mark(change);
            return;
        }
        let full_size = self
            .full_picture()
            .map(|p| (p.image.width(), p.image.height()));
        self.app.apply(command, full_size, self.view);
    }

    /// The first shot after the current burst, or the last shot.
    fn next_burst(&self) -> usize {
        self.burst().0.end.min(self.shots.len() - 1)
    }

    /// The first shot of the current burst if the current shot is not, else
    /// the first shot of the previous burst.
    fn previous_burst(&self) -> usize {
        let start = self.burst().0.start;
        if self.app.index > start || start == 0 {
            start
        } else {
            burst_around(&self.times, start - 1, bursts::MAX_GAP_MS)
                .0
                .start
        }
    }

    /// Writes marks for some shots, keeping the ones shown up to date.
    fn write_marks(&mut self, changes: &[(usize, Marks)]) -> Result<(), String> {
        let config = self
            .config
            .as_ref()
            .map_err(|err| format!("cannot write marks: {err}"))?;
        for &(index, marks) in changes {
            config
                .write_marks(&self.shots[index], &marks)
                .map_err(|err| format!("cannot write marks: {err}"))?;
            self.marks.insert(index, Ok(marks));
        }
        Ok(())
    }

    /// The marks of a shot, read now if not known yet.
    fn current_marks(&self, index: usize) -> Result<Marks, String> {
        match self.marks.get(&index) {
            Some(marks) => marks.clone(),
            None => rawtherapee::read_marks(&self.shots[index]).map_err(|err| err.to_string()),
        }
    }

    /// Changes the marks of the current shot and writes them right away.
    fn mark(&mut self, change: MarkChange) {
        let index = self.app.index;
        let result = self
            .current_marks(index)
            .map(|marks| change.apply(marks))
            .and_then(|marks| self.write_marks(&[(index, marks)]).map(|()| marks));
        self.message = Some(result.map(|marks| format!("{} saved", describe(&marks))));
    }

    /// Keeps the current shot of its burst and rejects the others, then
    /// moves to the next burst. The kept shot gets at least one star.
    fn keep_in_burst(&mut self) {
        let (burst, complete) = self.burst();
        if !complete {
            self.message = Some(Err(
                "this burst is still being read, try again in a moment".to_owned()
            ));
            return;
        }
        let index = self.app.index;
        let changes: Result<Vec<(usize, Marks)>, String> = burst
            .clone()
            .map(|i| {
                let marks = self.current_marks(i)?;
                Ok((
                    i,
                    if i == index {
                        Marks {
                            rank: marks.rank.max(1),
                            in_trash: false,
                            ..marks
                        }
                    } else {
                        Marks {
                            in_trash: true,
                            ..marks
                        }
                    },
                ))
            })
            .collect();
        let result = changes.and_then(|changes| self.write_marks(&changes));
        self.message = Some(result.map(|()| {
            format!(
                "kept {}, rejected the {} other shots of the burst",
                self.shots[index].stem.to_string_lossy(),
                burst.len() - 1
            )
        }));
        if burst.end < self.shots.len() {
            self.app.apply(Command::GoTo(burst.end), None, self.view);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [image_area, strip_area, status_area, info_area, help_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(THUMB_ROWS + 1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.view = (
            u32::from(image_area.width) * u32::from(self.font.width),
            u32::from(image_area.height) * u32::from(self.font.height),
        );

        self.draw_picture(frame, image_area);
        self.draw_strip(frame, strip_area);

        frame.render_widget(
            Paragraph::new(self.status_line()).style(Style::new().reversed()),
            status_area,
        );
        frame.render_widget(Paragraph::new(self.info_line()), info_area);
        let help = match &self.message {
            Some(Ok(message)) => Line::from(format!(" {message}")).fg(Color::Green),
            Some(Err(message)) => Line::from(format!(" {message}")).fg(Color::Red),
            None => Line::from(
                " ←/→ shot · ↑/↓ burst · k keep, reject the rest · 1-5 0 rank · r y g b p color · x reject · z zoom · q quit",
            )
            .fg(Color::DarkGray),
        };
        frame.render_widget(Paragraph::new(help), help_area);
    }

    fn draw_picture(&mut self, frame: &mut Frame, area: Rect) {
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
                    area: Size::new(area.width, area.height),
                };
                if self.requested.as_ref() != Some(&request) {
                    self.encoder.request(request.clone());
                    self.requested = Some(request);
                }
                None
            }
        };

        match (&self.shown, message) {
            (_, Some(message)) => frame.render_widget(Paragraph::new(message).centered(), area),
            (Some(shown), None) if Some(&shown.request) == self.requested.as_ref() => {
                match &shown.protocol {
                    Ok(protocol) => {
                        frame.render_widget(Image::new(protocol), centered(area, protocol.size()))
                    }
                    Err(err) => frame.render_widget(Paragraph::new(err.as_str()).centered(), area),
                }
            }
            _ => frame.render_widget(Paragraph::new("drawing…").centered(), area),
        }
    }

    /// The thumbnails of the current burst, centred on the current shot,
    /// each with its marks below.
    fn draw_strip(&mut self, frame: &mut Frame, area: Rect) {
        let (burst, complete) = self.burst();
        let index = self.app.index;
        let slot_width = self.thumbnail_size.width + 1;
        let visible = usize::from((area.width / slot_width).max(1));
        let first = if burst.len() <= visible {
            burst.start
        } else {
            index
                .saturating_sub(visible / 2)
                .clamp(burst.start, burst.end - visible)
        };

        for (slot, i) in (first..burst.end.min(first + visible)).enumerate() {
            let x = area.x + u16::try_from(slot).unwrap_or(0) * slot_width;
            let picture_area = Rect::new(x, area.y, self.thumbnail_size.width, THUMB_ROWS);
            let label_area = Rect::new(x, area.y + THUMB_ROWS, self.thumbnail_size.width, 1);

            if !self.thumbnail_protocols.contains_key(&i)
                && let Some(Some(thumbnail)) = self.thumbnails.get(&i)
                && let Ok(protocol) = self.picker.new_protocol(
                    thumbnail.clone(),
                    self.thumbnail_size,
                    Resize::Fit(None),
                )
            {
                self.thumbnail_protocols.insert(i, protocol);
            }
            match self.thumbnail_protocols.get(&i) {
                Some(protocol) => frame.render_widget(
                    Image::new(protocol),
                    centered(picture_area, protocol.size()),
                ),
                None => frame.render_widget(Paragraph::new("·").centered(), picture_area),
            }

            let (label, rejected) = match self.marks.get(&i) {
                Some(Ok(marks)) => (short_marks(marks), marks.in_trash),
                Some(Err(_)) => ("?".to_owned(), false),
                None => (String::new(), false),
            };
            let mut style = Style::new();
            if rejected {
                style = style.fg(Color::Red);
            }
            if i == index {
                style = style.reversed();
            }
            frame.render_widget(Paragraph::new(label).centered().style(style), label_area);
        }

        if !complete {
            let more = Rect::new(
                area.right().saturating_sub(1),
                area.y + THUMB_ROWS / 2,
                1,
                1,
            );
            frame.render_widget(Paragraph::new("…"), more);
        }
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
        let marks = match self.marks.get(&index) {
            Some(Ok(marks)) => describe(marks),
            Some(Err(_)) => "marks: unreadable sidecar".to_owned(),
            None => String::new(),
        };
        let (burst, complete) = self.burst();
        let burst = format!(
            "burst {}/{}{}",
            index - burst.start + 1,
            burst.len(),
            if complete { "" } else { "+" }
        );
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
        let read = self
            .times
            .iter()
            .filter(|&&time| time != Time::Unknown)
            .count();
        let reading = if read < self.shots.len() {
            format!(" · reading {read}/{}", self.shots.len())
        } else {
            String::new()
        };
        format!(
            " {}/{}  {}  {files}  {marks}  {burst}  {zoom} {origin}  decode {} · draw {} · {}{reading}",
            index + 1,
            self.shots.len(),
            shot.stem.to_string_lossy(),
            ms(self.timings.get(&job)),
            ms(encode),
            self.protocol_name,
        )
    }

    fn info_line(&self) -> Line<'static> {
        match self.exifs.get(&self.app.index) {
            Some(Ok(exif)) => {
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
            Some(Err(err)) => Line::from(format!(" exif: {err}")).fg(Color::Red),
            None => Line::default(),
        }
    }
}

fn describe(marks: &Marks) -> String {
    let stars = "★".repeat(marks.rank.into()) + &"☆".repeat((pp3::MAX_RANK - marks.rank).into());
    let trash = if marks.in_trash { " rejected" } else { "" };
    format!("{stars} {:?}{trash}", marks.color)
}

/// Marks in a few characters, for a thumbnail: `★3 G ✗`.
fn short_marks(marks: &Marks) -> String {
    let rank = (marks.rank > 0).then(|| format!("★{}", marks.rank));
    let color = match marks.color {
        ColorLabel::None => None,
        ColorLabel::Red => Some("R"),
        ColorLabel::Yellow => Some("Y"),
        ColorLabel::Green => Some("G"),
        ColorLabel::Blue => Some("B"),
        ColorLabel::Purple => Some("P"),
    };
    let trash = marks.in_trash.then_some("✗");
    [rank.as_deref(), color, trash]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rating a key sets: digits, or the keys under them on an AZERTY
/// keyboard, where digits need Shift.
fn rank_key(key: char) -> Option<u8> {
    match key {
        '0'..='5' => key.to_digit(10).and_then(|digit| u8::try_from(digit).ok()),
        'à' => Some(0),
        '&' => Some(1),
        'é' => Some(2),
        '"' => Some(3),
        '\'' => Some(4),
        '(' => Some(5),
        _ => None,
    }
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

    let config = Config::load().map_err(|err| err.to_string());
    let mut terminal = ratatui::init();
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    let mut viewer = Viewer::new(shots, picker, config);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_come_from_digits_or_the_azerty_keys_above_letters() {
        for (keys, rank) in [
            ("0à", 0),
            ("1&", 1),
            ("2é", 2),
            ("3\"", 3),
            ("4'", 4),
            ("5(", 5),
        ] {
            for key in keys.chars() {
                assert_eq!(rank_key(key), Some(rank), "{key}");
            }
        }
        assert_eq!(rank_key('6'), None);
        assert_eq!(rank_key('a'), None);
    }

    #[test]
    fn short_marks_fit_under_a_thumbnail() {
        let marks = |rank, color, in_trash| Marks {
            rank,
            color,
            in_trash,
        };
        assert_eq!(short_marks(&marks(3, ColorLabel::Green, true)), "★3 G ✗");
        assert_eq!(short_marks(&marks(0, ColorLabel::None, false)), "");
        assert_eq!(short_marks(&marks(0, ColorLabel::Red, false)), "R");
    }
}
