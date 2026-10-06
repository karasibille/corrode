//! State of the viewer and what the keys do to it, without any terminal
//! or decoding, so that it can be tested on its own.

use std::ops::Range;

use corrode_core::pp3::{ColorLabel, Marks};

/// What is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The whole picture, scaled to the screen.
    Fit,
    /// The full-size picture at 100%, one image pixel per screen pixel,
    /// centred on this point of the image.
    Zoom { x: u32, y: u32 },
}

/// A user command, decoded from a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Next,
    Previous,
    First,
    Last,
    GoTo(usize),
    ToggleZoom,
    /// Moves the zoomed view by a fraction of the screen, in eighths.
    Pan {
        dx: i32,
        dy: i32,
    },
    /// Changes the marks of the current shot; written by the viewer.
    Mark(MarkChange),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkChange {
    /// Sets the star rating, 0 clearing it.
    Rank(u8),
    /// Sets this color label, or clears it if the shot already has it.
    ToggleColor(ColorLabel),
    ToggleTrash,
}

impl MarkChange {
    pub fn apply(self, marks: Marks) -> Marks {
        match self {
            MarkChange::Rank(rank) => Marks { rank, ..marks },
            MarkChange::ToggleColor(color) => Marks {
                color: if marks.color == color {
                    ColorLabel::None
                } else {
                    color
                },
                ..marks
            },
            MarkChange::ToggleTrash => Marks {
                in_trash: !marks.in_trash,
                ..marks
            },
        }
    }
}

#[derive(Debug)]
pub struct App {
    pub index: usize,
    pub count: usize,
    pub mode: Mode,
    pub quit: bool,
}

impl App {
    pub fn new(count: usize) -> App {
        App {
            index: 0,
            count,
            mode: Mode::Fit,
            quit: false,
        }
    }

    /// Applies a command. `full_size` is the size of the full picture of
    /// the current shot when it is loaded, `view` the size of the screen
    /// area in pixels; both are needed to zoom and pan.
    pub fn apply(&mut self, command: Command, full_size: Option<(u32, u32)>, view: (u32, u32)) {
        match command {
            Command::Next => self.go_to(self.index.saturating_add(1)),
            Command::Previous => self.go_to(self.index.saturating_sub(1)),
            Command::First => self.go_to(0),
            Command::Last => self.go_to(self.count.saturating_sub(1)),
            Command::GoTo(index) => self.go_to(index),
            Command::ToggleZoom => {
                self.mode = match (self.mode, full_size) {
                    (Mode::Fit, Some((width, height))) => Mode::Zoom {
                        x: width / 2,
                        y: height / 2,
                    },
                    // Not loaded yet: stay in Fit, the key can be pressed again.
                    (Mode::Fit, None) => Mode::Fit,
                    (Mode::Zoom { .. }, _) => Mode::Fit,
                };
            }
            Command::Pan { dx, dy } => {
                if let (Mode::Zoom { x, y }, Some(size)) = (self.mode, full_size) {
                    let step = |position: u32, delta: i32, view: u32| {
                        let moved = i64::from(position) + i64::from(delta) * i64::from(view) / 8;
                        moved.max(0) as u32
                    };
                    let (x, y) =
                        clamp_center((step(x, dx, view.0), step(y, dy, view.1)), size, view);
                    self.mode = Mode::Zoom { x, y };
                }
            }
            Command::Mark(_) => {}
            Command::Quit => self.quit = true,
        }
    }

    /// Moves to another shot, leaving the zoom: the next photo may not be
    /// framed like this one.
    fn go_to(&mut self, index: usize) {
        let index = index.min(self.count.saturating_sub(1));
        if index != self.index {
            self.index = index;
            self.mode = Mode::Fit;
        }
    }
}

/// Which shots browsing goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    /// Neither rated, labelled nor rejected.
    Unsorted,
    /// Rated or labelled, and not rejected.
    Kept,
    Rejected,
}

impl Filter {
    /// The next filter, in the order the filter key goes through them.
    pub fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Unsorted,
            Filter::Unsorted => Filter::Kept,
            Filter::Kept => Filter::Rejected,
            Filter::Rejected => Filter::All,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Unsorted => "unsorted",
            Filter::Kept => "kept",
            Filter::Rejected => "rejected",
        }
    }

    /// Whether a shot with these marks is shown; marks not read yet only
    /// match `All`.
    pub fn matches(self, marks: Option<&Marks>) -> bool {
        let Some(marks) = marks else {
            return self == Filter::All;
        };
        let sorted = marks.rank > 0 || marks.color != ColorLabel::None;
        match self {
            Filter::All => true,
            Filter::Unsorted => !marks.in_trash && !sorted,
            Filter::Kept => !marks.in_trash && sorted,
            Filter::Rejected => marks.in_trash,
        }
    }
}

