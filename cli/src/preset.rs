//! `corrode preset`: a preset applied to a whole selection.

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// The folder of shots sorted into subfolders
    selection: PathBuf,
    /// The RawTherapee profile to apply, a .pp3
    #[arg(long)]
    preset: PathBuf,
    /// The shot the preset was made on: the exposure of each group follows
    /// how much lighter or darker it is than this shot
    #[arg(long = "ref")]
    reference: Option<PathBuf>,
    /// Settings by group, `[group]` then `Section.Key=value` lines, which
    /// win over the ones worked out
    #[arg(long)]
    adjustments: Option<PathBuf>,
    /// Write the settings chosen for each group to a file, to edit
    #[arg(long)]
    save_adjustments: Option<PathBuf>,
    /// List every setting that changes
    #[arg(long)]
    detail: bool,
    /// Write the sidecars, after a .bak copy of each; without it nothing is written
    #[arg(long)]
    apply: bool,
}

pub fn run(args: Args) -> Result<(), String> {
    corrode_preset::run(corrode_preset::Options {
        selection: args.selection,
        preset: args.preset,
        reference: args.reference,
        adjustments: args.adjustments,
        save_adjustments: args.save_adjustments,
        detail: args.detail,
        apply: args.apply,
    })
}
