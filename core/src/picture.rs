//! Decoded pictures of a shot, whether or not it has a JPEG.
//!
//! The camera JPEG is used when there is one: to browse with, the preview
//! it may embed (1440×1080 on a Panasonic GX9, about 20 ms instead of 180 ms
//! for the full image). Otherwise the RAW provides its embedded preview
//! (1920×1440), or a full demosaic (about 0.6 s for 20 Mpx) for 100% zoom.
//! The demosaiced RAW does not have the camera's look: it is darker and
//! flatter than the JPEG, but shows the same detail. Every picture is
//! turned upright according to the EXIF orientation of its file.
//!
//! Previews are read from the disk without the rest of the file: its head
//! tells where the preview lies. This matters on a spinning disk, where a
//! whole RAW file takes a large part of a second to read.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};

use image::metadata::Orientation;
use image::{DynamicImage, ImageFormat};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

use crate::pairing::Shot;
use crate::{exif, jpeg};

/// Where a picture comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The camera JPEG, full size.
    Jpeg,
    /// The preview embedded in the camera JPEG.
    JpegPreview,
    /// The preview embedded in the RAW, smaller than the sensor.
    RawPreview,
    /// The RAW demosaiced at full size.
    RawDevelop,
}

#[derive(Debug)]
pub struct Picture {
    pub image: DynamicImage,
    pub origin: Origin,
}

#[derive(Debug)]
pub enum Error {
    Jpeg {
        path: PathBuf,
        source: image::ImageError,
    },
    Raw {
        path: PathBuf,
        source: rawler::RawlerError,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Jpeg { path, source } => write!(f, "{}: {source}", path.display()),
            Error::Raw { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Jpeg { source, .. } => Some(source),
            Error::Raw { source, .. } => Some(source),
        }
    }
}

/// A picture to browse with: the JPEG's embedded preview, else the JPEG,
/// else the RAW's embedded preview, else the demosaiced RAW.
pub fn preview(shot: &Shot) -> Result<Picture, Error> {
    if let Some(jpeg) = &shot.jpeg {
        return jpeg_preview(jpeg);
    }
    if let Some(picture) = rw2_preview(raw_of(shot)) {
        return Ok(picture);
    }
    let raw = Raw::open(raw_of(shot))?;
    match raw.preview()? {
        Some(picture) => Ok(picture),
        None => raw.develop(),
    }
}

/// A full-size picture, for 100% zoom: the JPEG, else the demosaiced RAW.
pub fn full(shot: &Shot) -> Result<Picture, Error> {
    match &shot.jpeg {
        Some(jpeg) => Jpeg::read(jpeg)?.full(),
        None => Raw::open(raw_of(shot))?.develop(),
    }
}

/// The thumbnail stored in the metadata of a shot (160×120 on a Panasonic
/// GX9), turned upright: from the head of the JPEG, else of the RAW's
/// embedded preview. Reading it costs little more than reading the date;
/// `None` when the file has no thumbnail or it cannot be read.
pub fn thumbnail(shot: &Shot) -> Option<DynamicImage> {
    let (path, is_jpeg) = match (&shot.jpeg, &shot.raw) {
        (Some(jpeg), _) => (jpeg, true),
        (None, Some(raw)) => (raw, false),
        (None, None) => return None,
    };
    let head = exif::read_head(path).ok()?;
    let container = if is_jpeg {
        &head[..]
    } else {
        jpeg::rw2_preview(&head)?
    };
    let exif = jpeg::exif(container)?;
    let data = jpeg::exif_thumbnail(exif)?;
    let mut image = image::load_from_memory_with_format(data, ImageFormat::Jpeg).ok()?;
    let orientation = Orientation::from_exif_chunk(exif)
        .filter(|_| is_jpeg)
        .or_else(|| raw_orientation(&head))
        .unwrap_or(Orientation::NoTransforms);
    image.apply_orientation(orientation);
    Some(image)
}

/// The orientation of a RAW file, from the head of the file.
fn raw_orientation(head: &[u8]) -> Option<Orientation> {
    let source = RawSource::new_from_slice(head);
    let decoder = rawler::get_decoder(&source).ok()?;
    let metadata = decoder
        .raw_metadata(&source, &RawDecodeParams::default())
        .ok()?;
    Orientation::from_exif(u8::try_from(metadata.exif.orientation?).ok()?)
}

/// Reads a part of a file.
fn read_range(path: &Path, range: Range<usize>) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(range.start as u64))?;
    let mut data = Vec::with_capacity(range.len());
    file.take(range.len() as u64).read_to_end(&mut data)?;
    Ok(data)
}

/// The JPEG's embedded preview, reading only the head of the file and the
/// preview itself; the full image if there is none or it is broken.
fn jpeg_preview(path: &Path) -> Result<Picture, Error> {
    let head = exif::read_head(path).map_err(|err| Error::Jpeg {
        path: path.to_owned(),
        source: err.into(),
    })?;
    let orientation = jpeg::exif(&head)
        .and_then(Orientation::from_exif_chunk)
        .unwrap_or(Orientation::NoTransforms);
    if let Some(range) = jpeg::mpf_preview_range(&head)
        && let Ok(data) = read_range(path, range)
        && let Ok(picture) = decode_jpeg(path, &data, orientation, Origin::JpegPreview)
    {
        return Ok(picture);
    }
    Jpeg::read(path)?.full()
}

