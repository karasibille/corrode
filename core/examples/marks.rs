//! Shows or changes the marks stored in RawTherapee sidecars.
//!
//! Each file can be an image (its `.pp3` sidecar is used) or a `.pp3`.
//! `set` only changes the marks given as options and keeps the others.
//!
//! ```sh
//! cargo run -p corrode-core --example marks -- show /tmp/corrode-test/*.RW2
//! cargo run -p corrode-core --example marks -- set --rank 4 --color green /tmp/corrode-test/P1011259.RW2
//! cargo run -p corrode-core --example marks -- set --trash --base default.pp3 /tmp/corrode-test/P1011260.RW2
//! ```
//!
//! Without `--base`, `set` refuses to create a missing sidecar: an empty
//! one would make RawTherapee render the photo from neutral values.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use corrode_core::pp3::{self, ColorLabel, Marks, Profile};

const USAGE: &str = "\
usage: marks show <file>...
       marks set [--rank 0-5] [--color none|red|yellow|green|blue|purple]
                 [--trash | --keep] [--base <default.pp3>] <file>...";

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

fn sidecar(file: &Path) -> PathBuf {
    if file.extension().is_some_and(|ext| ext == "pp3") {
        file.to_path_buf()
    } else {
        pp3::sidecar_path(file)
    }
}

fn describe(marks: &Marks) -> String {
    let stars = "★".repeat(marks.rank.into()) + &"☆".repeat((pp3::MAX_RANK - marks.rank).into());
    let trash = if marks.in_trash { "  rejected" } else { "" };
    format!("{stars}  {:?}{trash}", marks.color)
}

fn show(files: &[PathBuf]) -> bool {
    let mut ok = true;
    for file in files {
        let path = sidecar(file);
        match Profile::load(&path).and_then(|profile| profile.marks()) {
            Ok(marks) => println!("{}  {}", describe(&marks), path.display()),
            Err(pp3::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("(no sidecar)  {}", path.display());
            }
            Err(err) => {
                eprintln!("{}: {err}", path.display());
                ok = false;
            }
        }
    }
    ok
}

fn set(changes: &Changes, base: Option<&Profile>, files: &[PathBuf]) -> bool {
    let mut ok = true;
    for file in files {
        let path = sidecar(file);
        let result = match (Profile::load(&path), base) {
            (Ok(profile), _) => profile.marks().and_then(|mut marks| {
                changes.apply(&mut marks);
                pp3::write_marks(&path, &marks, &profile).map(|()| marks)
            }),
            (Err(pp3::Error::Io(err)), Some(base))
                if err.kind() == std::io::ErrorKind::NotFound =>
            {
                let mut marks = Marks::default();
                changes.apply(&mut marks);
                pp3::write_marks(&path, &marks, base).map(|()| marks)
            }
            (Err(pp3::Error::Io(err)), None) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("{}: no sidecar, pass --base to create it", path.display());
                ok = false;
                continue;
            }
            (Err(err), _) => Err(err),
        };
        match result {
            Ok(marks) => println!("{}  {}", describe(&marks), path.display()),
            Err(err) => {
                eprintln!("{}: {err}", path.display());
                ok = false;
            }
        }
    }
    ok
}

fn run(args: Vec<String>) -> Result<bool, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("missing command")?;
    let mut changes = Changes::default();
    let mut base = None;
    let mut files = Vec::new();

    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--rank" => {
                let rank = value("--rank")?;
                changes.rank = Some(
                    rank.parse()
                        .ok()
                        .filter(|rank| *rank <= pp3::MAX_RANK)
                        .ok_or(format!("invalid rank: {rank}"))?,
                );
            }
            "--color" => {
                let color = value("--color")?;
                changes.color = Some(parse_color(&color).ok_or(format!("invalid color: {color}"))?);
            }
            "--trash" => changes.in_trash = Some(true),
            "--keep" => changes.in_trash = Some(false),
            "--base" => {
                let path = PathBuf::from(value("--base")?);
                let profile =
                    Profile::load(&path).map_err(|err| format!("{}: {err}", path.display()))?;
                base = Some(profile);
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if files.is_empty() {
        return Err("no file given".to_owned());
    }

    match command.as_str() {
        "show" => Ok(show(&files)),
        "set" => Ok(set(&changes, base.as_ref(), &files)),
        _ => Err(format!("unknown command: {command}")),
    }
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
