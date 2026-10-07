//! Basic shooting information of a shot: date, settings, camera and lens.
//!
//! The RAW is read when the shot has one, as it is the only file where a
//! Panasonic camera writes the lens name in a standard tag. A JPEG-only
//! shot gets everything but the lens. Only the beginning of the files is
//! read, where the metadata is: a whole shoot can be read in a moment.

use std::fmt;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};

use image::metadata::Orientation;
use rawler::decoders::RawDecodeParams;
use rawler::formats::tiff::reader::TiffReader;
use rawler::formats::tiff::{GenericTiffReader, IFD, Rational};
use rawler::rawsource::RawSource;
use rawler::tags::TiffCommonTag;

use crate::cameras::panasonic;
use crate::files::read_head;
use crate::formats::{jpeg, rw2};
use crate::pairing::{Kind, Shot};

/// Shooting information; any of it may be missing from a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exif {
    /// Local date and time the photo was taken, as `2020-08-03 08:03:35`,
    /// which sorts chronologically as text.
    pub taken: Option<String>,
    /// The same instant to the millisecond, as milliseconds since 1970 in
    /// the camera's local time: enough to compare photos of a shoot.
    pub taken_ms: Option<i64>,
    /// Exposure time in seconds, as a fraction.
    pub exposure_time: Option<(u32, u32)>,
    pub f_number: Option<f32>,
    pub iso: Option<u32>,
    /// Focal length in millimetres.
    pub focal_length: Option<f32>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Where the camera focused, as fractions of the width and height of
    /// the upright picture; only Panasonic cameras are read for now.
    pub focus_point: Option<(f32, f32)>,
}

impl Exif {
    /// The exposure time as photographers write it: `1/500 s`, `2 s`.
    pub fn exposure_text(&self) -> Option<String> {
        let (n, d) = self.exposure_time.filter(|&(n, d)| n > 0 && d > 0)?;
        Some(if n >= d {
            let seconds = f64::from(n) / f64::from(d);
            format!("{} s", trim(seconds))
        } else {
            format!("1/{} s", (f64::from(d) / f64::from(n)).round())
        })
    }
}

