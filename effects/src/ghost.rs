//! Ghosts: several frames of one scene, taken one after another, laid over
//! one another with the subject kept in the same place, so that the pose
//! of each leaves a ghost round it, a dancer with many arms and legs.
//!
//! Unlike the other effects it takes many pictures, not one, and does not
//! go in a recipe. Experimental: it may still change.

use std::fmt;
use std::str::FromStr;

use image::RgbImage;

use super::Error;
use super::zone::smoothstep;

/// How the frames are laid over the main one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GhostBlend {
    /// The brighter of the two, pixel by pixel: the poses stay sharp and
    /// stack, which suits a bright subject on a dark stage.
    #[default]
    Lighten,
    /// Screen: the lights add up without clipping.
    Screen,
    /// Every frame counts the same: the poses are all faint.
    Average,
    /// The frames mixed like see-through layers: the main frame weighs 1 and
    /// each ghost the opacity, 1 giving the same weight to all. No light is
    /// added, the poses show through one another.
    Over,
}

impl FromStr for GhostBlend {
    type Err = String;

    fn from_str(text: &str) -> Result<GhostBlend, String> {
        match text {
            "lighten" => Ok(GhostBlend::Lighten),
            "screen" => Ok(GhostBlend::Screen),
            "average" => Ok(GhostBlend::Average),
            "over" => Ok(GhostBlend::Over),
            _ => Err(format!("'{text}' is not lighten, screen, average or over")),
        }
    }
}

impl fmt::Display for GhostBlend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GhostBlend::Lighten => "lighten",
            GhostBlend::Screen => "screen",
            GhostBlend::Average => "average",
            GhostBlend::Over => "over",
        })
    }
}

/// Parameters of the ghosts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghost {
    pub blend: GhostBlend,
    /// How much each ghost shows, 0 to 1: the main frame is whole.
    pub opacity: f32,
    /// How much the picture is cut round the subject: the window is the
    /// frame divided by this, 1 or more.
    pub zoom: f32,
    /// Keep the background of the main frame: of the other frames, only
    /// what changes against the middle value of each pixel, the subject,
    /// is laid over it, so that a lamp or the floor is not repeated where
    /// the windows moved. At least three frames are needed.
    pub pin_background: bool,
}

impl Default for Ghost {
    fn default() -> Ghost {
        Ghost {
            blend: GhostBlend::Lighten,
            opacity: 1.0,
            zoom: 2.0,
            pin_background: false,
        }
    }
}

/// The window of `size` round `centre`, kept inside the frame, and where
/// its corner is in the frame.
fn window(frame: &RgbImage, centre: (f32, f32), size: (u32, u32)) -> (RgbImage, (u32, u32)) {
    let left = (centre.0 - size.0 as f32 / 2.0)
        .round()
        .clamp(0.0, (frame.width() - size.0) as f32);
    let top = (centre.1 - size.1 as f32 / 2.0)
        .round()
        .clamp(0.0, (frame.height() - size.1) as f32);
    let corner = (left as u32, top as u32);
    let cut = image::imageops::crop_imm(frame, corner.0, corner.1, size.0, size.1).to_image();
    (cut, corner)
}

/// The side, in pixels, the long side of a frame is reduced to to look
/// for the subject: it is the position that counts, not the detail.
const TRACK_SIDE: u32 = 192;
/// How much a pixel must differ from the background, in levels of any
/// channel, to belong to the subject.
const TRACK_MIN_DIFFERENCE: f32 = 24.0;
/// The side the frames are reduced to to tell the subject from the
/// background when the background is kept.
const SUBJECT_SIDE: u32 = 384;
/// The difference from the background, in levels, under which a pixel is
/// the background and over which it is the subject, with a fade between.
const SUBJECT_FROM: f32 = 12.0;
const SUBJECT_TO: f32 = 36.0;
/// How many cells the subject is spread outwards by, then how many times it
/// is softened.
const SUBJECT_SPREAD: u32 = 2;
const SUBJECT_SOFTEN: u32 = 3;