/// The first shot after `from` (or before it, going backward) that
/// matches, in `0..count`.
pub fn next_matching(
    from: usize,
    forward: bool,
    count: usize,
    matches: impl Fn(usize) -> bool,
) -> Option<usize> {
    if forward {
        (from.saturating_add(1)..count).find(|&i| matches(i))
    } else {
        (0..from.min(count)).rev().find(|&i| matches(i))
    }
}

/// What is known of the time a shot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Time {
    /// Not read yet.
    Unknown,
    /// Read, but the file has no time.
    Missing,
    /// Milliseconds, as `Exif::taken_ms`.
    At(i64),
}

/// The burst holding `index`, from the times read so far, and whether
/// both of its ends are known: an end next to a shot whose time is not
/// read yet may still grow.
pub fn burst_around(times: &[Time], index: usize, max_gap_ms: i64) -> (Range<usize>, bool) {
    // Whether shots `i` and `i + 1` belong to the same burst, if known.
    let linked = |i: usize| match (times[i], times[i + 1]) {
        (Time::Unknown, _) | (_, Time::Unknown) => None,
        (Time::At(a), Time::At(b)) => Some((0..=max_gap_ms).contains(&(b - a))),
        _ => Some(false),
    };
    let (mut start, mut end, mut complete) = (index, index + 1, true);
    while start > 0 {
        match linked(start - 1) {
            Some(true) => start -= 1,
            Some(false) => break,
            None => {
                complete = false;
                break;
            }
        }
    }
    while end < times.len() {
        match linked(end - 1) {
            Some(true) => end += 1,
            Some(false) => break,
            None => {
                complete = false;
                break;
            }
        }
    }
    (start..end, complete)
}

/// Keeps a zoom centre where the view stays inside the image.
fn clamp_center(center: (u32, u32), image: (u32, u32), view: (u32, u32)) -> (u32, u32) {
    let clamp = |center: u32, image: u32, view: u32| {
        if view >= image {
            image / 2
        } else {
            center.clamp(view / 2, image - view.div_ceil(2))
        }
    };
    (
        clamp(center.0, image.0, view.0),
        clamp(center.1, image.1, view.1),
    )
}

