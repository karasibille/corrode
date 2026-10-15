//! The recipe file: reading it, noticing that it changed, and what to
//! keep when what was saved is not a recipe.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use corrode_effects::Recipe;

/// How often the file is read again.
const POLL: Duration = Duration::from_millis(100);

/// What the text of the file says.
#[derive(Debug, PartialEq)]
pub enum Reading {
    /// Nothing but blanks and comments: an editor saving, or a recipe not
    /// begun, not a mistake to show.
    Empty,
    /// Not a recipe, with what is wrong.
    Invalid(String),
    /// A recipe, with its words, which are read again at each moment of
    /// an animation.
    Valid(Box<Recipe>, Vec<String>),
}

/// The recipe of `words` at a moment of the animation (0 to 1) and a step
/// of the seed, which is added to it when the seed moves on at each frame.
pub fn recipe_at(words: &[String], moment: f64, step: u64) -> Result<Recipe, String> {
    let mut recipe = Recipe::from_words_at(words, moment).map_err(|err| err.to_string())?;
    recipe.seed = recipe.seed.wrapping_add(step);
    Ok(recipe)
}

/// The text without its comments. A comment runs from a `#` to the end
/// of the line, the `#` being the first thing on the line or after a
/// blank, and outside quotes: `color=#ff963c` is a colour, not a comment.
pub fn without_comments(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut quoted = false;
            let mut before = ' ';
            for (at, c) in line.char_indices() {
                match c {
                    '"' => quoted = !quoted,
                    '#' if !quoted && before.is_whitespace() => return &line[..at],
                    _ => {}
                }
                before = c;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads the text of a recipe file.
pub fn read(text: &str) -> Reading {
    let words: Vec<String> = without_comments(text)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return Reading::Empty;
    }
    match Recipe::from_words(&words) {
        Ok(recipe) => Reading::Valid(Box::new(recipe), words),
        Err(err) => Reading::Invalid(err.to_string()),
    }
}

/// Notices when the content of a file changes. It compares the content,
/// not the date: editors often save by writing another file over the
/// first, and a date can be a second wide.
pub struct Watcher {
    path: PathBuf,
    last: Option<String>,
    checked: Option<Instant>,
}

impl Watcher {
    pub fn new(path: PathBuf) -> Watcher {
        Watcher {
            path,
            last: None,
            checked: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The new text when the file changed since the last time, if it is
    /// time to look again; a file that cannot be read for a moment (being
    /// replaced) is left for the next time.
    pub fn poll(&mut self) -> Option<String> {
        if self.checked.is_some_and(|at| at.elapsed() < POLL) {
            return None;
        }
        self.checked = Some(Instant::now());
        self.changed()
    }

    /// The same, without waiting for the time to look again.
    fn changed(&mut self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        (self.last.as_deref() != Some(text.as_str())).then(|| {
            self.last = Some(text.clone());
            text
        })
    }
}

/// Where a result is saved next to its source: `photo-live.jpg`, then
/// `photo-live-2.jpg`… never a file that exists.
pub fn output_path(source: &Path, extension: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "picture".to_owned());
    let dir = source.parent().unwrap_or(Path::new(""));
    (1..)
        .map(|n| match n {
            1 => dir.join(format!("{stem}-live.{extension}")),
            n => dir.join(format!("{stem}-live-{n}.{extension}")),
        })
        .find(|path| path != source && !exists(path))
        .expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_blanks_are_not_a_recipe() {
        assert_eq!(without_comments("a # b\n# c\nd"), "a \n\nd");
        assert_eq!(read(""), Reading::Empty);
        assert_eq!(read("  \n# nothing yet\n"), Reading::Empty);
    }

    #[test]
    fn a_hash_starts_a_comment_only_after_a_blank_and_not_in_a_colour() {
        assert_eq!(
            without_comments("leak color=#ff963c # warm"),
            "leak color=#ff963c "
        );
        assert_eq!(
            without_comments("#only\nfade#not a comment"),
            "\nfade#not a comment"
        );
        assert!(matches!(
            read("seed=1 leak color=#ff963c # warm"),
            Reading::Valid(..)
        ));
        assert!(matches!(
            read("seed=1 duotone dark=#102 light=#fc8"),
            Reading::Valid(..)
        ));
    }

    #[test]
    fn a_recipe_reads_over_several_lines_with_comments() {
        let text = "# warm\nseed=7\nfade   # lift the blacks\n+ grain strength=40\n";
        let Reading::Valid(recipe, words) = read(text) else {
            panic!("not valid");
        };
        assert_eq!(recipe.seed, 7);
        assert_eq!(recipe.steps.len(), 2);
        assert_eq!(words, ["seed=7", "fade", "+", "grain", "strength=40"]);
    }

    #[test]
    fn a_recipe_at_a_moment_takes_its_ranges_and_seed_step() {
        let words: Vec<String> = ["seed=5", "fade", "lift=0..40"].map(str::to_owned).to_vec();
        let (start, end) = (
            recipe_at(&words, 0.0, 0).unwrap(),
            recipe_at(&words, 1.0, 3).unwrap(),
        );
        assert_eq!((start.seed, end.seed), (5, 8));
        assert_ne!(
            start.to_string().replace("seed=5", ""),
            end.to_string().replace("seed=8", "")
        );
        assert!(recipe_at(&["nonsense".to_owned()], 0.0, 0).is_err());
    }

    #[test]
    fn a_mistake_says_what_is_wrong() {
        let Reading::Invalid(why) = read("seed=7 fade lift=oops") else {
            panic!("valid");
        };
        assert!(why.contains("lift"), "{why}");
        assert!(matches!(read("nonsense"), Reading::Invalid(_)));
    }

    #[test]
    fn the_watcher_sees_a_change_of_content_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recipe.cor");
        let mut watcher = Watcher::new(path.clone());
        assert_eq!(watcher.changed(), None, "no file yet");
        fs::write(&path, "fade").unwrap();
        assert_eq!(watcher.changed().as_deref(), Some("fade"));
        assert_eq!(watcher.changed(), None, "same content");
        // Saved by writing another file over it.
        let other = dir.path().join("recipe.cor.tmp");
        fs::write(&other, "grain").unwrap();
        fs::rename(&other, &path).unwrap();
        assert_eq!(watcher.changed().as_deref(), Some("grain"));
        fs::remove_file(&path).unwrap();
        assert_eq!(
            watcher.changed(),
            None,
            "a file being replaced is waited for"
        );
    }

    #[test]
    fn results_never_overwrite_the_source_or_an_earlier_result() {
        let source = Path::new("shots/photo.jpg");
        let taken = |paths: &'static [&'static str]| {
            move |path: &Path| paths.iter().any(|taken| Path::new(taken) == path)
        };
        assert_eq!(
            output_path(source, "jpg", taken(&[])),
            Path::new("shots/photo-live.jpg")
        );
        assert_eq!(
            output_path(
                source,
                "jpg",
                taken(&["shots/photo-live.jpg", "shots/photo-live-2.jpg"])
            ),
            Path::new("shots/photo-live-3.jpg")
        );
    }
}