/// How much each pixel of each frame differs from the middle value of that
/// pixel over all the frames, on frames reduced to `side` pixels long: the
/// background is what stays, and what differs from it is what moves.
struct Differences {
    size: (u32, u32),
    /// Pixels of the frame for a pixel of the reduced one.
    scale: (f32, f32),
    /// For each frame, the largest difference of a channel, per pixel.
    maps: Vec<Vec<f32>>,
}

fn differences(frames: &[RgbImage], side: u32) -> Result<Differences, Error> {
    if frames.len() < 3 {
        return Err(Error::Frames(
            "this needs at least three frames, for a middle value to mean something".to_owned(),
        ));
    }
    let (width, height) = frames[0].dimensions();
    if frames
        .iter()
        .any(|frame| frame.dimensions() != (width, height))
    {
        return Err(Error::Frames(
            "the frames are not all the same size".to_owned(),
        ));
    }
    let reduction = (side as f32 / width.max(height) as f32).min(1.0);
    let size = (
        ((width as f32 * reduction).round() as u32).max(1),
        ((height as f32 * reduction).round() as u32).max(1),
    );
    let small: Vec<RgbImage> = frames
        .iter()
        .map(|frame| {
            image::imageops::resize(frame, size.0, size.1, image::imageops::FilterType::Triangle)
        })
        .collect();
    let samples = (size.0 * size.1) as usize * 3;
    let mut middle = vec![0u8; samples];
    let mut column = vec![0u8; small.len()];
    for (i, value) in middle.iter_mut().enumerate() {
        for (slot, frame) in column.iter_mut().zip(&small) {
            *slot = frame.as_raw()[i];
        }
        column.sort_unstable();
        *value = column[column.len() / 2];
    }
    let maps = small
        .iter()
        .map(|frame| {
            frame
                .pixels()
                .enumerate()
                .map(|(i, pixel)| {
                    (0..3)
                        .map(|c| (f32::from(pixel.0[c]) - f32::from(middle[i * 3 + c])).abs())
                        .fold(0.0, f32::max)
                })
                .collect()
        })
        .collect();
    Ok(Differences {
        size,
        scale: (width as f32 / size.0 as f32, height as f32 / size.1 as f32),
        maps,
    })
}

/// Where the subject is in each frame, in pixels of the frame, or `None`
/// where it cannot be told. The camera must not move: the background is
/// taken as the middle value of each pixel over all the frames, so what
/// moves, a subject that crosses the frame, stands out against it, and what
/// stays, the stage and the lights, does not. The subject is the centre of
/// what stands out. A subject that stays where it is in every frame is in
/// the background too, and is not found: give its place instead. At least
/// three frames are needed, for a middle value to mean something.
pub fn track(frames: &[RgbImage]) -> Result<Vec<Option<(f32, f32)>>, Error> {
    let found = differences(frames, TRACK_SIDE)?;
    let (size, scale) = (found.size, found.scale);
    Ok(found
        .maps
        .iter()
        .map(|map| {
            // Where the weight is: what stands out enough.
            let weight: Vec<f32> = map
                .iter()
                .map(|&difference| {
                    if difference >= TRACK_MIN_DIFFERENCE {
                        difference
                    } else {
                        0.0
                    }
                })
                .collect();
            let centre = centroid(&weight, size.0, None)?;
            // A speck far from the rest, a flicker, pulls the first centre:
            // look again among what is near it.
            let spread = spread(&weight, size.0, centre);
            let centre =
                centroid(&weight, size.0, Some((centre, 2.0 * spread.max(2.0)))).unwrap_or(centre);
            Some(((centre.0 + 0.5) * scale.0, (centre.1 + 0.5) * scale.1))
        })
        .collect())
}

