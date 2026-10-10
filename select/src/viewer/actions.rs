//! What the keys do: browsing, marks, bursts, filters, RawTherapee.

use corrode_core::marks::{ColorLabel, Marks};
use corrode_core::pairing::{self, Shot};
use corrode_core::selection::Batch;
use corrode_core::similarity::{self, Settings};
use corrode_core::{rawtherapee, selection};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Viewer;
use crate::app::{Command, Filter, MarkChange, Mode, burst_around, next_matching};
use crate::culling;
use crate::text::rank_key;
use corrode_core::bursts;

/// The selection folder's shots and how `G` proposes to sort them.
pub(super) struct PendingGroups {
    pub shots: Vec<Shot>,
    pub batches: Vec<Batch>,
}

impl Viewer {
    pub fn key(&mut self, key: KeyEvent) {
        self.message = None;
        if self.help {
            self.help_key(key.code);
            return;
        }
        if self.confirm_send {
            self.confirm_send = false;
            if key.code == KeyCode::Char('m') {
                self.send_kept();
            } else {
                self.message = Some(Err("nothing sent".to_owned()));
            }
            return;
        }
        if let Some(pending) = self.pending_groups.take() {
            if key.code == KeyCode::Char('G') {
                self.sort_groups(&pending);
            } else {
                self.message = Some(Err("nothing sorted".to_owned()));
            }
            return;
        }
        // Zoomed, the arrows move around the picture; with Ctrl they
        // still change shot.
        let pan = matches!(self.app.mode, Mode::Zoom { .. })
            && !key.modifiers.contains(KeyModifiers::CONTROL);
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
            KeyCode::Char('m') => return self.ask_to_send(),
            KeyCode::Char('G') => return self.ask_to_group(),
            KeyCode::Char('d') => return self.deband(false),
            KeyCode::Char('D') => return self.deband(true),
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
            KeyCode::Char('+' | '=') => Command::ZoomIn,
            KeyCode::Char('-') => Command::ZoomOut,
            KeyCode::Left if pan => Command::Pan { dx: -1, dy: 0 },
            KeyCode::Right if pan => Command::Pan { dx: 1, dy: 0 },
            KeyCode::Up if pan => Command::Pan { dx: 0, dy: -1 },
            KeyCode::Down if pan => Command::Pan { dx: 0, dy: 1 },
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
        let matches = |i| self.matches(i);
        let filtered = culling::filtered(
            command,
            self.filter,
            self.app.index,
            self.shots.len(),
            matches,
        );
        let Some(command) = filtered else {
            self.message = Some(Err(format!("no other {} shot", self.filter.name())));
            return;
        };
        let full_size = self
            .full_picture()
            .map(|p| (p.image.width(), p.image.height()));
        self.app.apply(command, full_size, self.view);
    }

