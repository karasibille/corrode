//! Animates a recipe of effects on a picture into a looping GIF: the
//! seed moves on at each frame, so that whatever is drawn at random
//! shakes, and a parameter given as a range (`shift=20..160`) goes from
//! one end to the other over the animation.
//!
//! ```sh
//! cargo run --release -p corrode-core --example glitchgif -- sandbox/P1011259.JPG sandbox/out/x.gif frames=24 fps=12 width=800 seed=11 slice slices=10 shift=20..160 split=true + bend hits=0..12
//! ```
//!
//! Options before the recipe: `frames` (24), `fps` (12), `width` of the
//! GIF (800), `boomerang` (true: the animation goes back to its start,
//! so that the loop does not jump), `jitter` (true: the seed moves on
//! at each frame).

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::effects::{NAMES, Recipe};
use corrode_core::{pairing, picture};
use image::codecs::gif::{GifEncoder, Repeat};
use image::imageops::FilterType;
use image::{Delay, DynamicImage, Frame};

struct Options {
    frames: u32,
    fps: u32,
    width: u32,
    boomerang: bool,
    jitter: bool,
}

/// Takes the options off the front of the words, leaving the recipe.
fn options(words: &[String]) -> Result<(Options, &[String]), String> {
    let mut options = Options {
        frames: 24,
        fps: 12,
        width: 800,
        boomerang: true,
        jitter: true,
    };
    let mut rest = 0;
    for (i, word) in words.iter().enumerate() {
        let Some((name, value)) = word.split_once('=') else {
            break;
        };
        let bad = || format!("{name}: cannot read '{value}'");
        match name {
            "frames" => options.frames = value.parse().map_err(|_| bad())?,
            "fps" => options.fps = value.parse().map_err(|_| bad())?,
            "width" => options.width = value.parse().map_err(|_| bad())?,
            "boomerang" => options.boomerang = value.parse().map_err(|_| bad())?,
            "jitter" => options.jitter = value.parse().map_err(|_| bad())?,
            // The recipe's own.
            _ => break,
        }
        rest = i + 1;
    }
    if options.frames == 0 || options.fps == 0 || options.width == 0 {
        return Err("frames, fps and width must be above zero".to_owned());
    }
    Ok((options, &words[rest..]))
}

fn run(input: &Path, output: &Path, words: &[String]) -> Result<(), String> {
    let (options, words) = options(words)?;
    // Read once to report mistakes before any work.
    let recipe = Recipe::from_words(words).map_err(|err| err.to_string())?;
    let shot = pairing::shots_of(std::slice::from_ref(&input.to_path_buf()))
        .map_err(|err| err.to_string())?
        .into_iter()
        .next()
        .ok_or("no shot")?;
    let full = picture::full(&shot).map_err(|err| err.to_string())?.image;
    let image = if full.width() > options.width {
        let height =
            (u64::from(full.height()) * u64::from(options.width) / u64::from(full.width())) as u32;
        full.resize_exact(options.width, height.max(1), FilterType::Triangle)
    } else {
        full
    }
    .to_rgb8();

    let start = Instant::now();
    let last = options.frames.saturating_sub(1).max(1) as f64;
    let mut frames = Vec::with_capacity(options.frames as usize * 2);
    for i in 0..options.frames {
        let mut recipe =
            Recipe::from_words_at(words, f64::from(i) / last).map_err(|err| err.to_string())?;
        if options.jitter {
            recipe.seed = recipe.seed.wrapping_add(u64::from(i));
        }
        let frame = recipe.apply(&image).map_err(|err| err.to_string())?;
        frames.push(frame);
    }
    if options.boomerang && frames.len() > 2 {
        let back: Vec<_> = frames[1..frames.len() - 1].iter().rev().cloned().collect();
        frames.extend(back);
    }
    let elapsed = start.elapsed();

    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    let file = File::create(output).map_err(|err| format!("{}: {err}", output.display()))?;
    let mut encoder = GifEncoder::new_with_speed(BufWriter::new(file), 10);
    encoder
        .set_repeat(Repeat::Infinite)
        .map_err(|err| err.to_string())?;
    let delay = Delay::from_numer_denom_ms(1000, options.fps);
    let count = frames.len();
    encoder
        .encode_frames(
            frames.into_iter().map(|frame| {
                Frame::from_parts(DynamicImage::ImageRgb8(frame).to_rgba8(), 0, 0, delay)
            }),
        )
        .map_err(|err| format!("{}: {err}", output.display()))?;
    println!(
        "{recipe}\n  {count} frames of {}×{} at {} fps in {:.1} s -> {}",
        image.width(),
        image.height(),
        options.fps,
        elapsed.as_secs_f64(),
        output.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input, output, words @ ..] = args.as_slice() else {
        eprintln!(
            "usage: glitchgif <image> <output.gif> [frames=N fps=N width=N boomerang=B jitter=B] [seed=N] <effect> [name=value|name=a..b]... [+ <effect> ...]\neffects: {}",
            NAMES.join(", ")
        );
        return ExitCode::FAILURE;
    };
    match run(&PathBuf::from(input), &PathBuf::from(output), words) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
