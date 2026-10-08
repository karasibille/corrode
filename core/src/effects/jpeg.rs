//! Effects made of JPEG compression: its wear over generations, and the
//! failures of a damaged file.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{ImageFormat, RgbImage};

use super::Error;
use super::random::Random;

/// Parameters of the generation loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationLoss {
    /// How many times the image is saved as JPEG and read back.
    pub generations: u32,
    /// JPEG quality of each generation, 1 to 100: the lower, the faster
    /// the image wears.
    pub quality: u8,
    /// How far the image is moved before each generation, in pixels, so
    /// that the 8×8 blocks of a generation do not line up with the
    /// previous ones: the artifacts then build on each other instead of
    /// settling. The image is moved back at the end.
    pub shift: (i32, i32),
}

impl Default for GenerationLoss {
    fn default() -> GenerationLoss {
        GenerationLoss {
            generations: 30,
            quality: 25,
            shift: (1, 0),
        }
    }
}

/// Saves an image as JPEG over and over, each time a little further,
/// like a photocopy of a photocopy.
pub fn generation_loss(image: &RgbImage, params: GenerationLoss) -> RgbImage {
    let (dx, dy) = params.shift;
    let mut current = image.clone();
    for _ in 0..params.generations {
        current = translate(&current, dx, dy);
        current = jpeg_roundtrip(&current, params.quality);
    }
    let generations = i32::try_from(params.generations).unwrap_or(i32::MAX);
    translate(
        &current,
        dx.saturating_mul(-generations),
        dy.saturating_mul(-generations),
    )
}

/// Moves an image by `(dx, dy)` pixels, the edges filling what is left
/// uncovered.
fn translate(image: &RgbImage, dx: i32, dy: i32) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    let source = |coordinate: u32, delta: i32, size: u32| {
        (i64::from(coordinate) - i64::from(delta)).clamp(0, i64::from(size) - 1) as u32
    };
    RgbImage::from_fn(width, height, |x, y| {
        *image.get_pixel(source(x, dx, width), source(y, dy, height))
    })
}

/// Encodes an image as JPEG at a quality and decodes it again.
fn jpeg_roundtrip(image: &RgbImage, quality: u8) -> RgbImage {
    let mut data = Cursor::new(Vec::new());
    let encoded = JpegEncoder::new_with_quality(&mut data, quality.clamp(1, 100))
        .encode_image(image)
        .and_then(|()| image::load_from_memory_with_format(data.get_ref(), ImageFormat::Jpeg));
    match encoded {
        Ok(decoded) => decoded.to_rgb8(),
        // Cannot happen for an image that fits in memory: keep it as is.
        Err(_) => image.clone(),
    }
}

/// Parameters of the databending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Databend {
    /// JPEG quality the picture is encoded at before its bytes are
    /// bent: the lower, the larger the blocks the damage shows in.
    pub quality: u8,
    /// How many bytes of the compressed picture are changed.
    pub hits: u32,
}

impl Default for Databend {
    fn default() -> Databend {
        Databend {
            quality: 75,
            hits: 8,
        }
    }
}

/// Changes a few bytes of the picture once compressed as JPEG, in the
/// scan data after the headers, and decodes what comes out: each hit
/// throws the colours and the blocks off from there to the end of the
/// picture, or to the next hit. The seed chooses the bytes.
pub fn databend(image: &RgbImage, params: Databend, seed: u64) -> Result<RgbImage, Error> {
    let mut data = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut data, params.quality.clamp(1, 100))
        .encode_image(image)
        .map_err(|err| Error::Jpeg(err.to_string()))?;
    let mut data = data.into_inner();
    let start = scan_start(&data).ok_or_else(|| Error::Jpeg("no scan in the JPEG".into()))?;
    // The last two bytes are the end-of-image marker.
    let end = data.len().saturating_sub(2);
    if start >= end {
        return Err(Error::Jpeg("empty scan".into()));
    }
    let mut random = Random::new(seed);
    for _ in 0..params.hits {
        let at = start + random.below((end - start) as u64) as usize;
        // Never make a marker (0xFF followed by anything but 0): the
        // decoder would stop at it, or take it for a new segment.
        if data[at] == 0xFF || data.get(at.wrapping_sub(1)) == Some(&0xFF) {
            continue;
        }
        data[at] = random.below(0xFF) as u8;
    }
    image::load_from_memory_with_format(&data, ImageFormat::Jpeg)
        .map(|decoded| decoded.to_rgb8())
        .map_err(|err| Error::Jpeg(format!("the bent JPEG cannot be decoded: {err}")))
}

