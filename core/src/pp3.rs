//! RawTherapee processing profiles (`.pp3` sidecar files).
//!
//! A `.pp3` is an INI-like key file written next to the image
//! (`DSCF1234.RAF.pp3`). corrode reads and writes the marks of the
//! `[General]` section (`Rank`, `ColorLabel`, `InTrash`), and can set a
//! key or replace a whole section, for presets; every other line is kept
//! byte for byte, so RawTherapee's settings are never altered by accident.

use std::fmt;
use std::fs::{self, Permissions};
use std::io::{self, Write};
use std::ops::Range;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::marks::{ColorLabel, MAX_RANK, Marks};

const GENERAL: &str = "General";
const RANK: &str = "Rank";
const COLOR_LABEL: &str = "ColorLabel";
const IN_TRASH: &str = "InTrash";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The file is not text. Usually a corrupted file: it must not be
    /// overwritten, since it may still be recoverable.
    #[error("not a text file, probably corrupted")]
    NotText,
    /// A line is neither a section header, a `key=value` pair, a comment
    /// nor blank (1-based line number).
    #[error("malformed line {line}")]
    Malformed { line: usize },
    /// A mark has a value RawTherapee would not write.
    #[error("invalid {key} value: {value:?}")]
    InvalidMark { key: &'static str, value: String },
}

/// Path of the sidecar of an image: the full file name plus `.pp3`.
pub fn sidecar_path(image: &Path) -> PathBuf {
    let mut path = image.as_os_str().to_os_string();
    path.push(".pp3");
    PathBuf::from(path)
}

/// A parsed `.pp3`, kept line by line (line endings included) so that
/// writing it back reproduces the original bytes. The default value is an
/// empty profile, which RawTherapee reads as its neutral settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    lines: Vec<String>,
}

enum Line<'a> {
    /// Blank line or `#` comment.
    Other,
    Section(&'a str),
    Entry {
        key: &'a str,
        value: &'a str,
    },
}

fn classify(line: &str) -> Option<Line<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Some(Line::Other);
    }
    if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        return Some(Line::Section(name));
    }
    let (key, value) = line.split_once('=')?;
    Some(Line::Entry {
        key: key.trim(),
        value: value.trim(),
    })
}

impl Profile {
    /// Parses the text of a `.pp3`, rejecting anything that does not look
    /// like a key file.
    pub fn parse(text: &str) -> Result<Profile, Error> {
        if text.contains('\0') {
            return Err(Error::NotText);
        }
        let lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();
        let mut in_section = false;
        for (index, line) in lines.iter().enumerate() {
            match classify(line) {
                Some(Line::Other) => {}
                Some(Line::Section(_)) => in_section = true,
                Some(Line::Entry { .. }) if in_section => {}
                _ => return Err(Error::Malformed { line: index + 1 }),
            }
        }
        Ok(Profile { lines })
    }

    /// Reads and parses a `.pp3` file.
    pub fn load(path: &Path) -> Result<Profile, Error> {
        let bytes = fs::read(path)?;
        let text = String::from_utf8(bytes).map_err(|_| Error::NotText)?;
        Profile::parse(&text)
    }

