//! Animating a recipe: the frames of a recipe whose parameters are ranges
//! (`shift=20..160`), and an animated GIF made of them.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, DynamicImage, Frame, RgbImage};

use super::{Error, Recipe, Time};

/// How the ranges of a recipe move over the animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    /// Slow at both ends.
    #[default]
    Smooth,
    Linear,
    /// Slow at the start.
    In,
    /// Slow at the end.
    Out,
}

impl Easing {
    /// The moment of the ranges for a moment of the animation, both 0 to 1.
    pub fn at(self, t: f64) -> f64 {
        match self {
            Easing::Linear => t,
            Easing::Smooth => t * t * (3.0 - 2.0 * t),
            Easing::In => t * t,
            Easing::Out => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

/// What an animation is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Animation {
    /// How many frames before the way back.
    pub frames: u32,
    pub easing: Easing,
    /// Whether the animation comes back to its first frame.
    pub boomerang: bool,
    /// Whether the seed moves on at each frame, so that whatever is drawn
    /// at random shakes.
    pub jitter: bool,
}

impl Default for Animation {
    fn default() -> Animation {
        Animation {
            frames: 24,
            easing: Easing::Smooth,
            boomerang: true,
            jitter: true,
        }
    }
}

/// The frames of the recipe `words` on `image`: each parameter given as a
/// range goes from one end to the other, then, with `boomerang`, back.
/// The patterns of the recipe (`"0 40 80"`) play one cycle over these
/// frames; the way back repeats the frames reversed, so the patterns play
/// backwards there, which `boomerang: false` avoids.
pub fn animate<S: AsRef<str>>(
    image: &RgbImage,
    words: &[S],
    animation: Animation,
) -> Result<Vec<RgbImage>, Error> {
    let last = animation.frames.saturating_sub(1).max(1) as f64;
    let mut frames = Vec::with_capacity(animation.frames as usize * 2);
    for i in 0..animation.frames {
        let t = animation.easing.at(f64::from(i) / last);
        let cycle = f64::from(i) / f64::from(animation.frames.max(1));
        let mut recipe = Recipe::from_words_at_time(words, Time::new(t, cycle))?;
        if animation.jitter {
            recipe.seed = recipe.seed.wrapping_add(u64::from(i));
        }
        frames.push(recipe.apply(image)?);
    }
    if animation.boomerang && frames.len() > 2 {
        let back: Vec<_> = frames[1..frames.len() - 1].iter().rev().cloned().collect();
        frames.extend(back);
    }
    Ok(frames)
}

/// Writes frames as a GIF that loops, `fps` frames per second.
pub fn write_gif(frames: Vec<RgbImage>, fps: u32, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    let file = File::create(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut encoder = GifEncoder::new_with_speed(BufWriter::new(file), 10);
    encoder
        .set_repeat(Repeat::Infinite)
        .map_err(|err| err.to_string())?;
    let delay = Delay::from_numer_denom_ms(1000, fps.max(1));
    encoder
        .encode_frames(
            frames.into_iter().map(|frame| {
                Frame::from_parts(DynamicImage::ImageRgb8(frame).to_rgba8(), 0, 0, delay)
            }),
        )
        .map_err(|err| format!("{}: {err}", path.display()))?;
    // The encoder writes through a buffer: closing it writes the end of
    // the file.
    drop(encoder);
    Ok(())
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    #[test]
    fn easings_start_at_0_and_end_at_1() {
        for easing in [Easing::Smooth, Easing::Linear, Easing::In, Easing::Out] {
            assert_eq!(easing.at(0.0), 0.0);
            assert_eq!(easing.at(1.0), 1.0);
        }
        assert!(Easing::In.at(0.5) < 0.5 && Easing::Out.at(0.5) > 0.5);
    }

    #[test]
    fn frames_follow_the_range_and_come_back() {
        let image = RgbImage::from_pixel(40, 40, Rgb([100; 3]));
        let words = ["seed=1", "fade", "lift=0..40"];
        let plain = Animation {
            frames: 4,
            easing: Easing::Linear,
            ..Animation::default()
        };
        let there = animate(&image, &words, plain).unwrap();
        assert_eq!(there.len(), 6, "4 frames and the 2 on the way back");
        assert!(there[1] == there[5] && there[2] == there[4], "the way back");
        assert!(there[0] != there[2], "the range moves the picture");
        let one_way = animate(
            &image,
            &words,
            Animation {
                boomerang: false,
                ..plain
            },
        )
        .unwrap();
        assert_eq!(one_way.len(), 4);
        assert!(one_way[0] != one_way[3]);
    }
}
