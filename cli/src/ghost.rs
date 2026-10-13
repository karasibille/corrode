//! `corrode ghost`: the frames of a burst laid over one another, with the
//! subject kept in the same place, so that each pose leaves a ghost round
//! it. Experimental: the options may still change.

use std::path::PathBuf;

use clap::ValueEnum;
use corrode_core::{exif, pairing, picture};
use corrode_effects::{Ghost, GhostBlend, ghost, register, shifted, track};
use image::RgbImage;
use image::imageops::FilterType;

/// Where the subject of each frame is found.
#[derive(Clone, Copy, ValueEnum)]
enum Align {
    /// At the point the camera focused on if every file has one, else at `--centre`
    Auto,
    /// At the point the camera focused on, read in the file
    Af,
    /// At `--centre` in every frame: the camera and the subject stay where they are
    Fixed,
    /// Where the picture changes, found in the frames themselves: for a subject
    /// that crosses the frame and a camera that does not move; at least three frames
    Track,
}

#[derive(clap::Args)]
pub struct Args {
    /// The frames, in any order: at least two; the shots a JPEG and a RAW make up count once
    #[arg(required = true, num_args = 2..)]
    frames: Vec<PathBuf>,
    /// Where the JPEG result is written
    #[arg(short, long)]
    output: PathBuf,
    /// How the frames are laid over the main one: `lighten` (the poses stay
    /// sharp), `screen`, `average` (every pose faint) or `over` (see-through
    /// layers, every ghost weighing `--opacity` when the main frame weighs 1)
    #[arg(long, default_value_t = GhostBlend::default(), value_parser = |text: &str| text.parse::<GhostBlend>())]
    blend: GhostBlend,
    /// How much each ghost shows, 0 to 1; the main frame is whole
    #[arg(long, default_value_t = 1.0, value_parser = unit_interval)]
    opacity: f32,
    /// How much the picture is cut round the subject: the result is the
    /// frame divided by this
    #[arg(long, default_value_t = 2.0, value_parser = at_least_one)]
    zoom: f32,
    /// Where the subject is in each frame
    #[arg(long, value_enum, default_value = "auto")]
    align: Align,
    /// Where the subject is for `fixed`, `x,y` in percent of the width and
    /// the height of the frame
    #[arg(long, default_value = "50,50", value_parser = parse_centre)]
    centre: (f32, f32),
    /// The main frame, shown whole, counting from 1 in the order given;
    /// the middle one by default
    #[arg(long)]
    main: Option<usize>,
    /// Keep the background of the main frame: of the other frames, only what
    /// changes against the middle value of each pixel, the subject, is laid
    /// over it, so that a lamp or the floor is not repeated where the windows
    /// moved; at least three frames
    #[arg(long)]
    pin_background: bool,
    /// Line the background of the frames up on the main one first, the
    /// camera having moved a little between two shots: a lamp or the floor
    /// is then seen once, not once for each frame; slow on large pictures
    #[arg(long)]
    register: bool,
    /// Use the full-size picture, JPEG or developed RAW, instead of the
    /// preview of the file, which is smaller and better looking
    #[arg(long)]
    full: bool,
    /// Width of the result in pixels, smaller than the picture
    #[arg(long)]
    width: Option<u32>,
}

fn unit_interval(text: &str) -> Result<f32, String> {
    match text.parse::<f32>() {
        Ok(value) if (0.0..=1.0).contains(&value) => Ok(value),
        _ => Err("a number between 0 and 1".to_owned()),
    }
}

fn parse_centre(text: &str) -> Result<(f32, f32), String> {
    let parsed = text
        .split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?)))
        .filter(|(x, y)| (0.0..=100.0).contains(x) && (0.0..=100.0).contains(y));
    parsed.ok_or_else(|| "x,y in percent, 0 to 100 each".to_owned())
}

fn at_least_one(text: &str) -> Result<f32, String> {
    match text.parse::<f32>() {
        Ok(value) if value >= 1.0 => Ok(value),
        _ => Err("a number of 1 or more".to_owned()),
    }
}

