//! Finishing the development of a RAW for the screen.
//!
//! `rawler` demosaics, applies the white balance and the camera matrix to
//! linear light, and ends with the sRGB curve. Two of its steps spoil the
//! brightest and the most saturated colours. The matrix step squashes any
//! pixel whose brightest channel passes 1 into a mix of its own colour and
//! its norm (`clip_euclidean_norm_avg`): across the line where a channel
//! reaches 1 the red and green jump, which draws a ragged edge round every
//! blown light, and the colour inside is wrong. The sRGB step then clips
//! every channel at 1 and every negative one at 0: a saturated blue LED outside
//! the sRGB gamut loses its red and green and becomes pure blue with hard
//! edges, a clipped light goes white where the camera shows it coloured,
//! and 16 bits are cut to 8 without dither. This module replaces that last
//! step: the gain is matched to the camera's own preview, colours outside
//! the gamut are lifted with white instead of cut, highlights are
//! rolled off keeping their hue, the 8 bits are dithered, and where the
//! sensor itself clipped, the colour turns neutral through a soft mask
//! instead of stopping at a ragged edge.

use image::RgbImage;
use rawler::RawImage;
use rawler::imgop::chromatic_adaption::adapt_bradford;
use rawler::imgop::matrix::{IDENTITY_MATRIX_3, multiply, normalize, pseudo_inverse, transform_1d};
use rawler::imgop::xyz::Illuminant;
use rawler::imgop::xyz::SRGB_TO_XYZ_D65;
use rayon::prelude::*;

/// How far a highlight must go before it is rolled off, in linear light
/// after the gain: 1 is the white of the screen.
const KNEE: f32 = 0.6;
/// How far above the knee the shoulder takes to be half way to white, in
/// linear light: a hyperbola, so that a light ten times brighter than the
/// screen is still told from one five times brighter, where an exponential
/// would have both at the same white.
const SHOULDER: f32 = 1.2;
/// From where a rolled-off highlight turns towards white, and how much of
/// the way it goes at the brightest.
const WHITE_FROM: f32 = 1.0;
const WHITE_RANGE: f32 = 6.0;
const WHITE_AT_MOST: f32 = 0.3;
/// Where the sensor clips: a pixel whose brightest channel, before the white
/// balance, is above `CLIP_FROM` starts to count as clipped, and fully
/// above `CLIP_TO`.
const CLIP_FROM: f32 = 0.80;
const CLIP_TO: f32 = 0.99;
/// The mask of clipped pixels is made on cells this many pixels wide and
/// blurred over this many cells either way, so its edge is smooth.
const CLIP_CELL: usize = 4;
const CLIP_BLUR: usize = 3;
/// How much of the way a clipped colour goes to neutral.
const CLIP_NEUTRAL: f32 = 0.9;
/// The gain stays within these bounds, whatever the preview says.
const GAIN_RANGE: (f32, f32) = (0.5, 8.0);

/// The luminance of a linear RGB colour.
fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// How much of its depth a colour with a negative channel is lifted by.
const LIFT: f32 = 0.8;
/// The most white that is added, in linear light, however far out a colour is.
const LIFT_AT_MOST: f32 = 0.06;

/// A colour with a negative channel, outside the gamut, is lifted with
/// a little white: `LIFT` of the depth of the most negative channel, at most
/// `LIFT_AT_MOST`, is added to every channel, and what is still negative is
/// cut. A saturated blue keeps its brightness and
/// goes a little towards cyan, where moving it to its own grey would take it
/// to black, its luminance being nil or negative.
pub fn into_gamut(rgb: [f32; 3]) -> [f32; 3] {
    let low = rgb[0].min(rgb[1]).min(rgb[2]);
    if low >= 0.0 {
        return rgb;
    }
    let lift = (-low * LIFT).min(LIFT_AT_MOST);
    rgb.map(|channel| (channel + lift).max(0.0))
}

/// How far a colour of this brightest channel (before any roll-off) turns
/// towards white, 0 to `WHITE_AT_MOST`: the brighter, the more, as the camera
/// does, a saturated light not staying a pure colour.
fn whiteness(top: f32) -> f32 {
    ((top - WHITE_FROM) / WHITE_RANGE).clamp(0.0, 1.0) * WHITE_AT_MOST
}

