//! Removes the light bands of RAW files, writing each as a DNG next to
//! it (`_1094086.RW2` → `_1094086-deband.dng`) for RawTherapee. Files
//! whose preview shows no bands are skipped, unless `--force` is given.
//!
//! ```sh
//! cargo run --release -p corrode-core --example deband -- sandbox/banding/109/_1094086.RW2
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::{banding, debanding, pairing, picture};

fn main() -> ExitCode {
    let (flags, files): (Vec<String>, Vec<String>) =
        env::args().skip(1).partition(|arg| arg.starts_with("--"));
    let force = flags.iter().any(|flag| flag == "--force");
    let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
    if files.is_empty() || flags.iter().any(|flag| flag != "--force") {
        eprintln!("usage: deband [--force] <raw file>...");
        return ExitCode::FAILURE;
    }
    let mut ok = true;
    for raw in &files {
        if !force {
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
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
