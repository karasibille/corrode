//! Text for the terminal: lines that fit the width, marks in words or in
//! a few characters, colors, and the keys that set a rating.

use corrode_core::marks::{ColorLabel, Marks};
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Stylize};
use ratatui::text::{Line, Span};

/// A line of keys and what they do, the keys standing out: as many as
/// fit in `width` columns, in order, the last one always shown.
pub fn keys_line(keys: &[(&'static str, &'static str)], width: u16) -> Line<'static> {
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
pub fn fit(parts: &[(u8, Vec<Span<'static>>)], width: usize) -> Line<'static> {
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
pub fn label_color(label: ColorLabel) -> Color {
    match label {
        ColorLabel::None => Color::Reset,
        ColorLabel::Red => Color::Red,
        ColorLabel::Yellow => Color::Yellow,
        ColorLabel::Green => Color::Green,
        ColorLabel::Blue => Color::Blue,
        ColorLabel::Purple => Color::Magenta,
    }
}

/// Marks in a few characters, for a thumbnail: `★3 G ✗`.
pub fn short_marks(marks: &Marks) -> String {
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
pub fn rank_key(key: char) -> Option<u8> {
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
pub fn centered(area: Rect, size: Size) -> Rect {
    let width = size.width.min(area.width);
    let height = size.height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
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