    /// Iterates over the `(line index, key, value)` of the entries of a section.
    fn entries<'a>(&'a self, wanted: &str) -> impl Iterator<Item = (usize, &'a str, &'a str)> {
        let mut section = "";
        self.lines
            .iter()
            .enumerate()
            .filter_map(move |(index, line)| match classify(line)? {
                Line::Section(name) => {
                    section = name;
                    None
                }
                Line::Entry { key, value } if section == wanted => Some((index, key, value)),
                _ => None,
            })
    }

    /// The `(key, value)` pairs of a section, in file order.
    pub fn section_entries(&self, section: &str) -> Vec<(&str, &str)> {
        self.entries(section)
            .map(|(_, key, value)| (key, value))
            .collect()
    }

    /// The names of the sections, in file order, each once.
    pub fn section_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for line in &self.lines {
            if let Some(Line::Section(name)) = classify(line)
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }

    /// The line ranges of the sections called `wanted`: from the header to
    /// the line before the next header, blank lines after it included.
    fn section_ranges(&self, wanted: &str) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = None;
        for (index, line) in self.lines.iter().enumerate() {
            if let Some(Line::Section(name)) = classify(line) {
                if let Some(start) = start.take() {
                    ranges.push(start..index);
                }
                if name == wanted {
                    start = Some(index);
                }
            }
        }
        if let Some(start) = start {
            ranges.push(start..self.lines.len());
        }
        ranges
    }

    fn general_entries(&self) -> impl Iterator<Item = (usize, &str, &str)> {
        self.entries(GENERAL)
    }

    /// Value of a key, the last one winning if the key is repeated.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.entries(section)
            .filter(|(_, k, _)| *k == key)
            .map(|(_, _, value)| value)
            .last()
    }

    /// The marks stored in the profile; missing keys get their default.
    pub fn marks(&self) -> Result<Marks, Error> {
        let mut marks = Marks::default();
        for (_, key, value) in self.general_entries() {
            let invalid = |key| Error::InvalidMark {
                key,
                value: value.to_owned(),
            };
            match key {
                RANK => {
                    marks.rank = value
                        .parse()
                        .ok()
                        .filter(|rank| *rank <= MAX_RANK)
                        .ok_or_else(|| invalid(RANK))?;
                }
                COLOR_LABEL => {
                    marks.color = value
                        .parse()
                        .ok()
                        .and_then(ColorLabel::from_number)
                        .ok_or_else(|| invalid(COLOR_LABEL))?;
                }
                IN_TRASH => {
                    marks.in_trash = match value {
                        "true" | "1" => true,
                        "false" | "0" => false,
                        _ => return Err(invalid(IN_TRASH)),
                    };
                }
                _ => {}
            }
        }
        Ok(marks)
    }

    /// Writes the marks, touching only their lines. Missing keys are added
    /// at the end of `[General]` (created if needed), except `Rank` when it
    /// is 0, which RawTherapee leaves out.
    ///
    /// # Panics
    ///
    /// If `marks.rank` is greater than [`MAX_RANK`].
    pub fn set_marks(&mut self, marks: &Marks) {
        assert!(marks.rank <= MAX_RANK, "rank out of range: {}", marks.rank);
        self.set(RANK, &marks.rank.to_string(), marks.rank > 0);
        self.set(COLOR_LABEL, &(marks.color as u8).to_string(), true);
        self.set(IN_TRASH, &marks.in_trash.to_string(), true);
    }

    /// Sets a key of a section, adding the key at the end of the section,
    /// and the section at the end of the file, if they are missing.
    pub fn set_in(&mut self, section: &str, key: &str, value: &str) {
        self.set_value(section, key, value, true);
    }

    fn set(&mut self, key: &str, value: &str, add_if_missing: bool) {
        self.set_value(GENERAL, key, value, add_if_missing);
    }

    fn set_value(&mut self, section: &str, key: &str, value: &str, add_if_missing: bool) {
        let indexes: Vec<usize> = self
            .entries(section)
            .filter(|(_, k, _)| *k == key)
            .map(|(index, _, _)| index)
            .collect();

        if indexes.is_empty() {
            if add_if_missing {
                let at = self.insertion_point(section);
                let line = format!("{key}={value}{}", self.newline());
                self.lines.insert(at, line);
            }
            return;
        }
        for index in indexes {
            let line = &mut self.lines[index];
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            *line = format!("{key}={value}{ending}");
        }
    }

    /// Index right after the last entry of a section, appending the
    /// section at the end of the file if there is none.
    fn insertion_point(&mut self, section: &str) -> usize {
        let header = self.lines.iter().position(
            |line| matches!(classify(line), Some(Line::Section(name)) if name == section),
        );
        let Some(header) = header else {
            let newline = self.newline();
            if let Some(last) = self.lines.last_mut() {
                if !last.ends_with('\n') {
                    last.push_str(newline);
                }
                self.lines.push(newline.to_owned());
            }
            self.lines.push(format!("[{section}]{newline}"));
            return self.lines.len();
        };

        let mut at = header + 1;
        for (index, line) in self.lines.iter().enumerate().skip(header + 1) {
            match classify(line) {
                Some(Line::Section(_)) => break,
                Some(Line::Entry { .. }) => at = index + 1,
                _ => {}
            }
        }
        at
    }

    /// Replaces a section by the one of `source`, which is taken whole, the
    /// keys the target had and the source has not included: they go back to
    /// RawTherapee's defaults. A section the source lacks is removed. The
    /// section stays where it was, or goes to the end if it was missing;
    /// the target's line endings are used.
    pub fn replace_section(&mut self, name: &str, source: &Profile) {
        let newline = self.newline();
        let new_lines: Vec<String> = source
            .section_ranges(name)
            .first()
            .map(|range| {
                source.lines[range.clone()]
                    .iter()
                    .map(|line| format!("{}{newline}", line.trim_end_matches(['\r', '\n'])))
                    .collect()
            })
            .unwrap_or_default();

        let ranges = self.section_ranges(name);
        let at = ranges.first().map(|range| range.start);
        for range in ranges.into_iter().rev() {
            self.lines.drain(range);
        }
        match at {
            Some(at) => {
                self.lines.splice(at..at, new_lines);
            }
            None => {
                if let Some(last) = self.lines.last_mut()
                    && !last.ends_with('\n')
                {
                    last.push_str(newline);
                }
                self.lines.extend(new_lines);
            }
        }
    }

    /// Line ending used by the file, `\n` by default.
    fn newline(&self) -> &'static str {
        match self.lines.first() {
            Some(line) if line.ends_with("\r\n") => "\r\n",
            _ => "\n",
        }
    }

    /// Writes the profile atomically: a temporary file is written in the
    /// same directory then renamed, so a crash never leaves a truncated
    /// `.pp3`. An existing file keeps its permissions.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        let permissions = match fs::metadata(path) {
            Ok(metadata) => metadata.permissions(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Permissions::from_mode(0o644),
            Err(err) => return Err(err),
        };

        let mut file = tempfile::Builder::new()
            .prefix(".corrode-")
            .suffix(".pp3.tmp")
            .permissions(permissions)
            .tempfile_in(dir)?;
        file.write_all(self.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|err| err.error)?;
        Ok(())
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.lines.iter().try_for_each(|line| f.write_str(line))
    }
}

