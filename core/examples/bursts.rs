//! Lists the bursts of a directory: shots taken in quick succession.
//!
//! ```sh
//! cargo run --release -p corrode-core --example bursts -- sandbox
//! cargo run --release -p corrode-core --example bursts -- sandbox --gap 500 --list
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::{bursts, exif, pairing};

fn main() -> ExitCode {
    let mut dir = None;
    let mut gap = bursts::MAX_GAP_MS;
    let mut list = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--gap" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) => gap = value,
                None => {
                    eprintln!("--gap needs a number of milliseconds");
                    return ExitCode::FAILURE;
                }
            },
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("unexpected argument: {arg}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: bursts <directory> [--gap <ms>] [--list]");
        return ExitCode::FAILURE;
    };

    let start = Instant::now();
    let shots = match pairing::scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    let times: Vec<Option<i64>> = shots
        .iter()
        .map(|shot| exif::read(shot).ok().and_then(|exif| exif.taken_ms))
        .collect();
    let groups = bursts::group(&times, gap);
    let elapsed = start.elapsed();

    let mut sizes: Vec<usize> = groups.iter().map(|burst| burst.len()).collect();
    sizes.sort_unstable();
    let singles = sizes.iter().filter(|&&size| size == 1).count();
    println!(
        "{}: {} shots, {} groups with a gap of {gap} ms ({} bursts, {singles} single shots)",
        dir.display(),
        shots.len(),
        groups.len(),
        groups.len() - singles,
    );
    if let (Some(median), Some(largest)) = (sizes.get(sizes.len() / 2), sizes.last()) {
        println!("group size: median {median}, largest {largest}");
    }
    println!("read in {} ms", elapsed.as_millis());

    if list {
        for burst in &groups {
            let names: Vec<_> = shots[burst.clone()]
                .iter()
                .map(|shot| shot.stem.to_string_lossy())
                .collect();
            println!("{:>3}  {}", burst.len(), names.join(" "));
        }
    }
    ExitCode::SUCCESS
}
