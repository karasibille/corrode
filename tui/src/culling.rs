//! The culling rules, without any terminal: what keeping a shot or
//! rejecting a burst changes, which frame of a burst to suggest, how far
//! culling has gone, and where browsing goes with a filter.

use std::ops::Range;

use corrode_core::pp3::Marks;

use crate::app::{Command, Filter, next_matching};

/// What the preview of a shot tells about its quality.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    pub sharpness: f32,
    /// Whether LED lighting left light bands on it.
    pub banded: bool,
}

/// The marks of a burst once `kept` is kept, with at least one star and
/// no rejection, and every other shot rejected. Stops at the first shot
/// whose marks cannot be read, so that nothing is half done.
pub fn keep(
    burst: Range<usize>,
    kept: usize,
    marks: impl Fn(usize) -> Result<Marks, String>,
) -> Result<Vec<(usize, Marks)>, String> {
    burst
        .map(|i| {
            let marks = marks(i)?;
            let changed = if i == kept {
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
            };
            Ok((i, changed))
        })
        .collect()
}

/// The marks of a burst once every shot of it is rejected; ratings and
/// labels stay, so that a rejection can be undone.
pub fn reject(
    burst: Range<usize>,
    marks: impl Fn(usize) -> Result<Marks, String>,
) -> Result<Vec<(usize, Marks)>, String> {
    burst
        .map(|i| {
            let marks = marks(i)?;
            Ok((
                i,
                Marks {
                    in_trash: true,
                    ..marks
                },
            ))
        })
        .collect()
}

/// The sharpest shot of a burst among those measured, if more than one
/// is; shots with light bands only when all of them have some.
pub fn sharpest(
    burst: Range<usize>,
    assessment: impl Fn(usize) -> Option<Assessment>,
) -> Option<usize> {
    let measured: Vec<(usize, Assessment)> =
        burst.filter_map(|i| Some((i, assessment(i)?))).collect();
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

/// How many shots are kept, rejected and left to cull.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub kept: usize,
    pub rejected: usize,
    pub unsorted: usize,
    /// Shots whose marks are not read yet.
    pub unread: usize,
    /// Shots whose sidecar cannot be read.
    pub unreadable: usize,
}

impl Progress {
    /// Counts shots from what is known of their marks: `None` when not
    /// read yet, `Some(None)` when their sidecar cannot be read.
    pub fn count<'a>(marks: impl IntoIterator<Item = Option<Option<&'a Marks>>>) -> Progress {
        let mut progress = Progress::default();
        for marks in marks {
            match marks {
                None => progress.unread += 1,
                Some(None) => progress.unreadable += 1,
                Some(Some(marks)) => {
                    let marks = Some(marks);
                    if Filter::Kept.matches(marks) {
                        progress.kept += 1;
                    } else if Filter::Rejected.matches(marks) {
                        progress.rejected += 1;
                    } else {
                        progress.unsorted += 1;
                    }
                }
            }
        }
        progress
    }

    /// Whether every shot is read and sorted.
    pub fn done(&self) -> bool {
        self.unread == 0 && self.unsorted == 0
    }
}

/// The move a command makes with a filter: to the next or previous shot
/// the filter shows, instead of any shot. `None` if there is no such shot
/// in that direction; other commands are unchanged.
pub fn filtered(
    command: Command,
    filter: Filter,
    index: usize,
    count: usize,
    matches: impl Fn(usize) -> bool,
) -> Option<Command> {
    if filter == Filter::All {
        return Some(command);
    }
    let target = match command {
        Command::Next => next_matching(index, true, count, &matches),
        Command::Previous => next_matching(index, false, count, &matches),
        Command::First => (0..count).find(|&i| matches(i)),
        Command::Last => (0..count).rev().find(|&i| matches(i)),
        // A burst jump lands on the first shot of the filter from there.
        Command::GoTo(target) if target > index => (target..count).find(|&i| matches(i)),
        Command::GoTo(target) if target < index => (target..index).find(|&i| matches(i)),
        other => return Some(other),
    };
    target.map(Command::GoTo)
}

#[cfg(test)]
mod tests {
    use corrode_core::pp3::ColorLabel;

    use super::*;

