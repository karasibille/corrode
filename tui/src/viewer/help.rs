//! The full help screen, laid out for the width of the terminal.

use std::time::Duration;

use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

use super::Viewer;
use crate::loader::Job;

impl Viewer {
    /// The full help, with every key and what the marks mean, laid out
    /// for a terminal `width` columns wide; long lines wrap.
    pub(super) fn help_text(&self, width: u16) -> Vec<Line<'static>> {
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
}