    /// In the help: arrows scroll it, any other key closes it, q still quits.
    fn help_key(&mut self, code: KeyCode) {
        match code {
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
    }

    fn matches(&self, index: usize) -> bool {
        self.filter.matches(self.states[index].marks())
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
        let read = self
            .states
            .iter()
            .filter(|state| state.marks.is_some())
            .count();
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

    /// The kept shots not sent yet, if every mark is read.
    fn to_send(&self) -> Result<Vec<usize>, String> {
        let mut kept = Vec::new();
        for (index, state) in self.states.iter().enumerate() {
            if selection::is_sent(&self.shots[index], &self.dir) {
                continue;
            }
            match &state.marks {
                None => return Err("the marks are still being read, try again in a moment".into()),
                Some(Err(err)) => {
                    return Err(format!(
                        "{}: {err}",
                        self.shots[index].stem.to_string_lossy()
                    ));
                }
                Some(Ok(marks)) => {
                    if Filter::Kept.matches(Some(marks)) {
                        kept.push(index);
                    }
                }
            }
        }
        Ok(kept)
    }

    /// Asks before sending the kept shots to the selection folder.
    fn ask_to_send(&mut self) {
        self.message = Some(match self.to_send() {
            Err(err) => Err(err),
            Ok(kept) if kept.is_empty() => Err("no kept shot left to send".to_owned()),
            Ok(kept) => {
                self.confirm_send = true;
                Ok(format!(
                    "move {} kept shots (JPEG, RAW, sidecars) to {}/? m again to confirm, any other key to keep them here",
                    kept.len(),
                    selection::FOLDER
                ))
            }
        });
    }

    /// Moves the kept shots, whole, to the selection folder; they stay
    /// in the list, at their new place.
    fn send_kept(&mut self) {
        let kept = match self.to_send() {
            Ok(kept) => kept,
            Err(err) => {
                self.message = Some(Err(err));
                return;
            }
        };
        let folder = selection::folder(&self.dir);
        let (mut sent, mut failed) = (0, None);
        for index in kept {
            match selection::send(&self.shots[index], &folder) {
                Ok(shot) => {
                    // Moved, not changed: what was loaded still holds.
                    self.replace_shot(index, shot, false);
                    sent += 1;
                }
                Err(err) => {
                    failed.get_or_insert(format!(
                        "{}: {err}",
                        self.shots[index].stem.to_string_lossy()
                    ));
                }
            }
        }
        self.message = Some(match failed {
            None => Ok(format!("{sent} shots moved to {}/", folder.display())),
            Some(err) => Err(format!("{sent} shots moved, then {err}")),
        });
    }

    /// Proposes to sort the shots of the selection folder into subfolders
    /// of shots that look alike, whenever they were sent.
    fn ask_to_group(&mut self) {
        let folder = selection::folder(&self.dir);
        let shots = match pairing::scan_dir(&folder) {
            Ok(shots) => shots,
            Err(err) => {
                self.message = Some(Err(format!("{}: {err}", folder.display())));
                return;
            }
        };
        let batches: Vec<Batch> = similarity::group_shots(&shots, &Settings::default())
            .into_iter()
            .filter(|group| group.members.len() >= 2)
            .map(|group| Batch {
                label: group.colour.to_owned(),
                members: group.members,
            })
            .collect();
        if batches.is_empty() {
            self.message = Some(Err(format!(
                "no group of two shots or more among the {} of {}/, send the kept shots first with m",
                shots.len(),
                selection::FOLDER
            )));
            return;
        }
        let grouped: usize = batches.iter().map(|batch| batch.members.len()).sum();
        self.message = Some(Ok(format!(
            "move {grouped} of the {} shots of {}/ into {} folders of shots that look alike? G again to confirm, any other key to keep them there",
            shots.len(),
            selection::FOLDER,
            batches.len()
        )));
        self.pending_groups = Some(PendingGroups { shots, batches });
    }

    /// Does the sorting proposed, and gives the moved shots their new
    /// place in the list.
    fn sort_groups(&mut self, pending: &PendingGroups) {
        let folder = selection::folder(&self.dir);
        let sorting = selection::sort_into_groups(&folder, &pending.shots, &pending.batches);
        let mut moved = 0;
        for sorted in &sorting.sorted {
            for shot in &sorted.shots {
                moved += 1;
                // Shots sent in an earlier session are not in the list.
                if let Some(position) = self.shots.iter().position(|s| s.stem == shot.stem) {
                    self.replace_shot(position, shot.clone(), false);
                }
            }
        }
        self.message = Some(match sorting.error {
            None => Ok(format!(
                "{moved} shots sorted into {} folders of {}/",
                sorting.sorted.len(),
                selection::FOLDER
            )),
            Some(err) => Err(format!("{moved} shots sorted, then {err}")),
        });
    }

    /// Starts removing the light bands of the current shot into a DNG, in
    /// the background. Unless forced, only shots found banded are treated.
    fn deband(&mut self, force: bool) {
        let index = self.app.index;
        let shot = &self.shots[index];
        let stem = shot.stem.to_string_lossy();
        if shot.raw.is_none() {
            self.message = Some(Err(format!("{stem}: no RAW file to correct")));
            return;
        }
        let banded = self.states[index].assessment().map(|a| a.banded);
        self.message = Some(match (banded, force) {
            (Some(false), false) => Err(format!(
                "{stem}: no light bands found on its preview; D corrects it anyway"
            )),
            (None, false) => Err(format!("{stem}: not assessed yet, try again in a moment")),
            _ => {
                self.loader.push(crate::jobs::Job::Deband(self.id(index)));
                Ok(format!("{stem}: removing light bands in the background…"))
            }
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
            self.states[index].marks = Some(Ok(marks));
        }
        Ok(())
    }

    /// The marks of a shot, read now if not known yet.
    fn current_marks(&self, index: usize) -> Result<Marks, String> {
        match &self.states[index].marks {
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
        self.message = Some(result.map(|marks| format!("{marks} saved")));
    }

    /// Keeps the current shot of its burst and rejects the others, then
    /// moves to the next burst.
    fn keep_in_burst(&mut self) {
        let index = self.app.index;
        let stem = self.shots[index].stem.to_string_lossy().into_owned();
        self.change_burst(
            |burst, marks| culling::keep(burst, index, marks),
            |count| {
                format!(
                    "kept {stem}, rejected the {} other shots of the burst",
                    count - 1
                )
            },
        );
    }

    /// Rejects every shot of the current burst, then moves to the next one.
    fn reject_burst(&mut self) {
        self.change_burst(
            |burst, marks| culling::reject(burst, marks),
            |count| format!("rejected the {count} shots of the burst"),
        );
    }

    /// Writes the marks `changes` gives for the current burst, once it is
    /// read entirely, then moves to the next burst.
    fn change_burst(
        &mut self,
        changes: impl FnOnce(
            std::ops::Range<usize>,
            &dyn Fn(usize) -> Result<Marks, String>,
        ) -> Result<Vec<(usize, Marks)>, String>,
        done: impl FnOnce(usize) -> String,
    ) {
        let (burst, complete) = self.burst();
        if !complete {
            self.message = Some(Err(
                "this burst is still being read, try again in a moment".to_owned()
            ));
            return;
        }
        let changed = changes(burst.clone(), &|i| self.current_marks(i));
        let result = changed.and_then(|changes| self.write_marks(&changes));
        self.message = Some(result.map(|()| done(burst.len())));
        if burst.end < self.shots.len() {
            self.app.apply(Command::GoTo(burst.end), None, self.view);
        }
    }

    /// What culling left, printed when quitting. Marks not read yet are
    /// read now, so that the counts cover the whole directory.
    pub fn summary(&mut self) -> String {
        for (state, shot) in self.states.iter_mut().zip(self.shots.iter()) {
            if state.marks.is_none() {
                state.marks = Some(rawtherapee::read_marks(shot).map_err(|err| err.to_string()));
            }
        }
        let progress = self.progress();
        let mut summary = format!(
            "{}: {} shots, {} kept, {} rejected, {} unsorted.",
            self.dir.display(),
            self.shots.len(),
            progress.kept,
            progress.rejected,
            progress.unsorted,
        );
        if progress.unreadable > 0 {
            summary.push_str(&format!(
                " {} with an unreadable sidecar.",
                progress.unreadable
            ));
        }
        summary.push_str(" Marks are saved in RawTherapee's .pp3 sidecars.");
        summary
    }
}
