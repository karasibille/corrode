//! Removing the light bands LED stage lighting leaves on photos taken
//! with an electronic shutter, from the raw sensor data.
//!
//! The sensor is read row after row while the LEDs flicker, so each row
//! was lit a little more or less than its neighbours: the bands are a
//! gain per sensor row, the same across the row, and periodic since the
//! flicker is. The gain is estimated per color of the color filter array,
//! because red, green and blue LEDs do not flicker alike, by folding the
//! brightness of the rows over the period; rows are then divided by it.
//! Colors without a clear periodic component are left alone, so that
//! noise is not stamped into them. The bands are not the same everywhere:
//! each lamp flickers with its own phase and lights its own part of the
//! scene, so the gains are folded for each block of a grid over the
//! picture and interpolated between blocks. A block whose fold shows no
//! coherent pattern, because it is dark or lit by a steady lamp, is left
//! alone too: correcting it would stamp noise or bands into it.
//!
//! The correction works on the raw mosaic, before demosaicing and the
//! camera's tone curve, where light is still linear: a band is then a
//! plain factor. The result is meant to be written as a DNG.

use std::f64::consts::PI;

use image::DynamicImage;
use rawler::rawimage::{RawImage, RawImageData};
use serde::{Deserialize, Serialize};

/// Shortest and longest period searched, in sensor rows.
const PERIODS: std::ops::RangeInclusive<usize> = 20..=400;
/// Half the window of the moving average that removes the scene's slow
/// changes from a row profile, in rows of one color: wider than the
/// longest period, so that the bands themselves survive it.
const SMOOTHING: usize = 200;
/// How far above its neighbours the strongest peak must stand for the
/// picture to have bands. Measured on this library's photos: pictures
/// without bands reach 16, light bands start at 22. The detector on the
/// preview (`banding`) is the safer judge of whether to correct at all.
const MIN_PEAK: f64 = 20.0;
/// How far above its neighbours a color's peak must stand for its bands
/// to be corrected, once the picture is known to have some.
const MIN_COLOR_PEAK: f64 = 20.0;
/// Phase bins the period is folded into.
const BINS: usize = 32;
/// Periods a block of the grid spans at least: enough rows for every bin
/// to average out the noise.
const BLOCK_PERIODS: f64 = 8.0;
/// The largest gain a band can have: bands are a few percent, anything
/// beyond is noise from a dark block.
const MAX_GAIN: f64 = 1.25;
/// Harmonics of the period the gains of a block are fitted with: enough
/// for the sharp edges of a dimmed LED, few enough to leave out noise.
const HARMONICS: usize = 4;
/// Share of a block's folded pattern the harmonics explain when it is
/// noise (about `HARMONICS` of `BINS / 2` frequencies) and when it is
/// bands: the correction of a block fades in between.
const NOISE_COHERENCE: f32 = 0.4;
const BAND_COHERENCE: f32 = 0.8;
/// The gamma of a camera's preview, near enough to take the bands off it.
const PREVIEW_GAMMA: f32 = 2.2;
/// Fraction of the light range, below white, over which the correction
/// fades out: a clipped pixel has no bands, and dividing it would make
/// some, while just under white its band is partly clipped already.
const CLIP_FADE: f32 = 0.2;
/// Above this fraction of the white level a pixel is taken for clipped
/// and left out of the measurement.
const CLIP_MEASURE: f32 = 0.95;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `best` is how far the strongest peak stood above its neighbours.
    #[error("no light bands found (strongest peak ×{best:.0})")]
    NoBands { best: f64 },
    #[error("the raw data is not integer")]
    FloatData,
    #[error("{0}")]
    Dng(#[from] crate::dng::Error),
}

/// The bands found on a picture: a gain per phase of the period, per
/// color of the color filter array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    /// The period of the bands, in sensor rows.
    pub period: f64,
    /// For each color, the gains over one period in each block of the
    /// grid; `None` for a color without bands.
    gains: [Option<Grid>; 4],
    /// For each color, how far its peak stood above its neighbours.
    pub peaks: [f64; 4],
    /// The same, measured again after the correction.
    pub residual: [f64; 4],
}

