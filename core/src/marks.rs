//! The marks culling sets on a shot: a rating, a color label and a
//! rejection. They are stored in RawTherapee sidecars (see `pp3`), which
//! is where their values come from.

use std::fmt;

/// Highest star rating RawTherapee knows.
pub const MAX_RANK: u8 = 5;

/// Color label, numbered as in RawTherapee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorLabel {
    #[default]
    None = 0,
    Red = 1,
    Yellow = 2,
    Green = 3,
    Blue = 4,
    Purple = 5,
}

impl ColorLabel {
    /// The label RawTherapee numbers `n`, if any.
    pub fn from_number(n: u8) -> Option<ColorLabel> {
        match n {
            0 => Some(ColorLabel::None),
            1 => Some(ColorLabel::Red),
            2 => Some(ColorLabel::Yellow),
            3 => Some(ColorLabel::Green),
            4 => Some(ColorLabel::Blue),
            5 => Some(ColorLabel::Purple),
            _ => None,
        }
    }
}

/// The marks corrode reads and writes. A missing key means its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marks {
    /// Star rating, from 0 (unrated) to [`MAX_RANK`].
    pub rank: u8,
    pub color: ColorLabel,
    /// Rejected photo, shown in RawTherapee's trash.
    pub in_trash: bool,
}

impl Marks {
    /// The rating as stars: `★★★☆☆`.
    pub fn stars(&self) -> String {
        "★".repeat(self.rank.into()) + &"☆".repeat((MAX_RANK - self.rank).into())
    }
}

/// The marks in words: `★★★☆☆ Green rejected`.
impl fmt::Display for Marks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}", self.stars(), self.color)?;
        if self.in_trash {
            f.write_str(" rejected")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_read_as_stars_label_and_rejection() {
        let marks = Marks {
            rank: 3,
            color: ColorLabel::Green,
            in_trash: true,
        };
        assert_eq!(marks.stars(), "★★★☆☆");
        assert_eq!(marks.to_string(), "★★★☆☆ Green rejected");
        assert_eq!(Marks::default().to_string(), "☆☆☆☆☆ None");
    }
}
