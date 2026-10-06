//! Decoded pictures of a shot, whether or not it has a JPEG.
//!
//! The camera JPEG is used when there is one. Otherwise the RAW provides
//! its embedded preview (1920×1440 on a Panasonic GX9), quick enough to
//! browse with, or a full demosaic (about 0.6 s for 20 Mpx) for 100% zoom.
//! The demosaiced RAW does not have the camera's look: it is darker and
//! flatter than the JPEG, but shows the same detail.

use std::fmt;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageReader};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::pairing::Shot;

/// Where a picture comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The camera JPEG, full size.
    Jpeg,
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

/// A picture to browse with: the JPEG, else the RAW's embedded preview,
/// else the demosaiced RAW if it has no preview.
pub fn preview(shot: &Shot) -> Result<Picture, Error> {
    if let Some(jpeg) = &shot.jpeg {
        return decode_jpeg(jpeg);
    }
    let raw = raw_of(shot);
    match raw_preview(raw)? {
        Some(image) => Ok(Picture {
            image,
            origin: Origin::RawPreview,
        }),
        None => develop(raw),
    }
}

/// A full-size picture, for 100% zoom: the JPEG, else the demosaiced RAW.
pub fn full(shot: &Shot) -> Result<Picture, Error> {
    match &shot.jpeg {
        Some(jpeg) => decode_jpeg(jpeg),
        None => develop(raw_of(shot)),
    }
}

fn raw_of(shot: &Shot) -> &Path {
    shot.raw.as_deref().expect("a shot without JPEG has a RAW")
}

fn decode_jpeg(path: &Path) -> Result<Picture, Error> {
    let error = |source| Error::Jpeg {
        path: path.to_owned(),
        source,
    };
    let image = ImageReader::open(path)
        .map_err(|err| error(err.into()))?
        .with_guessed_format()
        .map_err(|err| error(err.into()))?
        .decode()
        .map_err(error)?;
    Ok(Picture {
        image,
        origin: Origin::Jpeg,
    })
}

fn raw_preview(path: &Path) -> Result<Option<DynamicImage>, Error> {
    let error = |source| Error::Raw {
        path: path.to_owned(),
        source,
    };
    let source = RawSource::new(path).map_err(|err| error(err.into()))?;
    let decoder = rawler::get_decoder(&source).map_err(error)?;
    decoder
        .preview_image(&source, &RawDecodeParams::default())
        .map_err(error)
}

fn develop(path: &Path) -> Result<Picture, Error> {
    let image =
        rawler::analyze::raw_to_srgb(path, &RawDecodeParams::default()).map_err(|source| {
            Error::Raw {
                path: path.to_owned(),
                source,
            }
        })?;
    Ok(Picture {
        image,
        origin: Origin::RawDevelop,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::{ImageFormat, RgbImage};

    use super::*;

    fn write_jpeg(path: &Path, width: u32, height: u32) {
        RgbImage::new(width, height)
            .save_with_format(path, ImageFormat::Jpeg)
            .unwrap();
    }

    fn shot(jpeg: Option<PathBuf>, raw: Option<PathBuf>) -> Shot {
        Shot {
            stem: "P1011259".into(),
            jpeg,
            raw,
        }
    }

    #[test]
    fn the_jpeg_is_used_for_preview_and_full_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = dir.path().join("P1011259.JPG");
        write_jpeg(&jpeg, 64, 48);
        // The RAW is garbage: it must not even be read.
        let raw = dir.path().join("P1011259.RW2");
        fs::write(&raw, b"not a raw").unwrap();
        let pair = shot(Some(jpeg), Some(raw));

        for picture in [preview(&pair).unwrap(), full(&pair).unwrap()] {
            assert_eq!(picture.origin, Origin::Jpeg);
            assert_eq!((picture.image.width(), picture.image.height()), (64, 48));
        }
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
