//! Decoded pictures of a shot, whether or not it has a JPEG.
//!
//! The camera JPEG is used when there is one: to browse with, the preview
//! it may embed (1440×1080 on a Panasonic GX9, about 20 ms instead of 180 ms
//! for the full image). Otherwise the RAW provides its embedded preview
//! (1920×1440), or a full demosaic (about 0.6 s for 20 Mpx) for 100% zoom.
//! The demosaiced RAW does not have the camera's look: it is darker and
//! flatter than the JPEG, but shows the same detail. Every picture is
//! turned upright according to the EXIF orientation of its file.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use image::metadata::Orientation;
use image::{DynamicImage, ImageFormat};
use rawler::decoders::{Decoder, RawDecodeParams};
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

use crate::jpeg;
use crate::pairing::Shot;

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
        return Jpeg::read(jpeg)?.preview();
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

fn raw_of(shot: &Shot) -> &Path {
    shot.raw.as_deref().expect("a shot without JPEG has a RAW")
}

/// A JPEG file read once for its orientation, preview and pixels.
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

    /// The embedded preview, or the full image if there is none or it
    /// cannot be decoded.
    fn preview(&self) -> Result<Picture, Error> {
        jpeg::mpf_preview(&self.data)
            .and_then(|data| self.decode(data, Origin::JpegPreview).ok())
            .map_or_else(|| self.full(), Ok)
    }

    fn full(&self) -> Result<Picture, Error> {
        self.decode(&self.data, Origin::Jpeg)
    }

    /// Decodes JPEG data from this file, turned upright: an embedded
    /// preview has the orientation of the main image.
    fn decode(&self, data: &[u8], origin: Origin) -> Result<Picture, Error> {
        let mut image =
            image::load_from_memory_with_format(data, ImageFormat::Jpeg).map_err(|source| {
                Error::Jpeg {
                    path: self.path.to_owned(),
                    source,
                }
            })?;
        image.apply_orientation(self.orientation);
        Ok(Picture { image, origin })
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
    use crate::jpeg::tests::{encode, exif_orientation, with_mpf_preview};

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