/// A highlight rolled off: the brightest channel is brought under 1 by a
/// smooth shoulder from `KNEE` up, and the others follow in proportion,
/// so the hue is kept. Bright colours turn towards white by `whiteness`.
pub fn roll_off(rgb: [f32; 3]) -> [f32; 3] {
    let top = rgb[0].max(rgb[1]).max(rgb[2]);
    let rolled = if top <= KNEE {
        top
    } else {
        let above = (top - KNEE) / SHOULDER;
        KNEE + (1.0 - KNEE) * above / (1.0 + above)
    };
    let scale = if top > 0.0 { rolled / top } else { 1.0 };
    let towards_white = whiteness(top);
    rgb.map(|channel| {
        let kept = channel * scale;
        kept + (rolled - kept) * towards_white
    })
}

/// The sRGB curve, from linear light 0 to 1 to 0 to 1.
pub fn srgb(linear: f32) -> f32 {
    let linear = linear.clamp(0.0, 1.0);
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// The sRGB curve backwards: from 0 to 1 to linear light.
fn linear_of(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// A number from 0 to 1 that differs from a pixel to the next without any
/// pattern: what dithers the 8 bits.
fn noise(x: usize, y: usize) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// The median luminance of linear pixels, from a sample of them.
fn median_luma(pixels: impl Iterator<Item = [f32; 3]>) -> f32 {
    let mut lumas: Vec<f32> = pixels.map(luma).collect();
    if lumas.is_empty() {
        return 0.0;
    }
    let middle = lumas.len() / 2;
    *lumas.select_nth_unstable_by(middle, f32::total_cmp).1
}

/// The gain that gives developed light the median luminance of the
/// camera's own preview: the exposure the camera chose, which the linear
/// development lacks. `None` when either is too dark to tell.
pub fn gain_for(developed: &[[f32; 3]], preview: &RgbImage) -> Option<f32> {
    // About 20 000 pixels of each are enough for a median.
    let step = (developed.len() / 20_000).max(1);
    let ours = median_luma(developed.iter().step_by(step).copied());
    let step = (preview.pixels().len() / 20_000).max(1);
    let theirs = median_luma(preview.pixels().step_by(step).map(|pixel| {
        [
            linear_of(f32::from(pixel[0]) / 255.0),
            linear_of(f32::from(pixel[1]) / 255.0),
            linear_of(f32::from(pixel[2]) / 255.0),
        ]
    }));
    (ours > 1e-6 && theirs > 1e-6).then(|| (theirs / ours).clamp(GAIN_RANGE.0, GAIN_RANGE.1))
}

/// The matrix from the camera's colours, white balanced, to linear sRGB,
/// the way rawler makes it from the colour matrix in the file: for the D65
/// illuminant, or another one adapted to it. `None` for a sensor that does
/// not have three colours.
pub fn camera_matrix(raw: &RawImage) -> Option<[[f32; 3]; 3]> {
    let (illuminant, matrix) = raw
        .color_matrix_find_first([
            Illuminant::D65,
            Illuminant::A,
            Illuminant::B,
            Illuminant::C,
            Illuminant::D50,
            Illuminant::D55,
            Illuminant::D75,
            Illuminant::Daylight,
            Illuminant::Flash,
        ])
        .unwrap_or_else(|| (Illuminant::D65, IDENTITY_MATRIX_3.as_flattened().to_vec()));
    let d65 = if illuminant == Illuminant::D65 {
        matrix
    } else if matrix.len() == 9 {
        adapt_bradford(
            &illuminant,
            &Illuminant::D65,
            &transform_1d::<3, 3>(&matrix)?,
        )
        .as_flattened()
        .to_vec()
    } else {
        return None;
    };
    if d65.len() != 9 {
        return None;
    }
    let mut xyz_to_camera = [[0.0f32; 3]; 4];
    for (row, values) in xyz_to_camera.iter_mut().zip(d65.as_chunks::<3>().0) {
        *row = *values;
    }
    let rgb_to_camera = normalize(multiply(&xyz_to_camera, &SRGB_TO_XYZ_D65));
    let camera_to_rgb = pseudo_inverse(rgb_to_camera);
    let mut matrix = [[0.0f32; 3]; 3];
    for (out, row) in matrix.iter_mut().zip(camera_to_rgb) {
        out.copy_from_slice(&row[..3]);
    }
    Some(matrix)
}

/// The picture demosaiced in the camera's colours, made linear sRGB: the
/// white balance, then the matrix. Negative channels (outside the gamut)
/// and values above 1 are kept, for [`finish`] to handle.
pub fn to_linear(
    camera: &[[f32; 3]],
    white_balance: [f32; 3],
    matrix: [[f32; 3]; 3],
) -> Vec<[f32; 3]> {
    camera
        .par_iter()
        .map(|pixel| {
            let balanced = [
                pixel[0] * white_balance[0],
                pixel[1] * white_balance[1],
                pixel[2] * white_balance[2],
            ];
            matrix.map(|row| row[0] * balanced[0] + row[1] * balanced[1] + row[2] * balanced[2])
        })
        .collect()
}

/// The whitest part of a colour, which is its least channel, taken out by
/// `vividness` of it: a pale blue (0.3, 0.3, 0.9) becomes a saturated one.
/// The camera's own pictures do this to the stage lights; it is what makes
/// their saturation, by the HSV measure, 1 - least / greatest, so that it
/// can be set to match theirs.
fn vivid(rgb: [f32; 3], vividness: f32) -> [f32; 3] {
    let least = rgb[0].min(rgb[1]).min(rgb[2]).max(0.0);
    rgb.map(|channel| channel - vividness * least)
}

/// The colour of a pixel as the screen gets it, before the noise and the
/// mask of the sensor's clip: the gain, the vividness, the gamut and the
/// roll-off.
fn render(rgb: [f32; 3], gain: f32, vividness: f32) -> [f32; 3] {
    roll_off(into_gamut(vivid(
        rgb.map(|channel| channel * gain),
        vividness,
    )))
}

/// The mean saturation of the pixels that are not dark (their brightest
/// channel above 0.15), sRGB-encoded colours seen as HSV.
fn mean_saturation(encoded: impl Iterator<Item = [f32; 3]>) -> f32 {
    let (mut sum, mut count) = (0.0f32, 0u32);
    for [r, g, b] in encoded {
        let top = r.max(g).max(b);
        if top > 0.15 {
            sum += (top - r.min(g).min(b)) / top;
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

/// How much of the white of each colour must be taken out for the mean
/// saturation of ours to be the camera preview's, between 0 and 0.9 (0 when
/// ours is already as saturated). The saturation only goes up with it, so
/// a bisection finds it.
pub fn vividness_for(developed: &[[f32; 3]], gain: f32, preview: &RgbImage) -> f32 {
    let step = (preview.pixels().len() / 20_000).max(1);
    let theirs = mean_saturation(preview.pixels().step_by(step).map(|pixel| {
        [
            f32::from(pixel[0]) / 255.0,
            f32::from(pixel[1]) / 255.0,
            f32::from(pixel[2]) / 255.0,
        ]
    }));
    let step = (developed.len() / 20_000).max(1);
    let sample: Vec<[f32; 3]> = developed.iter().step_by(step).copied().collect();
    let ours = |vividness: f32| {
        mean_saturation(
            sample
                .iter()
                .map(|&pixel| render(pixel, gain, vividness).map(srgb)),
        )
    };
    const MOST: f32 = 0.45;
    if ours(0.0) >= theirs {
        return 0.0;
    }
    if ours(MOST) <= theirs {
        return MOST;
    }
    let (mut low, mut high) = (0.0f32, MOST);
    for _ in 0..10 {
        let middle = (low + high) / 2.0;
        if ours(middle) < theirs {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}

/// Values on cells of `CLIP_CELL` pixels, read back smoothly anywhere.
struct Grid {
    columns: usize,
    rows: usize,
    data: Vec<f32>,
}

impl Grid {
    fn new(width: usize, height: usize) -> Grid {
        let (columns, rows) = (width.div_ceil(CLIP_CELL), height.div_ceil(CLIP_CELL));
        Grid {
            columns,
            rows,
            data: vec![0.0; columns * rows],
        }
    }

    /// Two passes of a box blur, along the rows then the columns, over
    /// `radius` cells either way.
    fn blur(&mut self, radius: usize) {
        for along_rows in [true, false] {
            let source = self.data.clone();
            let (outer, inner) = if along_rows {
                (self.rows, self.columns)
            } else {
                (self.columns, self.rows)
            };
            for line in 0..outer {
                for i in 0..inner {
                    let (lo, hi) = (i.saturating_sub(radius), (i + radius).min(inner - 1));
                    let index = |j: usize| {
                        if along_rows {
                            line * self.columns + j
                        } else {
                            j * self.columns + line
                        }
                    };
                    let sum: f32 = (lo..=hi).map(|j| source[index(j)]).sum();
                    self.data[index(i)] = sum / (hi - lo + 1) as f32;
                }
            }
        }
    }

    /// The four cells round pixel `x`, `y` and how much each counts, for
    /// reading several grids of the same size at one place.
    fn around(&self, x: usize, y: usize) -> ([usize; 4], [f32; 4]) {
        let (fx, fy) = (
            ((x as f32 + 0.5) / CLIP_CELL as f32 - 0.5).clamp(0.0, (self.columns - 1) as f32),
            ((y as f32 + 0.5) / CLIP_CELL as f32 - 0.5).clamp(0.0, (self.rows - 1) as f32),
        );
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.columns - 1), (y0 + 1).min(self.rows - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        (
            [
                y0 * self.columns + x0,
                y0 * self.columns + x1,
                y1 * self.columns + x0,
                y1 * self.columns + x1,
            ],
            [
                (1.0 - tx) * (1.0 - ty),
                tx * (1.0 - ty),
                (1.0 - tx) * ty,
                tx * ty,
            ],
        )
    }

    /// The value at the place `around` found.
    fn read(&self, (cells, shares): &([usize; 4], [f32; 4])) -> f32 {
        cells
            .iter()
            .zip(shares)
            .map(|(&cell, share)| self.data[cell] * share)
            .sum()
    }

    /// The value at pixel `x`, `y`, the cells interpolated.
    fn at(&self, x: usize, y: usize) -> f32 {
        self.read(&self.around(x, y))
    }
}

/// How much each pixel is clipped by the sensor, 0 to 1, smooth.
pub struct ClipMap {
    grid: Grid,
}

impl ClipMap {
    /// From the picture demosaiced before the white balance, where the
    /// white level of the sensor is 1: a cell holds the most clipped of
    /// its pixels, by their middle channel, then the cells are blurred.
    pub fn new(camera: &[[f32; 3]], width: usize, height: usize) -> ClipMap {
        let mut grid = Grid::new(width, height);
        let columns = grid.columns;
        grid.data = (0..grid.rows)
            .into_par_iter()
            .flat_map_iter(|row| (0..columns).map(move |column| (row, column)))
            .map(|(row, column)| {
                let (x0, y0) = (column * CLIP_CELL, row * CLIP_CELL);
                let mut top = 0.0f32;
                for y in y0..(y0 + CLIP_CELL).min(height) {
                    for x in x0..(x0 + CLIP_CELL).min(width) {
                        let [r, g, b] = camera[y * width + x];
                        // The middle channel: a light is blown when two of its
                        // channels are, not when only the blue is, which a
                        // blue light on a face does and which leaves the face
                        // its relief in the other two.
                        top = top.max(r + g + b - r.max(g).max(b) - r.min(g).min(b));
                    }
                }
                let t = ((top - CLIP_FROM) / (CLIP_TO - CLIP_FROM)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            })
            .collect();
        grid.blur(CLIP_BLUR);
        ClipMap { grid }
    }

    /// How clipped the pixel at `x`, `y` is.
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.grid.at(x, y)
    }
}

/// How many times the colour is median filtered.
const FALSE_COLOUR_PASSES: usize = 1;

/// The median of nine values, by a network of 19 compare-and-swaps (Paeth's
/// and Devillard's): no sort, and no branch that depends on more than two
/// values.
fn median9(mut p: [f32; 9]) -> f32 {
    fn order(p: &mut [f32; 9], a: usize, b: usize) {
        if p[a] > p[b] {
            p.swap(a, b);
        }
    }
    for (a, b) in [
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 1),
        (3, 4),
        (6, 7),
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 3),
        (5, 8),
        (4, 7),
        (3, 6),
        (1, 4),
        (2, 5),
        (4, 7),
        (4, 2),
        (6, 4),
        (4, 2),
    ] {
        order(&mut p, a, b);
    }
    p[4]
}

/// Removes the false colours the demosaic leaves: where the picture is
/// fine and low in contrast (a haze, a fishnet, the hair of a head), the
/// interpolation of the missing colours fails and leaves patches and fringes
/// of the wrong colour, one or two pixels wide, which at 100% make a
/// pixel mess. The green is kept as it is, it is the best sampled, and
/// the difference of the red and of the blue to the green is replaced by
/// its median over the 3 by 3 pixels round each, as dcraw does (it makes three passes; one costs 0.9 s here and takes most of the patches):
/// a median, not a mean, so that a real edge of colour stays an edge.
pub fn suppress_false_colour(camera: &mut [[f32; 3]], width: usize, height: usize) {
    if width < 3 || height < 3 {
        return;
    }
    for _ in 0..FALSE_COLOUR_PASSES {
        let differences: Vec<[f32; 2]> =
            camera.par_iter().map(|[r, g, b]| [r - g, b - g]).collect();
        camera
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let (above, below) = (y.saturating_sub(1), (y + 1).min(height - 1));
                for (x, pixel) in row.iter_mut().enumerate() {
                    let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
                    let mut around = [[0.0f32; 9]; 2];
                    let mut n = 0;
                    for yy in [above, y, below] {
                        for xx in [left, x, right] {
                            let d = differences[yy * width + xx];
                            around[0][n] = d[0];
                            around[1][n] = d[1];
                            n += 1;
                        }
                    }
                    let green = pixel[1];
                    pixel[0] = green + median9(around[0]);
                    pixel[2] = green + median9(around[1]);
                }
            });
    }
}

/// Under this luminance (after the gain) the colour of a pixel is noise as
/// much as signal; over the second, none of it is smoothed.
const DARK_FROM: f32 = 0.01;
const DARK_TO: f32 = 0.08;
/// How many cells the colour of the dark parts is blurred over.
const CHROMA_BLUR: usize = 2;

/// How much a pixel of this luminance has its colour smoothed, 0 to 1.
fn darkness(luminance: f32) -> f32 {
    let t = ((luminance - DARK_FROM) / (DARK_TO - DARK_FROM)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// The colour (what is left of a pixel once its luminance is taken) of the
/// dark parts, averaged over a few cells: the gain that lifts a dark scene
/// lifts the colour noise of the sensor with it, green and magenta speckle
/// in the shadows, which the eye takes for a fault while the luminance noise
/// reads as grain. Only the dark pixels count in the average, so that a
/// bright edge does not bleed into the shadow next to it.
struct ChromaMap {
    sums: [Grid; 3],
    weights: Grid,
}

impl ChromaMap {
    fn new(linear: &[[f32; 3]], width: usize, height: usize, gain: f32) -> ChromaMap {
        let mut sums = [
            Grid::new(width, height),
            Grid::new(width, height),
            Grid::new(width, height),
        ];
        let mut weights = Grid::new(width, height);
        let columns = weights.columns;
        // One cell of the sums and the weights from each of its pixels,
        // the rows of cells in parallel.
        let cells: Vec<[f32; 4]> = (0..weights.rows)
            .into_par_iter()
            .flat_map_iter(|row| (0..columns).map(move |column| (row, column)))
            .map(|(row, column)| {
                let mut cell = [0.0f32; 4];
                let (x0, y0) = (column * CLIP_CELL, row * CLIP_CELL);
                for y in y0..(y0 + CLIP_CELL).min(height) {
                    for x in x0..(x0 + CLIP_CELL).min(width) {
                        let rgb = linear[y * width + x].map(|channel| channel * gain);
                        let grey = luma(rgb);
                        let weight = darkness(grey);
                        if weight > 0.0 {
                            cell[3] += weight;
                            for c in 0..3 {
                                cell[c] += weight * (rgb[c] - grey);
                            }
                        }
                    }
                }
                cell
            })
            .collect();
        for (index, cell) in cells.iter().enumerate() {
            for c in 0..3 {
                sums[c].data[index] = cell[c];
            }
            weights.data[index] = cell[3];
        }
        for grid in sums.iter_mut().chain(std::iter::once(&mut weights)) {
            grid.blur(CHROMA_BLUR);
        }
        ChromaMap { sums, weights }
    }

    /// The smoothed colour round pixel `x`, `y`; none where nothing is dark.
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        let place = self.weights.around(x, y);
        let weight = self.weights.read(&place);
        if weight < 1e-6 {
            return [0.0; 3];
        }
        self.sums.each_ref().map(|sum| sum.read(&place) / weight)
    }
}

/// A colour brought towards its own grey by `amount`, 0 to 1.
fn neutralised(rgb: [f32; 3], amount: f32) -> [f32; 3] {
    let grey = luma(rgb);
    rgb.map(|channel| channel + (grey - channel) * amount)
}

/// Linear light, `width` pixels wide, made into 8-bit sRGB for the screen:
/// the gain, the gamut, the neutral of what the sensor clipped, the
/// roll-off of the highlights, the curve and the dither.
pub fn finish(
    linear: &[[f32; 3]],
    width: usize,
    height: usize,
    gain: f32,
    vividness: f32,
    clipped: Option<&ClipMap>,
) -> RgbImage {
    let chroma = ChromaMap::new(linear, width, height, gain);
    let mut bytes = vec![0u8; width * height * 3];
    bytes
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, out) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let rgb = linear[y * width + x].map(|channel| channel * gain);
                // The colour of the shadows is smoothed, their luminance is not.
                let grey = luma(rgb);
                let smooth = darkness(grey);
                let rgb = if smooth > 0.0 {
                    let around = chroma.at(x, y);
                    [0, 1, 2].map(|c| grey + (1.0 - smooth) * (rgb[c] - grey) + smooth * around[c])
                } else {
                    rgb
                };
                let rgb = vivid(rgb, vividness);
                let rgb = into_gamut(rgb);
                let rgb = match clipped {
                    Some(map) => neutralised(rgb, map.at(x, y) * CLIP_NEUTRAL),
                    None => rgb,
                };
                let rgb = roll_off(rgb);
                let dither = noise(x, y) - 0.5;
                for (byte, channel) in out.iter_mut().zip(rgb) {
                    *byte = (srgb(channel) * 255.0 + dither).round().clamp(0.0, 255.0) as u8;
                }
            }
        });
    RgbImage::from_raw(width as u32, height as u32, bytes).expect("the buffer has the size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_outside_the_gamut_is_lifted_with_white_not_cut_or_blackened() {
        // A saturated blue the camera matrix gives a negative red and green.
        let blue = [-0.05, 0.0, 0.9];
        let inside = into_gamut(blue);
        assert!(inside.iter().all(|&c| c >= 0.0), "{inside:?}");
        assert!(
            inside[2] >= 0.9 && inside[2] > 5.0 * inside[1],
            "still a bright blue: {inside:?}"
        );
        assert!(inside[1] > 0.0, "a little white added: {inside:?}");
        // A blue whose luminance is negative does not go to black.
        let deep = into_gamut([-0.3, -0.2, 1.0]);
        assert!(deep[2] > 0.9, "{deep:?}");
        // A colour inside the gamut is left alone.
        assert_eq!(into_gamut([0.2, 0.4, 0.1]), [0.2, 0.4, 0.1]);
        assert_eq!(into_gamut([-0.5, -0.5, -0.5]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_highlight_is_rolled_off_keeping_its_hue_and_staying_under_1() {
        let under = roll_off([0.1, 0.3, 0.5]);
        assert_eq!(under, [0.1, 0.3, 0.5], "under the knee");
        let mut last = 0.0;
        for top in [0.9, 1.0, 1.5, 3.0, 10.0, 100.0] {
            let out = roll_off([top * 0.3, top * 0.5, top]);
            assert!(out[2] <= 1.0 && out[2] >= last, "{top}: {out:?}");
            assert!(
                out[2] >= out[1] && out[1] >= out[0],
                "the order of the channels stays"
            );
            last = out[2];
        }
        // A light eight times brighter than the screen is still told from one
        // twice as bright, by more than a dozen levels of 8 bits.
        let (two, eight) = (roll_off([0.2, 0.5, 2.0])[2], roll_off([0.8, 2.0, 8.0])[2]);
        assert!(srgb(eight) - srgb(two) > 0.05, "{two} {eight}");
        // A clipped blue light is still blue, not white.
        let light = roll_off([1.2, 1.6, 4.0]);
        assert!(light[2] - light[0] > 0.2, "{light:?}");
    }

    #[test]
    fn the_curve_and_its_inverse_agree_and_the_noise_is_spread() {
        for value in [0.0, 0.002, 0.1, 0.5, 0.9, 1.0] {
            assert!((linear_of(srgb(value)) - value).abs() < 1e-4, "{value}");
        }
        let sample: Vec<f32> = (0..1000).map(|i| noise(i % 40, i / 40)).collect();
        let mean = sample.iter().sum::<f32>() / 1000.0;
        assert!((0.0..1.0).contains(&sample.iter().copied().fold(0.0, f32::max)));
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
    }

    #[test]
    fn the_clip_mask_is_smooth_where_the_sensor_clipped_with_a_ragged_edge() {
        // 120 by 40, a clipped block on the left with an edge that goes in
        // and out, and the rest well under the white level.
        let (width, height) = (120usize, 40usize);
        let camera: Vec<[f32; 3]> = (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                let edge = 50 + (y * 7 % 5);
                if x < edge {
                    [1.0, 0.9, 1.0]
                } else {
                    [0.3, 0.2, 0.4]
                }
            })
            .collect();
        let map = ClipMap::new(&camera, width, height);
        assert!(map.at(10, 20) > 0.98, "inside: {}", map.at(10, 20));
        assert!(map.at(100, 20) < 0.01, "outside: {}", map.at(100, 20));
        // Along a row across the edge the mask falls in small steps, none of
        // which is the jump of the raw clip.
        let row: Vec<f32> = (30..80).map(|x| map.at(x, 20)).collect();
        let biggest = row
            .windows(2)
            .map(|w| (w[0] - w[1]).abs())
            .fold(0.0, f32::max);
        assert!(biggest < 0.12, "{biggest}");
        assert!(
            row.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "falls steadily"
        );
        // Clipped light comes out neutral, the rest keeps its colour.
        let image = finish(&camera, width, height, 1.0, 0.0, Some(&map));
        let inside = image.get_pixel(10, 20);
        assert!(inside[0].abs_diff(inside[2]) < 30, "{inside:?}");
        let outside = image.get_pixel(100, 20);
        assert!(outside[2].abs_diff(outside[1]) > 30, "{outside:?}");
    }

    #[test]
    fn taking_out_the_white_makes_a_pale_blue_vivid_and_matches_the_preview() {
        let developed = vec![[0.3f32, 0.3, 0.9]; 400];
        let vivid_blue = vivid(developed[0], 0.7);
        assert!(vivid_blue[2] > 0.6 && vivid_blue[0] < 0.1, "{vivid_blue:?}");
        // Colours with a negative channel have no white to take out.
        assert_eq!(vivid([-0.1, 0.2, 0.5], 0.8), [-0.1, 0.2, 0.5]);
        // A preview made of ours with a vividness of 0.25 gives it back.
        let made =
            render(developed[0], 1.0, 0.25).map(|channel| (srgb(channel) * 255.0).round() as u8);
        let preview = RgbImage::from_pixel(20, 20, image::Rgb(made));
        let more = vividness_for(&developed, 1.0, &preview);
        assert!((more - 0.25).abs() < 0.03, "{more}");
        // A preview less saturated than ours leaves the colours alone.
        let grey = RgbImage::from_pixel(20, 20, image::Rgb([150, 150, 160]));
        assert_eq!(vividness_for(&developed, 1.0, &grey), 0.0);
    }

    #[test]
    fn the_colour_noise_of_the_shadows_is_smoothed_and_their_light_is_not() {
        // A dark flat picture whose pixels differ in colour only: red or green
        // is a little higher, at random, with the same luminance.
        let (width, height) = (64usize, 64usize);
        let dark: Vec<[f32; 3]> = (0..width * height)
            .map(|i| {
                let swing = if noise(i % width, i / width) < 0.5 {
                    0.0016
                } else {
                    -0.0016
                };
                [0.004 + swing, 0.004 - swing * 0.3, 0.004]
            })
            .collect();
        let image = finish(&dark, width, height, 4.0, 0.0, None);
        let spread = |channel: usize| {
            let values: Vec<f32> = image.pixels().map(|p| f32::from(p[channel])).collect();
            let mean = values.iter().sum::<f32>() / values.len() as f32;
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
        };
        // Without the smoothing the red would swing by about 0.0064 in linear
        // light, 9 levels or so; with it, only the dither is left.
        assert!(spread(0) < 2.5, "red swings by {}", spread(0));
        // A bright pixel next to the shadow keeps its own colour.
        let mut with_light = dark.clone();
        with_light[10 * width + 10] = [0.9, 0.2, 0.1];
        let lit = finish(&with_light, width, height, 4.0, 0.0, None);
        let pixel = lit.get_pixel(10, 10);
        assert!(pixel[0] > pixel[1] + 60, "{pixel:?}");
    }

    #[test]
    fn the_median_of_nine_is_the_fifth_whatever_the_order() {
        let sorted = [0.5f32, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5];
        // Every rotation and a reversal of the nine values.
        for shift in 0..9 {
            let mut values = sorted;
            values.rotate_left(shift);
            assert_eq!(median9(values), 2.5, "{values:?}");
            values.reverse();
            assert_eq!(median9(values), 2.5, "{values:?}");
        }
        assert_eq!(median9([7.0; 9]), 7.0);
    }

    #[test]
    fn a_patch_of_false_colour_goes_and_a_real_edge_of_colour_stays() {
        // A grey field with a 1 pixel speck of red, and on the right half a
        // real red wall: its edge is a straight line of 20 pixels.
        let (width, height) = (40usize, 20usize);
        let mut camera: Vec<[f32; 3]> = (0..width * height)
            .map(|i| {
                let x = i % width;
                if x >= 20 {
                    [0.6, 0.3, 0.3]
                } else {
                    [0.3, 0.3, 0.3]
                }
            })
            .collect();
        camera[10 * width + 5] = [0.8, 0.3, 0.3];
        suppress_false_colour(&mut camera, width, height);
        // The speck is grey again, the green never moved.
        assert_eq!(camera[10 * width + 5], [0.3, 0.3, 0.3]);
        // The wall is still red and the field still grey, a pixel from the
        // line between them.
        assert!((camera[10 * width + 25][0] - 0.6).abs() < 1e-6);
        assert!((camera[10 * width + 15][0] - 0.3).abs() < 1e-6);
        assert!(camera.iter().all(|p| p[1] == 0.3));
    }

    #[test]
    fn the_gain_matches_the_preview_and_dithering_breaks_up_steps() {
        let dark = vec![[0.02f32, 0.02, 0.02]; 400];
        // A preview whose grey is the sRGB of 0.08 in linear light.
        let level = (srgb(0.08) * 255.0).round() as u8;
        let preview = RgbImage::from_pixel(20, 20, image::Rgb([level; 3]));
        let gain = gain_for(&dark, &preview).unwrap();
        assert!((gain - 4.0).abs() < 0.2, "{gain}");
        assert_eq!(gain_for(&[[0.0; 3]; 10], &preview), None);
        assert_eq!(
            gain_for(&dark, &RgbImage::from_pixel(4, 4, image::Rgb([0; 3]))),
            None
        );
        // A smooth gradient between two 8-bit levels uses both.
        let ramp: Vec<[f32; 3]> = vec![[0.0041; 3]; 4000];
        let image = finish(&ramp, 100, 40, 1.0, 0.0, None);
        let levels: std::collections::BTreeSet<u8> = image.pixels().map(|p| p[0]).collect();
        assert!(levels.len() >= 2, "{levels:?}");
    }
}
