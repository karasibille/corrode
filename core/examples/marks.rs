//! Shows or changes the marks of shots, as stored in RawTherapee sidecars.
//!
//! Each file is an image (JPEG or RAW) and stands for its whole shot: the
//! marks go to the RAW's sidecar if it exists, else to the JPEG's. A
//! missing sidecar is created from RawTherapee's default profile, read from
//! its settings. `set` only changes the marks given as options.
//!
//! ```sh
//! cargo run -p corrode-core --example marks -- show /tmp/corrode-test/*.RW2
//! cargo run -p corrode-core --example marks -- set --rank 4 --color green /tmp/corrode-test/P1011259.RW2
//! cargo run -p corrode-core --example marks -- set --trash /tmp/corrode-test/P1011260.JPG
//! ```

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use corrode_core::marks::{ColorLabel, MAX_RANK, Marks};
use corrode_core::pairing::{self, Shot};
use corrode_core::rawtherapee::{self, Config};

const USAGE: &str = "\
usage: marks show <image>...
       marks set [--rank 0-5] [--color none|red|yellow|green|blue|purple]
                 [--trash | --keep] <image>...";

#[derive(Default)]
struct Changes {
    rank: Option<u8>,
    color: Option<ColorLabel>,
    in_trash: Option<bool>,
}

impl Changes {
    fn apply(&self, marks: &mut Marks) {
        if let Some(rank) = self.rank {
            marks.rank = rank;
        }
        if let Some(color) = self.color {
            marks.color = color;
        }
        if let Some(in_trash) = self.in_trash {
            marks.in_trash = in_trash;
        }
    }
}

fn parse_color(name: &str) -> Option<ColorLabel> {
    match name {
        "none" => Some(ColorLabel::None),
        "red" => Some(ColorLabel::Red),
        "yellow" => Some(ColorLabel::Yellow),
        "green" => Some(ColorLabel::Green),
        "blue" => Some(ColorLabel::Blue),
        "purple" => Some(ColorLabel::Purple),
        _ => None,
    }
}

fn describe(marks: &Marks) -> String {
    let stars = "★".repeat(marks.rank.into()) + &"☆".repeat((MAX_RANK - marks.rank).into());
    let trash = if marks.in_trash { "  rejected" } else { "" };
    format!("{stars}  {:?}{trash}", marks.color)
}

/// Finds the shot of each image by scanning its directory, so that the
/// JPEG and the RAW of a pair are handled together.
fn shots(images: &[PathBuf]) -> Result<Vec<Shot>, String> {
    let mut by_dir: BTreeMap<PathBuf, Vec<Shot>> = BTreeMap::new();
    let mut found = Vec::new();
    for image in images {
        let dir = match image.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if !by_dir.contains_key(&dir) {
            let shots =
                pairing::scan_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            by_dir.insert(dir.clone(), shots);
        }
        let stem = image.file_stem().unwrap_or_default();
        let shot = by_dir[&dir]
            .iter()
            .find(|shot| shot.stem == stem)
            .ok_or_else(|| format!("{}: not a JPEG or RAW image", image.display()))?;
        if !found.contains(shot) {
            found.push(shot.clone());
        }
    }
    Ok(found)
}

fn report(shot: &Shot, result: Result<Marks, rawtherapee::Error>) -> bool {
    let sidecar = rawtherapee::sidecar(shot).path;
    let shown = if sidecar.exists() {
        sidecar.as_path()
    } else {
        Path::new("(no sidecar)")
    };
    match result {
        Ok(marks) => {
            println!("{}  {}", describe(&marks), shown.display());
            true
        }
        Err(err) => {
            eprintln!("{}: {err}", shot.stem.to_string_lossy());
            false
        }
    }
}

fn run(args: Vec<String>) -> Result<bool, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("missing command")?;
    let mut changes = Changes::default();
    let mut images = Vec::new();

    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--rank" => {
                let rank = value("--rank")?;
                changes.rank = Some(
                    rank.parse()
                        .ok()
                        .filter(|rank| *rank <= MAX_RANK)
                        .ok_or(format!("invalid rank: {rank}"))?,
                );
            }
            "--color" => {
                let color = value("--color")?;
                changes.color = Some(parse_color(&color).ok_or(format!("invalid color: {color}"))?);
            }
            "--trash" => changes.in_trash = Some(true),
            "--keep" => changes.in_trash = Some(false),
            _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}")),
            _ => images.push(PathBuf::from(arg)),
        }
    }
    if images.is_empty() {
        return Err("no image given".to_owned());
    }
    let shots = shots(&images)?;

    let mut ok = true;
    match command.as_str() {
        "show" => {
            for shot in &shots {
                ok &= report(shot, rawtherapee::read_marks(shot));
            }
        }
        "set" => {
            let config = Config::load().map_err(|err| err.to_string())?;
            for shot in &shots {
                let result = rawtherapee::read_marks(shot).and_then(|mut marks| {
                    changes.apply(&mut marks);
                    config.write_marks(shot, &marks).map(|()| marks)
                });
                ok &= report(shot, result);
            }
        }
        _ => return Err(format!("unknown command: {command}")),
    }
    Ok(ok)
}

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
