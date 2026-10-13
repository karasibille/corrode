//! `corrode effect` and `corrode effects`: a recipe of effects on a picture,
//! and what the effects are told by.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use corrode_core::{pairing, picture};
use corrode_effects::{NAMES, Recipe, describe};

#[derive(clap::Args)]
pub struct Args {
    /// The picture; for a shot of a RAW and a JPEG, its full-size image
    image: PathBuf,
    /// Where the JPEG result is written
    output: PathBuf,
    /// Show the result in the terminal once saved (kitty, else chafa)
    #[arg(long)]
    show: bool,
    /// The recipe: `seed=N` then effects, each its name and `name=value`
    /// parameters, joined by `+`; `corrode effects` lists them
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "RECIPE"
    )]
    recipe: Vec<String>,
}

#[derive(clap::Args)]
pub struct HelpArgs {
    /// An effect; its parameters, defaults and what they do are told
    name: Option<String>,
}

/// Shows a picture in the terminal with kitty's icat, else chafa.
fn show(path: &Path) {
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

pub fn run(args: Args) -> Result<(), String> {
    let recipe = Recipe::from_words(&args.recipe).map_err(|err| err.to_string())?;
    let shot = pairing::shots_of(std::slice::from_ref(&args.image))
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
    if let Some(dir) = args.output.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    result
        .save(&args.output)
        .map_err(|err| format!("{}: {err}", args.output.display()))?;
    println!(
        "{recipe}\n  on {}×{} in {:.1} s -> {}",
        image.width(),
        image.height(),
        elapsed.as_secs_f64(),
        args.output.display()
    );
    if args.show {
        show(&args.output);
    }
    Ok(())
}

/// What an effect is told by: its parameters, their defaults and what
/// they do; the names of all the effects when none is given.
pub fn help(args: HelpArgs) -> Result<(), String> {
    let Some(effect) = args.name else {
        println!("effects: {}", NAMES.join(", "));
        println!("corrode effects <effect> tells the parameters of one");
        return Ok(());
    };
    let params = describe(&effect).map_err(|err| err.to_string())?;
    println!("{effect}");
    for param in params {
        println!(
            "  {}={}  {}",
            param.name,
            param.default,
            param.help.replace('`', "")
        );
    }
    Ok(())
}
