//! The units the parameters of the effects are given in, each a type of
//! its own so that a share cannot be passed for a size, and so that the
//! checks and the scaling are written once.

use std::fmt;
use std::str::FromStr;

/// The width the sizes of the parameters are given for.
pub const REFERENCE_WIDTH: u32 = 1000;

/// A share of something, 0 to 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Percent(u8);

impl Percent {
    /// A percentage, anything above 100 being 100.
    pub const fn new(value: u8) -> Percent {
        Percent(if value > 100 { 100 } else { value })
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// The share as a number from 0 to 1.
    pub fn fraction(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl FromStr for Percent {
    type Err = String;

    fn from_str(text: &str) -> Result<Percent, String> {
        match text.parse::<u8>() {
            Ok(value) if value <= 100 => Ok(Percent(value)),
            _ => Err(format!("'{text}' is not a percentage (0 to 100)")),
        }
    }
}

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A size in pixels for a picture [`REFERENCE_WIDTH`] pixels wide, which
/// scales with the picture it is used on, so that a recipe looks the
/// same on a preview and on the full-size picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Size(u32);

impl Size {
    pub const fn new(pixels: u32) -> Size {
        Size(pixels)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// The size on a picture of this width, in its pixels.
    pub fn on(self, width: u32) -> u32 {
        self.on_f32(width).round() as u32
    }

    /// The same, not rounded.
    pub fn on_f32(self, width: u32) -> f32 {
        self.0 as f32 * scale_of(width)
    }
}

impl FromStr for Size {
    type Err = String;

    fn from_str(text: &str) -> Result<Size, String> {
        text.parse::<u32>()
            .map(Size)
            .map_err(|_| format!("'{text}' is not a size in pixels"))
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How many actual pixels a reference pixel is on a picture of this
/// width.
pub fn scale_of(width: u32) -> f32 {
    width as f32 / REFERENCE_WIDTH as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_stay_within_bounds() {
        assert_eq!("42".parse::<Percent>(), Ok(Percent::new(42)));
        assert!("101".parse::<Percent>().is_err());
        assert!("-1".parse::<Percent>().is_err());
        assert_eq!(Percent::new(150).value(), 100);
        assert_eq!(Percent::new(50).fraction(), 0.5);
        assert_eq!(Percent::new(7).to_string(), "7");
    }

    #[test]
    fn sizes_scale_with_the_width() {
        let size: Size = "120".parse().unwrap();
        assert_eq!(size.on(1000), 120);
        assert_eq!(size.on(5184), 622);
        assert_eq!(size.on(500), 60);
        assert_eq!(size.to_string(), "120");
        assert!("big".parse::<Size>().is_err());
    }
}
