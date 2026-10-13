//! `corrode select`: the culling viewer.

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// The directory of the shoot
    #[arg(default_value = ".")]
    dir: PathBuf,
}

pub fn run(args: Args) -> Result<(), String> {
    corrode_select::run(args.dir).map_err(|err| err.to_string())
}