pub fn run(args: Args) -> Result<(), String> {
    let shots = pairing::shots_of(&args.frames).map_err(|err| err.to_string())?;
    let mut frames: Vec<RgbImage> = Vec::with_capacity(shots.len());
    let mut centres = Vec::with_capacity(shots.len());
    let mut from_focus = Vec::with_capacity(shots.len());
    for shot in &shots {
        let name = shot.stem.to_string_lossy();
        let picture = if args.full {
            picture::full(shot)
        } else {
            picture::preview(shot)
        }
        .map_err(|err| err.to_string())?;
        let (width, height) = (picture.image.width() as f32, picture.image.height() as f32);
        let fixed = (
            width * args.centre.0 / 100.0,
            height * args.centre.1 / 100.0,
        );
        let focus = exif::read(shot)
            .ok()
            .and_then(|exif| exif.focus_point)
            .map(|point| (point.0 * width, point.1 * height));
        let centre = match args.align {
            Align::Fixed => fixed,
            Align::Auto => focus.unwrap_or(fixed),
            // Found once all the frames are known; `--centre` stands in
            // where the subject cannot be told.
            Align::Track => fixed,
            Align::Af => focus
                .ok_or_else(|| format!("{name}: no focus point in the file, try --align fixed"))?,
        };
        if !matches!(args.align, Align::Track) {
            println!("{name}: subject at {:.0},{:.0}", centre.0, centre.1);
        }
        centres.push(centre);
        from_focus.push(
            matches!(args.align, Align::Af)
                || (matches!(args.align, Align::Auto) && focus.is_some()),
        );
        frames.push(picture.image.to_rgb8());
    }
    let main = match args.main {
        Some(number) if (1..=frames.len()).contains(&number) => number - 1,
        Some(number) => {
            return Err(format!(
                "--main {number}: there are {} frames",
                frames.len()
            ));
        }
        None => frames.len() / 2,
    };
    if args.register {
        let found = register(&frames, main).map_err(|err| err.to_string())?;
        for (i, shift) in found.into_iter().enumerate() {
            if i != main {
                let name = shots[i].stem.to_string_lossy();
                println!("{name}: background moved by {},{}", shift.0, shift.1);
                frames[i] = shifted(&frames[i], shift);
                // Its focus point moved with the picture.
                if from_focus[i] {
                    centres[i] = (centres[i].0 - shift.0 as f32, centres[i].1 - shift.1 as f32);
                }
            }
        }
    }
    if let Align::Track = args.align {
        let mut last = None;
        let found = track(&frames).map_err(|err| err.to_string())?;
        for (i, found) in found.into_iter().enumerate() {
            if let Some(centre) = found.or(last) {
                centres[i] = centre;
                last = Some(centre);
            }
            let name = shots[i].stem.to_string_lossy();
            match found {
                Some(c) => println!("{name}: tracked at {:.0},{:.0}", c.0, c.1),
                None => println!(
                    "{name}: no subject found, kept at {:.0},{:.0}",
                    centres[i].0, centres[i].1
                ),
            }
        }
    }
    let params = Ghost {
        blend: args.blend,
        opacity: args.opacity,
        zoom: args.zoom,
        pin_background: args.pin_background,
    };
    let mut result = ghost(&frames, &centres, main, params).map_err(|err| err.to_string())?;
    if let Some(width) = args.width.filter(|&width| width < result.width()) {
        let height =
            (u64::from(result.height()) * u64::from(width) / u64::from(result.width())) as u32;
        result = image::imageops::resize(&result, width, height.max(1), FilterType::Triangle);
    }
    if let Some(dir) = args.output.parent() {
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    }
    result
        .save(&args.output)
        .map_err(|err| format!("{}: {err}", args.output.display()))?;
    println!(
        "{} frames, main {}, {} at {} -> {}×{} in {}",
        frames.len(),
        main + 1,
        args.blend,
        args.opacity,
        result.width(),
        result.height(),
        args.output.display()
    );
    Ok(())
}