impl Pattern {
    /// The strength of the bands of a color: the largest deviation of its
    /// gain from 1, as a fraction, in the median block among those
    /// corrected. Zero for a color left alone.
    pub fn amplitude(&self, color: usize) -> f32 {
        self.gains[color].as_ref().map_or(0.0, |grid| {
            let mut amplitudes: Vec<f32> = grid
                .blocks
                .iter()
                .map(|block| block.iter().map(|g| (g - 1.0).abs()).fold(0.0, f32::max))
                .filter(|&amplitude| amplitude > 0.0)
                .collect();
            amplitudes.sort_by(|a, b| a.total_cmp(b));
            amplitudes.get(amplitudes.len() / 2).copied().unwrap_or(0.0)
        })
    }

    /// Whether any color is corrected.
    pub fn is_empty(&self) -> bool {
        self.gains.iter().all(Option::is_none)
    }

    /// Takes the bands off a preview of the picture, which shows the
    /// sensor `area` as `(left, top, width, height)` in the orientation of
    /// the sensor: each channel of a pixel is divided by the gain of the
    /// sensor rows it stands for, through the gamma of the preview. The
    /// greens of the mosaic both stand for the green channel.
    pub fn correct_preview(&self, preview: &mut DynamicImage, area: (usize, usize, usize, usize)) {
        let (left, top, width, height) = area;
        let mut rgb = preview.to_rgb8();
        let (columns, rows) = (rgb.width() as usize, rgb.height() as usize);
        if columns == 0 || rows == 0 || width == 0 || height == 0 {
            return;
        }
        let channels = [
            self.gains[0].as_ref(),
            self.gains[1].as_ref(),
            self.gains[2].as_ref(),
        ];
        for (py, row) in rgb.rows_mut().enumerate() {
            // The sensor rows this preview row stands for.
            let y0 = top + py * height / rows;
            let y1 = (top + (py + 1) * height / rows).max(y0 + 1);
            for (px, pixel) in row.enumerate() {
                let x = left + px * width / columns;
                for (channel, grid) in channels.iter().enumerate() {
                    let Some(grid) = grid else {
                        continue;
                    };
                    let gain = (y0..y1).map(|y| grid.gain(x, y, self.bin(y))).sum::<f32>()
                        / (y1 - y0) as f32;
                    let value = f32::from(pixel[channel]) / gain.powf(1.0 / PREVIEW_GAMMA);
                    pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        *preview = DynamicImage::ImageRgb8(rgb);
    }

    fn bin(&self, row: usize) -> usize {
        ((row as f64 / self.period).fract() * BINS as f64) as usize % BINS
    }
}

/// The gains of one color over one period, folded in each block of a
/// grid over the picture: `STRIPS` columns of blocks, each at least
/// `BLOCK_PERIODS` periods high.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    rows: usize,
    block_width: usize,
    block_height: usize,
    /// Row-major, `BINS` gains per block.
    blocks: Vec<Vec<f32>>,
}

impl Grid {
    /// The gain at a pixel for the phase bin of its row, interpolated
    /// between the centres of the four blocks around it.
    fn gain(&self, x: usize, y: usize, bin: usize) -> f32 {
        let position = |coordinate: usize, block: usize, count: usize| {
            let centred = (coordinate as f32 + 0.5) / block as f32 - 0.5;
            let clamped = centred.clamp(0.0, (count - 1) as f32);
            let low = clamped.floor() as usize;
            (low, (low + 1).min(count - 1), clamped - low as f32)
        };
        let (x0, x1, fx) = position(x, self.block_width, STRIPS);
        let (y0, y1, fy) = position(y, self.block_height, self.rows);
        let at = |bx: usize, by: usize| self.blocks[by * STRIPS + bx][bin];
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// A raw mosaic: `data[y * width + x]`, whose color is `color_at(y, x)`.
pub struct Mosaic<'a> {
    pub width: usize,
    pub height: usize,
    pub data: &'a [u16],
    /// The black level of each color, below which there is no light.
    pub black: [f32; 4],
    /// The white level of each color, where the sensor clips.
    pub white: [f32; 4],
    pub color_at: &'a dyn Fn(usize, usize) -> usize,
}

/// Column strips the mosaic is cut into: bands cross all of them, while
/// a feature of the scene seldom does.
const STRIPS: usize = 32;

/// For each of `strips` column strips, the rows holding a color as
/// `(row, light)`: the mean light of the row above black, in sensor
/// levels. Every strip has the same rows.
fn strip_profiles(mosaic: &Mosaic, color: usize, strips: usize) -> Vec<Vec<(usize, f64)>> {
    let Mosaic {
        width,
        height,
        data,
        black,
        white,
        color_at,
    } = mosaic;
    let strip_width = width / strips;
    if strip_width == 0 {
        return Vec::new();
    }
    let clip = f64::from(black[color] + CLIP_MEASURE * (white[color] - black[color]));
    let mut profiles: Vec<Vec<(usize, f64)>> = vec![Vec::new(); strips];
    for y in 0..*height {
        for (strip, profile) in profiles.iter_mut().enumerate() {
            // Clipped pixels show no bands: left out, unless the whole
            // row of the strip is clipped.
            let (mut sum, mut count) = (0f64, 0usize);
            let (mut clipped_sum, mut clipped) = (0f64, 0usize);
            for x in strip * strip_width..(strip + 1) * strip_width {
                if color_at(y, x) == color {
                    let value = f64::from(data[y * width + x]);
                    let light = (value - f64::from(black[color])).max(1.0);
                    if value < clip {
                        sum += light;
                        count += 1;
                    } else {
                        clipped_sum += light;
                        clipped += 1;
                    }
                }
            }
            if count == 0 {
                (sum, count) = (clipped_sum, clipped);
            }
            if count > 0 {
                profile.push((y, sum / count as f64));
            }
        }
    }
    let n = profiles[0].len();
    if n == 0 || profiles.iter().any(|p| p.len() != n) {
        return Vec::new();
    }
    profiles
}

/// The light of the scene along a profile, without the bands and the
/// noise: the mean of `light` over a window of `half` rows of the profile
/// on each side. Takes `light` and gives a value per row.
fn local_light(profile: &[(usize, f64)], half: usize, light: impl Fn(f64) -> f64) -> Vec<f64> {
    let n = profile.len();
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(n));
            profile[a..b].iter().map(|r| light(r.1)).sum::<f64>() / (b - a) as f64
        })
        .collect()
}

