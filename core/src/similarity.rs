//! Similarity between shots, to sort a selection into subfolders: shots
//! of the same scene, with the same light, end up together.
//!
//! A shot is reduced to a small signature, a colour histogram plus a
//! tiny greyscale picture of its layout, read from the thumbnail stored in
//! its metadata. Signatures are compared two by two, then clustered.

use image::DynamicImage;
use image::imageops::FilterType;
use rayon::prelude::*;

use crate::pairing::Shot;
use crate::{exif, picture};

/// Bins per colour channel of the histogram.
const BINS: usize = 4;
/// Size of the tiny greyscale picture.
const TINY_WIDTH: u32 = 8;
const TINY_HEIGHT: u32 = 6;
/// Pixels whose brightest channel is under this are left out of the
/// histogram: on a dark stage, black would drown the colour of the lights.
const DARK: u8 = 48;
/// The gamma of the thumbnails, to bring their luminance back to linear.
const GAMMA: f32 = 2.2;
/// A difference of 1/TINT_RANGE in the tint counts as a full difference.
const TINT_RANGE: f32 = 0.25;

/// Under this share of saturation in the mean tint of a group, its light
/// has no colour worth a name.
const NEUTRAL: f32 = 0.08;

/// How much each part of the signature counts in the distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub colour: f32,
    pub layout: f32,
    pub tint: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            colour: 1.0,
            layout: 1.0,
            tint: 0.0,
        }
    }
}

/// What a shot looks like, in a few dozen numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// Share of the lit pixels in each colour bin, summing to 1 (0 for a black picture).
    histogram: [f32; BINS * BINS * BINS],
    /// Luminance of each cell of the tiny picture, from 0 to 1.
    tiny: [f32; (TINY_WIDTH * TINY_HEIGHT) as usize],
    /// Red and blue share of the light, from the mean colour of the lit
    /// pixels: the cast of the light, whatever it falls on.
    tint: [f32; 2],
}

impl Signature {
    /// The signature of a picture; any size will do, a thumbnail is enough.
    pub fn of(image: &DynamicImage) -> Signature {
        let rgb = image.to_rgb8();
        let lit = rgb
            .pixels()
            .filter(|pixel| pixel.0.iter().max().is_some_and(|&max| max >= DARK))
            .count()
            .max(1) as f32;
        let mut histogram = [0.0; BINS * BINS * BINS];
        let mut sum = [0.0_f32; 3];
        for pixel in rgb
            .pixels()
            .filter(|pixel| pixel.0.iter().max().is_some_and(|&max| max >= DARK))
        {
            let [r, g, b] = pixel.0.map(|value| usize::from(value) * BINS / 256);
            histogram[(r * BINS + g) * BINS + b] += 1.0 / lit;
            for (total, value) in sum.iter_mut().zip(pixel.0) {
                *total += f32::from(value);
            }
        }
        let light = sum.iter().sum::<f32>().max(1.0);
        let tint = [sum[0] / light, sum[2] / light];

        let small = image
            .resize_exact(TINY_WIDTH, TINY_HEIGHT, FilterType::Triangle)
            .to_luma8();
        let mut tiny = [0.0; (TINY_WIDTH * TINY_HEIGHT) as usize];
        for (cell, pixel) in tiny.iter_mut().zip(small.pixels()) {
            *cell = f32::from(pixel.0[0]) / 255.0;
        }
        Signature {
            histogram,
            tiny,
            tint,
        }
    }

    /// The mean luminance of the picture, linear, from 0 to 1: the
    /// brightness a scene has before any edit.
    pub fn luminance(&self) -> f32 {
        let linear: f32 = self.tiny.iter().map(|cell| cell.powf(GAMMA)).sum();
        linear / self.tiny.len() as f32
    }

    /// How different two shots look, from 0 (the same) to 1 (nothing in
    /// common): the mean of the colour difference and the layout difference.
    pub fn distance(&self, other: &Signature) -> f32 {
        self.distance_with(other, Weights::default())
    }

