//! `corrode group`: the shots of a directory grouped by look, to sort a
//! selection into subfolders. By default nothing is moved: the groups are
//! listed, and a contact sheet of each can be written to check them by
//! eye. With `--move`, each group of two shots or more goes to its own
//! subfolder (`bleu-IMG_0042`…), with the RAW, the JPEG and the sidecars;
//! shots alone stay.

use std::fs;
use std::path::{Path, PathBuf};
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

#[derive(clap::Args)]
pub struct Args {
    /// The directory of shots (the selection folder)
    dir: PathBuf,
    /// Distance under which two groups are merged, 0 to 1: lower gives more,
    /// smaller groups
    #[arg(long, default_value_t = Settings::default().threshold)]
    threshold: f32,
    /// How much shots taken close together are brought nearer, 0 to 1; 0
    /// ignores the time taken
    #[arg(long, default_value_t = Settings::default().time_weight, value_parser = unit_interval)]
    time: f32,
    /// The gap in seconds from which the time taken does not count any more
    #[arg(long, default_value_t = Settings::default().time_scale_s, value_parser = clap::value_parser!(i64).range(1..))]
    scale: i64,
    /// How much colour, layout and tint count in the look, `colour,layout,tint`
    #[arg(long, value_parser = parse_weights)]
    weights: Option<Weights>,
    /// Write a contact sheet of each group to this directory, to check them
    #[arg(long)]
    sheets: Option<PathBuf>,
    /// Sort the shots into one subfolder per group
    #[arg(long = "move")]
    sort: bool,
}

fn unit_interval(text: &str) -> Result<f32, String> {
    match text.parse::<f32>() {
        Ok(value) if (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err("a number between 0 and 1".to_owned()),
    }
}

fn parse_weights(text: &str) -> Result<Weights, String> {
    let parsed: Option<Vec<f32>> = text.split(',').map(|part| part.parse().ok()).collect();
    match parsed.as_deref() {
        Some(&[colour, layout, tint]) if colour + layout + tint > 0.0 => Ok(Weights {
            colour,
            layout,
            tint,
        }),
        _ => Err("colour,layout,tint, 1,1,0 for instance".to_owned()),
    }
}

pub fn run(args: Args) -> Result<(), String> {
    let dir = &args.dir;
    let shots = pairing::scan_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let weights = args.weights.unwrap_or_default();

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
        threshold: args.threshold,
        time_weight: args.time,
        time_scale_s: args.scale,
        weights,
    };
    let groups = similarity::group(&signatures, &times, &settings);
    let clustered = start.elapsed();

    let singles = groups.iter().filter(|group| group.len() == 1).count();
    println!(
        "{}: {} shots, {} compared, {} groups at {}, time {} over {} s, weights {}/{}/{} ({singles} alone)",
        dir.display(),
        shots.len(),
        kept.len(),
        groups.len(),
        args.threshold,
        args.time,
        args.scale,
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

    if let Some(sheets) = &args.sheets {
        write_sheets(sheets, &groups, &kept, &thumbnails)
            .map_err(|err| format!("{}: {err}", sheets.display()))?;
        println!("contact sheets in {}", sheets.display());
    }
    if args.sort {
        let batches: Vec<Batch> = groups
            .iter()
            .map(|group| Batch {
                label: similarity::colour_of(&signatures, group).to_owned(),
                members: group.iter().map(|&i| kept[i]).collect(),
            })
            .collect();
        let sorting = selection::sort_into_groups(dir, &shots, &batches);
        let count: usize = sorting.sorted.iter().map(|folder| folder.shots.len()).sum();
        println!(
            "{count} shots moved into {} folders of {}",
            sorting.sorted.len(),
            dir.display()
        );
        if let Some(err) = sorting.error {
            return Err(format!("sorting stopped: {err}"));
        }
    }
    Ok(())
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
