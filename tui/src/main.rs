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
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Image, Resize};

use app::{App, Command, Filter, MarkChange, Mode, Time, burst_around, next_matching, zoom_crop};
use encoder::{Encoded, Encoder, Request};
use loader::{Assessment, Job, Loaded, Loader};

/// Shots decoded ahead on each side of the current one.
const PRELOAD: usize = 2;
/// Previews kept in memory on each side, a bit more than preloaded so
/// that going back and forth does not decode again.
const KEEP: usize = 3;
/// Height of the burst thumbnails, in rows; a line of marks goes below.
const THUMB_ROWS: u16 = 5;

struct Viewer {
    dir: PathBuf,
    shots: Arc<Vec<Shot>>,
    filter: Filter,
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
    assessments: HashMap<usize, Option<Assessment>>,
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
    fn new(
        dir: PathBuf,
        shots: Vec<Shot>,
        picker: Picker,
        config: Result<Config, String>,
    ) -> Viewer {
        let shots = Arc::new(shots);
        let (loaded_tx, loaded) = mpsc::channel();
        let (encoded_tx, encoded) = mpsc::channel();
        let font = picker.font_size();
        // Thumbnails are 4:3, the shape of the sensor.
        let thumbnail_columns = (u32::from(THUMB_ROWS) * u32::from(font.height) * 4 / 3)
            .div_ceil(u32::from(font.width.max(1)));
        Viewer {
            dir,
            filter: Filter::All,
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
            assessments: HashMap::new(),
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

    /// Asks for the current shot first, then its neighbours, then the
    /// full picture for the zoom, then the quality of its burst, then the
    /// head of every other file, the closest first, so that bursts take
    /// shape around the current shot. Runs again when the shot changes, or
    /// when its burst grows as files are read.
    fn schedule(&mut self) {
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
            Job::Assess(i) => !self.assessments.contains_key(&i),
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
                Loaded::Assessment { index, assessment } => {
                    self.assessments.insert(index, assessment);
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
        if self.help {
            // Arrows scroll the help; any other key closes it, q still quits.
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                KeyCode::Char('q') => self.app.quit = true,
                _ => self.help = false,
            }
            return;
        }
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
            KeyCode::Char('X') => return self.reject_burst(),
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
                return;
            }
            KeyCode::Char('f') => return self.next_filter(),
            KeyCode::Char('o') => return self.open(false),
            KeyCode::Char('O') => return self.open(true),
            KeyCode::Char('s') => match self.sharpest() {
                Some(index) => Command::GoTo(index),
                None => {
                    self.message = Some(Err("sharpness not measured yet".to_owned()));
                    return;
                }
            },
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
        let Some(command) = self.filtered(command) else {
            self.message = Some(Err(format!("no other {} shot", self.filter.name())));
            return;
        };
        let full_size = self
            .full_picture()
            .map(|p| (p.image.width(), p.image.height()));
        self.app.apply(command, full_size, self.view);
    }

    fn matches(&self, index: usize) -> bool {
        let marks = self.marks.get(&index).and_then(|marks| marks.as_ref().ok());
        self.filter.matches(marks)
    }

    /// The command that moves to a shot of the filter instead of any shot,
    /// or `None` if there is no such shot in that direction.
    fn filtered(&self, command: Command) -> Option<Command> {
        if self.filter == Filter::All {
            return Some(command);
        }
        let count = self.shots.len();
        let index = self.app.index;
        let matches = |i| self.matches(i);
        let target = match command {
            Command::Next => next_matching(index, true, count, matches),
            Command::Previous => next_matching(index, false, count, matches),
            Command::First => (0..count).find(|&i| matches(i)),
            Command::Last => (0..count).rev().find(|&i| matches(i)),
            // A burst jump lands on the first shot of the filter from there.
            Command::GoTo(target) if target > index => (target..count).find(|&i| matches(i)),
            Command::GoTo(target) if target < index => (target..index).find(|&i| matches(i)),
            other => return Some(other),
        };
        target.map(Command::GoTo)
    }

    /// Moves to the next filter, and to a shot it shows if the current one
    /// is not.
    fn next_filter(&mut self) {
        self.filter = self.filter.next();
        let index = self.app.index;
        let count = self.shots.len();
        let target = if self.matches(index) {
            Some(index)
        } else {
            next_matching(index, true, count, |i| self.matches(i))
                .or_else(|| next_matching(index, false, count, |i| self.matches(i)))
        };
        let read = self.marks.len();
        let shown = (0..count).filter(|&i| self.matches(i)).count();
        self.message = Some(match target {
            Some(target) => {
                self.app.apply(Command::GoTo(target), None, self.view);
                Ok(format!(
                    "showing {} shots: {shown} of {read} read",
                    self.filter.name()
                ))
            }
            None => Err(format!(
                "no {} shot among the {read} read",
                self.filter.name()
            )),
        });
    }

    /// Opens the current shot in RawTherapee's editor, or the directory in
    /// its file browser, which shows the marks and can filter by them.
    fn open(&mut self, directory: bool) {
        let path = if directory {
            self.dir.clone()
        } else {
            rawtherapee::file_to_open(&self.shots[self.app.index]).to_owned()
        };
        self.message = Some(
            rawtherapee::open(&path)
                .map(|()| format!("opening {} in RawTherapee", path.display()))
                .map_err(|err| format!("cannot start RawTherapee: {err}")),
        );
    }

    /// The quality of a shot, if measured.
    fn assessment(&self, index: usize) -> Option<Assessment> {
        self.assessments.get(&index).copied().flatten()
    }

    /// The sharpest shot of the current burst among those measured, if
    /// there is more than one; shots with light bands only when all of
    /// them have some.
    fn sharpest(&self) -> Option<usize> {
        let measured: Vec<(usize, Assessment)> = self
            .burst()
            .0
            .filter_map(|i| Some((i, self.assessment(i)?)))
            .collect();
        if measured.len() < 2 {
            return None;
        }
        let clean = measured.iter().any(|(_, a)| !a.banded);
        measured
            .into_iter()
            .filter(|(_, a)| !clean || !a.banded)
            .max_by(|a, b| a.1.sharpness.total_cmp(&b.1.sharpness))
            .map(|(i, _)| i)
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

    /// Rejects every shot of the current burst, then moves to the next one.
    fn reject_burst(&mut self) {
        let (burst, complete) = self.burst();
        if !complete {
            self.message = Some(Err(
                "this burst is still being read, try again in a moment".to_owned()
            ));
            return;
        }
        let changes: Result<Vec<(usize, Marks)>, String> = burst
            .clone()
            .map(|i| {
                let marks = self.current_marks(i)?;
                Ok((
                    i,
                    Marks {
                        in_trash: true,
                        ..marks
                    },
                ))
            })
            .collect();
        let result = changes.and_then(|changes| self.write_marks(&changes));
        self.message =
            Some(result.map(|()| format!("rejected the {} shots of the burst", burst.len())));
        if burst.end < self.shots.len() {
            self.app.apply(Command::GoTo(burst.end), None, self.view);
        }
    }

    /// What culling left, printed when quitting. Marks not read yet are
    /// read now, so that the counts cover the whole directory.
    fn summary(&mut self) -> String {
        for index in 0..self.shots.len() {
            if !self.marks.contains_key(&index) {
                let marks =
                    rawtherapee::read_marks(&self.shots[index]).map_err(|err| err.to_string());
                self.marks.insert(index, marks);
            }
        }
        let progress = self.progress();
        let unreadable = self.marks.values().filter(|marks| marks.is_err()).count();
        let mut summary = format!(
            "{}: {} shots, {} kept, {} rejected, {} unsorted.",
            self.dir.display(),
            self.shots.len(),
            progress.kept,
            progress.rejected,
            progress.unsorted,
        );
        if unreadable > 0 {
            summary.push_str(&format!(" {unreadable} with an unreadable sidecar."));
        }
        summary.push_str(" Marks are saved in RawTherapee's .pp3 sidecars.");
        summary
    }

    /// How far culling has gone, from the marks read so far.
    fn progress(&self) -> Progress {
        let count = |filter: Filter| {
            self.marks
                .values()
                .filter(|marks| filter.matches(marks.as_ref().ok()))
                .count()
        };
        Progress {
            kept: count(Filter::Kept),
            rejected: count(Filter::Rejected),
            unsorted: count(Filter::Unsorted),
            unread: self.shots.len() - self.marks.len(),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        if self.help {
            let area = frame.area();
            let lines = self.help_text(area.width);
            // Keep the end of the help on screen when scrolling down.
            let last = u16::try_from(lines.len()).unwrap_or(u16::MAX);
            self.help_scroll = self.help_scroll.min(last.saturating_sub(area.height / 2));
            let help = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.help_scroll, 0));
            frame.render_widget(help, area);
            return;
        }
        let [
            photo_area,
            shooting_area,
            image_area,
            strip_area,
            progress_area,
            help_area,
        ] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(THUMB_ROWS + 1),
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
            Paragraph::new(self.photo_line(photo_area.width)),
            photo_area,
        );
        frame.render_widget(
            Paragraph::new(self.shooting_line(shooting_area.width)),
            shooting_area,
        );
        frame.render_widget(
            Paragraph::new(self.progress_line(progress_area.width)),
            progress_area,
        );
        let help = match &self.message {
            Some(Ok(message)) => Line::from(format!(" {message}")).fg(Color::Green),
            Some(Err(message)) => Line::from(format!(" {message}")).fg(Color::Red),
            None => keys_line(
                &[
                    ("←→", "shot"),
                    ("↑↓", "burst"),
                    ("k", "keep, reject the rest"),
                    ("X", "reject burst"),
                    ("s", "sharpest"),
                    ("1-5", "rank"),
                    ("x", "reject"),
                    ("f", "filter"),
                    ("q", "quit"),
                    ("?", "help"),
                ],
                help_area.width,
            ),
        };
        frame.render_widget(Paragraph::new(help), help_area);
    }

