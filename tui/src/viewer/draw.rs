//! Drawing the screen: the shot's lines on top, the picture, the burst
//! strip, the progress and the keys.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui_image::{Image, Resize};

use super::{THUMB_ROWS, Viewer};
use crate::app::{Mode, scaled_view, zoom_crop};
use crate::encoder::Request;
use crate::text::{centered, keys_line, short_marks};

impl Viewer {
    pub fn draw(&mut self, frame: &mut Frame) {
        if self.help {
            self.draw_help(frame);
            return;
        }
        let [
            photo_area,
            shooting_area,
            image_area,
            strip_area,
            progress_area,
            keys_area,
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
        let keys = match &self.message {
            Some(Ok(message)) => Line::from(format!(" {message}")).fg(Color::Green),
            Some(Err(message)) => Line::from(format!(" {message}")).fg(Color::Red),
            None => keys_line(
                &[
                    ("←→", "shot"),
                    ("↑↓", "burst"),
                    ("k", "keep, reject the rest"),
                    ("X", "reject burst"),
                    ("d", "deband"),
                    ("s", "sharpest"),
                    ("1-5", "rank"),
                    ("x", "reject"),
                    ("f", "filter"),
                    ("q", "quit"),
                    ("?", "help"),
                ],
                keys_area.width,
            ),
        };
        frame.render_widget(Paragraph::new(keys), keys_area);
    }

    /// The full help instead of the shots, wrapped and scrolled.
    fn draw_help(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let lines = self.help_text(area.width);
        // Keep the end of the help on screen when scrolling down.
        let last = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        self.help_scroll = self.help_scroll.min(last.saturating_sub(area.height / 2));
        let help = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.help_scroll, 0));
        frame.render_widget(help, area);
    }

    fn draw_picture(&mut self, frame: &mut Frame, area: Rect) {
        let index = self.app.index;
        let wanted = match self.app.mode {
            Mode::Fit => self
                .previews
                .get(&self.id(index))
                .map(|p| (p.clone(), None)),
            Mode::Zoom { x, y, scale } => self.full_picture().map(|picture| {
                let size = (picture.image.width(), picture.image.height());
                (
                    Ok(Arc::clone(picture)),
                    Some((
                        zoom_crop((x, y), size, scaled_view(self.view, scale)),
                        scale,
                    )),
                )
            }),
        };
        let message = match wanted {
            None => Some("loading…".to_owned()),
            Some((Err(err), _)) => Some(err),
            Some((Ok(picture), crop)) => {
                let request = Request {
                    picture,
                    crop: crop.map(|(crop, _)| crop),
                    scale: crop.map_or(1, |(_, scale)| scale),
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

            let state = &mut self.states[i];
            if state.thumbnail_protocol.is_none()
                && let Some(thumbnail) = &state.thumbnail
                && let Ok(protocol) = self.picker.new_protocol(
                    thumbnail.clone(),
                    self.thumbnail_size,
                    Resize::Fit(None),
                )
            {
                state.thumbnail_protocol = Some(protocol);
            }
            match &state.thumbnail_protocol {
                Some(protocol) => frame.render_widget(
                    Image::new(protocol),
                    centered(picture_area, protocol.size()),
                ),
                None => frame.render_widget(Paragraph::new("·").centered(), picture_area),
            }

            let (mut label, rejected) = match &state.marks {
                Some(Ok(marks)) => (short_marks(marks), marks.in_trash),
                Some(Err(_)) => ("?".to_owned(), false),
                None => (String::new(), false),
            };
            if state.assessment().is_some_and(|a| a.banded) {
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
}