    /// The same, with the parts of the signature weighted as given.
    pub fn distance_with(&self, other: &Signature, weights: Weights) -> f32 {
        let colour: f32 = self
            .histogram
            .iter()
            .zip(&other.histogram)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / 2.0;
        let layout: f32 = self
            .tiny
            .iter()
            .zip(&other.tiny)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / self.tiny.len() as f32;
        let tint = ((self.tint[0] - other.tint[0]).abs() + (self.tint[1] - other.tint[1]).abs())
            / TINT_RANGE;
        let total = weights.colour + weights.layout + weights.tint;
        (weights.colour * colour + weights.layout * layout + weights.tint * tint.min(1.0)) / total
    }
}

/// What decides which shots end up together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Groups are merged while their mean distance stays under this.
    pub threshold: f32,
    /// How much shots taken close together are brought nearer, 0 to 1.
    pub time_weight: f32,
    /// The gap in seconds from which time does not count any more.
    pub time_scale_s: i64,
    pub weights: Weights,
}

/// Calibrated on a concert shoot sorted by hand: shots a photographer
/// treated with the same settings are found with about 0.6 of the pairs
/// right, against 0.25 without the time.
impl Default for Settings {
    fn default() -> Self {
        Settings {
            threshold: 0.2,
            time_weight: 0.7,
            time_scale_s: 600,
            weights: Weights::default(),
        }
    }
}

/// Groups shots from their signatures and the time they were taken.
pub fn group(
    signatures: &[Signature],
    times: &[Option<i64>],
    settings: &Settings,
) -> Vec<Vec<usize>> {
    let distance = |i: usize, j: usize| {
        let gap = times[i].zip(times[j]).map(|(a, b)| b - a);
        with_time(
            signatures[i].distance_with(&signatures[j], settings.weights),
            gap,
            settings.time_weight,
            settings.time_scale_s * 1000,
        )
    };
    cluster_by(signatures.len(), distance, settings.threshold)
}

/// Shots that look alike, and the name of the colour of their light.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// Indices into the shots grouped, ascending.
    pub members: Vec<usize>,
    pub colour: &'static str,
}

/// The mean luminance of the shots, linear, from their thumbnails;
/// `None` when none of them has a readable one.
pub fn mean_luminance(shots: &[Shot]) -> Option<f32> {
    let luminances: Vec<f32> = shots
        .par_iter()
        .filter_map(|shot| Some(Signature::of(&picture::thumbnail(shot)?).luminance()))
        .collect();
    (!luminances.is_empty()).then(|| luminances.iter().sum::<f32>() / luminances.len() as f32)
}

/// Groups shots by reading the thumbnail and the time of each one.
/// Indices are those of `shots`; a shot with no readable thumbnail is
/// in no group.
pub fn group_shots(shots: &[Shot], settings: &Settings) -> Vec<Group> {
    let read: Vec<Option<(Signature, Option<i64>)>> = shots
        .par_iter()
        .map(|shot| {
            let signature = Signature::of(&picture::thumbnail(shot)?);
            let taken = exif::read(shot).ok().and_then(|exif| exif.taken_ms);
            Some((signature, taken))
        })
        .collect();
    let (mut kept, mut signatures, mut times) = (Vec::new(), Vec::new(), Vec::new());
    for (index, read) in read.into_iter().enumerate() {
        if let Some((signature, taken)) = read {
            kept.push(index);
            signatures.push(signature);
            times.push(taken);
        }
    }
    group(&signatures, &times, settings)
        .into_iter()
        .map(|members| Group {
            colour: colour_of(&signatures, &members),
            members: members.into_iter().map(|i| kept[i]).collect(),
        })
        .collect()
}

/// The name of the colour of the light in a group of shots, from the mean
/// tint of its members: `rouge`, `orange`, `jaune`, `vert`, `cyan`, `bleu`,
/// `violet`, `rose`, or `neutre` when no colour stands out.
pub fn colour_of(signatures: &[Signature], members: &[usize]) -> &'static str {
    let count = members.len().max(1) as f32;
    let red = members.iter().map(|&i| signatures[i].tint[0]).sum::<f32>() / count;
    let blue = members.iter().map(|&i| signatures[i].tint[1]).sum::<f32>() / count;
    colour_name([red, 1.0 - red - blue, blue])
}