/// The rows holding a color, as `(row, brightness)`, with the scene's slow
/// changes removed. The brightness of a row is the median over column
/// strips, so that a feature of the scene in a few strips does not pass
/// for a band.
fn high_passed(mosaic: &Mosaic, color: usize) -> Vec<(usize, f64)> {
    let profiles = strip_profiles(mosaic, color, STRIPS);
    let Some(first) = profiles.first() else {
        return Vec::new();
    };
    let locals: Vec<Vec<f64>> = profiles
        .iter()
        .map(|strip| local_light(strip, SMOOTHING, f64::ln))
        .collect();
    (0..first.len())
        .map(|i| {
            let mut values: Vec<f64> = profiles
                .iter()
                .zip(&locals)
                .map(|(strip, local)| strip[i].1.ln() - local[i])
                .collect();
            values.sort_by(|a, b| a.total_cmp(b));
            (first[i].0, values[values.len() / 2])
        })
        .collect()
}

/// The power of the periodic component of a signal at a period.
fn power(signal: &[(usize, f64)], period: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for &(row, value) in signal {
        let angle = 2.0 * PI * row as f64 / period;
        re += value * angle.cos();
        im += value * angle.sin();
    }
    re * re + im * im
}

/// The power of a signal at every period searched, in tenths of a row.
struct Spectrum {
    periods: Vec<f64>,
    powers: Vec<f64>,
}

impl Spectrum {
    fn of(signal: &[(usize, f64)]) -> Spectrum {
        let periods: Vec<f64> = PERIODS
            .flat_map(|rows| (0..10).map(move |tenth| rows as f64 + tenth as f64 / 10.0))
            .collect();
        let powers = periods.iter().map(|&p| power(signal, p)).collect();
        Spectrum { periods, powers }
    }

    fn index_of(&self, period: f64) -> usize {
        let first = self.periods[0];
        ((period - first) * 10.0)
            .round()
            .clamp(0.0, (self.periods.len() - 1) as f64) as usize
    }

    /// How far the power at a period stands above the powers at the
    /// periods around it (from three quarters to four thirds of it,
    /// leaving out the peak itself): the scene has power at every period,
    /// more at long ones, while bands stand out from their neighbours.
    fn ratio_at(&self, index: usize) -> f64 {
        let period = self.periods[index];
        let (lo, hi) = (
            self.index_of(period * 0.75),
            self.index_of(period * 4.0 / 3.0),
        );
        let mut floor: Vec<f64> = (lo..=hi)
            .filter(|&i| (self.periods[i] - period).abs() > 0.03 * period)
            .map(|i| self.powers[i])
            .collect();
        if floor.is_empty() {
            return 0.0;
        }
        floor.sort_by(|a, b| a.total_cmp(b));
        self.powers[index] / floor[floor.len() / 2].max(f64::MIN_POSITIVE)
    }

