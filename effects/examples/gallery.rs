//! Makes the gallery of the README again from `docs/examples/recipes.txt`
//! and from what the effects say of their parameters, so that the options
//! shown under a picture are always the ones the effect has.
//!
//! ```sh
//! cargo run -p corrode-effects --example gallery
//! ```
//!
//! Each line of `recipes.txt` is `name | title | recipe`; the picture is
//! `docs/examples/<name>.jpg` (see `generate.sh`). The table goes between
//! the markers `<!-- gallery:start -->` and `<!-- gallery:end -->`.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use corrode_effects::describe;

const START: &str = "<!-- gallery:start -->";
const END: &str = "<!-- gallery:end -->";
const COLUMNS: usize = 3;

/// Text for HTML, with `code` between backticks.
fn html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut out = String::new();
    for (i, part) in escaped.split('`').enumerate() {
        if i > 0 {
            out.push_str(if i % 2 == 1 { "<code>" } else { "</code>" });
        }
        out.push_str(part);
    }
    out
}

/// The fold-out list of the options of the effect a recipe starts with.
fn options(recipe: &str) -> Result<String, String> {
    let body = if recipe.contains('+') {
        "effects joined by <code>+</code> run in order, each with its own options<br>".to_owned()
    } else {
        let effect = recipe.split(' ').next().unwrap_or_default();
        let params = describe(effect).map_err(|err| format!("{recipe}: {err}"))?;
        params
            .iter()
            .map(|p| format!("<code>{}={}</code> {}<br>", p.name, p.default, html(p.help)))
            .collect()
    };
    Ok(format!(
        "<details><summary>options</summary><div align=\"left\">{body}</div></details>"
    ))
}

fn run() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no workspace root")?;
    let recipes = fs::read_to_string(root.join("docs/examples/recipes.txt"))
        .map_err(|err| format!("recipes.txt: {err}"))?;
    let mut cells = Vec::new();
    for line in recipes
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let parts: Vec<&str> = line.splitn(3, '|').map(str::trim).collect();
        let [name, title, recipe] = parts[..] else {
            return Err(format!("not `name | title | recipe`: {line}"));
        };
        cells.push(format!(
            "<td width=\"33%\" align=\"center\" valign=\"top\"><img src=\"docs/examples/{name}.jpg\" width=\"100%\"><br><b>{}</b><br><code>{}</code>{}</td>",
            html(title),
            html(recipe),
            options(recipe)?
        ));
    }
    let rows: Vec<String> = cells
        .chunks(COLUMNS)
        .map(|row| {
            let mut tds = row.concat();
            tds.push_str(&"<td width=\"33%\"></td>".repeat(COLUMNS - row.len()));
            format!("<tr>{tds}</tr>")
        })
        .collect();
    let table = format!(
        "{START}\n<table width=\"100%\">\n{}\n</table>\n{END}",
        rows.join("\n")
    );

    let path = root.join("README.md");
    let readme = fs::read_to_string(&path).map_err(|err| format!("README.md: {err}"))?;
    let start = readme
        .find(START)
        .ok_or("no gallery:start marker in the README")?;
    let end = readme
        .find(END)
        .ok_or("no gallery:end marker in the README")?
        + END.len();
    let new = format!("{}{table}{}", &readme[..start], &readme[end..]);
    if new != readme {
        fs::write(&path, new).map_err(|err| format!("README.md: {err}"))?;
        println!("gallery of the README made again: {} pictures", cells.len());
    } else {
        println!(
            "gallery of the README is up to date: {} pictures",
            cells.len()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
