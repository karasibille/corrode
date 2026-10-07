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
//! noise is not stamped into them.
//!
//! The correction works on the raw mosaic, before demosaicing and the
//! camera's tone curve, where light is still linear: a band is then a
//! plain factor. The result is meant to be written as a DNG.

use std::f64::consts::PI;

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
const BINS: usize = 64;

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
    /// For each color, the gain over one period, in `BINS` steps; `None`
    /// for a color without bands.
    gains: [Option<Vec<f32>>; 4],
    /// For each color, how far its peak stood above its neighbours.
    pub peaks: [f64; 4],
    /// The same, measured again after the correction.
    pub residual: [f64; 4],
}

impl Pattern {
    /// The strength of the bands of a color: the largest deviation of its
    /// gain from 1, as a fraction. Zero for a color left alone.
    pub fn amplitude(&self, color: usize) -> f32 {
        self.gains[color].as_ref().map_or(0.0, |gains| {
            gains.iter().map(|g| (g - 1.0).abs()).fold(0.0, f32::max)
        })
    }

    /// Whether any color is corrected.
    pub fn is_empty(&self) -> bool {
        self.gains.iter().all(Option::is_none)
    }

    fn bin(&self, row: usize) -> usize {
        ((row as f64 / self.period).fract() * BINS as f64) as usize % BINS
    }
}

/// A raw mosaic: `data[y * width + x]`, whose color is `color_at(y, x)`.
pub struct Mosaic<'a> {
    pub width: usize,
    pub height: usize,
    pub data: &'a [u16],
    /// The black level of each color, below which there is no light.
    pub black: [f32; 4],
    pub color_at: &'a dyn Fn(usize, usize) -> usize,
}

/// Column strips the mosaic is cut into: bands cross all of them, while
/// a feature of the scene seldom does.
const STRIPS: usize = 16;

/// The rows holding a color, as `(row, brightness)`, with the scene's slow
/// changes removed: what remains is bands and noise. The brightness of a
/// row is the median, over column strips, of the strip's mean brightness
/// in log units, so that a feature of the scene in a few strips does not
/// pass for a band.
fn high_passed(mosaic: &Mosaic, color: usize) -> Vec<(usize, f64)> {
    let Mosaic {
        width,
        height,
        data,
        black,
        color_at,
    } = mosaic;
    let strip_width = width / STRIPS;
    if strip_width == 0 {
        return Vec::new();
    }
    // Per strip, the log mean of the color's pixels of each row.
    let mut profiles: Vec<Vec<(usize, f64)>> = vec![Vec::new(); STRIPS];
    for y in 0..*height {
        for (strip, profile) in profiles.iter_mut().enumerate() {
            let (mut sum, mut count) = (0f64, 0usize);
            for x in strip * strip_width..(strip + 1) * strip_width {
                if color_at(y, x) == color {
                    sum += (f64::from(data[y * width + x]) - f64::from(black[color])).max(1.0);
                    count += 1;
                }
            }
            if count > 0 {
                profile.push((y, (sum / count as f64).ln()));
            }
        }
    }
    let n = profiles[0].len();
    if n == 0 || profiles.iter().any(|p| p.len() != n) {
        return Vec::new();
    }
    let high_pass = |profile: &[(usize, f64)]| -> Vec<f64> {
        (0..n)
            .map(|i| {
                let (a, b) = (i.saturating_sub(SMOOTHING), (i + SMOOTHING + 1).min(n));
                let local = profile[a..b].iter().map(|r| r.1).sum::<f64>() / (b - a) as f64;
                profile[i].1 - local
            })
            .collect()
    };
    let passed: Vec<Vec<f64>> = profiles.iter().map(|p| high_pass(p)).collect();
    (0..n)
        .map(|i| {
            let mut values: Vec<f64> = passed.iter().map(|strip| strip[i]).collect();
            values.sort_by(|a, b| a.total_cmp(b));
            (profiles[0][i].0, values[values.len() / 2])
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
    let mut gains: [Option<Vec<f32>>; 4] = [None, None, None, None];
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
        let (mut sum, mut count) = ([0f64; BINS], [0usize; BINS]);
        for &(row, value) in signal {
            let bin = ((row as f64 / period).fract() * BINS as f64) as usize % BINS;
            sum[bin] += value;
            count[bin] += 1;
        }
        let gain: Vec<f32> = (0..BINS)
            .map(|bin| {
                if count[bin] > 0 {
                    (sum[bin] / count[bin] as f64).exp() as f32
                } else {
                    1.0
                }
            })
            .collect();
        gains[color] = Some(gain);
    }
    Ok(Pattern {
        period,
        gains,
        peaks,
        residual: [0.0; 4],
    })
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
    color_at: &dyn Fn(usize, usize) -> usize,
    pattern: &Pattern,
) {
    let height = data.len() / width;
    for y in 0..height {
        let bin = pattern.bin(y);
        for x in 0..width {
            let color = color_at(y, x);
            let Some(gains) = &pattern.gains[color] else {
                continue;
            };
            let value = &mut data[y * width + x];
            let light = (f32::from(*value) - black[color]) / gains[bin];
            *value = (light + black[color]).round().clamp(0.0, 65535.0) as u16;
        }
    }
}

/// The DNG a debanded RAW is written to: next to it, with `-deband`
/// added to its name, so that it is a shot of its own.
pub fn output_path(raw: &std::path::Path) -> std::path::PathBuf {
    let stem = raw.file_stem().unwrap_or_default().to_string_lossy();
    raw.with_file_name(format!("{stem}-deband.dng"))
}

/// Reads a RAW file, removes its bands and writes the result as a DNG.
pub fn to_dng(raw: &std::path::Path, dng: &std::path::Path) -> Result<Pattern, Error> {
    crate::dng::rewrite(raw, dng, deband)
}

/// Finds and removes the bands of a raw image, in place.
pub fn deband(raw: &mut RawImage) -> Result<Pattern, Error> {
    let RawImageData::Integer(data) = &mut raw.data else {
        return Err(Error::FloatData);
    };
    let cfa = raw.camera.cfa.clone();
    let color_at = |y: usize, x: usize| cfa.color_at(y, x);
    let black = raw.blacklevel.as_bayer_array();
    let (width, height) = (raw.width, raw.height);
    let mut pattern = estimate(&Mosaic {
        width,
        height,
        data,
        black,
        color_at: &color_at,
    })?;
    remove(data, width, black, &color_at, &pattern);
    pattern.residual = residual(
        &Mosaic {
            width,
            height,
            data,
            black,
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

        remove(&mut data, WIDTH, BLACK, &color_at, &pattern);
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
            color_at: &color_at,
        })
        .unwrap();
        remove(&mut data, WIDTH, BLACK, &color_at, &pattern);
        let mean =
            |data: &[u16]| data.iter().map(|&v| f64::from(v)).sum::<f64>() / data.len() as f64;
        assert!((mean(&data) - mean(&reference)).abs() < 2.0);
    }

    #[test]
    fn a_picture_without_bands_has_no_pattern() {
        let data = mosaic(|_, _| 1.0);
        let result = estimate(&Mosaic {
            width: WIDTH,
            height: HEIGHT,
            data: &data,
            black: BLACK,
            color_at: &color_at,
        });
        assert!(matches!(result, Err(Error::NoBands { .. })));
    }
}
