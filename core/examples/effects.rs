//! Applies a recipe of creative effects to a picture and saves the result
//! as JPEG, printing the recipe in full so that it can be made again.
//!
//! ```sh
//! cargo run --release -p corrode-core --example effects -- sandbox/P1011259.JPG sandbox/out/x.jpg seed=7 sort low=40 high=220 + slice slices=12 split=true
//! ```
//!
//! Effects: loss (generation loss), bend (databending), sort (pixel
//! sorting), slice (slice shift), stretch (pixel stretch), split (channel
//! split), dither, duotone, scanlines, drag, aberration, grain, fade,
//! vignette, leak, bloom; `name=value` sets a parameter,
//! the rest take their defaults. `show=true` before the recipe shows
//! the result in the terminal once saved (kitty, else chafa).
//! The picture is the shot's full-size image (JPEG, else developed RAW).
//! Use `--release`: effects go through the whole picture many times.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use corrode_core::effects::{NAMES, Recipe};
use corrode_core::{pairing, picture};

/// Shows a picture in the terminal with kitty's icat, else chafa.
pub fn show(path: &Path) {
    let shown = Command::new("kitten")
        .args(["icat", "--align", "left"])
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
        || Command::new("chafa")
            .arg(path)
            .status()
            .is_ok_and(|status| status.success());
    if !shown {
        eprintln!(
            "cannot show {}: neither kitten icat nor chafa worked",
            path.display()
        );
    }
}

fn run(input: &Path, output: &Path, words: &[String]) -> Result<(), String> {
    let (show_result, words) = match words.first().map(String::as_str) {
        Some("show=true") => (true, &words[1..]),
        Some("show=false") => (false, &words[1..]),
        _ => (false, words),
    };
    let recipe = Recipe::from_words(words).map_err(|err| err.to_string())?;
    let shot = pairing::shots_of(std::slice::from_ref(&input.to_path_buf()))
        .map_err(|err| err.to_string())?
        .into_iter()
        .next()
        .ok_or("no shot")?;
    let image = picture::full(&shot)
        .map_err(|err| err.to_string())?
        .image
        .to_rgb8();
    let start = Instant::now();
    let result = recipe.apply(&image).map_err(|err| err.to_string())?;
    let elapsed = start.elapsed();
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    result
        .save(output)
        .map_err(|err| format!("{}: {err}", output.display()))?;
    println!(
        "{recipe}\n  on {}×{} in {:.1} s -> {}",
        image.width(),
        image.height(),
        elapsed.as_secs_f64(),
        output.display()
    );
    if show_result {
        show(output);
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input, output, words @ ..] = args.as_slice() else {
        eprintln!(
            "usage: effects <image> <output.jpg> [show=true] [seed=N] <effect> [name=value]... [+ <effect> ...]\neffects: {}",
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