/// Names a colour given as the shares of red, green and blue of the light.
fn colour_name([r, g, b]: [f32; 3]) -> &'static str {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max <= 0.0 || (max - min) / max < NEUTRAL {
        return "neutre";
    }
    let spread = max - min;
    let hue = if max == r {
        ((g - b) / spread).rem_euclid(6.0)
    } else if max == g {
        (b - r) / spread + 2.0
    } else {
        (r - g) / spread + 4.0
    } * 60.0;
    match hue {
        h if !(15.0..345.0).contains(&h) => "rouge",
        h if h < 45.0 => "orange",
        h if h < 70.0 => "jaune",
        h if h < 160.0 => "vert",
        h if h < 200.0 => "cyan",
        h if h < 260.0 => "bleu",
        h if h < 300.0 => "violet",
        _ => "rose",
    }
}

/// Brings two shots taken close together nearer: the look distance is
/// reduced by `weight` (0 to 1) when they were taken at the same moment,
/// less and less as the gap grows, and not at all from `scale_ms` on or
/// when a time is unknown. Shots far apart in time are never pushed
/// away, so a light seen twice in a show still groups; and a burst whose
/// colours change is only helped, not forced together.
pub fn with_time(look: f32, gap_ms: Option<i64>, weight: f32, scale_ms: i64) -> f32 {
    let Some(gap) = gap_ms else {
        return look;
    };
    let closeness = 1.0 - (gap.abs() as f32 / scale_ms.max(1) as f32).min(1.0);
    look * (1.0 - weight * closeness)
}

/// Groups signatures that look alike: groups are merged, closest first,
/// while their mean distance stays under `threshold` (average linkage).
/// Each group lists its indices in ascending order, and groups come in
/// the order of their first index.
pub fn cluster(signatures: &[Signature], threshold: f32) -> Vec<Vec<usize>> {
    cluster_by(
        signatures.len(),
        |i, j| signatures[i].distance(&signatures[j]),
        threshold,
    )
}