    /// The full help, with every key and what the marks mean.
    /// The full help, with every key and what the marks mean, laid out
    /// for a terminal `width` columns wide; long lines wrap.
    fn help_text(&self, width: u16) -> Vec<Line<'static>> {
        // Below this width, keys go above their description.
        let narrow = width < 64;
        let mut lines = Vec::new();
        let section = |lines: &mut Vec<Line<'static>>, title: &'static str| {
            lines.push(Line::default());
            lines.push(Line::from(format!(" {title}")).bold().fg(Color::Yellow));
        };
        let key = |lines: &mut Vec<Line<'static>>, keys: &'static str, what: &'static str| {
            let keys = Span::from(if narrow {
                format!("  {keys}")
            } else {
                format!("  {keys:<18}")
            })
            .bold()
            .fg(Color::Cyan);
            if narrow {
                lines.push(Line::from(keys));
                lines.push(Line::from(format!("      {what}")));
            } else {
                lines.push(Line::from(vec![keys, Span::from(what)]));
            }
        };
        let text = |lines: &mut Vec<Line<'static>>, text: &'static str| {
            lines.push(Line::from(format!("    {text}")));
        };

        lines.push(Line::from(" ↑↓ scroll · any other key closes this help").italic());
        section(&mut lines, "Browsing");
        key(&mut lines, "← →  h l  space", "previous / next shot");
        key(&mut lines, "↑ ↓  [ ]", "previous / next burst");
        key(&mut lines, "Home End", "first / last shot");
        key(
            &mut lines,
            "s",
            "go to the sharpest shot of the burst without light bands (◆)",
        );
        key(
            &mut lines,
            "z  Enter",
            "zoom to 100%, then arrows to move; again or Esc to leave",
        );

        section(
            &mut lines,
            "Marking, saved at once in RawTherapee's .pp3 sidecars",
        );
        key(
            &mut lines,
            "k",
            "keep this shot (at least ★1), reject the rest of the burst, next burst",
        );
        key(&mut lines, "X", "reject the whole burst, next burst");
        key(&mut lines, "1-5  & é \" ' (", "rating; 0 or à clears it");
        key(
            &mut lines,
            "r y g b p",
            "red, yellow, green, blue, purple label; again to clear",
        );
        key(&mut lines, "x  Delete", "reject / restore this shot");

        section(&mut lines, "Filters");
        key(
            &mut lines,
            "f",
            "next filter: all → unsorted → kept → rejected → all",
        );
        text(
            &mut lines,
            "Browsing only goes through the shots of the filter; the strip still shows the whole burst.",
        );
        text(
            &mut lines,
            "unsorted: neither rated, labelled nor rejected, what is left to cull.",
        );
        text(&mut lines, "kept: rated or labelled, and not rejected.");
        text(
            &mut lines,
            "rejected: to check that nothing good went there.",
        );

        section(&mut lines, "Under the thumbnails");
        text(
            &mut lines,
            "★3 rating · R Y G B P label · ✗ rejected · ◆ sharpest · ≋ light bands",
        );

        section(&mut lines, "Done?");
        text(
            &mut lines,
            "There is nothing to save: marks are written as you set them. The line above the keys counts kept (✓), rejected (✗) and unsorted (?) shots; the unsorted filter shows what is left. q quits and prints a summary.",
        );

        section(&mut lines, "Other keys");
        key(
            &mut lines,
            "o / O",
            "open this shot / the directory in RawTherapee",
        );
        key(&mut lines, "q", "quit");

        let ms = |duration: Option<&Duration>| {
            duration.map_or("–".to_owned(), |d| format!("{} ms", d.as_millis()))
        };
        let decode = self.timings.get(&Job::Preview(self.app.index));
        let draw = self.shown.as_ref().map(|shown| &shown.elapsed);
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "  {} graphics · last preview decoded in {} · drawn in {}",
                self.protocol_name,
                ms(decode),
                ms(draw)
            ))
            .fg(Color::Gray),
        );
        lines
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
        let sharpest = self.sharpest();
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

            let (mut label, rejected) = match self.marks.get(&i) {
                Some(Ok(marks)) => (short_marks(marks), marks.in_trash),
                Some(Err(_)) => ("?".to_owned(), false),
                None => (String::new(), false),
            };
            if self.assessment(i).is_some_and(|a| a.banded) {
                label = format!("≋ {label}").trim_end().to_owned();
            }
            if Some(i) == sharpest {
                label = format!("◆ {label}").trim_end().to_owned();
            }
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

    /// What changes from one photo to the next and matters for culling:
    /// position, name, marks, place in the burst, sharpness, light bands.
    fn photo_line(&self, width: u16) -> Line<'static> {
        let index = self.app.index;
        let shot = &self.shots[index];
        let span = |text: String| vec![Span::from(text)];
        let mut parts: Vec<(u8, Vec<Span<'static>>)> = vec![
            (1, span(format!("{}/{}", index + 1, self.shots.len()))),
            (
                1,
                vec![Span::from(shot.stem.to_string_lossy().into_owned()).bold()],
            ),
        ];
        match self.marks.get(&index) {
            Some(Ok(marks)) => {
                let stars = "★".repeat(marks.rank.into())
                    + &"☆".repeat((pp3::MAX_RANK - marks.rank).into());
                parts.push((1, vec![Span::from(stars).fg(Color::Yellow)]));
                if marks.color != ColorLabel::None {
                    let color = label_color(marks.color);
                    parts.push((
                        2,
                        vec![Span::from(format!("{:?}", marks.color)).fg(color).bold()],
                    ));
                }
                if marks.in_trash {
                    parts.push((1, vec![Span::from("✗ rejected").fg(Color::Red).bold()]));
                }
            }
            Some(Err(_)) => parts.push((1, vec![Span::from("unreadable sidecar").fg(Color::Red)])),
            None => {}
        }
        let (burst, complete) = self.burst();
        parts.push((
            3,
            span(format!(
                "burst {}/{}{}",
                index - burst.start + 1,
                burst.len(),
                if complete { "" } else { "+" }
            )),
        ));
        let best = self.sharpest();
        let best_score = best
            .and_then(|best| self.assessment(best))
            .map(|a| a.sharpness);
        match (self.assessment(index), best_score) {
            _ if best == Some(index) => parts.push((2, vec![Span::from("◆ sharpest").bold()])),
            (Some(current), Some(best)) if best > 0.0 => parts.push((
                2,
                span(format!(
                    "sharpness {:.0}%",
                    100.0 * current.sharpness / best
                )),
            )),
            _ => {}
        }
        if self.assessment(index).is_some_and(|a| a.banded) {
            parts.push((
                1,
                vec![Span::from("≋ light bands").fg(Color::Magenta).bold()],
            ));
        }
        if matches!(self.app.mode, Mode::Zoom { .. }) {
            parts.push((1, vec![Span::from("100%").bold()]));
        }
        fit(&parts, usize::from(width))
    }

    /// How the photo was taken: the settings that change from shot to shot
    /// stand out, the date, files, camera and lens follow.
    fn shooting_line(&self, width: u16) -> Line<'static> {
        let index = self.app.index;
        let shot = &self.shots[index];
        let files = [&shot.jpeg, &shot.raw]
            .into_iter()
            .flatten()
            .filter_map(|path| path.extension())
            .map(|ext| ext.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("+");
        let mut parts: Vec<(u8, Vec<Span<'static>>)> = Vec::new();
        match self.exifs.get(&index) {
            Some(Ok(exif)) => {
                let settings = exif.to_string();
                if !settings.is_empty() {
                    parts.push((1, vec![Span::from(settings).bold()]));
                }
                if let Some(taken) = &exif.taken {
                    parts.push((2, vec![Span::from(taken.clone())]));
                }
                parts.push((3, vec![Span::from(files)]));
                if let Some(camera) = &exif.camera {
                    parts.push((5, vec![Span::from(camera.clone())]));
                }
                if let Some(lens) = &exif.lens {
                    parts.push((4, vec![Span::from(lens.clone())]));
                }
            }
            Some(Err(err)) => {
                parts.push((1, vec![Span::from(format!("exif: {err}")).fg(Color::Red)]))
            }
            None => parts.push((3, vec![Span::from(files)])),
        }
        fit(&parts, usize::from(width))
    }

    /// How far culling has gone, for the whole directory.
    fn progress_line(&self, width: u16) -> Line<'static> {
        let progress = self.progress();
        let mut parts: Vec<(u8, Vec<Span<'static>>)> = Vec::new();
        if self.filter != Filter::All {
            parts.push((
                1,
                vec![Span::from(format!("[{}]", self.filter.name())).bold()],
            ));
        }
        parts.push((
            1,
            vec![
                Span::from(format!("✓{}", progress.kept))
                    .fg(Color::Green)
                    .bold(),
                Span::from(" kept"),
            ],
        ));
        parts.push((
            1,
            vec![
                Span::from(format!("✗{}", progress.rejected))
                    .fg(Color::Red)
                    .bold(),
                Span::from(" rejected"),
            ],
        ));
        parts.push((
            1,
            vec![
                Span::from(format!("?{}", progress.unsorted))
                    .fg(Color::Yellow)
                    .bold(),
                Span::from(" to sort"),
            ],
        ));
        if progress.unread == 0 && progress.unsorted == 0 {
            parts.push((1, vec![Span::from("all sorted").fg(Color::Green).bold()]));
        }
        let read = self
            .times
            .iter()
            .filter(|&&time| time != Time::Unknown)
            .count();
        if read < self.shots.len() {
            parts.push((
                2,
                vec![Span::from(format!("reading {read}/{}", self.shots.len()))],
            ));
        }
        fit(&parts, usize::from(width))
    }
}