    /// The ratio at the period nearest to `period`, or at its half when
    /// the light peaks twice per cycle.
    fn ratio(&self, period: f64) -> f64 {
        let at = |p: f64| {
            if p < self.periods[0] {
                0.0
            } else {
                self.ratio_at(self.index_of(p))
            }
        };
        at(period).max(at(period / 2.0))
    }

    /// The period standing the most above its neighbours, and its ratio.
    fn strongest(&self) -> (f64, f64) {
        (0..self.periods.len())
            .map(|i| (self.periods[i], self.ratio_at(i)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("periods are not empty")
    }
}

/// Refines a period to a hundredth of a row on the raw power of the
/// signal: the fold needs it exact over thousands of rows.
fn refine(signal: &[(usize, f64)], period: f64) -> f64 {
    (-50..=50)
        .map(|hundredth| period + f64::from(hundredth) / 100.0)
        .max_by(|a, b| power(signal, *a).total_cmp(&power(signal, *b)))
        .unwrap_or(period)
}

/// Finds the bands of a mosaic, if it has any.
pub fn estimate(mosaic: &Mosaic) -> Result<Pattern, Error> {
    let signals: Vec<Vec<(usize, f64)>> = (0..4).map(|c| high_passed(mosaic, c)).collect();
    let spectra: Vec<Option<Spectrum>> = signals
        .iter()
        .map(|signal| (signal.len() >= 3 * *PERIODS.end()).then(|| Spectrum::of(signal)))
        .collect();
    // The period comes from the color with the clearest bands; the others
    // share it, since the same lights flicker for all of them.
    let (mut period, best) = spectra
        .iter()
        .flatten()
        .map(Spectrum::strongest)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0.0, 0.0));
    if best < MIN_PEAK {
        return Err(Error::NoBands { best });
    }
    // The strongest peak may be a harmonic: when a color peaks at twice
    // the period as well, that is the fundamental, and folding over it
    // keeps both.
    if 2.0 * period <= *PERIODS.end() as f64
        && spectra
            .iter()
            .flatten()
            .any(|spectrum| spectrum.ratio_at(spectrum.index_of(2.0 * period)) >= MIN_COLOR_PEAK)
    {
        period *= 2.0;
    }
    let sharpest = spectra
        .iter()
        .zip(&signals)
        .filter_map(|(spectrum, signal)| Some((spectrum.as_ref()?, signal)))
        .max_by(|a, b| a.0.ratio(period).total_cmp(&b.0.ratio(period)))
        .map(|(_, signal)| signal)
        .expect("a spectrum gave the period");
    let period = refine(sharpest, period);

