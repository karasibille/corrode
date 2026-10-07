//! The lines around the picture: the shot on top, how it was taken, and
//! the progress of culling at the bottom.

use corrode_core::marks::ColorLabel;
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

use super::Viewer;
use crate::app::{Filter, Mode, Time};
use crate::text::{fit, label_color};

impl Viewer {
    /// What changes from one photo to the next and matters for culling:
    /// position, name, marks, place in the burst, sharpness, light bands.
    pub(super) fn photo_line(&self, width: u16) -> Line<'static> {
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
        match self.states[index].marks.as_ref() {
            Some(Ok(marks)) => {
                parts.push((1, vec![Span::from(marks.stars()).fg(Color::Yellow)]));
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
            .and_then(|best| self.states[best].assessment())
            .map(|a| a.sharpness);
        match (self.states[index].assessment(), best_score) {
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
        if self.states[index].assessment().is_some_and(|a| a.banded) {
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
    pub(super) fn shooting_line(&self, width: u16) -> Line<'static> {
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
        match self.states[index].exif.as_ref() {
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
    pub(super) fn progress_line(&self, width: u16) -> Line<'static> {
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
        if progress.done() {
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