/// `2.0` as `2`, `2.5` as `2.5`.
fn trim(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

/// One line with the settings that are known: `1/500 s · f/5.6 · ISO 1600 · 27 mm`.
impl fmt::Display for Exif {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = [
            self.exposure_text(),
            self.f_number.map(|n| format!("f/{}", trim(n.into()))),
            self.iso.map(|iso| format!("ISO {iso}")),
            self.focal_length
                .map(|mm| format!("{} mm", trim(mm.into()))),
        ];
        let parts: Vec<String> = parts.into_iter().flatten().collect();
        f.write_str(&parts.join(" · "))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {source}", path.display())]
    Raw {
        path: PathBuf,
        source: rawler::RawlerError,
    },
}

/// Reads the shooting information of a shot, from its RAW if it has one,
/// else from its JPEG. A file without EXIF data gives an empty `Exif`.
pub fn read(shot: &Shot) -> Result<Exif, Error> {
    Head::read(shot)?.exif()
}

/// The head of a shot's file, read once for everything it holds: the
/// shooting information, and the thumbnail (see `picture`). The RAW is
/// read when the shot has one, else the JPEG.
pub struct Head {
    path: PathBuf,
    kind: Kind,
    data: Vec<u8>,
}

impl Head {
    pub fn read(shot: &Shot) -> Result<Head, Error> {
        match (&shot.raw, &shot.jpeg) {
            (Some(raw), _) => Head::of(raw, Kind::Raw),
            (None, Some(jpeg)) => Head::of(jpeg, Kind::Jpeg),
            (None, None) => unreachable!("a shot has a JPEG or a RAW"),
        }
    }

    pub fn of(path: &Path, kind: Kind) -> Result<Head, Error> {
        let data = read_head(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
        Ok(Head {
            path: path.to_owned(),
            kind,
            data,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The shooting information held by the head.
    pub fn exif(&self) -> Result<Exif, Error> {
        match self.kind {
            Kind::Raw => read_raw(&self.path, &self.data),
            Kind::Jpeg => Ok(read_jpeg(&self.data)),
        }
    }
}

fn read_raw(path: &Path, head: &[u8]) -> Result<Exif, Error> {
    let error = |source| Error::Raw {
        path: path.to_owned(),
        source,
    };
    let metadata = |source: &RawSource| {
        rawler::get_decoder(source)?.raw_metadata(source, &RawDecodeParams::default())
    };
    // The head is enough for the formats tried; others may point further.
    let metadata = match metadata(&RawSource::new_from_slice(head)) {
        Ok(metadata) => metadata,
        Err(_) => {
            let source = RawSource::new(path).map_err(|err: io::Error| error(err.into()))?;
            metadata(&source).map_err(error)?
        }
    };

    let mut exif = from_rawler(&metadata.exif);
    exif.camera = camera(&metadata.make, &metadata.model);
    exif.lens = metadata
        .lens
        .map(|lens| format!("{} {}", lens.lens_make, lens.lens_model))
        .or_else(|| metadata.exif.lens_model.clone());
    // The maker notes are in the EXIF data of the embedded preview.
    let orientation = metadata.exif.orientation.unwrap_or(1);
    exif.focus_point = rw2::preview(head)
        .and_then(jpeg::exif)
        .and_then(|exif| panasonic::focus_point(exif, orientation));
    Ok(exif)
}

fn read_jpeg(head: &[u8]) -> Exif {
    let Some(tiff) = jpeg::exif(head) else {
        return Exif::default();
    };
    let mut exif = parse_tiff(tiff).unwrap_or_default();
    let orientation = Orientation::from_exif_chunk(tiff).map_or(1, |o| u16::from(o.to_exif()));
    exif.focus_point = panasonic::focus_point(tiff, orientation);
    exif
}

/// Parses EXIF data stored as a TIFF structure, as in a JPEG's APP1 segment.
fn parse_tiff(tiff: &[u8]) -> Option<Exif> {
    let sub_ifds = [TiffCommonTag::ExifIFDPointer.into()];
    let reader = GenericTiffReader::new(&mut Cursor::new(tiff), 0, 0, None, &sub_ifds).ok()?;
    let root = reader.file().chain.first()?;
    let mut exif = from_rawler(&rawler::exif::Exif::new(root).ok()?);
    exif.camera = camera(
        &ascii(root, TiffCommonTag::Make).unwrap_or_default(),
        &ascii(root, TiffCommonTag::Model).unwrap_or_default(),
    );
    Some(exif)
}

fn ascii(ifd: &IFD, tag: TiffCommonTag) -> Option<String> {
    Some(ifd.get_entry(tag)?.value.as_string()?.trim().to_owned())
}

/// `Panasonic DC-GX9`, without repeating the make when the model has it.
fn camera(make: &str, model: &str) -> Option<String> {
    let (make, model) = (make.trim(), model.trim());
    match (make, model) {
        ("", "") => None,
        ("", name) | (name, "") => Some(name.to_owned()),
        _ if model.starts_with(make) => Some(model.to_owned()),
        _ => Some(format!("{make} {model}")),
    }
}

fn from_rawler(exif: &rawler::exif::Exif) -> Exif {
    let ratio = |r: &Rational| (r.d != 0).then(|| r.n as f32 / r.d as f32);
    Exif {
        taken: exif.date_time_original.as_deref().and_then(date),
        taken_ms: exif
            .date_time_original
            .as_deref()
            .and_then(|text| timestamp_ms(text, exif.sub_sec_time_original.as_deref())),
        exposure_time: exif.exposure_time.map(|r| (r.n, r.d)),
        f_number: exif.fnumber.as_ref().and_then(ratio),
        iso: exif
            .iso_speed_ratings
            .map(u32::from)
            .or(exif.iso_speed)
            .or(exif.recommended_exposure_index),
        focal_length: exif.focal_length.as_ref().and_then(ratio),
        camera: None,
        lens: exif.lens_model.clone(),
        focus_point: None,
    }
}

/// `2020:08:03 08:03:35` as `2020-08-03 08:03:35`; `None` for the blank
/// dates some cameras write.
fn date(text: &str) -> Option<String> {
    let text = text.trim_end_matches('\0').trim();
    let (day, time) = text.split_once(' ')?;
    let digits = |part: &str| part.chars().filter(char::is_ascii_digit).count();
    if digits(day) != 8 || digits(time) != 6 || day.starts_with("0000") {
        return None;
    }
    Some(format!("{} {time}", day.replace(':', "-")))
}

/// `2020:08:03 08:03:35` plus sub-second digits (`667` for .667 s) as
/// milliseconds since 1970-01-01 00:00:00, without time zone.
fn timestamp_ms(text: &str, sub_second: Option<&str>) -> Option<i64> {
    let normalized = date(text)?;
    let numbers: Vec<i64> = normalized
        .split(['-', ' ', ':'])
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let &[year, month, day, hour, minute, second] = numbers.as_slice() else {
        return None;
    };
    // Sub-second digits are a decimal fraction: "5" is 500 ms, "29" 290 ms.
    let millis = sub_second
        .map(|digits| digits.trim_matches(|c: char| !c.is_ascii_digit()))
        .filter(|digits| !digits.is_empty())
        .map_or(0, |digits| {
            format!("{digits:0<3}")[..3].parse::<i64>().unwrap_or(0)
        });
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar,
/// after Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::formats::testing;

    #[test]
    fn reads_the_focus_point_of_a_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011261.JPG");
        let exif = testing::exif_with_af_point((128, 256), (64, 256));
        fs::write(&path, testing::encode(16, 8, Some(exif))).unwrap();
        let shot = Shot {
            stem: "P1011261".into(),
            jpeg: Some(path),
            raw: None,
        };
        assert_eq!(read(&shot).unwrap().focus_point, Some((0.5, 0.25)));
    }

    #[test]
    fn timestamps_count_milliseconds_across_days_and_years() {
        let at = |text, sub| timestamp_ms(text, sub);
        assert_eq!(at("2020:08:03 08:03:35", None), Some(1_596_441_815_000));
        assert_eq!(
            at("2020:08:03 08:03:35", Some("667")),
            Some(1_596_441_815_667)
        );
        assert_eq!(
            at("2020:08:03 08:03:35", Some("5")),
            Some(1_596_441_815_500)
        );
        assert_eq!(
            at("2020:08:03 08:03:35", Some("29")),
            Some(1_596_441_815_290)
        );
        assert_eq!(
            at("2020:08:03 08:03:35", Some("1234")),
            Some(1_596_441_815_123)
        );
        assert_eq!(
            at("2020:08:03 08:03:35", Some("  ")),
            Some(1_596_441_815_000)
        );
        assert_eq!(at("2000:02:29 23:59:59", None), Some(951_868_799_000));
        assert_eq!(at("2000:03:01 00:00:00", None), Some(951_868_800_000));
        assert_eq!(at("0000:00:00 00:00:00", None), None);
    }

    #[test]
    fn exposure_times_read_like_on_a_camera() {
        let exposure = |n, d| Exif {
            exposure_time: Some((n, d)),
            ..Exif::default()
        };
        assert_eq!(
            exposure(10, 5000).exposure_text().as_deref(),
            Some("1/500 s")
        );
        assert_eq!(exposure(1, 3).exposure_text().as_deref(), Some("1/3 s"));
        assert_eq!(exposure(2, 1).exposure_text().as_deref(), Some("2 s"));
        assert_eq!(exposure(25, 10).exposure_text().as_deref(), Some("2.5 s"));
        assert_eq!(exposure(0, 1).exposure_text(), None);
        assert_eq!(exposure(1, 0).exposure_text(), None);
    }

    #[test]
    fn summary_lists_the_known_settings() {
        let exif = Exif {
            exposure_time: Some((10, 5000)),
            f_number: Some(5.6),
            iso: Some(1600),
            focal_length: Some(27.0),
            ..Exif::default()
        };
        assert_eq!(exif.to_string(), "1/500 s · f/5.6 · ISO 1600 · 27 mm");
        let partial = Exif {
            iso: Some(200),
            ..Exif::default()
        };
        assert_eq!(partial.to_string(), "ISO 200");
        assert_eq!(Exif::default().to_string(), "");
    }

    #[test]
    fn dates_are_normalized_and_blank_ones_dropped() {
        assert_eq!(
            date("2020:08:03 08:03:35").as_deref(),
            Some("2020-08-03 08:03:35")
        );
        assert_eq!(
            date("2020:08:03 08:03:35\0").as_deref(),
            Some("2020-08-03 08:03:35")
        );
        assert_eq!(date("0000:00:00 00:00:00"), None);
        assert_eq!(date("    :  :     :  :  "), None);
        assert_eq!(date("2020:08:03"), None);
    }

    #[test]
    fn camera_names_do_not_repeat_the_make() {
        assert_eq!(
            camera("Panasonic", "DC-GX9").as_deref(),
            Some("Panasonic DC-GX9")
        );
        assert_eq!(
            camera("Canon", "Canon EOS R6").as_deref(),
            Some("Canon EOS R6")
        );
        assert_eq!(camera(" ", "X100V").as_deref(), Some("X100V"));
        assert_eq!(camera("", ""), None);
    }

    /// Little-endian EXIF data with Make, Model and an Exif IFD holding
    /// the exposure settings, as cameras write it in a JPEG.
    fn camera_exif() -> Vec<u8> {
        fn entry(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
            [
                &tag.to_le_bytes()[..],
                &kind.to_le_bytes(),
                &count.to_le_bytes(),
                &value.to_le_bytes(),
            ]
            .concat()
        }
        const ASCII: u16 = 2;
        const SHORT: u16 = 3;
        const LONG: u16 = 4;
        const RATIONAL: u16 = 5;
        // Layout: header (8), root IFD at 8 with 3 entries (2 + 36 + 4 = 42),
        // Exif IFD at 50 with 5 entries (2 + 60 + 4 = 66), data from 116.
        let make = b"Panasonic\0";
        let model = b"DC-GX9\0";
        let date = b"2020:08:03 08:03:35\0";
        let (make_at, model_at, date_at) = (116, 126, 133);
        let (exposure_at, f_number_at, focal_at) = (154, 162, 170);

        let mut tiff = b"II*\0".to_vec();
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(3u16.to_le_bytes());
        tiff.extend(entry(0x010f, ASCII, make.len() as u32, make_at));
        tiff.extend(entry(0x0110, ASCII, model.len() as u32, model_at));
        tiff.extend(entry(0x8769, LONG, 1, 50));
        tiff.extend(0u32.to_le_bytes());
        assert_eq!(tiff.len(), 50);
        tiff.extend(5u16.to_le_bytes());
        tiff.extend(entry(0x829a, RATIONAL, 1, exposure_at));
        tiff.extend(entry(0x829d, RATIONAL, 1, f_number_at));
        tiff.extend(entry(0x8827, SHORT, 1, 1600));
        tiff.extend(entry(0x9003, ASCII, date.len() as u32, date_at));
        tiff.extend(entry(0x920a, RATIONAL, 1, focal_at));
        tiff.extend(0u32.to_le_bytes());
        assert_eq!(tiff.len(), 116);
        tiff.extend(make);
        tiff.extend(model);
        tiff.extend(date);
        tiff.push(0); // word alignment of the rationals
        assert_eq!(tiff.len(), exposure_at as usize);
        for (n, d) in [(10u32, 5000u32), (56, 10), (270, 10)] {
            tiff.extend(n.to_le_bytes());
            tiff.extend(d.to_le_bytes());
        }
        tiff
    }

    #[test]
    fn reads_a_jpeg_only_shot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011261.JPG");
        fs::write(&path, testing::encode(16, 8, Some(camera_exif()))).unwrap();
        let shot = Shot {
            stem: "P1011261".into(),
            jpeg: Some(path),
            raw: None,
        };

        let exif = read(&shot).unwrap();

        assert_eq!(
            exif,
            Exif {
                taken: Some("2020-08-03 08:03:35".into()),
                taken_ms: Some(1_596_441_815_000),
                exposure_time: Some((10, 5000)),
                f_number: Some(5.6),
                iso: Some(1600),
                focal_length: Some(27.0),
                camera: Some("Panasonic DC-GX9".into()),
                lens: None,
                focus_point: None,
            }
        );
    }

    #[test]
    fn a_jpeg_without_exif_gives_empty_information() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.JPG");
        fs::write(&path, testing::encode(16, 8, None)).unwrap();
        let shot = Shot {
            stem: "plain".into(),
            jpeg: Some(path),
            raw: None,
        };
        assert_eq!(read(&shot).unwrap(), Exif::default());
    }

    #[test]
    fn unreadable_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("broken.RW2");
        fs::write(&raw, b"not a raw").unwrap();
        let shot = |jpeg, raw| Shot {
            stem: "broken".into(),
            jpeg,
            raw,
        };
        assert!(matches!(
            read(&shot(None, Some(raw))),
            Err(Error::Raw { .. })
        ));
        let missing = dir.path().join("missing.JPG");
        assert!(matches!(
            read(&shot(Some(missing), None)),
            Err(Error::Io { .. })
        ));
    }
}
