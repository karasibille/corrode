//! Groups the shots of a directory by look, to sort a selection into
//! subfolders. By default nothing is moved: the groups are listed, and a
//! contact sheet of each can be written to check them by eye. With
//! `--move`, each group of two shots or more goes to its own subfolder
//! (`bleu-_1117041`…), with the RAW, the JPEG and the sidecars; shots alone stay.
//!
//! ```sh
//! cargo run --release -p corrode-core --example similar -- sandbox/selection
//! cargo run --release -p corrode-core --example similar -- sandbox/selection --threshold 0.2 --sheets /tmp/sheets
//! cargo run --release -p corrode-core --example similar -- sandbox/selection --time 0 (time off) or --time 0.7 --scale 600
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use corrode_core::picture;
use corrode_core::selection::{self, Batch};
use corrode_core::similarity::{self, Settings, Signature, Weights};
use corrode_core::{exif, pairing};
use image::imageops::{self, FilterType};
use image::{DynamicImage, Rgb, RgbImage};
use rayon::prelude::*;

/// Cell of a contact sheet, and thumbnails per row.
const CELL: u32 = 160;
const COLUMNS: u32 = 8;

fn main() -> ExitCode {
    let mut dir = None;
    let defaults = Settings::default();
    let mut threshold = defaults.threshold;
    let mut sheets = None;
    let mut time_weight = defaults.time_weight;
    let mut time_scale = defaults.time_scale_s;
    let mut weights = defaults.weights;
    let mut sort = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--threshold" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) => threshold = value,
                None => {
                    eprintln!("--threshold needs a number, 0.2 for instance");
                    return ExitCode::FAILURE;
                }
            },
            "--time" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) if (0.0..=1.0).contains(&value) => time_weight = value,
                _ => {
                    eprintln!("--time needs a weight between 0 and 1");
                    return ExitCode::FAILURE;
                }
            },
            "--scale" => match args.next().and_then(|value| value.parse().ok()) {
                Some(value) if value > 0 => time_scale = value,
                _ => {
                    eprintln!("--scale needs a number of seconds");
                    return ExitCode::FAILURE;
                }
            },
            "--weights" => {
                let parsed: Option<Vec<f32>> = args
                    .next()
                    .and_then(|value| value.split(',').map(|part| part.parse().ok()).collect());
                match parsed.as_deref() {
                    Some(&[colour, layout, tint]) if colour + layout + tint > 0.0 => {
                        weights = Weights {
                            colour,
                            layout,
                            tint,
                        }
                    }
                    _ => {
                        eprintln!("--weights needs colour,layout,tint, 1,1,0 for instance");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--move" => sort = true,
            "--sheets" => match args.next() {
                Some(path) => sheets = Some(PathBuf::from(path)),
                None => {
                    eprintln!("--sheets needs a directory");
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
        eprintln!(
            "usage: similar <directory> [--threshold <0..1>] [--time <0..1>] [--scale <s>] [--weights <colour,layout,tint>] [--sheets <directory>] [--move]"
        );
        return ExitCode::FAILURE;
    };

    let shots = match pairing::scan_dir(&dir) {
        Ok(shots) => shots,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };

    let start = Instant::now();
    let thumbnails: Vec<Option<DynamicImage>> = shots.par_iter().map(picture::thumbnail).collect();
    let read = start.elapsed();
    // Shots without a thumbnail cannot be compared: they stay out.
    let (kept, signatures): (Vec<usize>, Vec<Signature>) = thumbnails
        .iter()
        .enumerate()
        .filter_map(|(index, thumbnail)| Some((index, Signature::of(thumbnail.as_ref()?))))
        .unzip();
    let times: Vec<Option<i64>> = kept
        .iter()
        .map(|&index| {
            exif::read(&shots[index])
                .ok()
                .and_then(|exif| exif.taken_ms)
        })
        .collect();
    let start = Instant::now();
    let settings = Settings {
        threshold,
        time_weight,
        time_scale_s: time_scale,
        weights,
    };
    let groups = similarity::group(&signatures, &times, &settings);
    let clustered = start.elapsed();

    let singles = groups.iter().filter(|group| group.len() == 1).count();
    println!(
        "{}: {} shots, {} compared, {} groups at {threshold}, time {time_weight} over {time_scale} s, weights {}/{}/{} ({singles} alone)",
        dir.display(),
        shots.len(),
        kept.len(),
        groups.len(),
        weights.colour,
        weights.layout,
        weights.tint,
    );
    println!(
        "thumbnails read in {} ms, clustered in {} ms",
        read.as_millis(),
        clustered.as_millis()
    );
    for (number, group) in groups.iter().enumerate() {
        let names: Vec<_> = group
            .iter()
            .map(|&i| shots[kept[i]].stem.to_string_lossy())
            .collect();
        println!(
            "{:>3}  {:>3}  {:<7} {}",
            number + 1,
            group.len(),
            similarity::colour_of(&signatures, group),
            names.join(" ")
        );
    }

    if let Some(sheets) = sheets {
        if let Err(err) = write_sheets(&sheets, &groups, &kept, &thumbnails) {
            eprintln!("{}: {err}", sheets.display());
            return ExitCode::FAILURE;
        }
        println!("contact sheets in {}", sheets.display());
    }
    if sort {
        let batches: Vec<Batch> = groups
            .iter()
            .map(|group| Batch {
                label: similarity::colour_of(&signatures, group).to_owned(),
                members: group.iter().map(|&i| kept[i]).collect(),
            })
            .collect();
        let sorting = selection::sort_into_groups(&dir, &shots, &batches);
        let count: usize = sorting.sorted.iter().map(|folder| folder.shots.len()).sum();
        println!(
            "{count} shots moved into {} folders of {}",
            sorting.sorted.len(),
            dir.display()
        );
        if let Some(err) = sorting.error {
            eprintln!("sorting stopped: {err}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// One JPEG contact sheet per group, `group-01.jpg`…, thumbnails in shooting order.
fn write_sheets(
    out: &Path,
    groups: &[Vec<usize>],
    kept: &[usize],
    thumbnails: &[Option<DynamicImage>],
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(out)?;
    for (number, group) in groups.iter().enumerate() {
        let columns = (group.len() as u32).min(COLUMNS);
        let rows = (group.len() as u32).div_ceil(COLUMNS);
        let mut sheet = RgbImage::from_pixel(columns * CELL, rows * CELL, Rgb([24, 24, 24]));
        for (slot, &i) in group.iter().enumerate() {
            let index = kept[i];
            let Some(thumbnail) = &thumbnails[index] else {
                continue;
            };
            let fitted = thumbnail.resize(CELL, CELL, FilterType::Triangle).to_rgb8();
            let (column, row) = (slot as u32 % COLUMNS, slot as u32 / COLUMNS);
            let x = column * CELL + (CELL - fitted.width()) / 2;
            let y = row * CELL + (CELL - fitted.height()) / 2;
            imageops::replace(&mut sheet, &fitted, i64::from(x), i64::from(y));
        }
        sheet.save(out.join(format!("group-{:02}.jpg", number + 1)))?;
    }
    Ok(())
}
