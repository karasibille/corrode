//! Scores the sharpness of every shot of a directory, burst by burst,
//! around the point the camera focused on, and marks the sharpest of each
//! burst.
//!
//! ```sh
//! cargo run --release -p corrode-core --example sharpness -- sandbox
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::{bursts, exif, pairing, picture, sharpness};

fn main() -> ExitCode {
    let Some(dir) = env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: sharpness <directory>");
        return ExitCode::FAILURE;
    };
    let shots = match pairing::scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    let exifs: Vec<_> = shots.iter().map(|shot| exif::read(shot).ok()).collect();
    let times: Vec<Option<i64>> = exifs
        .iter()
        .map(|exif| exif.as_ref().and_then(|exif| exif.taken_ms))
        .collect();

    for burst in bursts::group(&times, bursts::MAX_GAP_MS) {
        let scores: Vec<Option<f32>> = burst
            .clone()
            .map(|i| {
                let start = Instant::now();
                let score = picture::preview(&shots[i]).ok().map(|p| {
                    let focus = exifs[i].as_ref().and_then(|exif| exif.focus_point);
                    sharpness::score(&p.image, focus)
                });
                eprintln!(
                    "  {} in {} ms",
                    shots[i].stem.to_string_lossy(),
                    start.elapsed().as_millis()
                );
                score
            })
            .collect();
        let best = scores
            .iter()
            .enumerate()
            .filter_map(|(k, score)| score.map(|s| (k, s)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k);
        println!("burst of {}", burst.len());
        for (k, i) in burst.enumerate() {
            let score = scores[k].map_or("?".to_owned(), |s| format!("{s:6.1}"));
            let mark = if Some(k) == best {
                "  ◆ sharpest"
            } else {
                ""
            };
            println!("  {}  {score}{mark}", shots[i].stem.to_string_lossy());
        }
    }
    ExitCode::SUCCESS
}