/// How many shots are kept, rejected and left to cull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Progress {
    kept: usize,
    rejected: usize,
    unsorted: usize,
    /// Shots whose marks are not read yet.
    unread: usize,
}

/// A line of keys and what they do, the keys standing out: as many as
/// fit in `width` columns, in order, the last one always shown.
fn keys_line(keys: &[(&'static str, &'static str)], width: u16) -> Line<'static> {
    let entry = |(key, what): &(&'static str, &'static str)| {
        [
            Span::from(*key).bold().fg(Color::Cyan),
            Span::from(format!(" {what}   ")),
        ]
    };
    let width = usize::from(width);
    let Some((last, others)) = keys.split_last() else {
        return Line::default();
    };
    let last = entry(last);
    let mut used = 1 + last.iter().map(Span::width).sum::<usize>();
    let mut spans = vec![Span::from(" ")];
    for key in others {
        let spans_of_key = entry(key);
        let needed: usize = spans_of_key.iter().map(Span::width).sum();
        if used + needed > width {
            break;
        }
        used += needed;
        spans.extend(spans_of_key);
    }
    spans.extend(last);
    Line::from(spans)
}

/// Joins the parts with two spaces, dropping the least important ones
/// (highest priority number, last first) until the line fits in `width`
/// columns. Parts of priority 1 always stay.
fn fit(parts: &[(u8, Vec<Span<'static>>)], width: usize) -> Line<'static> {
    let part_width = |spans: &[Span<'static>]| spans.iter().map(Span::width).sum::<usize>();
    let mut kept: Vec<&(u8, Vec<Span<'static>>)> = parts
        .iter()
        .filter(|(_, spans)| part_width(spans) > 0)
        .collect();
    let line_width = |kept: &[&(u8, Vec<Span<'static>>)]| {
        1 + kept
            .iter()
            .map(|(_, spans)| part_width(spans))
            .sum::<usize>()
            + 2 * kept.len().saturating_sub(1)
    };
    while line_width(&kept) > width {
        let Some(least) = kept
            .iter()
            .enumerate()
            .max_by_key(|&(position, (priority, _))| (*priority, position))
            .map(|(position, _)| position)
        else {
            break;
        };
        if kept[least].0 == 1 {
            break; // Even the essentials do not fit: let the terminal cut them.
        }
        kept.remove(least);
    }
    let mut spans = vec![Span::from(" ")];
    for (position, (_, part)) in kept.iter().enumerate() {
        if position > 0 {
            spans.push(Span::from("  "));
        }
        spans.extend(part.iter().cloned());
    }
    Line::from(spans)
}

/// The terminal color of a label.
fn label_color(label: ColorLabel) -> Color {
    match label {
        ColorLabel::None => Color::Reset,
        ColorLabel::Red => Color::Red,
        ColorLabel::Yellow => Color::Yellow,
        ColorLabel::Green => Color::Green,
        ColorLabel::Blue => Color::Blue,
        ColorLabel::Purple => Color::Magenta,
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
    let mut viewer = Viewer::new(dir, shots, picker, config);

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
    println!("{}", viewer.summary());
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
    fn the_least_important_parts_go_first() {
        let part = |priority, text: &str| (priority, vec![Span::from(text.to_owned())]);
        let parts = [
            part(1, "12/653"),
            part(3, "burst 2/6"),
            part(2, "P1011259"),
            part(1, ""),
            part(1, "│ ✓1 ✗2 ?3"),
        ];
        let text = |width| fit(&parts, width).to_string();
        assert_eq!(text(80), " 12/653  burst 2/6  P1011259  │ ✓1 ✗2 ?3");
        assert_eq!(text(32), " 12/653  P1011259  │ ✓1 ✗2 ?3");
        assert_eq!(text(20), " 12/653  │ ✓1 ✗2 ?3");
        // The essentials stay even when they do not fit.
        assert_eq!(text(5), " 12/653  │ ✓1 ✗2 ?3");
    }

    #[test]
    fn keys_that_do_not_fit_are_left_out_but_help_stays() {
        let keys = [("←→", "shot"), ("k", "keep"), ("?", "help")];
        let text = |width| keys_line(&keys, width).to_string();
        assert_eq!(text(80).trim_end(), " ←→ shot   k keep   ? help");
        assert!(text(20).contains("? help"));
        assert!(!text(20).contains("keep"));
        assert!(text(5).contains("? help"));
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
