//! Applies a creative effect to a picture and saves the result as JPEG.
//!
//! ```sh
//! cargo run --release -p corrode-core --example effects -- sandbox/P1011259.JPG sandbox/out/loss.jpg loss generations=30 quality=25 shift=1,0
//! cargo run --release -p corrode-core --example effects -- sandbox/P1011259.JPG sandbox/out/bend.jpg bend quality=75 hits=8 seed=1
//! cargo run --release -p corrode-core --example effects -- sandbox/P1011259.JPG sandbox/out/sort.jpg sort direction=horizontal low=40 high=220 reverse=false
//! ```
//!
//! The picture is the shot's full-size image (JPEG, else developed RAW).
//! Use `--release`: effects go through the whole picture many times.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::effects::{self, Databend, GenerationLoss, PixelSort};
use corrode_core::{pairing, picture};

fn parse<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
    let prefix = format!("{name}=");
    match args.iter().find_map(|arg| arg.strip_prefix(&prefix)) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("{name}: cannot read '{value}'")),
        None => Ok(default),
    }
}

fn run(input: &PathBuf, output: &PathBuf, effect: &str, args: &[String]) -> Result<(), String> {
    let shot = pairing::shots_of(std::slice::from_ref(input))
        .map_err(|err| err.to_string())?
        .into_iter()
        .next()
        .ok_or("no shot")?;
    let image = picture::full(&shot)
        .map_err(|err| err.to_string())?
        .image
        .to_rgb8();
    let start = Instant::now();
    let result = match effect {
        "loss" => {
            let defaults = GenerationLoss::default();
            let shift: String = parse(args, "shift", "1,0".to_owned())?;
            let (dx, dy) = shift
                .split_once(',')
                .and_then(|(dx, dy)| Some((dx.parse().ok()?, dy.parse().ok()?)))
                .ok_or_else(|| format!("shift: cannot read '{shift}'"))?;
            effects::generation_loss(
                &image,
                GenerationLoss {
                    generations: parse(args, "generations", defaults.generations)?,
                    quality: parse(args, "quality", defaults.quality)?,
                    shift: (dx, dy),
                },
            )
        }
        "bend" => {
            let defaults = Databend::default();
            effects::databend(
                &image,
                Databend {
                    quality: parse(args, "quality", defaults.quality)?,
                    hits: parse(args, "hits", defaults.hits)?,
                    seed: parse(args, "seed", defaults.seed)?,
                },
            )?
        }
        "sort" => {
            let defaults = PixelSort::default();
            effects::pixel_sort(
                &image,
                PixelSort {
                    direction: parse(args, "direction", defaults.direction)?,
                    low: parse(args, "low", defaults.low)?,
                    high: parse(args, "high", defaults.high)?,
                    reverse: parse(args, "reverse", defaults.reverse)?,
                },
            )
        }
        other => {
            return Err(format!(
                "unknown effect '{other}' (known: loss, bend, sort)"
            ));
        }
    };
    let elapsed = start.elapsed();
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    result
        .save(output)
        .map_err(|err| format!("{}: {err}", output.display()))?;
    println!(
        "{effect} on {}×{} in {:.1} s -> {}",
        image.width(),
        image.height(),
        elapsed.as_secs_f64(),
        output.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input, output, effect, params @ ..] = args.as_slice() else {
        eprintln!("usage: effects <image> <output.jpg> <effect> [name=value]...");
        return ExitCode::FAILURE;
    };
    match run(
        &PathBuf::from(input),
        &PathBuf::from(output),
        effect,
        params,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
