//! Colours as the parameters of the effects take them: written in hex,
//! by name (in English or French), or as a hue in degrees.

use std::fmt;
use std::str::FromStr;

use image::Rgb;

/// The named colours, each in English and French. They are picked to
/// look good in a picture, not to be the pure primaries: pure red or
/// blue on a screen is harsh.
const NAMES: [(&[&str], [u8; 3]); 12] = [
    (&["black", "noir"], [0, 0, 0]),
    (&["white", "blanc"], [255, 255, 255]),
    (&["grey", "gray", "gris"], [128, 128, 128]),
    (&["red", "rouge"], [224, 32, 32]),
    (&["orange"], [255, 140, 26]),
    (&["yellow", "jaune"], [255, 213, 0]),
    (&["green", "vert"], [34, 177, 76]),
    (&["cyan"], [0, 200, 215]),
    (&["blue", "bleu"], [40, 80, 224]),
    (&["violet", "purple"], [138, 43, 226]),
    (&["magenta"], [224, 32, 192]),
    (&["pink", "rose"], [255, 105, 180]),
];

/// Under this saturation, a colour is a grey: it has no hue.
const NO_HUE: f32 = 0.08;

/// A colour, written `rrggbb` or `#rgb` as on the web, by name (`red`,
/// `rouge`…), or as a hue in degrees (`210deg`) taken at full strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub Rgb<u8>);

impl Color {
    /// The colour of a hue, in degrees, at full saturation and value.
    pub fn from_hue(hue: Hue) -> Color {
        let h = hue.degrees() / 60.0;
        let x = 1.0 - (h % 2.0 - 1.0).abs();
        let (r, g, b) = match h as u32 {
            0 => (1.0, x, 0.0),
            1 => (x, 1.0, 0.0),
            2 => (0.0, 1.0, x),
            3 => (0.0, x, 1.0),
            4 => (x, 0.0, 1.0),
            _ => (1.0, 0.0, x),
        };
        let byte = |value: f32| (value * 255.0).round() as u8;
        Color(Rgb([byte(r), byte(g), byte(b)]))
    }

    /// The hue of the colour, or `None` for a grey.
    pub fn hue(self) -> Option<Hue> {
        let [r, g, b] = self.0.0.map(|channel| f32::from(channel) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max <= 0.0 || (max - min) / max < NO_HUE {
            return None;
        }
        let spread = max - min;
        let sector = if max == r {
            ((g - b) / spread).rem_euclid(6.0)
        } else if max == g {
            (b - r) / spread + 2.0
        } else {
            (r - g) / spread + 4.0
        };
        Some(Hue::new(sector * 60.0))
    }
}

/// A hue in degrees, from 0 up to 360 not included: 0 red, 120 green,
/// 240 blue. Written as a number of degrees (`210`, `210deg`, `210°`),
/// or as a colour, whose hue is taken (`blue`, `2850e0`; a hex colour of
/// digits only needs its `#`, `#123456`, to not be read as degrees).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hue(f32);

impl Hue {
    /// A hue, any number of degrees being brought back into the circle.
    pub fn new(degrees: f32) -> Hue {
        Hue(degrees.rem_euclid(360.0))
    }

    pub fn degrees(self) -> f32 {
        self.0
    }

    /// How far two hues are from each other, 0 to 180 degrees, round the
    /// circle.
    pub fn distance(self, other: Hue) -> f32 {
        let gap = (self.0 - other.0).abs();
        gap.min(360.0 - gap)
    }
}

impl FromStr for Hue {
    type Err = String;

