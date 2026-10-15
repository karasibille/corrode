//! `corrode live`: working on a picture with the recipe of a file, live.

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// The picture; for a shot of a RAW and a JPEG, the shot
    image: PathBuf,
    /// The recipe file, watched: saved again, it is applied again. Made
    /// with an example when it does not exist
    recipe: PathBuf,
}

pub fn run(args: Args) -> Result<(), String> {
    corrode_live::run(args.image, args.recipe).map_err(|err| err.to_string())
}