/// The preview embedded at the start of a Panasonic RW2, without reading
/// the raw data after it. `None` for other files, or if anything fails:
/// rawler then reads the whole file.
fn rw2_preview(path: &Path) -> Option<Picture> {
    let head = exif::read_head(path).ok()?;
    let range = jpeg::rw2_preview_range(&head)?;
    let data = match head.get(range.clone()) {
        Some(data) => data.to_vec(),
        None => read_range(path, range).ok()?,
    };
    let orientation = raw_orientation(&head).unwrap_or(Orientation::NoTransforms);
    decode_jpeg(path, &data, orientation, Origin::RawPreview).ok()
}

/// Decodes JPEG data, turned upright with the given orientation.
fn decode_jpeg(
    path: &Path,
    data: &[u8],
    orientation: Orientation,
    origin: Origin,
) -> Result<Picture, Error> {
    let mut image =
        image::load_from_memory_with_format(data, ImageFormat::Jpeg).map_err(|source| {
            Error::Jpeg {
                path: path.to_owned(),
                source,
            }
        })?;
    image.apply_orientation(orientation);
    Ok(Picture { image, origin })
}

fn raw_of(shot: &Shot) -> &Path {
    shot.raw.as_deref().expect("a shot without JPEG has a RAW")
}

/// A whole JPEG file, read for its orientation and pixels.
struct Jpeg<'a> {
    path: &'a Path,
    data: Vec<u8>,
    orientation: Orientation,
}

impl<'a> Jpeg<'a> {
    fn read(path: &'a Path) -> Result<Jpeg<'a>, Error> {
        let data = fs::read(path).map_err(|err| Error::Jpeg {
            path: path.to_owned(),
            source: err.into(),
        })?;
        let orientation = jpeg::exif(&data)
            .and_then(Orientation::from_exif_chunk)
            .unwrap_or(Orientation::NoTransforms);
        Ok(Jpeg {
            path,
            data,
            orientation,
        })
    }

    fn full(&self) -> Result<Picture, Error> {
        decode_jpeg(self.path, &self.data, self.orientation, Origin::Jpeg)
    }
}

/// A RAW file opened once for both its orientation and its pixels.
struct Raw<'a> {
    path: &'a Path,
    source: RawSource,
    decoder: Box<dyn Decoder>,
    orientation: Orientation,
}

impl<'a> Raw<'a> {
    fn open(path: &'a Path) -> Result<Raw<'a>, Error> {
        let error = |source| Error::Raw {
            path: path.to_owned(),
            source,
        };
        let source = RawSource::new(path).map_err(|err| error(err.into()))?;
        let decoder = rawler::get_decoder(&source).map_err(error)?;
        // Missing or unreadable metadata only costs the rotation.
        let orientation = decoder
            .raw_metadata(&source, &RawDecodeParams::default())
            .ok()
            .and_then(|metadata| metadata.exif.orientation)
            .and_then(|value| u8::try_from(value).ok())
            .and_then(Orientation::from_exif)
            .unwrap_or(Orientation::NoTransforms);
        Ok(Raw {
            path,
            source,
            decoder,
            orientation,
        })
    }

    fn preview(&self) -> Result<Option<Picture>, Error> {
        let image = self
            .decoder
            .preview_image(&self.source, &RawDecodeParams::default())
            .map_err(|err| self.error(err))?;
        Ok(image.map(|image| self.upright(image, Origin::RawPreview)))
    }

    fn develop(&self) -> Result<Picture, Error> {
        let raw_image = self
            .decoder
            .raw_image(&self.source, &RawDecodeParams::default(), false)
            .map_err(|err| self.error(err))?;
        let image = RawDevelop::default()
            .develop_intermediate(&raw_image)
            .map_err(|err| self.error(err))?
            .to_dynamic_image()
            .ok_or_else(|| self.error("cannot convert the developed image".into()))?;
        Ok(self.upright(image, Origin::RawDevelop))
    }

    fn upright(&self, mut image: DynamicImage, origin: Origin) -> Picture {
        image.apply_orientation(self.orientation);
        Picture { image, origin }
    }

    fn error(&self, source: rawler::RawlerError) -> Error {
        Error::Raw {
            path: self.path.to_owned(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder, Rgb, RgbImage};

    use super::*;
    use crate::jpeg::tests::{encode, exif_orientation, exif_with_thumbnail, with_mpf_preview};

    fn shot(jpeg: Option<PathBuf>, raw: Option<PathBuf>) -> Shot {
        Shot {
            stem: "P1011259".into(),
            jpeg,
            raw,
        }
    }

    fn size(picture: &Picture) -> (u32, u32) {
        (picture.image.width(), picture.image.height())
    }

    #[test]
    fn the_jpeg_is_used_for_preview_and_full_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = dir.path().join("P1011259.JPG");
        fs::write(&jpeg, encode(64, 48, None)).unwrap();
        // The RAW is garbage: it must not even be read.
        let raw = dir.path().join("P1011259.RW2");
        fs::write(&raw, b"not a raw").unwrap();
        let pair = shot(Some(jpeg), Some(raw));

        for picture in [preview(&pair).unwrap(), full(&pair).unwrap()] {
            assert_eq!(picture.origin, Origin::Jpeg);
            assert_eq!(size(&picture), (64, 48));
        }
    }

    #[test]
    fn the_preview_embedded_in_the_jpeg_is_used_to_browse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011259.JPG");
        let main = encode(64, 48, Some(exif_orientation(6)));
        fs::write(&path, with_mpf_preview(&main, &encode(32, 24, None), 0)).unwrap();
        let jpeg_only = shot(Some(path.clone()), None);

        let picture = preview(&jpeg_only).unwrap();
        assert_eq!(picture.origin, Origin::JpegPreview);
        assert_eq!(size(&picture), (24, 32), "turned like the main image");
        let picture = full(&jpeg_only).unwrap();
        assert_eq!(picture.origin, Origin::Jpeg);
        assert_eq!(size(&picture), (48, 64));

        // A broken preview falls back to the main image.
        let broken = b"\xff\xd8 not a jpeg";
        fs::write(&path, with_mpf_preview(&main, broken, 0)).unwrap();
        assert_eq!(preview(&jpeg_only).unwrap().origin, Origin::Jpeg);
    }

    #[test]
    fn the_thumbnail_comes_from_the_exif_data_of_the_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011259.JPG");
        let exif = exif_with_thumbnail(&encode(16, 12, None));
        fs::write(&path, encode(64, 48, Some(exif))).unwrap();

        let thumbnail = thumbnail(&shot(Some(path.clone()), None)).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (16, 12));