    let mut peaks = [0.0; 4];
    let mut gains: [Option<Grid>; 4] = [None, None, None, None];
    let block_height = ((BLOCK_PERIODS * period).ceil() as usize).max(1);
    let rows = (mosaic.height / block_height).max(1);
    let block_width = (mosaic.width / STRIPS).max(1);
    for color in 0..4 {
        let signal = &signals[color];
        if signal.is_empty() {
            continue;
        }
        let Some(spectrum) = &spectra[color] else {
            continue;
        };
        let peak = spectrum.ratio(period);
        peaks[color] = peak;
        if peak < MIN_COLOR_PEAK {
            continue;
        }
        // Fold each block's rows over the period: its own phase and
        // amplitude of the bands. The gain of a bin is the light it got
        // over the light the scene gives it, summed in linear units: in a
        // dark block the noise then averages out instead of blowing up.
        let fold = |profile: &[(usize, f64, f64)]| -> Vec<f32> {
            let (mut light, mut local) = ([0f64; BINS], [0f64; BINS]);
            for &(row, row_light, row_local) in profile {
                let bin = ((row as f64 / period).fract() * BINS as f64) as usize % BINS;
                light[bin] += row_light;
                local[bin] += row_local;
            }
            let folded: Vec<f32> = (0..BINS)
                .map(|bin| {
                    if local[bin] > 0.0 {
                        (light[bin] / local[bin]).clamp(1.0 / MAX_GAIN, MAX_GAIN) as f32
                    } else {
                        1.0
                    }
                })
                .collect();
            smooth_gains(&folded)
        };
        let strips = strip_profiles(mosaic, color, STRIPS);
        // The light of the scene is the mean over exactly two periods, a
        // window the bands sum to nothing in: it follows a feature of
        // the scene as small as a spotlight without taking in the bands,
        // where the wide window of the detection would pass the feature
        // for bands. The rows of a color are every other sensor row, so
        // a window of `period` rows of the profile spans two periods.
        let half = (period.round() as usize) / 2;
        let mut blocks = Vec::with_capacity(rows * STRIPS);
        let rows_of: Vec<Vec<(usize, f64, f64)>> = strips
            .iter()
            .map(|strip| {
                let local = local_light(strip, half, |light| light);
                strip
                    .iter()
                    .zip(local)
                    .map(|(&(row, light), local)| (row, light, local))
                    .collect()
            })
            .collect();
        for by in 0..rows {
            let top = by * block_height;
            // The last row of blocks takes the rows left over.
            let bottom = if by + 1 == rows {
                mosaic.height
            } else {
                top + block_height
            };
            for strip in &rows_of {
                let rows_in_block: Vec<(usize, f64, f64)> = strip
                    .iter()
                    .copied()
                    .filter(|(row, _, _)| (top..bottom).contains(row))
                    .collect();
                blocks.push(fold(&rows_in_block));
            }
        }
        gains[color] = Some(Grid {
            rows,
            block_width,
            block_height,
            blocks,
        });
    }
    Ok(Pattern {
        period,
        gains,
        peaks,
        residual: [0.0; 4],
    })
}

/// The gains of a block from its folded pattern: the first harmonics of
/// the period fitted to it, so that the noise of the fold is left out,
/// scaled by how much of the pattern they explain, so that a block
/// whose fold is noise, as in the dark or under a steady light, is left
/// alone. The gains average to 1: a block keeps its light.
fn smooth_gains(folded: &[f32]) -> Vec<f32> {
    let n = folded.len();
    let mean = folded.iter().sum::<f32>() / n as f32;
    let total: f32 = folded.iter().map(|g| (g - mean).powi(2)).sum();
    if total <= 0.0 {
        return vec![1.0; n];
    }
    let mut fit = vec![0f32; n];
    let mut explained = 0.0;
    for k in 1..=HARMONICS {
        let angle = |bin: usize| 2.0 * PI as f32 * (k * bin) as f32 / n as f32;
        let (mut re, mut im) = (0.0, 0.0);
        for (bin, g) in folded.iter().enumerate() {
            re += (g - mean) * angle(bin).cos();
            im += (g - mean) * angle(bin).sin();
        }
        explained += 2.0 * (re * re + im * im) / n as f32;
        for (bin, f) in fit.iter_mut().enumerate() {
            *f += 2.0 / n as f32 * (re * angle(bin).cos() + im * angle(bin).sin());
        }
    }
    let coherence = explained / total;
    let weight =
        ((coherence - NOISE_COHERENCE) / (BAND_COHERENCE - NOISE_COHERENCE)).clamp(0.0, 1.0);
    fit.iter().map(|f| 1.0 + f * weight).collect()
}

/// How far the bands of each color still stand out after a correction.
fn residual(mosaic: &Mosaic, period: f64) -> [f64; 4] {
    let mut residual = [0.0; 4];
    for (color, value) in residual.iter_mut().enumerate() {
        let signal = high_passed(mosaic, color);
        if signal.len() >= 3 * *PERIODS.end() {
            *value = Spectrum::of(&signal).ratio(period);
        }
    }
    residual
}

/// Divides every row by the gain of its phase, leaving the black level
/// and the colors without bands untouched.
pub fn remove(
    data: &mut [u16],
    width: usize,
    black: [f32; 4],
    white: [f32; 4],
    color_at: &dyn Fn(usize, usize) -> usize,
    pattern: &Pattern,
) {
    let height = data.len() / width;
    for y in 0..height {
        let bin = pattern.bin(y);
        for x in 0..width {
            let color = color_at(y, x);
            let Some(grid) = &pattern.gains[color] else {
                continue;
            };
            let value = &mut data[y * width + x];
            let light = f32::from(*value) - black[color];
            let range = white[color] - black[color];
            // The correction fades out towards white: a clipped pixel
            // has no bands to remove, and dividing it would make some.
            let fade = ((range - light) / (CLIP_FADE * range)).clamp(0.0, 1.0);
            let gain = 1.0 + (grid.gain(x, y, bin) - 1.0) * fade;
            *value = (light / gain + black[color])
                .round()
                .clamp(0.0, white[color]) as u16;
        }
    }
}

