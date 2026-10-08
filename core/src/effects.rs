//! Creative effects: what the compression and the wear of images do to
//! them, done on purpose. Each effect is a function of an image and its
//! parameters to a new image, so that they can be chained.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{ImageFormat, RgbImage};

/// Which way the pixels are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Along the rows, so that the streaks are horizontal.
    #[default]
    Horizontal,
    /// Along the columns.
    Vertical,
}

impl std::str::FromStr for Direction {
    type Err = String;

    fn from_str(text: &str) -> Result<Direction, String> {
        match text {
            "horizontal" | "h" | "x" => Ok(Direction::Horizontal),
            "vertical" | "v" | "y" => Ok(Direction::Vertical),
            other => Err(format!(
                "unknown direction '{other}' (horizontal or vertical)"
            )),
        }
    }
}

/// Parameters of the pixel sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSort {
    pub direction: Direction,
    /// Brightness range, 0 to 255, of the pixels that are sorted: a run
    /// of neighbouring pixels within it is sorted, the others stay. The
    /// darkest shadows and the brightest highlights are usually left
    /// out, so that the streaks start and end on them.
    pub low: u8,
    pub high: u8,
    /// Whether the runs go from bright to dark instead of dark to bright.
    pub reverse: bool,
}

impl Default for PixelSort {
    fn default() -> PixelSort {
        PixelSort {
            direction: Direction::Horizontal,
            low: 40,
            high: 220,
            reverse: false,
        }
    }
}

/// Sorts the pixels by brightness along the rows or the columns, in
/// runs of neighbours whose brightness is within a range: the picture
/// melts into streaks between the shadows and the highlights that stop
/// them.
pub fn pixel_sort(image: &RgbImage, params: PixelSort) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    let (lines, length) = match params.direction {
        Direction::Horizontal => (height, width),
        Direction::Vertical => (width, height),
    };
    let at = |line: u32, i: u32| match params.direction {
        Direction::Horizontal => (i, line),
        Direction::Vertical => (line, i),
    };
    let (low, high) = (params.low.min(params.high), params.high.max(params.low));
    let mut sorted = image.clone();
    let mut run: Vec<(u8, image::Rgb<u8>)> = Vec::with_capacity(length as usize);
    for line in 0..lines {
        let mut i = 0;
        while i < length {
            // Collect a run of pixels within the range.
            run.clear();
            let start = i;
            while i < length {
                let pixel = *image.get_pixel(at(line, i).0, at(line, i).1);
                let light = brightness(pixel);
                if !(low..=high).contains(&light) {
                    break;
                }
                run.push((light, pixel));
                i += 1;
            }
            if run.len() > 1 {
                run.sort_unstable_by_key(|(light, _)| *light);
                if params.reverse {
                    run.reverse();
                }
                for (k, (_, pixel)) in run.iter().enumerate() {
                    let (x, y) = at(line, start + k as u32);
                    sorted.put_pixel(x, y, *pixel);
                }
            }
            // Past the pixel that ended the run.
            i += 1;
        }
    }
    sorted
}

/// Perceived brightness of a pixel, 0 to 255.
fn brightness(pixel: image::Rgb<u8>) -> u8 {
    let [r, g, b] = pixel.0;
    ((299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000) as u8
}

/// Parameters of the databending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Databend {
    /// JPEG quality the picture is encoded at before its bytes are
    /// bent: the lower, the larger the blocks the damage shows in.
    pub quality: u8,
    /// How many bytes of the compressed picture are changed.
    pub hits: u32,
    /// Seed of the random choice of bytes: the same seed bends the same
    /// picture the same way.
    pub seed: u64,
}

impl Default for Databend {
    fn default() -> Databend {
        Databend {
            quality: 75,
            hits: 8,
            seed: 1,
        }
    }
}

/// Changes a few bytes of the picture once compressed as JPEG, in the
/// scan data after the headers, and decodes what comes out: each hit
/// throws the colours and the blocks off from there to the end of the
/// picture, or to the next hit.
pub fn databend(image: &RgbImage, params: Databend) -> Result<RgbImage, String> {
    let mut data = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut data, params.quality.clamp(1, 100))
        .encode_image(image)
        .map_err(|err| err.to_string())?;
    let mut data = data.into_inner();
    let start = scan_start(&data).ok_or("no scan in the JPEG")?;
    // The last two bytes are the end-of-image marker.
    let end = data.len().saturating_sub(2);
    if start >= end {
        return Err("empty scan".to_owned());
    }
    let mut random = Random(params.seed);
    for _ in 0..params.hits {
        let at = start + random.below(end - start);
        // Never make a marker (0xFF followed by anything but 0): the
        // decoder would stop at it, or take it for a new segment.
        if data[at] == 0xFF || data.get(at.wrapping_sub(1)) == Some(&0xFF) {
            continue;
        }
        data[at] = random.below(0xFF) as u8;
    }
    image::load_from_memory_with_format(&data, ImageFormat::Jpeg)
        .map(|decoded| decoded.to_rgb8())
        .map_err(|err| format!("the bent JPEG cannot be decoded: {err}"))
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

/// A small deterministic random source (SplitMix64), enough to pick
/// bytes to bend, so that a seed names a result.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound`.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

/// Parameters of the generation loss.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    fn databending_changes_the_picture_the_same_way_for_a_seed() {
        let original = picture();
        let params = Databend {
            quality: 50,
            hits: 4,
            seed: 7,
        };
        let bent = databend(&original, params).unwrap();
        assert_eq!((bent.width(), bent.height()), (64, 48));
        assert!(distance(&bent, &original) > 1.0);
        assert_eq!(databend(&original, params).unwrap(), bent);
        let other = databend(&original, Databend { seed: 8, ..params }).unwrap();
        assert_ne!(other, bent);
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

    #[test]
    fn sorting_melts_the_middle_tones_between_the_extremes() {
        // A row: dark, then a jumble of middle tones, then bright.
        let tones = [10u8, 120, 60, 200, 90, 150, 250, 80];
        let image = RgbImage::from_fn(8, 2, |x, _| Rgb([tones[x as usize]; 3]));
        let sorted = pixel_sort(
            &image,
            PixelSort {
                low: 40,
                high: 220,
                ..PixelSort::default()
            },
        );
        let row: Vec<u8> = (0..8).map(|x| sorted.get_pixel(x, 1)[0]).collect();
        assert_eq!(row, vec![10, 60, 90, 120, 150, 200, 250, 80]);

        let reversed = pixel_sort(
            &image,
            PixelSort {
                reverse: true,
                ..PixelSort::default()
            },
        );
        let row: Vec<u8> = (0..8).map(|x| reversed.get_pixel(x, 0)[0]).collect();
        assert_eq!(row, vec![10, 200, 150, 120, 90, 60, 250, 80]);

        // Vertically, nothing to sort here: every column is one tone.
        let vertical = pixel_sort(
            &image,
            PixelSort {
                direction: Direction::Vertical,
                ..PixelSort::default()
            },
        );
        assert_eq!(vertical, image);
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
}