/// For each frame, how much of each pixel of the reduced frame belongs to
/// what moves, 0 to 1. What differs from the background is spread a little
/// outwards then softened: a limb is whole inside and its edge is not a
/// line, where a threshold alone leaves holes and ragged bits.
fn subject_masks(found: &Differences) -> Vec<Vec<f32>> {
    found
        .maps
        .iter()
        .map(|map| {
            let mut mask: Vec<f32> = map
                .iter()
                .map(|&difference| smoothstep(SUBJECT_FROM, SUBJECT_TO, difference))
                .collect();
            for _ in 0..SUBJECT_SPREAD {
                mask = neighbours(&mask, found.size, f32::max);
            }
            for _ in 0..SUBJECT_SOFTEN {
                mask = box_blur(&mask, found.size);
            }
            mask
        })
        .collect()
}

/// Each cell combined with its eight neighbours by `combine`.
fn neighbours(cells: &[f32], size: (u32, u32), combine: fn(f32, f32) -> f32) -> Vec<f32> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut out = cells.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut value = cells[(y * w + x) as usize];
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nx, ny) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                value = combine(value, cells[(ny * w + nx) as usize]);
            }
            out[(y * w + x) as usize] = value;
        }
    }
    out
}

/// The mean of each cell and its eight neighbours.
fn box_blur(cells: &[f32], size: (u32, u32)) -> Vec<f32> {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let mut out = cells.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = ((x + dx).clamp(0, w - 1), (y + dy).clamp(0, h - 1));
                    sum += cells[(ny * w + nx) as usize];
                }
            }
            out[(y * w + x) as usize] = sum / 9.0;
        }
    }
    out
}

/// How much of each pixel of a frame belongs to what moves, read at any
/// pixel of the frame by interpolation.
struct Subject<'a> {
    found: &'a Differences,
    mask: &'a [f32],
}

impl Subject<'_> {
    fn at(&self, x: u32, y: u32) -> f32 {
        let found = self.found;
        let share = |cell: (u32, u32)| {
            let (cx, cy) = (cell.0.min(found.size.0 - 1), cell.1.min(found.size.1 - 1));
            self.mask[(cy * found.size.0 + cx) as usize]
        };
        let fx = ((x as f32 + 0.5) / found.scale.0 - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) / found.scale.1 - 0.5).max(0.0);
        let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = share((x0, y0)) * (1.0 - tx) + share((x0 + 1, y0)) * tx;
        let bottom = share((x0, y0 + 1)) * (1.0 - tx) + share((x0 + 1, y0 + 1)) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// The centre of the weights of a picture `width` wide, among those within
/// a distance of a point when one is given; `None` where there are none.
fn centroid(weight: &[f32], width: u32, near: Option<((f32, f32), f32)>) -> Option<(f32, f32)> {
    let (mut total, mut x_sum, mut y_sum) = (0.0f32, 0.0f32, 0.0f32);
    for (i, &value) in weight.iter().enumerate() {
        if value <= 0.0 {
            continue;
        }
        let (x, y) = ((i as u32 % width) as f32, (i as u32 / width) as f32);
        if let Some((point, reach)) = near
            && ((x - point.0).powi(2) + (y - point.1).powi(2)).sqrt() > reach
        {
            continue;
        }
        total += value;
        x_sum += value * x;
        y_sum += value * y;
    }
    (total > 0.0).then(|| (x_sum / total, y_sum / total))
}

/// How far the weights spread round a point: the root mean square of the
/// distances, weighted.
fn spread(weight: &[f32], width: u32, point: (f32, f32)) -> f32 {
    let (mut total, mut square) = (0.0f32, 0.0f32);
    for (i, &value) in weight.iter().enumerate() {
        if value > 0.0 {
            let (x, y) = ((i as u32 % width) as f32, (i as u32 / width) as f32);
            total += value;
            square += value * ((x - point.0).powi(2) + (y - point.1).powi(2));
        }
    }
    if total > 0.0 {
        (square / total).sqrt()
    } else {
        0.0
    }
}

