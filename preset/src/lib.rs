//! Applies a RawTherapee preset to a whole selection.
//!
//! The selection is a folder of shots sorted into subfolders by look, as
//! `corrode select` does with `G`. The preset is a `.pp3` made on one shot.
//! Each group gets the preset with its own adjustments: the exposure
//! compensation is worked out from the light of the group, and any setting
//! can be changed by hand in a file. By default nothing is written.

use std::path::PathBuf;

use corrode_core::look;
use corrode_core::pairing;
use corrode_core::pp3::Profile;
use corrode_core::preset::{self, Adjustments, Group};
use corrode_core::rawtherapee::Config;
use corrode_core::similarity;

/// What the command is told.
pub struct Options {
    /// The folder of shots sorted into subfolders.
    pub selection: PathBuf,
    /// The RawTherapee profile to apply, a `.pp3`.
    pub preset: PathBuf,
    /// The shot the preset was made on: the exposure of each group
    /// follows how much lighter or darker it is than this shot.
    pub reference: Option<PathBuf>,
    /// Settings by group, `[group]` then `Section.Key=value` lines, which
    /// win over the ones worked out.
    pub adjustments: Option<PathBuf>,
    /// Where to write the settings chosen for each group, to edit.
    pub save_adjustments: Option<PathBuf>,
    /// List every setting that changes.
    pub detail: bool,
    /// Write the sidecars, after a `.bak` copy of each.
    pub apply: bool,
}

/// Applies the preset to the groups of the selection, or shows what it
/// would do.
pub fn run(args: Options) -> Result<(), String> {
    let preset =
        Profile::load(&args.preset).map_err(|err| format!("{}: {err}", args.preset.display()))?;
    let groups = preset::groups(&args.selection)
        .map_err(|err| format!("{}: {err}", args.selection.display()))?;
    if groups.is_empty() {
        return Err(format!(
            "{}: no shot to apply the preset to",
            args.selection.display()
        ));
    }
    let manual = match &args.adjustments {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            Adjustments::parse(&text).map_err(|err| format!("{}: {err}", path.display()))?
        }
        None => Adjustments::default(),
    };
    let reference = match &args.reference {
        Some(path) => {
            let shots =
                pairing::shots_of(std::slice::from_ref(path)).map_err(|err| err.to_string())?;
            let light = similarity::mean_luminance(&shots).ok_or_else(|| {
                format!("{}: no thumbnail to measure the light of", path.display())
            })?;
            println!("preset made on {}: light {light:.3}", path.display());
            Some(light)
        }
        None => {
            println!("no --ref: the exposure of the preset is kept as it is for every group");
            None
        }
    };

    let config = Config::load().ok();
    let mut chosen = Adjustments::default();
    let (mut shots_changed, mut failures) = (0, 0);
    for Group { name, shots } in &groups {
        let light = similarity::mean_luminance(shots);
        let settings = preset::merge(
            preset::automatic(&preset, reference, light),
            manual.settings(name),
        );
        println!("\n{name}: {} shots", shots.len());
        match light {
            Some(light) => println!("  light {light:.3}"),
            None => println!("  light unknown, no thumbnail"),
        }
        for setting in &settings {
            println!("  {setting}");
        }
        for shot in shots {
            let stem = shot.stem.to_string_lossy();
            let (target, path) = match preset::profile_of(shot, config.as_ref()) {
                Ok(found) => found,
                Err(err) => {
                    eprintln!("  {stem}: {err}, skipped");
                    failures += 1;
                    continue;
                }
            };
            let result = preset::adapt(&target, &preset, &settings);
            let changes = look::changes(&target, &result);
            println!("  {stem}: {} settings change", changes.len());
            if args.detail {
                for change in &changes {
                    println!(
                        "      {}.{}: {} -> {}",
                        change.section,
                        change.key,
                        change.from.as_deref().unwrap_or("(none)"),
                        change.to.as_deref().unwrap_or("(removed)"),
                    );
                }
            }
            if args.apply && !changes.is_empty() {
                match preset::save_with_backup(&result, &path) {
                    Ok(()) => shots_changed += 1,
                    Err(err) => {
                        eprintln!("  {stem}: {}: {err}", path.display());
                        failures += 1;
                    }
                }
            }
        }
        chosen.set(name, settings);
    }

    if let Some(path) = &args.save_adjustments {
        std::fs::write(path, chosen.to_string())
            .map_err(|err| format!("{}: {err}", path.display()))?;
        println!("\nsettings of each group written to {}", path.display());
    }
    let count: usize = groups.iter().map(|group| group.shots.len()).sum();
    if args.apply {
        println!("\n{shots_changed} of {count} sidecars written ({failures} failed)");
    } else {
        println!("\nnothing written, --apply to write the {count} sidecars");
    }
    if failures > 0 {
        return Err(format!("{failures} shots failed"));
    }
    Ok(())
}
