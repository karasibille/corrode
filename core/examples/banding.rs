//! Looks for the light bands LED stage lighting leaves on photos taken
//! with an electronic shutter, in every shot of a directory.
//!
//! ```sh
//! cargo run --release -p corrode-core --example banding -- sandbox
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::{banding, pairing, picture};

fn main() -> ExitCode {
    let Some(dir) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: banding <directory>");
        return ExitCode::FAILURE;
    };
    let shots = match pairing::scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };
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
    ExitCode::SUCCESS
}
