//! State of the viewer and what the keys do to it, without any terminal
//! or decoding, so that it can be tested on its own.

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
    ToggleZoom,
    /// Moves the zoomed view by a fraction of the screen, in eighths.
    Pan {
        dx: i32,
        dy: i32,
    },
    Quit,
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