/// The DNG a debanded RAW is written to: next to it, with `-deband`
/// added to its name, so that it is a shot of its own.
pub fn output_path(raw: &std::path::Path) -> std::path::PathBuf {
    let stem = raw.file_stem().unwrap_or_default().to_string_lossy();
    raw.with_file_name(format!("{stem}-deband.dng"))
}

/// Reads a RAW file, removes its bands, from its preview too, and writes
/// the result as a DNG.
pub fn to_dng(raw: &std::path::Path, dng: &std::path::Path) -> Result<Pattern, Error> {
    crate::dng::rewrite(raw, dng, |raw, preview| {
        let pattern = deband(raw)?;
        // The preview shows the cropped area of the sensor.
        let area = match raw.crop_area {
            Some(crop) => (crop.p.x, crop.p.y, crop.d.w, crop.d.h),
            None => (0, 0, raw.width, raw.height),
        };
        pattern.correct_preview(preview, area);
        Ok(pattern)
    })
}

/// Finds and removes the bands of a raw image, in place.
pub fn deband(raw: &mut RawImage) -> Result<Pattern, Error> {
    let RawImageData::Integer(data) = &mut raw.data else {
        return Err(Error::FloatData);
    };
    let cfa = raw.camera.cfa.clone();
    let color_at = |y: usize, x: usize| cfa.color_at(y, x);
    let black = raw.blacklevel.as_bayer_array();
    let white = raw.whitelevel.as_bayer_array();
    let (width, height) = (raw.width, raw.height);
    let mut pattern = estimate(&Mosaic {
        width,
        height,
        data,
        black,
        white,
        color_at: &color_at,
    })?;
    remove(data, width, black, white, &color_at, &pattern);
    pattern.residual = residual(
        &Mosaic {
            width,
            height,
            data,
            black,
            white,
            color_at: &color_at,
        },
        pattern.period,
    );
    Ok(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 256;
    const HEIGHT: usize = 3200;
    const BLACK: [f32; 4] = [144.0, 143.0, 143.0, 144.0];
    const WHITE: [f32; 4] = [4095.0; 4];

    /// RGGB, as most sensors.
    fn color_at(y: usize, x: usize) -> usize {
        match (y % 2, x % 2) {
            (0, 0) => 0,
            (1, 1) => 2,
            _ => 1,
        }
    }

    /// A mosaic of a smooth scene with noise, banded by `band(row, color)`
    /// as a gain.
    fn mosaic(band: impl Fn(usize, usize) -> f64) -> Vec<u16> {
        let mut state = 7u32;
        let mut noise = move || {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            f64::from((state >> 16) % 41) - 20.0
        };
        (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| (y, x)))
            .map(|(y, x)| {
                let color = color_at(y, x);
                let base = [900.0, 1500.0, 700.0, 1500.0][color];
                // A scene: a slow vertical gradient and a horizontal blob.
                let scene = base
                    * (0.6 + 0.4 * (y as f64 / HEIGHT as f64))
                    * (0.7 + 0.3 * (-(x as f64 - 128.0).powi(2) / 4000.0).exp());
                (scene * band(y, color) + noise() + f64::from(BLACK[color])).round() as u16
            })
            .collect()
    }

    fn residual_peak(data: &[u16], color: usize, period: f64) -> f64 {
        let mosaic = Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data,
            black: BLACK,
            white: WHITE,
            color_at: &color_at,
        };
        Spectrum::of(&high_passed(&mosaic, color)).ratio(period)
    }

    #[test]
    fn finds_and_removes_periodic_bands_of_the_red_rows() {
        let period = 101.7;
        let band = |y: usize, color: usize| {
            if color == 0 {
                1.0 + 0.04 * (2.0 * PI * y as f64 / period).sin()
            } else {
                1.0
            }
        };
        let mut data = mosaic(band);
        let before = residual_peak(&data, 0, period);
        let pattern = estimate(&Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data: &data,
            black: BLACK,
            white: WHITE,
            color_at: &color_at,
        })
        .unwrap();
        assert!((pattern.period - period).abs() < 0.2, "{}", pattern.period);
        assert!(
            (pattern.amplitude(0) - 0.04).abs() < 0.01,
            "{}",
            pattern.amplitude(0)
        );
        assert_eq!(pattern.amplitude(1), 0.0, "green left alone");
        assert_eq!(pattern.amplitude(2), 0.0, "blue left alone");

        remove(&mut data, WIDTH, BLACK, WHITE, &color_at, &pattern);
        let after = residual_peak(&data, 0, period);
        assert!(
            after < before / 20.0,
            "before ×{before:.0}, after ×{after:.0}"
        );
    }

    #[test]
    fn corrected_rows_keep_their_mean_light() {
        let band = |y: usize, _: usize| 1.0 + 0.05 * (2.0 * PI * y as f64 / 60.0).sin();
        let mut data = mosaic(band);
        let reference = mosaic(|_, _| 1.0);
        let pattern = estimate(&Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data: &data,
            black: BLACK,
            white: WHITE,
            color_at: &color_at,
        })
        .unwrap();
        remove(&mut data, WIDTH, BLACK, WHITE, &color_at, &pattern);
        let mean =
            |data: &[u16]| data.iter().map(|&v| f64::from(v)).sum::<f64>() / data.len() as f64;
        assert!((mean(&data) - mean(&reference)).abs() < 2.0);
    }

    #[test]
    fn the_preview_loses_its_bands_too() {
        use image::{Rgb, RgbImage};
        let period = 40.0;
        // One block of a gain over the whole picture: green only, as a
        // sine over the period, in a preview three times smaller.
        let bins: Vec<f32> = (0..BINS)
            .map(|bin| 1.0 + 0.06 * (2.0 * PI * bin as f64 / BINS as f64).sin() as f32)
            .collect();
        let pattern = Pattern {
            period,
            gains: [
                None,
                Some(Grid {
                    rows: 1,
                    block_width: WIDTH / STRIPS,
                    block_height: HEIGHT,
                    blocks: vec![bins; STRIPS],
                }),
                None,
                None,
            ],
            peaks: [0.0; 4],
            residual: [0.0; 4],
        };
        let (columns, rows) = (WIDTH as u32 / 3, HEIGHT as u32 / 3);
        let mut preview = DynamicImage::ImageRgb8(RgbImage::from_fn(columns, rows, |_, py| {
            let gain = pattern.gains[1].as_ref().unwrap().gain(
                0,
                py as usize * 3 + 1,
                pattern.bin(py as usize * 3 + 1),
            );
            let green = 150.0 * gain.powf(1.0 / PREVIEW_GAMMA);
            Rgb([100, green.round() as u8, 50])
        }));
        pattern.correct_preview(&mut preview, (0, 0, WIDTH, HEIGHT));
        let rgb = preview.to_rgb8();
        for (_, _, pixel) in rgb.enumerate_pixels() {
            assert_eq!(pixel[0], 100, "red left alone");
            assert_eq!(pixel[2], 50, "blue left alone");
            assert!(pixel[1].abs_diff(150) <= 1, "green {}", pixel[1]);
        }
    }

    #[test]
    fn clipped_pixels_are_left_alone() {
        let period = 60.0;
        let band = |y: usize, _: usize| 1.0 + 0.05 * (2.0 * PI * y as f64 / period).sin();
        let mut data = mosaic(band);
        // A spotlight: a clipped disc in the middle of the picture.
        let (cx, cy, radius) = (WIDTH as f64 / 2.0, HEIGHT as f64 / 2.0, 60.0);
        let clipped = |x: usize, y: usize| {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            dx * dx + dy * dy < radius * radius
        };
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if clipped(x, y) {
                    data[y * WIDTH + x] = WHITE[0] as u16;
                }
            }
        }
        let pattern = estimate(&Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data: &data,
            black: BLACK,
            white: WHITE,
            color_at: &color_at,
        })
        .unwrap();
        assert!((pattern.period - period).abs() < 0.2, "{}", pattern.period);
        remove(&mut data, WIDTH, BLACK, WHITE, &color_at, &pattern);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if clipped(x, y) {
                    assert_eq!(data[y * WIDTH + x], WHITE[0] as u16, "at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn a_picture_without_bands_has_no_pattern() {
        let data = mosaic(|_, _| 1.0);
        let result = estimate(&Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data: &data,
            black: BLACK,
            white: WHITE,
            color_at: &color_at,
        });
        assert!(matches!(result, Err(Error::NoBands { .. })));
    }
}