    fn from_str(text: &str) -> Result<Hue, String> {
        let trimmed = text.trim();
        let number = trimmed
            .strip_suffix("deg")
            .or_else(|| trimmed.strip_suffix('°'))
            .unwrap_or(trimmed);
        // Plain digits only: `2850e0` is a colour, not 2850 in scientific notation.
        let plain = number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
        if plain && let Ok(degrees) = number.trim().parse::<f32>() {
            return Ok(Hue::new(degrees));
        }
        let color: Color = trimmed
            .parse()
            .map_err(|_| format!("'{text}' is not a hue (degrees, a colour name or rrggbb)"))?;
        color
            .hue()
            .ok_or_else(|| format!("'{text}' is a grey, it has no hue"))
    }
}

impl fmt::Display for Hue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}deg", (self.0 * 10.0).round() / 10.0)
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(text: &str) -> Result<Color, String> {
        let trimmed = text.trim();
        let lower = trimmed.to_lowercase();
        if let Some((_, [r, g, b])) = NAMES
            .iter()
            .find(|(names, _)| names.contains(&lower.as_str()))
        {
            return Ok(Color(Rgb([*r, *g, *b])));
        }
        if let Some(degrees) = lower
            .strip_suffix("deg")
            .or_else(|| lower.strip_suffix('°'))
            && let Ok(degrees) = degrees.trim().parse::<f32>()
            && degrees.is_finite()
        {
            return Ok(Color::from_hue(Hue::new(degrees)));
        }
        let hex = lower.strip_prefix('#').unwrap_or(&lower);
        let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        let parsed = match hex.len() {
            6 => pair(0)
                .zip(pair(2))
                .zip(pair(4))
                .map(|((r, g), b)| [r, g, b]),
            3 => digit(0)
                .zip(digit(1))
                .zip(digit(2))
                .map(|((r, g), b)| [r * 17, g * 17, b * 17]),
            _ => None,
        };
        parsed.map(|channels| Color(Rgb(channels))).ok_or_else(|| {
            format!("'{text}' is not a colour (rrggbb, a name, or degrees such as 210deg)")
        })
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b] = self.0.0;
        write!(f, "{r:02x}{g:02x}{b:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(text: &str) -> [u8; 3] {
        text.parse::<Color>().unwrap().0.0
    }

    #[test]
    fn colours_are_read_in_hex_by_name_and_by_hue() {
        assert_eq!(color("ff4fa3"), [255, 79, 163]);
        assert_eq!(color("#FF4FA3"), [255, 79, 163]);
        assert_eq!(color("#f0a"), [255, 0, 170]);
        assert_eq!(color("red"), color("rouge"));
        assert_eq!(color("Bleu"), color("blue"));
        assert_eq!(color("gray"), color("gris"));
        assert_eq!(color("0deg"), [255, 0, 0]);
        assert_eq!(color("120deg"), [0, 255, 0]);
        assert_eq!(color("240°"), [0, 0, 255]);
        assert_eq!(color("-120deg"), [0, 0, 255]);
        for bad in ["", "ff4fa", "ff4fa3a", "chartreuse", "deg", "#ggg"] {
            assert!(bad.parse::<Color>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_colour_is_written_back_as_hex_and_read_again() {
        for name in ["rouge", "cyan", "ff4fa3", "210deg"] {
            let color: Color = name.parse().unwrap();
            assert_eq!(color.to_string().parse::<Color>(), Ok(color));
        }
        assert_eq!("rose".parse::<Color>().unwrap().to_string(), "ff69b4");
    }

    #[test]
    fn hues_come_from_degrees_names_and_colours() {
        let hue = |text: &str| text.parse::<Hue>().unwrap().degrees();
        assert_eq!(hue("210"), 210.0);
        assert_eq!(hue("210deg"), 210.0);
        assert_eq!(hue("390°"), 30.0);
        assert_eq!(hue("-30"), 330.0);
        assert!((hue("green") - hue("vert")).abs() < 1e-3);
        assert!((hue("cyan") - 185.0).abs() < 10.0, "{}", hue("cyan"));
        assert!((hue("2850e0") - hue("blue")).abs() < 1e-3);
        assert!("white".parse::<Hue>().unwrap_err().contains("no hue"));
        assert!("#808080".parse::<Hue>().is_err());
        assert!("lemon".parse::<Hue>().is_err());
        // Digits only need the #, or they are degrees.
        assert_eq!(hue("#e02020"), hue("red"));
        assert_eq!(hue("120"), 120.0);
    }

    #[test]
    fn the_hue_of_a_colour_follows_the_circle() {
        for degrees in [0.0, 30.0, 90.0, 150.0, 210.0, 270.0, 330.0] {
            let hue = Color::from_hue(Hue::new(degrees)).hue().unwrap();
            assert!(hue.distance(Hue::new(degrees)) < 1.0, "{degrees}: {hue}");
        }
        assert_eq!(Hue::new(350.0).distance(Hue::new(10.0)), 20.0);
        assert_eq!(Hue::new(0.0).distance(Hue::new(180.0)), 180.0);
        assert_eq!(Hue::new(210.0).to_string(), "210deg");
    }
}