        fs::write(&path, encode(64, 48, None)).unwrap();
        assert!(super::thumbnail(&shot(Some(path), None)).is_none());
        let missing = dir.path().join("missing.JPG");
        assert!(super::thumbnail(&shot(Some(missing), None)).is_none());
    }

    #[test]
    fn the_preview_embedded_in_a_rw2_is_read_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("P1011259.RW2");
        let embedded = encode(32, 24, None);
        // RW2 header, IFD at 8 with the JpgFromRaw tag, preview at 26,
        // then what stands for the raw data.
        let mut rw2 = b"IIU\0".to_vec();
        rw2.extend(8u32.to_le_bytes());
        rw2.extend(1u16.to_le_bytes());
        rw2.extend(0x002eu16.to_le_bytes());
        rw2.extend(7u16.to_le_bytes());
        rw2.extend((embedded.len() as u32).to_le_bytes());
        rw2.extend(26u32.to_le_bytes());
        rw2.extend(0u32.to_le_bytes());
        rw2.extend(&embedded);
        rw2.extend(vec![0; 1000]);
        fs::write(&path, rw2).unwrap();

        let picture = preview(&shot(None, Some(path))).unwrap();
        assert_eq!(picture.origin, Origin::RawPreview);
        assert_eq!(size(&picture), (32, 24));
    }

    #[test]
    fn jpegs_are_turned_upright() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portrait.JPG");
        // Stored landscape, left half red, as a camera held vertically writes it.
        let stored = RgbImage::from_fn(64, 48, |x, _| {
            if x < 32 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        let mut encoder = JpegEncoder::new_with_quality(fs::File::create(&path).unwrap(), 95);
        encoder.set_exif_metadata(exif_orientation(6)).unwrap(); // rotate 90° clockwise
        encoder
            .write_image(stored.as_raw(), 64, 48, ExtendedColorType::Rgb8)
            .unwrap();

        let picture = preview(&shot(Some(path), None)).unwrap();

        let image = picture.image.to_rgb8();
        assert_eq!(image.dimensions(), (48, 64));
        // Turned clockwise, the left half of the stored image is now on top.
        let [red, _, blue] = image.get_pixel(24, 8).0;
        assert!(red > 200 && blue < 50, "top should be red");
        let [red, _, blue] = image.get_pixel(24, 56).0;
        assert!(blue > 200 && red < 50, "bottom should be blue");
    }

    #[test]
    fn decoding_errors_name_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = dir.path().join("broken.JPG");
        fs::write(&jpeg, b"not a jpeg").unwrap();
        let raw = dir.path().join("broken.RW2");
        fs::write(&raw, b"not a raw").unwrap();

        let err = preview(&shot(Some(jpeg.clone()), None)).unwrap_err();
        assert!(matches!(&err, Error::Jpeg { path, .. } if *path == jpeg));
        assert!(err.to_string().starts_with(&jpeg.display().to_string()));

        for result in [
            preview(&shot(None, Some(raw.clone()))),
            full(&shot(None, Some(raw.clone()))),
        ] {
            assert!(matches!(result, Err(Error::Raw { path, .. }) if path == raw));
        }

        let missing = dir.path().join("missing.JPG");
        assert!(matches!(
            preview(&shot(Some(missing), None)),
            Err(Error::Jpeg { .. })
        ));
    }
}
