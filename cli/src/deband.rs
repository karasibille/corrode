//! `corrode deband`: the light bands of RAW files removed, into DNG files.

use std::path::PathBuf;
use std::time::Instant;

use corrode_core::{banding, debanding, pairing, picture};

#[derive(clap::Args)]
pub struct Args {
    /// Correct even the files whose preview shows no bands
    #[arg(long)]
    force: bool,
    /// The RAW files; each is written as a DNG next to it (`x.RW2` gives `x-deband.dng`)
    #[arg(required = true)]
    files: Vec<PathBuf>,
}

pub fn run(args: Args) -> Result<(), String> {
    let mut failed = 0;
    for raw in &args.files {
        if !args.force {
            let banded = pairing::shots_of(std::slice::from_ref(raw))
                .ok()
                .and_then(|shots| shots.into_iter().next())
                .and_then(|shot| picture::preview(&shot).ok())
                .and_then(|preview| banding::analyze(&preview.image))
                .is_some_and(|bands| bands.is_banded());
            if !banded {
                println!(
                    "{}: no light bands on the preview, skipped (--force to correct anyway)",
                    raw.display()
                );
                continue;
            }
        }
        let dng = debanding::output_path(raw);
        let start = Instant::now();
        match debanding::to_dng(raw, &dng) {
            Ok(pattern) => println!(
                "{}: bands every {:.1} rows, amplitude R {:.1}% G {:.1}% B {:.1}%, peaks ×{:.0}/{:.0}/{:.0} → ×{:.0}/{:.0}/{:.0} after, written to {} in {:.1} s",
                raw.display(),
                pattern.period,
                100.0 * pattern.amplitude(0),
                100.0 * pattern.amplitude(1),
                100.0 * pattern.amplitude(2),
                pattern.peaks[0],
                pattern.peaks[1],
                pattern.peaks[2],
                pattern.residual[0],
                pattern.residual[1],
                pattern.residual[2],
                dng.display(),
                start.elapsed().as_secs_f64()
            ),
            Err(err) => {
                println!("{}: {err}", raw.display());
                failed += 1;
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} files failed"))
    }
}