/// Lays the frames over the main one, each cut to a window round its own
/// subject, `centres` giving the subject of each in pixels, so that the
/// subject stays where it is in the main frame and the poses come apart
/// round it. The frames must have the same size.
pub fn ghost(
    frames: &[RgbImage],
    centres: &[(f32, f32)],
    main: usize,
    params: Ghost,
) -> Result<RgbImage, Error> {
    if frames.len() < 2 {
        return Err(Error::Frames("at least two frames are needed".to_owned()));
    }
    if centres.len() != frames.len() || main >= frames.len() {
        return Err(Error::Frames(
            "a subject for each frame, and a main frame among them".to_owned(),
        ));
    }
    let (width, height) = frames[0].dimensions();
    if frames
        .iter()
        .any(|frame| frame.dimensions() != (width, height))
    {
        return Err(Error::Frames(
            "the frames are not all the same size".to_owned(),
        ));
    }
    let zoom = params.zoom.max(1.0);
    let size = (
        ((width as f32 / zoom).round() as u32).clamp(1, width),
        ((height as f32 / zoom).round() as u32).clamp(1, height),
    );
    let cut: Vec<(RgbImage, (u32, u32))> = frames
        .iter()
        .zip(centres)
        .map(|(frame, &centre)| window(frame, centre, size))
        .collect();
    // For each frame, how much of each pixel of its window is the subject,
    // or everything when the background is not kept.
    let found = if params.pin_background {
        let found = differences(frames, SUBJECT_SIDE)?;
        let masks = subject_masks(&found);
        Some((found, masks))
    } else {
        None
    };
    let weight = |frame: usize, x: u32, y: u32| match &found {
        Some((found, masks)) => {
            let corner = cut[frame].1;
            let subject = Subject {
                found,
                mask: &masks[frame],
            };
            subject.at(corner.0 + x, corner.1 + y)
        }
        None => 1.0,
    };

    let opacity = params.opacity.clamp(0.0, 1.0);
    let count = frames.len() as f32;
    let mut result = cut[main].0.clone();
    let layers: Vec<usize> = (0..frames.len()).filter(|&i| i != main).collect();
    // For `Over`, how much each ghost weighs when the main frame weighs 1.
    let share = opacity / (1.0 + opacity * layers.len() as f32);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let base = result.get_pixel(x, y).0.map(f32::from);
            let mut mixed = base;
            for &i in &layers {
                let ghost = cut[i].0.get_pixel(x, y).0.map(f32::from);
                let strength = weight(i, x, y);
                for channel in 0..3 {
                    mixed[channel] = match params.blend {
                        // Each frame moves the picture a share of the way
                        // to itself, so that all of them weigh the same.
                        GhostBlend::Average => {
                            mixed[channel] + (ghost[channel] - base[channel]) * strength / count
                        }
                        GhostBlend::Over => {
                            mixed[channel] + (ghost[channel] - base[channel]) * share * strength
                        }
                        GhostBlend::Lighten => {
                            let lighter = mixed[channel].max(ghost[channel]);
                            mixed[channel] + (lighter - mixed[channel]) * opacity * strength
                        }
                        GhostBlend::Screen => {
                            let screened =
                                255.0 - (255.0 - mixed[channel]) * (255.0 - ghost[channel]) / 255.0;
                            mixed[channel] + (screened - mixed[channel]) * opacity * strength
                        }
                    };
                }
            }
            result.put_pixel(
                x,
                y,
                image::Rgb(mixed.map(|v| v.round().clamp(0.0, 255.0) as u8)),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// A dark frame with a bright dot at `at`.
    fn frame_with_dot(at: (u32, u32)) -> RgbImage {
        let mut frame = RgbImage::from_pixel(40, 30, Rgb([10, 10, 10]));
        frame.put_pixel(at.0, at.1, Rgb([250, 250, 250]));
        frame
    }

    fn bright(image: &RgbImage) -> usize {
        image.pixels().filter(|pixel| pixel.0[0] > 100).count()
    }

    #[test]
    fn frames_kept_on_their_subject_stack_in_one_place_and_without_it_they_spread() {
        let dots = [(10, 15), (20, 15), (30, 15)];
        let frames: Vec<RgbImage> = dots.iter().map(|&at| frame_with_dot(at)).collect();
        let aligned: Vec<(f32, f32)> = dots.iter().map(|&(x, y)| (x as f32, y as f32)).collect();
        let params = Ghost {
            zoom: 2.0,
            ..Ghost::default()
        };
        let stacked = ghost(&frames, &aligned, 1, params).unwrap();
        assert_eq!(bright(&stacked), 1, "the three dots fall on one");
        let fixed = vec![(20.0, 15.0); 3];
        let spread = ghost(
            &frames,
            &fixed,
            1,
            Ghost {
                zoom: 1.0,
                ..params
            },
        )
        .unwrap();
        assert_eq!(
            bright(&spread),
            3,
            "three dots when the window does not follow"
        );
    }

    /// A dark frame with a bright square, `side` wide, centred at `at`, and
    /// a bright spot that is in every frame, like a lamp.
    fn frame_with_square(at: (u32, u32), side: u32) -> RgbImage {
        let mut frame = RgbImage::from_pixel(160, 100, Rgb([12, 12, 12]));
        for dy in 0..side {
            for dx in 0..side {
                frame.put_pixel(
                    at.0 - side / 2 + dx,
                    at.1 - side / 2 + dy,
                    Rgb([220, 200, 190]),
                );
            }
        }
        for (x, y) in [(120, 20), (121, 20), (120, 21), (121, 21)] {
            frame.put_pixel(x, y, Rgb([255, 255, 255]));
        }
        frame
    }

    #[test]
    fn the_subject_that_crosses_the_frame_is_followed_and_a_lamp_is_not() {
        let places = [(20, 50), (45, 50), (70, 55), (95, 50), (120, 60)];
        let frames: Vec<RgbImage> = places.iter().map(|&at| frame_with_square(at, 14)).collect();
        let found = track(&frames).unwrap();
        for (place, found) in places.iter().zip(found) {
            let (x, y) = found.expect("the subject is found");
            assert!((x - place.0 as f32).abs() < 4.0, "{x} for {}", place.0);
            assert!((y - place.1 as f32).abs() < 4.0, "{y} for {}", place.1);
        }
    }

    #[test]
    fn a_subject_that_stays_is_in_the_background_and_a_few_frames_are_refused() {
        let still: Vec<RgbImage> = (0..4).map(|_| frame_with_square((80, 50), 14)).collect();
        assert!(track(&still).unwrap().iter().all(Option::is_none));
        assert!(track(&still[..2]).is_err());
        let other = vec![still[0].clone(), still[1].clone(), RgbImage::new(10, 10)];
        assert!(track(&other).is_err());
    }

    #[test]
    fn a_flicker_far_from_the_subject_does_not_pull_the_centre() {
        let places = [(20, 50), (45, 50), (70, 50), (95, 50), (120, 50)];
        let mut frames: Vec<RgbImage> =
            places.iter().map(|&at| frame_with_square(at, 14)).collect();
        // A bright speck in one frame only, far from the square.
        for (x, y) in [(10, 5), (11, 5), (10, 6), (11, 6)] {
            frames[2].put_pixel(x, y, Rgb([240, 240, 240]));
        }
        let (x, y) = track(&frames).unwrap()[2].unwrap();
        assert!((x - 70.0).abs() < 6.0 && (y - 50.0).abs() < 6.0, "{x},{y}");
    }

    #[test]
    fn pinning_the_background_keeps_one_lamp_and_not_one_per_window() {
        let places = [(30, 50), (60, 50), (90, 50), (120, 50), (150, 50)];
        let frames: Vec<RgbImage> = places.iter().map(|&at| frame_with_square(at, 14)).collect();
        let centres: Vec<(f32, f32)> = places.iter().map(|&(x, y)| (x as f32, y as f32)).collect();
        let lamps = |pin_background: bool| {
            let params = Ghost {
                zoom: 1.25,
                pin_background,
                ..Ghost::default()
            };
            let result = ghost(&frames, &centres, 2, params).unwrap();
            result.pixels().filter(|pixel| pixel.0[0] >= 250).count()
        };
        // The lamp is 2 by 2 pixels, in every frame; in the windows that
        // follow the square it lands in a different place each time.
        let spread = lamps(false);
        let pinned = lamps(true);
        assert!(spread > pinned, "{spread} against {pinned}");
        assert!(pinned <= 4, "the lamp of the main frame only: {pinned}");
        let params = Ghost {
            pin_background: true,
            ..Ghost::default()
        };
        assert!(ghost(&frames[..2], &centres[..2], 0, params).is_err());
    }

    #[test]
    fn opacity_decides_how_much_a_ghost_shows() {
        let frames = vec![frame_with_dot((5, 5)), frame_with_dot((30, 20))];
        let centres = vec![(20.0, 15.0); 2];
        let at = |opacity: f32| {
            let params = Ghost {
                zoom: 1.0,
                opacity,
                ..Ghost::default()
            };
            ghost(&frames, &centres, 0, params)
                .unwrap()
                .get_pixel(30, 20)
                .0[0]
        };
        assert_eq!(at(0.0), 10, "no ghost");
        assert_eq!(at(1.0), 250, "a whole ghost");
        let half = at(0.5);
        assert!(half > 100 && half < 150, "{half}");
    }

    #[test]
    fn the_blends_differ_and_the_main_frame_stays_whole_with_lighten() {
        let frames = vec![frame_with_dot((5, 5)), frame_with_dot((30, 20))];
        let centres = vec![(20.0, 15.0); 2];
        let blend = |blend| {
            let params = Ghost {
                blend,
                zoom: 1.0,
                ..Ghost::default()
            };
            ghost(&frames, &centres, 0, params).unwrap()
        };
        assert_eq!(blend(GhostBlend::Lighten).get_pixel(5, 5).0[0], 250);
        let average = blend(GhostBlend::Average);
        assert_eq!(average.get_pixel(5, 5).0[0], 130, "half of 250 and 10");
        let over = blend(GhostBlend::Over);
        assert_eq!(over.get_pixel(5, 5).0[0], 130, "the same weight to both");
        let params = Ghost {
            blend: GhostBlend::Over,
            opacity: 0.5,
            zoom: 1.0,
            ..Ghost::default()
        };
        let faint = ghost(&frames, &centres, 0, params).unwrap();
        assert_eq!(
            faint.get_pixel(5, 5).0[0],
            170,
            "the ghost weighs half the main frame"
        );
        let screen = blend(GhostBlend::Screen);
        assert!(screen.get_pixel(30, 20).0[0] >= 250 && screen.get_pixel(5, 5).0[0] == 250);
    }

    #[test]
    fn windows_stay_inside_the_frame_and_mistakes_are_named() {
        let frames = vec![frame_with_dot((1, 1)), frame_with_dot((2, 2))];
        // A subject at the very corner: the window is moved, not cut.
        let result = ghost(&frames, &[(0.0, 0.0), (0.0, 0.0)], 0, Ghost::default()).unwrap();
        assert_eq!(result.dimensions(), (20, 15));
        assert!(ghost(&frames[..1], &[(0.0, 0.0)], 0, Ghost::default()).is_err());
        assert!(ghost(&frames, &[(0.0, 0.0)], 0, Ghost::default()).is_err());
        assert!(ghost(&frames, &[(0.0, 0.0); 2], 5, Ghost::default()).is_err());
        let other = vec![frame_with_dot((1, 1)), RgbImage::new(10, 10)];
        assert!(ghost(&other, &[(0.0, 0.0); 2], 0, Ghost::default()).is_err());
    }

    #[test]
    fn blends_are_read_and_written_back() {
        for name in ["lighten", "screen", "average", "over"] {
            assert_eq!(name.parse::<GhostBlend>().unwrap().to_string(), name);
        }
        assert!("multiply".parse::<GhostBlend>().is_err());
    }
}
