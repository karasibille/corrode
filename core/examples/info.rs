//! Lists the shots of a directory with their files, marks and shooting
//! information, everything the core library knows about them.
//!
//! ```sh
//! cargo run --release -p corrode-core --example info -- sandbox
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use corrode_core::marks::{MAX_RANK, Marks};
use corrode_core::pairing::{self, Shot};
use corrode_core::{exif, rawtherapee};

fn files(shot: &Shot) -> String {
    [&shot.jpeg, &shot.raw]
        .into_iter()
        .flatten()
        .filter_map(|path| path.extension())
        .map(|ext| ext.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("+")
}

fn marks(marks: &Marks) -> String {
    let stars = "★".repeat(marks.rank.into()) + &"☆".repeat((MAX_RANK - marks.rank).into());
    let trash = if marks.in_trash { " rejected" } else { "" };
    format!("{stars} {:?}{trash}", marks.color)
}

fn main() -> ExitCode {
    let Some(dir) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: info <directory>");
        return ExitCode::FAILURE;
    };
    let shots = match pairing::scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };

    let mut ok = true;
    for shot in &shots {
        println!("{}  {}", shot.stem.to_string_lossy(), files(shot));
        match rawtherapee::read_marks(shot) {
            Ok(found) => println!("  marks   {}", marks(&found)),
            Err(err) => {
                println!("  marks   error: {err}");
                ok = false;
            }
        }
        match exif::read(shot) {
            Ok(info) => {
                let unknown = || "?".to_owned();
                println!("  taken   {}", info.taken.clone().unwrap_or_else(unknown));
                println!("  camera  {}", info.camera.clone().unwrap_or_else(unknown));
                println!("  lens    {}", info.lens.clone().unwrap_or_else(unknown));
                println!("  setting {info}");
            }
            Err(err) => {
                println!("  exif    error: {err}");
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