/// The part of the image shown at 100%: `(x, y, width, height)`, at most
/// the size of the view and always inside the image.
pub fn zoom_crop(center: (u32, u32), image: (u32, u32), view: (u32, u32)) -> (u32, u32, u32, u32) {
    let (cx, cy) = clamp_center(center, image, view);
    let width = view.0.min(image.0);
    let height = view.1.min(image.1);
    let x = cx.saturating_sub(width / 2).min(image.0 - width);
    let y = cy.saturating_sub(height / 2).min(image.1 - height);
    (x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: (u32, u32) = (5184, 3888);
    const VIEW: (u32, u32) = (1920, 1080);

    #[test]
    fn navigation_stays_within_the_shots() {
        let mut app = App::new(3);
        app.apply(Command::Previous, None, VIEW);
        assert_eq!(app.index, 0);
        app.apply(Command::Next, None, VIEW);
        app.apply(Command::Next, None, VIEW);
        app.apply(Command::Next, None, VIEW);
        assert_eq!(app.index, 2);
        app.apply(Command::First, None, VIEW);
        assert_eq!(app.index, 0);
        app.apply(Command::Last, None, VIEW);
        assert_eq!(app.index, 2);

        let mut empty = App::new(0);
        empty.apply(Command::Next, None, VIEW);
        empty.apply(Command::Last, None, VIEW);
        assert_eq!(empty.index, 0);
    }

    #[test]
    fn zoom_needs_the_full_picture_and_starts_at_its_centre() {
        let mut app = App::new(2);
        app.apply(Command::ToggleZoom, None, VIEW);
        assert_eq!(app.mode, Mode::Fit);
        app.apply(Command::ToggleZoom, Some(IMAGE), VIEW);
        assert_eq!(app.mode, Mode::Zoom { x: 2592, y: 1944 });
        app.apply(Command::ToggleZoom, Some(IMAGE), VIEW);
        assert_eq!(app.mode, Mode::Fit);
    }

    #[test]
    fn bursts_are_found_around_a_shot() {
        use Time::{At, Missing, Unknown};
        let times = [
            At(0),
            At(180),
            At(360),
            At(5000),
            At(5180),
            Missing,
            At(9000),
        ];
        assert_eq!(burst_around(&times, 1, 300), (0..3, true));
        assert_eq!(burst_around(&times, 0, 300), (0..3, true));
        assert_eq!(burst_around(&times, 4, 300), (3..5, true));
        assert_eq!(burst_around(&times, 5, 300), (5..6, true));
        assert_eq!(burst_around(&times, 6, 300), (6..7, true));

        let partly_read = [At(0), At(180), Unknown, At(5000)];
        assert_eq!(burst_around(&partly_read, 0, 300), (0..2, false));
        assert_eq!(burst_around(&partly_read, 3, 300), (3..4, false));
        assert_eq!(burst_around(&[Unknown], 0, 300), (0..1, true));
    }

    #[test]
    fn filters_sort_shots_by_their_marks() {
        let marks = |rank, color, in_trash| Marks {
            rank,
            color,
            in_trash,
        };
        let unsorted = marks(0, ColorLabel::None, false);
        let rated = marks(2, ColorLabel::None, false);
        let labelled = marks(0, ColorLabel::Green, false);
        let rejected = marks(3, ColorLabel::Red, true);
        let cases = [
            (Filter::All, [true, true, true, true, true]),
            (Filter::Unsorted, [true, false, false, false, false]),
            (Filter::Kept, [false, true, true, false, false]),
            (Filter::Rejected, [false, false, false, true, false]),
        ];
        for (filter, expected) in cases {
            let found = [
                filter.matches(Some(&unsorted)),
                filter.matches(Some(&rated)),
                filter.matches(Some(&labelled)),
                filter.matches(Some(&rejected)),
                filter.matches(None),
            ];
            assert_eq!(found, expected, "{filter:?}");
        }
        let mut filter = Filter::All;
        for _ in 0..4 {
            filter = filter.next();
        }
        assert_eq!(filter, Filter::All);
    }

    #[test]
    fn next_matching_skips_other_shots() {
        let even = |i: usize| i.is_multiple_of(2);
        assert_eq!(next_matching(0, true, 10, even), Some(2));
        assert_eq!(next_matching(3, false, 10, even), Some(2));
        assert_eq!(next_matching(8, true, 10, even), None);
        assert_eq!(next_matching(0, false, 10, even), None);
        assert_eq!(next_matching(20, false, 10, even), Some(8));
    }

    #[test]
    fn mark_changes_set_or_toggle() {
        let marks = Marks {
            rank: 2,
            color: ColorLabel::Red,
            in_trash: false,
        };
        assert_eq!(MarkChange::Rank(5).apply(marks).rank, 5);
        assert_eq!(MarkChange::Rank(0).apply(marks).rank, 0);
        let blue = MarkChange::ToggleColor(ColorLabel::Blue).apply(marks);
        assert_eq!((blue.rank, blue.color), (2, ColorLabel::Blue));
        let red = MarkChange::ToggleColor(ColorLabel::Red);
        assert_eq!(red.apply(marks).color, ColorLabel::None);
        let rejected = MarkChange::ToggleTrash.apply(marks);
        assert!(rejected.in_trash);
        assert!(!MarkChange::ToggleTrash.apply(rejected).in_trash);
    }

    #[test]
    fn changing_shot_leaves_the_zoom() {
        let mut app = App::new(2);
        app.apply(Command::ToggleZoom, Some(IMAGE), VIEW);
        app.apply(Command::Next, None, VIEW);
        assert_eq!((app.index, app.mode), (1, Mode::Fit));
    }

    #[test]
    fn panning_moves_by_eighths_of_the_view_and_stops_at_the_edges() {
        let mut app = App::new(1);
        app.apply(Command::ToggleZoom, Some(IMAGE), VIEW);
        app.apply(Command::Pan { dx: 1, dy: -1 }, Some(IMAGE), VIEW);
        assert_eq!(
            app.mode,
            Mode::Zoom {
                x: 2592 + 240,
                y: 1944 - 135
            }
        );

        for _ in 0..100 {
            app.apply(Command::Pan { dx: -1, dy: -1 }, Some(IMAGE), VIEW);
        }
        assert_eq!(app.mode, Mode::Zoom { x: 960, y: 540 });
        assert_eq!(zoom_crop((960, 540), IMAGE, VIEW), (0, 0, 1920, 1080));

        for _ in 0..100 {
            app.apply(Command::Pan { dx: 1, dy: 1 }, Some(IMAGE), VIEW);
        }
        let Mode::Zoom { x, y } = app.mode else {
            panic!("left the zoom")
        };
        assert_eq!(
            zoom_crop((x, y), IMAGE, VIEW),
            (5184 - 1920, 3888 - 1080, 1920, 1080)
        );
    }

    #[test]
    fn zoom_crop_stays_inside_the_image() {
        assert_eq!(
            zoom_crop((2592, 1944), IMAGE, VIEW),
            (1632, 1404, 1920, 1080)
        );
        assert_eq!(
            zoom_crop((0, 99999), IMAGE, VIEW),
            (0, 3888 - 1080, 1920, 1080)
        );
        // A view larger than the image shows all of it.
        assert_eq!(zoom_crop((10, 10), (800, 600), VIEW), (0, 0, 800, 600));
        // Odd sizes.
        assert_eq!(
            zoom_crop((5183, 3887), IMAGE, (1921, 1081)),
            (5184 - 1921, 3888 - 1081, 1921, 1081)
        );
    }
}