    fn marks(rank: u8, color: ColorLabel, in_trash: bool) -> Marks {
        Marks {
            rank,
            color,
            in_trash,
        }
    }

    #[test]
    fn keeping_a_shot_rejects_the_rest_of_its_burst() {
        let known = [
            marks(0, ColorLabel::None, false),
            marks(3, ColorLabel::Green, true),
            marks(2, ColorLabel::Red, false),
        ];
        let changes = keep(0..3, 1, |i| Ok(known[i])).unwrap();
        assert_eq!(
            changes,
            [
                (0, marks(0, ColorLabel::None, true)),
                // Kept: still ★3 and green, no longer rejected.
                (1, marks(3, ColorLabel::Green, false)),
                (2, marks(2, ColorLabel::Red, true)),
            ]
        );
        // An unrated kept shot gets one star.
        let changes = keep(0..1, 0, |i| Ok(known[i])).unwrap();
        assert_eq!(changes, [(0, marks(1, ColorLabel::None, false))]);
    }

    #[test]
    fn rejecting_a_burst_keeps_ratings_and_labels() {
        let changes = reject(4..6, |_| Ok(marks(2, ColorLabel::Blue, false))).unwrap();
        assert_eq!(
            changes,
            [
                (4, marks(2, ColorLabel::Blue, true)),
                (5, marks(2, ColorLabel::Blue, true)),
            ]
        );
    }

    #[test]
    fn nothing_changes_when_a_sidecar_cannot_be_read() {
        let marks = |i| {
            if i == 1 {
                Err("broken".to_owned())
            } else {
                Ok(Marks::default())
            }
        };
        assert_eq!(keep(0..3, 0, marks), Err("broken".to_owned()));
        assert_eq!(reject(0..3, marks), Err("broken".to_owned()));
    }

    #[test]
    fn the_sharpest_frame_avoids_light_bands() {
        let a = |sharpness, banded| Some(Assessment { sharpness, banded });
        let burst = [a(20.0, false), a(35.0, true), a(25.0, false), None];
        assert_eq!(sharpest(0..4, |i| burst[i]), Some(2));
        // All banded: the sharpest anyway.
        let banded = [a(20.0, true), a(35.0, true)];
        assert_eq!(sharpest(0..2, |i| banded[i]), Some(1));
        // A single measured frame suggests nothing.
        assert_eq!(sharpest(0..2, |i| [a(20.0, false), None][i]), None);
    }

    #[test]
    fn progress_counts_each_kind_of_shot() {
        let kept = marks(1, ColorLabel::None, false);
        let labelled = marks(0, ColorLabel::Green, false);
        let rejected = marks(4, ColorLabel::None, true);
        let unsorted = Marks::default();
        let progress = Progress::count([
            Some(Some(&kept)),
            Some(Some(&labelled)),
            Some(Some(&rejected)),
            Some(Some(&unsorted)),
            Some(None),
            None,
        ]);
        assert_eq!(
            progress,
            Progress {
                kept: 2,
                rejected: 1,
                unsorted: 1,
                unread: 1,
                unreadable: 1,
            }
        );
        assert!(!progress.done());
        assert!(Progress::count([Some(Some(&kept)), Some(Some(&rejected))]).done());
    }

    #[test]
    fn filters_change_where_browsing_goes() {
        let kept = |i: usize| [false, true, false, true, false][i];
        let go = |command, index| filtered(command, Filter::Kept, index, 5, kept);
        assert_eq!(go(Command::Next, 1), Some(Command::GoTo(3)));
        assert_eq!(go(Command::Next, 3), None);
        assert_eq!(go(Command::Previous, 3), Some(Command::GoTo(1)));
        assert_eq!(go(Command::First, 4), Some(Command::GoTo(1)));
        assert_eq!(go(Command::Last, 0), Some(Command::GoTo(3)));
        // A burst jump forward to shot 2 lands on the next kept shot.
        assert_eq!(go(Command::GoTo(2), 0), Some(Command::GoTo(3)));
        assert_eq!(go(Command::ToggleZoom, 0), Some(Command::ToggleZoom));
        // Without filter, commands are unchanged.
        assert_eq!(
            filtered(Command::Next, Filter::All, 0, 5, kept),
            Some(Command::Next)
        );
    }
}
