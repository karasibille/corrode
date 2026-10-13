//! Lines the background of several frames up: the camera moves a little
//! between two shots, and a lamp that should stay in one place is then
//! seen twice in the ghosts.

use image::RgbImage;
use image::imageops::{FilterType, resize};

use super::Error;

/// Widths of the pictures the shift is looked for in, from the coarsest
/// to the finest, and how far the search goes round the shift of the
/// level before, in pixels of its level.
const LEVELS: [(u32, i32); 3] = [(192, 16), (768, 4), (1536, 3)];
/// Level of grey under which a pixel does not count as a light.
const LIGHT_FROM: f32 = 110.0;

fn luminance(image: &RgbImage, width: u32) -> (Vec<f32>, u32, u32) {
    let width = width.min(image.width()).max(1);
    let height =
        (u64::from(image.height()) * u64::from(width) / u64::from(image.width())).max(1) as u32;
    let small = resize(image, width, height, FilterType::Triangle);
    let grey = small
        .pixels()
        .map(|p| {
            let grey =
                0.299 * f32::from(p.0[0]) + 0.587 * f32::from(p.0[1]) + 0.114 * f32::from(p.0[2]);
            // Only the lights count: the haze and the dark change from a
            // frame to the next, and the lamp is what must not be seen twice.
            ((grey - LIGHT_FROM) / (255.0 - LIGHT_FROM))
                .clamp(0.0, 1.0)
                .powi(2)
        })
        .collect::<Vec<f32>>();
    (grey, width, height)
}

/// How far two pictures are once the second is shifted.
fn cost(main: &[f32], frame: &[f32], size: (u32, u32), shift: (i32, i32), step: usize) -> f32 {
    let (width, height) = (size.0 as i32, size.1 as i32);
    // The middle of the picture only, so that every shift compares the
    // same pixels: a shift that leaves the dark edges out would look good.
    let (x0, x1) = (width / 5, width - width / 5);
    let (y0, y1) = (height * 3 / 10, height - height / 5);
    let mut diffs = Vec::new();
    for y in (y0..y1).step_by(step) {
        let fy = y + shift.1;
        if !(0..height).contains(&fy) {
            return f32::MAX;
        }
        for x in (x0..x1).step_by(step) {
            let fx = x + shift.0;
            if !(0..width).contains(&fx) {
                return f32::MAX;
            }
            diffs.push((main[(y * width + x) as usize] - frame[(fy * width + fx) as usize]).abs());
        }
    }
    if diffs.is_empty() {
        return f32::MAX;
    }
    diffs.iter().sum::<f32>() / diffs.len() as f32
}

/// For each frame, the shift `(dx, dy)` in pixels such that the frame
/// moved by it, `shifted`, has its background on the one of `frames[main]`.
pub fn register(frames: &[RgbImage], main: usize) -> Result<Vec<(i32, i32)>, Error> {
    if main >= frames.len() {
        return Err(Error::Frames("a main frame among the frames".to_owned()));
    }
    let size = frames[main].dimensions();
    if frames.iter().any(|frame| frame.dimensions() != size) {
        return Err(Error::Frames(
            "the frames are not all the same size".to_owned(),
        ));
    }
    let levels: Vec<_> = LEVELS
        .iter()
        .map(|&(width, _)| luminance(&frames[main], width))
        .collect();
    frames
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            if i == main {
                return Ok((0, 0));
            }
            let mut shift = (0, 0);
            for (level, &(width, radius)) in LEVELS.iter().enumerate() {
                let (reference, w, h) = &levels[level];
                let (grey, _, _) = luminance(frame, width);
                if level > 0 {
                    let ratio = *w as f32 / levels[level - 1].1 as f32;
                    shift = (
                        (shift.0 as f32 * ratio).round() as i32,
                        (shift.1 as f32 * ratio).round() as i32,
                    );
                }
                let step = (*w as usize / 384).max(1);
                let mut best = (f32::MAX, shift);
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let candidate = (shift.0 + dx, shift.1 + dy);
                        let c = cost(reference, &grey, (*w, *h), candidate, step);
                        if c < best.0 {
                            best = (c, candidate);
                        }
                    }
                }
                shift = best.1;
            }
            let scale = size.0 as f32 / levels[LEVELS.len() - 1].1 as f32;
            Ok((
                (shift.0 as f32 * scale).round() as i32,
                (shift.1 as f32 * scale).round() as i32,
            ))
        })
        .collect()
}

/// The frame with each pixel taken `shift` away, the edges repeated: the
/// background of a frame given by `register` lies on the main frame.
pub fn shifted(frame: &RgbImage, shift: (i32, i32)) -> RgbImage {
    let (width, height) = frame.dimensions();
    RgbImage::from_fn(width, height, |x, y| {
        let fx = (x as i32 + shift.0).clamp(0, width as i32 - 1) as u32;
        let fy = (y as i32 + shift.1).clamp(0, height as i32 - 1) as u32;
        *frame.get_pixel(fx, fy)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// A dark picture with a bright lamp and a bar at `(ox, oy)` from the
    /// corner, so that moving it is moving the camera.
    fn scene(ox: i32, oy: i32, subject: Option<(i32, i32)>) -> RgbImage {
        RgbImage::from_fn(400, 300, |x, y| {
            let (x, y) = (x as i32 - ox, y as i32 - oy);
            let lamp = (x - 200).pow(2) + (y - 80).pow(2) < 30 * 30;
            let bar = (100..130).contains(&x) && (150..250).contains(&y);
            let man = subject.is_some_and(|(sx, sy)| (x - sx).abs() < 20 && (y - sy).abs() < 40);
            if lamp {
                Rgb([250, 250, 250])
            } else if man {
                Rgb([200, 120, 80])
            } else if bar {
                Rgb([120, 120, 160])
            } else {
                Rgb([10, 10, 30])
            }
        })
    }

    #[test]
    fn the_shift_of_the_camera_is_found_and_undone() {
        let frames = vec![
            scene(0, 0, Some((100, 200))),
            scene(12, -7, Some((300, 220))),
        ];
        let found = register(&frames, 0).unwrap();
        assert_eq!(found[0], (0, 0));
        let (dx, dy) = found[1];
        assert!((dx - 12).abs() <= 3 && (dy + 7).abs() <= 3, "{found:?}");
        let back = shifted(&frames[1], found[1]);
        assert!(back.get_pixel(200, 80).0[0] > 200, "the lamp is back");
    }

    #[test]
    fn mistakes_are_named() {
        let frames = vec![scene(0, 0, None), RgbImage::new(10, 10)];
        assert!(register(&frames, 0).is_err());
        assert!(register(&frames, 5).is_err());
    }
}
