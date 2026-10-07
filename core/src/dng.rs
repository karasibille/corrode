//! Writing a raw image as a DNG, so that RawTherapee develops it like the
//! original file: same camera, same metadata, same embedded preview for
//! its tone curve to match.

use std::io::BufWriter;
use std::path::Path;

use image::DynamicImage;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::dng::writer::DngWriter;
use rawler::dng::{CropMode, DNG_VERSION_V1_4, DngCompression, DngPhotometricConversion};
use rawler::imgop::develop::RawDevelop;
use rawler::rawimage::RawImage;
use rawler::rawsource::RawSource;
use rawler::tags::ExifTag;

/// JPEG quality of the preview embedded in the DNG.
const PREVIEW_QUALITY: f32 = 0.75;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Raw(#[from] rawler::RawlerError),
    #[error("{0}")]
    Dng(#[from] rawler::dng::writer::DngError),
}

/// Writes `raw`, decoded from `source` by `decoder`, as a lossless DNG
/// at `path`, with the metadata and the preview of the original file. The
/// file is written whole or not at all.
pub fn write(
    path: &Path,
    raw: &RawImage,
    source: &RawSource,
    decoder: &dyn Decoder,
) -> Result<(), Error> {
    let params = RawDecodeParams::default();
    let metadata: RawMetadata = decoder.raw_metadata(source, &params)?;
    // The camera's preview, or a development when it has none.
    let preview: DynamicImage = match decoder.preview_image(source, &params)? {
        Some(image) => image,
        None => RawDevelop::default()
            .develop_intermediate(raw)?
            .to_dynamic_image()
            .ok_or_else(|| rawler::RawlerError::DecoderFailed("cannot develop".into()))?,
    };

    let dir = path.parent().unwrap_or(Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".corrode-")
        .suffix(".dng")
        .tempfile_in(dir)?;
    {
        let mut dng = DngWriter::new(BufWriter::new(temporary.as_file()), DNG_VERSION_V1_4)?;
        let mut frame = dng.subframe(0);
        frame.raw_image(
            raw,
            CropMode::Best,
            DngCompression::Lossless,
            DngPhotometricConversion::Original,
            1,
        )?;
        frame.finalize()?;
        let mut frame = dng.subframe(1);
        frame.preview(&preview, PREVIEW_QUALITY)?;
        frame.finalize()?;
        dng.thumbnail(&preview)?;
        dng.load_base_tags(raw)?;
        dng.load_metadata(&metadata)?;
        if !dng.root_ifd().contains(ExifTag::Orientation) {
            dng.root_ifd_mut()
                .add_tag(ExifTag::Orientation, raw.orientation.to_u16());
        }
        dng.close()?;
    }
    temporary.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// Reads a RAW file, lets `change` alter its image, and writes the result
/// as a DNG at `path`.
pub fn rewrite<T, E: From<Error>>(
    raw_path: &Path,
    path: &Path,
    change: impl FnOnce(&mut RawImage) -> Result<T, E>,
) -> Result<T, E> {
    let source = RawSource::new(raw_path).map_err(Error::from)?;
    let decoder = rawler::get_decoder(&source).map_err(Error::from)?;
    let mut raw = decoder
        .raw_image(&source, &RawDecodeParams::default(), false)
        .map_err(Error::from)?;
    let result = change(&mut raw)?;
    write(path, &raw, &source, decoder.as_ref())?;
    Ok(result)
}