/// Writes the marks into the `.pp3` at `path`.
///
/// An existing file is updated in place and must be readable: a corrupted
/// file is reported and left untouched. A missing file is created from
/// `base`, which should be RawTherapee's default profile, so that the
/// photo keeps the rendering RawTherapee would give it.
pub fn write_marks(path: &Path, marks: &Marks, base: &Profile) -> Result<(), Error> {
    let mut profile = match Profile::load(path) {
        Ok(profile) => profile,
        Err(Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => base.clone(),
        Err(err) => return Err(err),
    };
    profile.set_marks(marks);
    profile.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shortened from a real sidecar written by RawTherapee 5.11.
    const RAWTHERAPEE_PP3: &str = "\
[Version]
AppVersion=5.11
Version=351

[General]
ColorLabel=0
InTrash=false

[Exposure]
Auto=false
Clip=0.02
Curve=1;0.0071174400000000001;0;0.090969999999999995;

[RAW]
CA=true
";

    /// Shortened from RawTherapee's default profile, which has no `[General]`.
    const DEFAULT_PROFILE: &str = "\
[Exposure]
Auto=false
HistogramMatching=true

[RAW]
CA=true
";

    fn parse(text: &str) -> Profile {
        Profile::parse(text).unwrap()
    }

    fn marks(rank: u8, color: ColorLabel, in_trash: bool) -> Marks {
        Marks {
            rank,
            color,
            in_trash,
        }
    }

    #[test]
    fn sidecar_path_appends_pp3_to_the_full_name() {
        assert_eq!(
            sidecar_path(Path::new("/photos/P1011259.RW2")),
            Path::new("/photos/P1011259.RW2.pp3")
        );
    }

    #[test]
    fn writing_back_unchanged_keeps_every_byte() {
        for text in [
            RAWTHERAPEE_PP3,
            DEFAULT_PROFILE,
            "",
            "[General]\r\nInTrash=false\r\n",
            "[General]\nInTrash=false",
            "# comment\n[A]\nkey = value with ; and = signs\n",
        ] {
            assert_eq!(parse(text).to_string(), text);
        }
    }

    #[test]
    fn reads_marks_and_defaults_missing_rank_to_zero() {
        assert_eq!(parse(RAWTHERAPEE_PP3).marks().unwrap(), Marks::default());
        let text = "[General]\nRank=4\nColorLabel=3\nInTrash=true\n";
        assert_eq!(
            parse(text).marks().unwrap(),
            marks(4, ColorLabel::Green, true)
        );
    }

    #[test]
    fn get_reads_a_key_of_any_section() {
        let profile = parse("[Profiles]\nRawDefault=${G}/Auto\n[Other]\nRawDefault=no\n");
        assert_eq!(profile.get("Profiles", "RawDefault"), Some("${G}/Auto"));
        assert_eq!(profile.get("Profiles", "ImgDefault"), None);
        assert_eq!(profile.get("Missing", "RawDefault"), None);
        assert_eq!(parse("[A]\nk=1\nk=2\n").get("A", "k"), Some("2"));
    }

    #[test]
    fn ignores_marks_outside_general() {
        let text = "[Other]\nRank=4\n[General]\nColorLabel=1\n";
        assert_eq!(
            parse(text).marks().unwrap(),
            marks(0, ColorLabel::Red, false)
        );
    }

    #[test]
    fn rejects_invalid_mark_values() {
        for (text, key) in [
            ("[General]\nRank=6\n", RANK),
            ("[General]\nRank=-1\n", RANK),
            ("[General]\nColorLabel=9\n", COLOR_LABEL),
            ("[General]\nInTrash=maybe\n", IN_TRASH),
        ] {
            match parse(text).marks() {
                Err(Error::InvalidMark { key: k, .. }) => assert_eq!(k, key),
                other => panic!("{text:?}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_files_that_are_not_key_files() {
        assert!(matches!(Profile::parse("a\0b"), Err(Error::NotText)));
        assert!(matches!(
            Profile::parse("[General]\nnot a key\n"),
            Err(Error::Malformed { line: 2 })
        ));
        assert!(matches!(
            Profile::parse("Rank=1\n[General]\n"),
            Err(Error::Malformed { line: 1 })
        ));
    }

    #[test]
    fn set_marks_only_changes_the_mark_lines() {
        let mut profile = parse(RAWTHERAPEE_PP3);
        profile.set_marks(&marks(3, ColorLabel::Blue, true));

        let expected = RAWTHERAPEE_PP3.replace(
            "ColorLabel=0\nInTrash=false\n",
            "ColorLabel=4\nInTrash=true\nRank=3\n",
        );
        assert_eq!(profile.to_string(), expected);
        assert_eq!(profile.marks().unwrap(), marks(3, ColorLabel::Blue, true));
    }

    #[test]
    fn set_marks_does_not_add_a_zero_rank_but_updates_an_existing_one() {
        let mut profile = parse(RAWTHERAPEE_PP3);
        profile.set_marks(&Marks::default());
        assert_eq!(profile.to_string(), RAWTHERAPEE_PP3);

        let mut profile = parse("[General]\nRank=2\n");
        profile.set_marks(&Marks::default());
        assert_eq!(
            profile.to_string(),
            "[General]\nRank=0\nColorLabel=0\nInTrash=false\n"
        );
    }

    #[test]
    fn set_in_changes_adds_and_creates() {
        let mut profile = parse("[Exposure]\nContrast=10\n\n[Vibrance]\nEnabled=true\n");
        profile.set_in("Exposure", "Contrast", "20");
        profile.set_in("Exposure", "Saturation", "5");
        profile.set_in("Dehaze", "Enabled", "true");
        assert_eq!(
            profile.to_string(),
            "[Exposure]\nContrast=20\nSaturation=5\n\n[Vibrance]\nEnabled=true\n\n[Dehaze]\nEnabled=true\n"
        );
    }

    #[test]
    fn replace_section_takes_the_source_whole_and_keeps_the_rest() {
        let mut target =
            parse("[General]\nRank=3\n\n[Vibrance]\nEnabled=false\nPastels=1\n\n[Crop]\nX=5\n");
        let source = parse("[Vibrance]\nEnabled=true\nSaturated=-27\n\n[Crop]\nX=0\n");
        target.replace_section("Vibrance", &source);
        assert_eq!(
            target.to_string(),
            "[General]\nRank=3\n\n[Vibrance]\nEnabled=true\nSaturated=-27\n\n[Crop]\nX=5\n"
        );
        assert_eq!(target.get("Vibrance", "Pastels"), None);
    }

    #[test]
    fn replace_section_removes_what_the_source_lacks_and_appends_what_the_target_lacks() {
        let mut target = parse("[Dehaze]\nEnabled=true\n\n[Crop]\nX=5\n");
        let source = parse("[Vibrance]\nEnabled=true\n");
        target.replace_section("Dehaze", &source);
        assert_eq!(target.to_string(), "[Crop]\nX=5\n");
        target.replace_section("Vibrance", &source);
        assert_eq!(
            target.to_string(),
            "[Crop]\nX=5\n[Vibrance]\nEnabled=true\n"
        );
        assert_eq!(target.section_names(), ["Crop", "Vibrance"]);
    }

    #[test]
    fn replace_section_uses_the_line_endings_of_the_target() {
        let mut target = parse("[Crop]\r\nX=5\r\n\r\n[Vibrance]\r\nEnabled=false\r\n");
        let source = parse("[Vibrance]\nEnabled=true\nPastels=3");
        target.replace_section("Vibrance", &source);
        assert_eq!(
            target.to_string(),
            "[Crop]\r\nX=5\r\n\r\n[Vibrance]\r\nEnabled=true\r\nPastels=3\r\n"
        );
        assert_eq!(
            target.section_entries("Vibrance"),
            [("Enabled", "true"), ("Pastels", "3")]
        );
    }

    #[test]
    fn set_marks_adds_general_to_a_profile_without_it() {
        let mut profile = parse(DEFAULT_PROFILE);
        profile.set_marks(&marks(5, ColorLabel::Red, false));
        let expected =
            format!("{DEFAULT_PROFILE}\n[General]\nRank=5\nColorLabel=1\nInTrash=false\n");
        assert_eq!(profile.to_string(), expected);

        let mut profile = parse("[RAW]\nCA=true");
        profile.set_marks(&Marks::default());
        assert_eq!(
            profile.to_string(),
            "[RAW]\nCA=true\n\n[General]\nColorLabel=0\nInTrash=false\n"
        );
    }

    #[test]
    fn set_marks_keeps_windows_line_endings() {
        let mut profile = parse("[General]\r\nInTrash=false\r\n");
        profile.set_marks(&marks(1, ColorLabel::None, true));
        assert_eq!(
            profile.to_string(),
            "[General]\r\nInTrash=true\r\nRank=1\r\nColorLabel=0\r\n"
        );
    }

    #[test]
    #[should_panic(expected = "rank out of range")]
    fn set_marks_panics_on_a_rank_above_five() {
        parse("").set_marks(&marks(6, ColorLabel::None, false));
    }

    #[test]
    fn write_marks_updates_an_existing_file_and_keeps_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011259.RW2.pp3");
        fs::write(&path, RAWTHERAPEE_PP3).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();

        write_marks(
            &path,
            &marks(2, ColorLabel::Yellow, false),
            &parse(DEFAULT_PROFILE),
        )
        .unwrap();

        let profile = Profile::load(&path).unwrap();
        assert_eq!(
            profile.marks().unwrap(),
            marks(2, ColorLabel::Yellow, false)
        );
        assert!(
            profile
                .to_string()
                .contains("Curve=1;0.0071174400000000001;")
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "temporary file left"
        );
    }

    #[test]
    fn write_marks_creates_a_missing_file_from_the_base_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011259.RW2.pp3");

        write_marks(
            &path,
            &marks(1, ColorLabel::None, false),
            &parse(DEFAULT_PROFILE),
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(DEFAULT_PROFILE));
        assert!(text.ends_with("[General]\nRank=1\nColorLabel=0\nInTrash=false\n"));
    }

    #[test]
    fn write_marks_leaves_a_corrupted_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("_1174030.RW2.pp3");
        let garbage = [0x8b, 0x25, 0x00, 0xff, 0x22, 0x0a];
        fs::write(&path, garbage).unwrap();

        let result = write_marks(
            &path,
            &marks(3, ColorLabel::None, false),
            &parse(DEFAULT_PROFILE),
        );

        assert!(matches!(result, Err(Error::NotText)));
        assert_eq!(fs::read(&path).unwrap(), garbage);
    }
}
