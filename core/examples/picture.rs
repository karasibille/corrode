//! Decodes the preview and the full-size picture of shots, prints where
//! they come from and how long they took, and saves them as JPEG.
//!
//! ```sh
//! cargo run --release -p corrode-core --example picture -- sandbox/out sandbox/P1011259.JPG sandbox/_1023948.RW2
//! ```
//!
//! Each image stands for its shot, so passing the RAW of a pair still
//! uses the JPEG. Use `--release`: decoding is much slower in debug builds.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::pairing::{self, Shot};
use corrode_core::picture::{self, Picture};

fn decode(
    shot: &Shot,
    kind: &str,
    decode: fn(&Shot) -> Result<Picture, picture::Error>,
    out: &Path,
) -> Result<(), String> {
    let start = Instant::now();
    let picture = decode(shot).map_err(|err| err.to_string())?;
    let elapsed = start.elapsed();

    let path = out.join(format!("{}-{kind}.jpg", shot.stem.to_string_lossy()));
    picture
        .image
        .to_rgb8()
        .save(&path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    println!(
        "  {kind:<7} {:>4}×{:<4} {:<10} {:>6.0} ms  -> {}",
        picture.image.width(),
        picture.image.height(),
        format!("{:?}", picture.origin),
        elapsed.as_secs_f64() * 1000.0,
        path.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1).map(PathBuf::from);
    let out = args.next();
    let images: Vec<PathBuf> = args.collect();
    let Some(out) = out.filter(|_| !images.is_empty()) else {
        eprintln!("usage: picture <output directory> <image>...");
        return ExitCode::FAILURE;
    };
    if let Err(err) = fs::create_dir_all(&out) {
        eprintln!("{}: {err}", out.display());
        return ExitCode::FAILURE;
    }

    let mut ok = true;
    for image in &images {
        println!("{}", image.display());
        let shot = pairing::shots_of(std::slice::from_ref(image))
            .map_err(|err| err.to_string())
            .map(|shots| shots.into_iter().next().expect("one image gives one shot"));
        let result = shot.and_then(|shot| {
            decode(&shot, "preview", picture::preview, &out)?;
            decode(&shot, "full", picture::full, &out)
        });
        if let Err(err) = result {
            eprintln!("  error: {err}");
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
