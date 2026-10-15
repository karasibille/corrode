//! `corrode gif`: an animated GIF from a picture and a recipe. The seed
//! moves on at each frame, so that whatever is drawn at random shakes, and
//! a parameter given as a range (`shift=20..160`) goes from one end to the
//! other over the animation.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use clap::ValueEnum;
use corrode_core::{pairing, picture};
use corrode_effects::{Animation, Recipe, animate, write_gif};
use image::imageops::FilterType;

/// How the ranges of a recipe move over the animation.
#[derive(Clone, Copy, ValueEnum)]
enum Easing {
    /// Slow at both ends
    Smooth,
    Linear,
    /// Slow at the start
    In,
    /// Slow at the end
    Out,
}

impl From<Easing> for corrode_effects::Easing {
    fn from(easing: Easing) -> corrode_effects::Easing {
        match easing {
            Easing::Smooth => corrode_effects::Easing::Smooth,
            Easing::Linear => corrode_effects::Easing::Linear,
            Easing::In => corrode_effects::Easing::In,
            Easing::Out => corrode_effects::Easing::Out,
        }
    }
}

#[derive(clap::Args)]
pub struct Args {
    /// The picture; for a shot of a RAW and a JPEG, its full-size image
    image: PathBuf,
    /// Where the GIF is written
    output: PathBuf,
    /// How many frames before the way back
    #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u32).range(1..))]
    frames: u32,
    /// Frames per second
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u32).range(1..))]
    fps: u32,
    /// Width of the GIF in pixels
    #[arg(long, default_value_t = 800, value_parser = clap::value_parser!(u32).range(1..))]
    width: u32,
    /// How the ranges move over the animation
    #[arg(long, value_enum, default_value = "smooth")]
    easing: Easing,
    /// Stop at the last frame instead of coming back to the first, which makes the loop jump
    #[arg(long)]
    no_boomerang: bool,
    /// Keep the seed the same at every frame
    #[arg(long)]
    no_jitter: bool,
    /// Play the GIF in the terminal once saved, with kitty's icat
    #[arg(long)]
    show: bool,
    /// The recipe, with ranges `a..b` for what moves: `seed=N` then effects
    /// joined by `+`; `corrode effects` lists them
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "RECIPE"
    )]
    recipe: Vec<String>,
}

pub fn run(args: Args) -> Result<(), String> {
    let words = args.recipe.as_slice();
    // Read once to report mistakes before any work.
    let recipe = Recipe::from_words(words).map_err(|err| err.to_string())?;
    let shot = pairing::shots_of(std::slice::from_ref(&args.image))
        .map_err(|err| err.to_string())?
        .into_iter()
        .next()
        .ok_or("no shot")?;
    let full = picture::full(&shot).map_err(|err| err.to_string())?.image;
    let image = if full.width() > args.width {
        let height =
            (u64::from(full.height()) * u64::from(args.width) / u64::from(full.width())) as u32;
        full.resize_exact(args.width, height.max(1), FilterType::Triangle)
    } else {
        full
    }
    .to_rgb8();

    let start = Instant::now();
    let frames = animate(
        &image,
        words,
        Animation {
            frames: args.frames,
            easing: args.easing.into(),
            boomerang: !args.no_boomerang,
            jitter: !args.no_jitter,
        },
    )
    .map_err(|err| err.to_string())?;
    let elapsed = start.elapsed();
    let count = frames.len();
    let output = &args.output;
    write_gif(frames, args.fps, output)?;
    println!(
        "{recipe}\n  {count} frames of {}×{} at {} fps in {:.1} s -> {}",
        image.width(),
        image.height(),
        args.fps,
        elapsed.as_secs_f64(),
        output.display()
    );
    if args.show {
        // Plays the animation; chafa would show only the first frame.
        let shown = Command::new("kitten")
            .args(["icat", "--align", "left"])
            .arg(output)
            .status()
            .is_ok_and(|status| status.success());
        if !shown {
            eprintln!("cannot show {}: kitten icat did not work", output.display());
        }
    }
    Ok(())
}
