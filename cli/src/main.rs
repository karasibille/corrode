//! `corrode`: one command for what the project does with photos and
//! pictures, each thing a subcommand.

mod bands;
mod deband;
mod effect;
mod ghost;
mod gif;
mod group;
mod live;
mod preset;
mod select;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "corrode",
    version,
    about = "Cull and sort photos, retouch them in bulk, and glitch pictures",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Cull the shots of a directory in the terminal, with marks written to RawTherapee
    Select(select::Args),
    /// Sort a selection into folders of shots that look alike
    Group(group::Args),
    /// Apply a RawTherapee preset to a whole selection, group by group
    Preset(preset::Args),
    /// Apply effects to a picture
    Effect(effect::Args),
    /// List the effects, or tell the parameters of one
    Effects(effect::HelpArgs),
    /// Lay the frames of a burst over one another, so that each pose leaves a ghost (experimental)
    Ghost(ghost::Args),
    /// Make an animated GIF from a picture and a recipe whose ranges move
    Gif(gif::Args),
    /// Work on a picture with the recipe of a file, applied again each time it is saved
    Live(live::Args),
    /// Look for the light bands LED lighting leaves, in every shot of a directory
    Bands(bands::Args),
    /// Remove the light bands of RAW files, into DNG files
    Deband(deband::Args),
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Select(args) => select::run(args),
        Command::Group(args) => group::run(args),
        Command::Preset(args) => preset::run(args),
        Command::Effect(args) => effect::run(args),
        Command::Effects(args) => effect::help(args),
        Command::Ghost(args) => ghost::run(args),
        Command::Gif(args) => gif::run(args),
        Command::Live(args) => live::run(args),
        Command::Bands(args) => bands::run(args),
        Command::Deband(args) => deband::run(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