/// The same for `count` shots whose distance is given by a function, so
/// that other things than the look, the time for instance, can count.
pub fn cluster_by(
    count: usize,
    distance: impl Fn(usize, usize) -> f32,
    threshold: f32,
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Option<Vec<usize>>> = (0..count).map(|i| Some(vec![i])).collect();
    // Mean distance between two groups, kept up to date at every merge.
    let mut distances: Vec<Vec<f32>> = (0..count)
        .map(|i| (0..count).map(|j| distance(i, j)).collect())
        .collect();

    loop {
        let mut closest: Option<(usize, usize, f32)> = None;
        for i in 0..count {
            for j in 0..i {
                if groups[i].is_none() || groups[j].is_none() {
                    continue;
                }
                if closest.is_none_or(|(_, _, best)| distances[i][j] < best) {
                    closest = Some((j, i, distances[i][j]));
                }
            }
        }
        let Some((keep, absorb, distance)) = closest else {
            break;
        };
        if distance > threshold {
            break;
        }
        let absorbed = groups[absorb].take().expect("checked above");
        let kept = groups[keep].as_mut().expect("checked above");
        let (kept_size, absorbed_size) = (kept.len() as f32, absorbed.len() as f32);
        kept.extend(absorbed);
        for other in (0..count).filter(|&other| other != keep && other != absorb) {
            let merged = (distances[keep][other] * kept_size
                + distances[absorb][other] * absorbed_size)
                / (kept_size + absorbed_size);
            distances[keep][other] = merged;
            distances[other][keep] = merged;
        }
    }

    let mut groups: Vec<Vec<usize>> = groups.into_iter().flatten().collect();
    for group in &mut groups {
        group.sort_unstable();
    }
    groups.sort_unstable_by_key(|group| group[0]);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// A picture of one colour, `top` on its upper half.
    fn picture(top: [u8; 3], bottom: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(32, 24, |_, y| {
            Rgb(if y < 12 { top } else { bottom })
        }))
    }

    fn signature(top: [u8; 3], bottom: [u8; 3]) -> Signature {
        Signature::of(&picture(top, bottom))
    }

    #[test]
    fn a_picture_is_at_distance_zero_of_itself() {
        let a = signature([200, 30, 30], [20, 20, 90]);
        assert_eq!(a.distance(&a), 0.0);
    }

    #[test]
    fn distance_grows_with_the_difference() {
        let red = signature([200, 30, 30], [180, 20, 20]);
        let almost = signature([205, 35, 30], [185, 25, 20]);
        let blue = signature([20, 30, 200], [10, 10, 220]);
        assert!(red.distance(&almost) < red.distance(&blue));
        assert_eq!(red.distance(&blue), blue.distance(&red));
        assert!((0.0..=1.0).contains(&red.distance(&blue)));
    }

    #[test]
    fn similar_shots_are_grouped_and_different_ones_apart() {
        let signatures = [
            signature([200, 30, 30], [180, 20, 20]),
            signature([20, 30, 200], [10, 10, 220]),
            signature([205, 35, 30], [185, 25, 20]),
            signature([25, 30, 205], [10, 15, 220]),
            signature([20, 200, 30], [30, 220, 20]),
        ];
        assert_eq!(cluster(&signatures, 0.2), [vec![0, 2], vec![1, 3], vec![4]]);
    }

    #[test]
    fn the_threshold_decides_how_far_groups_merge() {
        let signatures = [
            signature([200, 30, 30], [180, 20, 20]),
            signature([20, 30, 200], [10, 10, 220]),
        ];
        assert_eq!(cluster(&signatures, 0.0).len(), 2);
        assert_eq!(cluster(&signatures, 1.0), [vec![0, 1]]);
    }

    #[test]
    fn the_tint_follows_the_cast_of_the_light_not_its_layout() {
        let tint_only = Weights {
            colour: 0.0,
            layout: 0.0,
            tint: 1.0,
        };
        let warm = signature([200, 120, 60], [180, 100, 50]);
        let warm_flipped = signature([180, 100, 50], [200, 120, 60]);
        let cold = signature([60, 120, 200], [50, 100, 180]);
        assert!(warm.distance_with(&warm_flipped, tint_only) < 0.01);
        assert!(warm.distance_with(&cold, tint_only) > 0.5);
    }

    #[test]
    fn closeness_in_time_reduces_the_distance_only_up_to_the_scale() {
        assert_eq!(with_time(0.4, Some(0), 0.5, 60_000), 0.2);
        assert!((with_time(0.4, Some(30_000), 0.5, 60_000) - 0.3).abs() < 1e-6);
        assert_eq!(with_time(0.4, Some(-60_000), 0.5, 60_000), 0.4);
        assert_eq!(with_time(0.4, Some(3_600_000), 0.5, 60_000), 0.4);
        assert_eq!(with_time(0.4, None, 0.5, 60_000), 0.4);
        assert_eq!(with_time(0.4, Some(0), 0.0, 60_000), 0.4);
    }

    #[test]
    fn a_burst_with_changing_colours_can_group_thanks_to_time() {
        let signatures = [
            signature([200, 30, 30], [180, 20, 20]),
            signature([20, 30, 200], [10, 10, 220]),
        ];
        let look = signatures[0].distance(&signatures[1]);
        let apart = cluster_by(2, |i, j| signatures[i].distance(&signatures[j]), look * 0.6);
        assert_eq!(apart.len(), 2);
        let near = cluster_by(
            2,
            |i, j| {
                with_time(
                    signatures[i].distance(&signatures[j]),
                    Some(100),
                    0.5,
                    60_000,
                )
            },
            look * 0.6,
        );
        assert_eq!(near, [vec![0, 1]]);
    }

    #[test]
    fn luminance_is_linear_and_grows_with_the_light() {
        let black = signature([0, 0, 0], [0, 0, 0]);
        let grey = signature([128, 128, 128], [128, 128, 128]);
        let white = signature([255, 255, 255], [255, 255, 255]);
        assert_eq!(black.luminance(), 0.0);
        // Mid grey in the thumbnail is about a fifth of the light of white.
        assert!(
            (grey.luminance() - 0.2).abs() < 0.02,
            "{}",
            grey.luminance()
        );
        assert!((white.luminance() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_colour_of_the_light_gets_a_name() {
        let group = |colour: [u8; 3]| vec![signature(colour, colour)];
        for (colour, name) in [
            ([200, 40, 40], "rouge"),
            ([220, 130, 40], "orange"),
            ([200, 190, 40], "jaune"),
            ([40, 200, 60], "vert"),
            ([40, 190, 200], "cyan"),
            ([40, 70, 210], "bleu"),
            ([130, 40, 210], "violet"),
            ([210, 40, 150], "rose"),
            ([120, 120, 120], "neutre"),
        ] {
            assert_eq!(colour_of(&group(colour), &[0]), name);
        }
    }

    #[test]
    fn nothing_to_cluster_gives_no_group() {
        assert!(cluster(&[], 0.3).is_empty());
    }
}
