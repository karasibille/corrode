//! `corrode bands`: which shots of a directory have light bands.

use std::path::PathBuf;
use std::time::Instant;

use corrode_core::{banding, pairing, picture};

#[derive(clap::Args)]
pub struct Args {
    /// The directory of shots
    dir: PathBuf,
}

pub fn run(args: Args) -> Result<(), String> {
    let shots =
        pairing::scan_dir(&args.dir).map_err(|err| format!("{}: {err}", args.dir.display()))?;
    let mut banded = 0;
    for shot in &shots {
        let name = shot.stem.to_string_lossy();
        let preview = match picture::preview(shot) {
            Ok(preview) => preview,
            Err(err) => {
                println!("{name}  error: {err}");
                continue;
            }
        };
        let start = Instant::now();
        match banding::analyze(&preview.image) {
            Some(bands) => {
                banded += usize::from(bands.is_banded());
                println!(
                    "{name}  {}  period {:5.1} px, {:3.0}% of blocks, peak ×{:.0}  ({} ms)",
                    if bands.is_banded() { "BANDS" } else { "  -  " },
                    bands.period,
                    bands.coverage * 100.0,
                    bands.peak,
                    start.elapsed().as_millis()
                );
            }
            None => println!("{name}  too small to analyse"),
        }
    }
    println!("{banded} of {} shots banded", shots.len());
    Ok(())
}