/// Where the entropy-coded data of a baseline JPEG starts: after the
/// start-of-scan marker (FF DA) and its header.
fn scan_start(jpeg: &[u8]) -> Option<usize> {
    let mut i = 2; // after the start-of-image marker
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        let length = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        if marker == 0xDA {
            return Some(i + 2 + length);
        }
        i += 2 + length;
    }
    None
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A picture with sharp edges: a white square on a grey ground.
    fn picture() -> RgbImage {
        RgbImage::from_fn(64, 48, |x, y| {
            if (16..32).contains(&x) && (16..32).contains(&y) {
                Rgb([255, 255, 255])
            } else {
                Rgb([100, 100, 100])
            }
        })
    }

    /// Mean absolute difference between two images of the same size.
    fn distance(a: &RgbImage, b: &RgbImage) -> f64 {
        let sum: u64 = a
            .pixels()
            .zip(b.pixels())
            .flat_map(|(a, b)| a.0.into_iter().zip(b.0))
            .map(|(a, b)| u64::from(a.abs_diff(b)))
            .sum();
        sum as f64 / (3 * a.width() * a.height()) as f64
    }

    #[test]
    fn translation_moves_the_picture_and_fills_the_edge() {
        let moved = translate(&picture(), 3, -2);
        assert_eq!(moved.get_pixel(19, 14), &Rgb([255, 255, 255]));
        assert_eq!(moved.get_pixel(18, 14), &Rgb([100, 100, 100]));
        assert_eq!(moved.get_pixel(0, 47), &Rgb([100, 100, 100]));
        assert_eq!(translate(&picture(), 0, 0), picture());
    }

    #[test]
    fn generations_wear_the_picture_without_moving_it() {
        let original = picture();
        let none = generation_loss(
            &original,
            GenerationLoss {
                generations: 0,
                ..GenerationLoss::default()
            },
        );
        assert_eq!(none, original);

        let worn = generation_loss(
            &original,
            GenerationLoss {
                generations: 20,
                quality: 10,
                shift: (1, 1),
            },
        );
        assert_eq!((worn.width(), worn.height()), (64, 48));
        let wear = distance(&worn, &original);
        assert!(wear > 2.0, "wear {wear}");
        // The square is still where it was: brighter inside than out.
        assert!(
            worn.get_pixel(24, 24)[0] > 200,
            "{:?}",
            worn.get_pixel(24, 24)
        );
        assert!(
            worn.get_pixel(40, 40)[0] < 150,
            "{:?}",
            worn.get_pixel(40, 40)
        );
        // More generations wear more.
        let more = generation_loss(
            &original,
            GenerationLoss {
                generations: 60,
                quality: 10,
                shift: (1, 1),
            },
        );
        assert!(distance(&more, &original) > wear);
    }

    #[test]
    fn databending_changes_the_picture_the_same_way_for_a_seed() {
        let original = picture();
        let params = Databend {
            quality: 50,
            hits: 4,
        };
        let bent = databend(&original, params, 7).unwrap();
        assert_eq!((bent.width(), bent.height()), (64, 48));
        assert!(distance(&bent, &original) > 1.0);
        assert_eq!(databend(&original, params, 7).unwrap(), bent);
        assert_ne!(databend(&original, params, 8).unwrap(), bent);
    }

    #[test]
    fn the_scan_starts_after_the_headers() {
        let mut data = Cursor::new(Vec::new());
        JpegEncoder::new_with_quality(&mut data, 50)
            .encode_image(&picture())
            .unwrap();
        let data = data.into_inner();
        let start = scan_start(&data).unwrap();
        assert!(start > 100 && start < data.len() - 2);
        assert_eq!(&data[data.len() - 2..], &[0xFF, 0xD9]);
        assert!(scan_start(&[0xFF, 0xD8, 0x00]).is_none());
    }
}
