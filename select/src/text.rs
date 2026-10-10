//! Text of the culling viewer: marks in words or in a few characters,
//! their colours, and the keys that set a rating.

use corrode_core::marks::{ColorLabel, Marks};
use ratatui::style::Color;

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
