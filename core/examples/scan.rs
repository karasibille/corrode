//! Prints how the files of a directory are paired, without modifying anything.
//!
//! ```sh
//! cargo run -p corrode-core --example scan -- ~/Pictures/DCIM/101_PANA
//! cargo run -p corrode-core --example scan -- ~/Pictures/DCIM/101_PANA --list
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use corrode_core::pairing::scan_dir;

fn main() -> ExitCode {
    let mut dir = None;
    let mut list = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--list" => list = true,
            _ if dir.is_none() => dir = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("unexpected argument: {arg}");
                return ExitCode::FAILURE;
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: scan <directory> [--list]");
        return ExitCode::FAILURE;
    };

    let shots = match scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("cannot read {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };

    let paired = shots.iter().filter(|s| s.is_paired()).count();
    let jpeg_only = shots.iter().filter(|s| s.raw.is_none()).count();
    let raw_only = shots.iter().filter(|s| s.jpeg.is_none()).count();

    println!("{}", dir.display());
    println!("  shots:     {}", shots.len());
    println!("  paired:    {paired}");
    println!("  JPEG only: {jpeg_only}");
    println!("  RAW only:  {raw_only}");

    if list {
        println!();
        for shot in &shots {
            let name = |path: &Option<PathBuf>| match path {
                Some(path) => path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                None => "-".to_owned(),
            };
            println!(
                "  {:<12} {:<16} {}",
                shot.stem.to_string_lossy(),
                name(&shot.jpeg),
                name(&shot.raw)
            );
        }
    }

    ExitCode::SUCCESS
}
