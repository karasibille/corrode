//! What a camera and film do when pushed: a flash over a slow shutter,
//! a lens that does not keep the colours together, grain, a faded print,
//! dark corners, a leak of light, lights that glow.
//!
//! Sizes are in pixels for a picture [`REFERENCE_SIDE`](super::units::REFERENCE_SIDE) on its longer side, as in the
//! glitch effects. The blurs are made on a reduced copy of the picture,
//! which they hide anyway, and blended back at full size.

use std::fmt;

use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

use super::Error;
use super::colour::{Color, from_hsv, to_hsv};
use super::parallel;
use super::random::Random;
use super::recipe::{Params, Spec};
use super::sort::brightness;
use super::units::{Percent, Size, long_side};
use super::warp::Noise;

/// Longer side the blurs are made at: enough for a blur, a fraction of the
/// work.
const BLUR_SIDE: u32 = 1000;
/// Samples along a smear at most: two pixels apart at the longest, which
/// the blending hides.
const SMEAR_SAMPLES: usize = 32;

/// How a blurred or lit copy goes over the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// The brighter of the two, where the blur only adds light, as a
    /// slow shutter does.
    #[default]
    Lighten,
    /// Screen: both add up without clipping.
    Screen,
    /// A plain mix.
    Normal,
}

impl std::str::FromStr for Blend {
    type Err = String;

    fn from_str(text: &str) -> Result<Blend, String> {
        match text {
            "lighten" => Ok(Blend::Lighten),
            "screen" => Ok(Blend::Screen),
            "normal" => Ok(Blend::Normal),
            other => Err(format!(
                "unknown blend '{other}' (lighten, screen or normal)"
            )),
        }
    }
}

impl std::fmt::Display for Blend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Blend::Lighten => "lighten",
            Blend::Screen => "screen",
            Blend::Normal => "normal",
        })
    }
}

impl Blend {
    /// One channel of `over` blended on `under`, by `mix` of 0 to 1.
    fn apply(self, under: f32, over: f32, mix: f32) -> f32 {
        let blended = match self {
            Blend::Lighten => under.max(over),
            Blend::Screen => 255.0 - (255.0 - under) * (255.0 - over) / 255.0,
            Blend::Normal => over,
        };
        under + (blended - under) * mix
    }
}

/// Which way the blur of a drag goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragKind {
    /// A straight smear, as a camera moved during the exposure.
    #[default]
    Motion,
    /// A smear away from the centre, as a zoom turned during it.
    Zoom,
    /// A smear along an arc round the centre, as a camera turned during
    /// it.
    Arc,
}

impl std::str::FromStr for DragKind {
    type Err = String;

    fn from_str(text: &str) -> Result<DragKind, String> {
        match text {
            "motion" => Ok(DragKind::Motion),
            "zoom" => Ok(DragKind::Zoom),
            "arc" => Ok(DragKind::Arc),
            other => Err(format!("unknown kind '{other}' (motion, zoom or arc)")),
        }
    }
}

impl std::fmt::Display for DragKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DragKind::Motion => "motion",
            DragKind::Zoom => "zoom",
            DragKind::Arc => "arc",
        })
    }
}

/// Parameters of the drag: a flash over a slow shutter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub kind: DragKind,
    /// How far the smear goes, in pixels for a picture
    /// [`REFERENCE_SIDE`](super::units::REFERENCE_SIDE) on its longer side: the length of a motion smear, or how far
    /// the edge of the picture moves in a zoom or an arc smear.
    pub length: Size,
    /// Direction of a motion smear, in degrees, 0 being to the right
    /// and 90 downwards.
    pub angle: f32,
    /// The centre of a zoom or an arc smear, as shares of the width and
    /// the height.
    pub center: (Percent, Percent),
    /// How much of the smear shows, 0 to 100.
    pub mix: Percent,
    pub blend: Blend,
}

impl Default for Drag {
    fn default() -> Drag {
        Drag {
            kind: DragKind::Motion,
            length: Size::new(60),
            angle: 0.0,
            center: (Percent::new(50), Percent::new(50)),
            mix: Percent::new(70),
            blend: Blend::Lighten,
        }
    }
}

/// A flash over a slow shutter: the sharp picture with a smeared copy
/// of itself over it, the lights trailing.
pub fn drag(image: &RgbImage, params: Drag) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let small = reduced(image);
    let length = params.length.on_f32(long_side(&small));
    let samples = (length.ceil() as usize).clamp(1, SMEAR_SAMPLES);
    let (sw, sh) = (small.width() as f32, small.height() as f32);
    let cx = params.center.0.fraction() * (sw - 1.0);
    let cy = params.center.1.fraction() * (sh - 1.0);
    let (dx, dy) = (
        params.angle.to_radians().cos(),
        params.angle.to_radians().sin(),
    );
    let smeared = parallel::from_fn(small.width(), small.height(), |x, y| {
        let (x, y) = (x as f32, y as f32);
        let mut sum = [0f32; 3];
        for i in 0..samples {
            let t = i as f32 / samples as f32;
            let (sx, sy) = match params.kind {
                DragKind::Motion => (x + dx * length * t, y + dy * length * t),
                DragKind::Zoom => {
                    // Towards the centre, further for points further out.
                    let reach = length / (sw / 2.0);
                    (x + (cx - x) * reach * t, y + (cy - y) * reach * t)
                }
                DragKind::Arc => {
                    // Turning round the centre, by the angle that moves
                    // the edge of the picture by `length`.
                    let (sin, cos) = (length / (sw / 2.0) * t).sin_cos();
                    let (ox, oy) = (x - cx, y - cy);
                    (cx + ox * cos - oy * sin, cy + ox * sin + oy * cos)
                }
            };
            let sample = bilinear(&small, sx, sy);
            for channel in 0..3 {
                sum[channel] += sample[channel];
            }
        }
        Rgb(sum.map(|v| (v / samples as f32).round() as u8))
    });
    blend_over(
        image,
        &enlarged(&smeared, width, height),
        params.blend,
        params.mix,
    )
}

/// Parameters of the chromatic aberration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aberration {
    /// How much larger the red is drawn than the green, and the blue
    /// smaller, as a fraction: 0.01 is a percent, already strong.
    pub amount: f32,
}

impl Default for Aberration {
    fn default() -> Aberration {
        Aberration { amount: 0.006 }
    }
}

/// A lens that does not focus the colours at the same size: red and
/// blue fringes growing towards the edges.
pub fn aberration(image: &RgbImage, params: Aberration) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let scales = [1.0 + params.amount, 1.0, 1.0 - params.amount];
    parallel::from_fn(width, height, |x, y| {
        let mut pixel = Rgb([0; 3]);
        for (channel, scale) in scales.iter().enumerate() {
            let sx = cx + (x as f32 - cx) / scale;
            let sy = cy + (y as f32 - cy) / scale;
            pixel[channel] = bilinear(image, sx, sy)[channel].round() as u8;
        }
        pixel
    })
}

/// Parameters of the grain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grain {
    /// How strong the grain is, 0 to 100.
    pub strength: Percent,
    /// Size of a grain, in pixels for a picture [`REFERENCE_SIDE`](super::units::REFERENCE_SIDE) on its longer side.
    pub size: Size,
    /// Whether the grain colours the picture, or only lightens and
    /// darkens it.
    pub color: bool,
}

impl Default for Grain {
    fn default() -> Grain {
        Grain {
            strength: Percent::new(30),
            size: Size::new(2),
            color: false,
        }
    }
}

/// Film grain: noise in clumps the size of a grain, stronger in the
/// middle tones, as on a pushed film.
pub fn grain(image: &RgbImage, params: Grain, seed: u64) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let dot = params.size.on(long_side(image)).max(1);
    let (gw, gh) = (width.div_ceil(dot), height.div_ceil(dot));
    let mut random = Random::new(seed);
    // Noise of -1 to 1 per grain, one value or three.
    let noise: Vec<[f32; 3]> = (0..gw * gh)
        .map(|_| {
            let draw = |random: &mut Random| random.below(2001) as f32 / 1000.0 - 1.0;
            let n = draw(&mut random);
            if params.color {
                [n, draw(&mut random), draw(&mut random)]
            } else {
                [n; 3]
            }
        })
        .collect();
    let strength = params.strength.fraction() * 80.0;
    let mut grained = image.clone();
    parallel::for_each_pixel(&mut grained, |x, y, pixel| {
        let n = noise[((y / dot) * gw + x / dot) as usize];
        let light = f32::from(brightness(*pixel)) / 255.0;
        // Little grain in the deep shadows and the blown highlights.
        let weight = 1.0 - (2.0 * light - 1.0).abs() * 0.7;
        for channel in 0..3 {
            let value = f32::from(pixel[channel]) + n[channel] * strength * weight;
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
    });
    grained
}

/// Parameters of the fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade {
    /// How far the blacks are lifted, 0 to 100: the veil of an old print.
    pub lift: Percent,
    /// Saturation, 100 leaving it, 0 taking all colour out, 200
    /// doubling it.
    pub saturation: u8,
    /// A colour the picture leans towards, by `tint_amount` of 0 to 100.
    pub tint: Color,
    pub tint_amount: Percent,
}

impl Default for Fade {
    fn default() -> Fade {
        Fade {
            lift: Percent::new(12),
            saturation: 80,
            tint: Color(Rgb([255, 140, 66])),
            tint_amount: Percent::new(10),
        }
    }
}

/// A faded print: blacks lifted, colours washed, a tint over all.
pub fn fade(image: &RgbImage, params: Fade) -> RgbImage {
    let lift = params.lift.fraction() * 255.0 * 0.5;
    let saturation = f32::from(params.saturation.min(200)) / 100.0;
    let tint = params.tint.0.0.map(f32::from);
    let tint_amount = params.tint_amount.fraction();
    let mut faded = image.clone();
    parallel::for_each_pixel(&mut faded, |_, _, pixel| {
        let grey = f32::from(brightness(*pixel));
        for channel in 0..3 {
            let value = f32::from(pixel[channel]);
            // Saturation around the grey, then the lift and the tint.
            let saturated = grey + (value - grey) * saturation;
            let lifted = lift + saturated * (255.0 - lift) / 255.0;
            let tinted = lifted + (tint[channel] - lifted) * tint_amount * (1.0 - lifted / 255.0);
            pixel[channel] = tinted.round().clamp(0.0, 255.0) as u8;
        }
    });
    faded
}

/// Parameters of the vignette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vignette {
    /// How dark the corners get, 0 to 100.
    pub strength: Percent,
    /// Where the darkening starts, 0 to 100 of the way from the centre
    /// to the corners.
    pub start: Percent,
}

impl Default for Vignette {
    fn default() -> Vignette {
        Vignette {
            strength: Percent::new(50),
            start: Percent::new(40),
        }
    }
}

/// Dark corners, as a cheap lens or a hood gives.
pub fn vignette(image: &RgbImage, params: Vignette) -> RgbImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let (cx, cy) = (width / 2.0, height / 2.0);
    let corner = (cx * cx + cy * cy).sqrt();
    let start = params.start.fraction();
    let strength = params.strength.fraction();
    let mut shaded = image.clone();
    parallel::for_each_pixel(&mut shaded, |x, y, pixel| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let distance = (dx * dx + dy * dy).sqrt() / corner;
        let t = ((distance - start) / (1.0 - start).max(0.01)).clamp(0.0, 1.0);
        let keep = 1.0 - strength * t * t;
        *pixel = Rgb(pixel.0.map(|v| (f32::from(v) * keep).round() as u8));
    });
    shaded
}

/// Where the light of a leak comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeakEdge {
    /// A point on one of the four edges, chosen by the seed.
    #[default]
    Random,
    /// The whole of one side.
    Left,
    Right,
    Top,
    Bottom,
}

impl std::str::FromStr for LeakEdge {
    type Err = String;

    fn from_str(text: &str) -> Result<LeakEdge, String> {
        match text {
            "random" => Ok(LeakEdge::Random),
            "left" => Ok(LeakEdge::Left),
            "right" => Ok(LeakEdge::Right),
            "top" => Ok(LeakEdge::Top),
            "bottom" => Ok(LeakEdge::Bottom),
            other => Err(format!(
                "unknown edge '{other}' (random, left, right, top or bottom)"
            )),
        }
    }
}

impl std::fmt::Display for LeakEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LeakEdge::Random => "random",
            LeakEdge::Left => "left",
            LeakEdge::Right => "right",
            LeakEdge::Top => "top",
            LeakEdge::Bottom => "bottom",
        })
    }
}

/// Parameters of the light leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leak {
    pub color: Color,
    /// How strong the leak is, 0 to 100.
    pub strength: Percent,
    /// How far it reaches into the picture, 0 to 100 of the width.
    pub size: Percent,
    /// Where the light comes from.
    pub edge: LeakEdge,
    /// How far the hue wanders round the colour, in degrees either way,
    /// from one place to another.
    pub spread: u16,
    /// How uneven the glow is: how far it reaches and how strong it is
    /// change from place to place, 0 to 100.
    pub wobble: Percent,
}

impl Default for Leak {
    fn default() -> Leak {
        Leak {
            color: Color(Rgb([255, 150, 60])),
            strength: Percent::new(70),
            size: Percent::new(50),
            edge: LeakEdge::Random,
            spread: 0,
            wobble: Percent::new(0),
        }
    }
}

/// Size, for a picture whose longer side is 1000 pixels, of the patches of the uneven
/// reach and strength of a leak, and of its patches of hue.
const LEAK_PATCH: u32 = 200;
const LEAK_TINT_PATCH: u32 = 350;

/// Light that got into the camera: a soft glow of colour from a point
/// on an edge, chosen by the seed, screened over the picture.
pub fn leak(image: &RgbImage, params: Leak, seed: u64) -> RgbImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let mut random = Random::new(seed);
    // The point the light comes from, or none for a whole side.
    let origin = match params.edge {
        LeakEdge::Random => {
            let along = random.below(1001) as f32 / 1000.0;
            Some(match random.below(4) {
                0 => (0.0, height * along),
                1 => (width, height * along),
                2 => (width * along, 0.0),
                _ => (width * along, height),
            })
        }
        _ => None,
    };
    let reach = width * f32::from(params.size.value()) / 100.0;
    let strength = params.strength.fraction();
    let color = params.color.0.0.map(f32::from);
    let base_hsv = to_hsv(color.map(|channel| channel / 255.0));
    let wobble = params.wobble.fraction() * 0.8;
    let spread = f32::from(params.spread);
    // Patches of noise, from seeds of their own so that the point
    // drawn above stays the one a leak always had.
    let patches = (wobble > 0.0 || spread > 0.0).then(|| {
        let lattice = |patch: u32, seed: u64| {
            let cell = Size::new(patch).on_f32(long_side(image)).max(1.0);
            let noise = Noise::new(
                seed,
                (width / cell).ceil() as usize + 2,
                (height / cell).ceil() as usize + 2,
            );
            (noise, cell)
        };
        (
            lattice(LEAK_PATCH, seed.wrapping_add(0x1EA4)),
            lattice(LEAK_PATCH, seed.wrapping_add(0x5CA1)),
            lattice(LEAK_TINT_PATCH, seed.wrapping_add(0x7147)),
        )
    });
    let mut lit = image.clone();
    parallel::for_each_pixel(&mut lit, |x, y, pixel| {
        let (px, py) = (x as f32, y as f32);
        let distance = match (params.edge, origin) {
            (_, Some((ox, oy))) => ((px - ox).powi(2) + (py - oy).powi(2)).sqrt(),
            (LeakEdge::Left, _) => px,
            (LeakEdge::Right, _) => width - px,
            (LeakEdge::Top, _) => py,
            _ => height - py,
        };
        let (mut reach, mut strength, mut color) = (reach, strength, color);
        if let Some(((shape, cell), (shade, _), (tint, tint_cell))) = &patches {
            // From 1 - wobble to 1 + wobble, evenly over the picture.
            let unevenness =
                |noise: &Noise, cell: f32| (noise.at(px / cell, py / cell) - 0.5) * 2.0;
            reach *= (1.0 + unevenness(shape, *cell) * wobble).max(0.2);
            strength *= (1.0 + unevenness(shade, *cell) * wobble).max(0.0);
            if spread > 0.0 {
                let hue = base_hsv[0] + unevenness(tint, *tint_cell) * spread;
                color = from_hsv([hue, base_hsv[1], base_hsv[2]]).map(|channel| channel * 255.0);
            }
        }
        let glow = (1.0 - distance / reach.max(1.0)).clamp(0.0, 1.0);
        let glow = glow * glow * strength;
        if glow > 0.0 {
            for channel in 0..3 {
                let value = Blend::Screen.apply(f32::from(pixel[channel]), color[channel], glow);
                pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    lit
}

/// Where the first streak of the light streaks is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreakPosition {
    /// Anywhere, drawn from the seed.
    #[default]
    Random,
    /// The middle of the streak, in percent of the width.
    At(Percent),
}

impl std::str::FromStr for StreakPosition {
    type Err = String;

    fn from_str(text: &str) -> Result<StreakPosition, String> {
        match text {
            "random" => Ok(StreakPosition::Random),
            other => other
                .parse::<Percent>()
                .map(StreakPosition::At)
                .map_err(|_| format!("'{other}' is not a position (random, or 0 to 100)")),
        }
    }
}

impl std::fmt::Display for StreakPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreakPosition::Random => f.write_str("random"),
            StreakPosition::At(percent) => write!(f, "{percent}"),
        }
    }
}

/// Parameters of the light streaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Streaks {
    /// Colour of the streaks where they are half bright.
    pub color: Color,
    /// How strong the streaks are, 0 to 100.
    pub strength: Percent,
    /// How many streaks, 1 to 8.
    pub count: u8,
    /// How wide a streak is, 0 to 100 of the width.
    pub width: Percent,
    /// How far the hue goes either way, in degrees, between the dim edge
    /// of a streak and its bright core.
    pub spread: u16,
    /// How much the streaks wander and fade from top to bottom, 0 to 100.
    pub wobble: Percent,
    /// How soft the edge of a streak is where the light stops short, 0 to
    /// 100: 0 is a cut, 100 fades over half of the width of the streak.
    pub softness: Percent,
    /// Where the first streak is; the others are drawn from the seed.
    pub position: StreakPosition,
}

impl Default for Streaks {
    fn default() -> Streaks {
        Streaks {
            color: Color(Rgb([255, 58, 16])),
            strength: Percent::new(85),
            count: 3,
            width: Percent::new(28),
            spread: 20,
            wobble: Percent::new(0),
            softness: Percent::new(40),
            position: StreakPosition::Random,
        }
    }
}

/// Size, for a picture whose longer side is 1000 pixels, of the patches along which the
/// streaks wander.
const STREAK_PATCH: u32 = 400;

/// Bands of light across the whole height, as film in a camera that was
/// not shut tight: a soft rise of light, a hard edge on one side, red at
/// the dim border and yellow towards the core, screened over the picture.
/// The seed draws where they are and which side is hard.
pub fn streaks(image: &RgbImage, params: Streaks, seed: u64) -> RgbImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let mut random = Random::new(seed);
    let wide = width * params.width.fraction().max(0.01);
    // For each streak: where it starts, how wide it is, and whether the
    // hard edge is on the right.
    let bands: Vec<(f32, f32, bool)> = (0..params.count.clamp(1, 8))
        .enumerate()
        .map(|(rank, _)| {
            let drawn = width * random.below(1001) as f32 / 1000.0;
            let centre = match (rank, params.position) {
                (0, StreakPosition::At(percent)) => width * percent.fraction(),
                _ => drawn,
            };
            let size = wide * (0.6 + random.below(801) as f32 / 1000.0);
            (centre - size / 2.0, size, random.once_in(2))
        })
        .collect();
    let wobble = params.wobble.fraction();
    let strength = params.strength.fraction();
    let spread = f32::from(params.spread);
    let fade_out = params.softness.fraction() * 0.5;
    let [hue, saturation, value] =
        to_hsv(params.color.0.0.map(|channel| f32::from(channel) / 255.0));
    let cell = Size::new(STREAK_PATCH).on_f32(long_side(image)).max(1.0);
    let rows = (height / cell).ceil() as usize + 2;
    // Along each streak, how bright it is and how far its edge wanders.
    let fade = Noise::new(seed.wrapping_add(0xFADE), rows, 1);
    let sway = Noise::new(seed.wrapping_add(0x5AAA), rows, 1);
    let mut lit = image.clone();
    parallel::for_each_pixel(&mut lit, |x, y, pixel| {
        let along = y as f32 / cell;
        let brightness = 1.0 + (fade.at(along, 0.0) - 0.5) * 2.0 * wobble * 0.8;
        let moved = (sway.at(along, 0.0) - 0.5) * 2.0 * wobble * wide * 0.3;
        let mut light = 0.0f32;
        for &(start, size, hard_right) in &bands {
            let mut u = (x as f32 - start - moved) / size;
            if !hard_right {
                u = 1.0 - u;
            }
            if (0.0..1.0).contains(&u) {
                // Rises smoothly across the streak, up to its edge.
                light += u * u * (3.0 - 2.0 * u);
            } else if u >= 1.0 && u < 1.0 + fade_out {
                // Past the edge, it fades over a short stretch.
                let gone = (u - 1.0) / fade_out;
                light += 1.0 - gone * gone * (3.0 - 2.0 * gone);
            }
        }
        let light = (light * brightness).clamp(0.0, 1.0);
        if light <= 0.0 {
            return;
        }
        // Dim: the hue goes one way; bright: the other, and towards white.
        let core = ((light - 0.75) / 0.25).clamp(0.0, 1.0);
        let color = from_hsv([
            hue + (2.0 * light - 1.0) * spread,
            saturation * (1.0 - core * 0.3),
            value,
        ]);
        for channel in 0..3 {
            let value = Blend::Screen.apply(
                f32::from(pixel[channel]),
                color[channel] * 255.0,
                light * strength,
            );
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
    });
    lit
}

/// Parameters of the bloom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bloom {
    /// Brightness, 0 to 255, above which a light glows.
    pub threshold: u8,
    /// How far the glow spreads, in pixels for a picture
    /// [`REFERENCE_SIDE`](super::units::REFERENCE_SIDE) on its longer side.
    pub radius: Size,
    /// How bright the glow is, 0 to 100.
    pub strength: Percent,
}

impl Default for Bloom {
    fn default() -> Bloom {
        Bloom {
            threshold: 200,
            radius: Size::new(30),
            strength: Percent::new(60),
        }
    }
}

/// The lights glow: what is brighter than the threshold is blurred and
/// added back, as haze or a cheap lens does.
pub fn bloom(image: &RgbImage, params: Bloom) -> RgbImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return image.clone();
    }
    let small = reduced(image);
    let threshold = f32::from(params.threshold);
    // The lights alone, the rest black, then blurred.
    let lights = parallel::from_fn(small.width(), small.height(), |x, y| {
        let pixel = *small.get_pixel(x, y);
        let light = f32::from(brightness(pixel));
        if light >= threshold {
            let keep = ((light - threshold) / (255.0 - threshold).max(1.0)).min(1.0);
            Rgb(pixel.0.map(|v| (f32::from(v) * keep).round() as u8))
        } else {
            Rgb([0, 0, 0])
        }
    });
    let sigma = (params.radius.on_f32(long_side(&small)) / 2.0).max(0.5);
    let glow = image::imageops::fast_blur(&lights, sigma);
    blend_over(
        image,
        &enlarged(&glow, width, height),
        Blend::Screen,
        params.strength,
    )
}

/// A copy of the picture whose longer side is at most [`BLUR_SIDE`].
fn reduced(image: &RgbImage) -> RgbImage {
    let side = long_side(image);
    if side <= BLUR_SIDE {
        return image.clone();
    }
    let reduce =
        |length: u32| (u64::from(length) * u64::from(BLUR_SIDE) / u64::from(side)).max(1) as u32;
    DynamicImage::ImageRgb8(image.clone())
        .resize_exact(
            reduce(image.width()),
            reduce(image.height()),
            FilterType::Triangle,
        )
        .to_rgb8()
}

/// A small picture brought back to a size.
fn enlarged(small: &RgbImage, width: u32, height: u32) -> RgbImage {
    if (small.width(), small.height()) == (width, height) {
        return small.clone();
    }
    DynamicImage::ImageRgb8(small.clone())
        .resize_exact(width, height, FilterType::Triangle)
        .to_rgb8()
}

/// `over` blended on `under`, both the same size, by `mix` of 0 to 100.
fn blend_over(under: &RgbImage, over: &RgbImage, blend: Blend, mix: Percent) -> RgbImage {
    let mix = mix.fraction();
    let mut result = under.clone();
    parallel::for_each_pixel(&mut result, |x, y, pixel| {
        let top = over.get_pixel(x, y);
        for channel in 0..3 {
            let value = blend.apply(f32::from(pixel[channel]), f32::from(top[channel]), mix);
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
    });
    result
}

/// The colour at a point between pixels, the edges extended.
pub(super) fn bilinear(image: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (width, height) = (image.width() as i64, image.height() as i64);
    let clamp = |v: i64, size: i64| v.clamp(0, size - 1) as u32;
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |dx: i64, dy: i64| {
        image
            .get_pixel(clamp(x0 + dx, width), clamp(y0 + dy, height))
            .0
    };
    let (p00, p10, p01, p11) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
    let mut out = [0f32; 3];
    for channel in 0..3 {
        let top = f32::from(p00[channel]) * (1.0 - fx) + f32::from(p10[channel]) * fx;
        let bottom = f32::from(p01[channel]) * (1.0 - fx) + f32::from(p11[channel]) * fx;
        out[channel] = top * (1.0 - fy) + bottom * fy;
    }
    out
}

impl Spec for Drag {
    const NAME: &'static str = "drag";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        (
            "kind",
            "`motion` (along a line), `zoom` (from the centre) or `arc` (round the centre)",
        ),
        ("length", "how far the smear goes, in pixels"),
        (
            "angle",
            "direction of a motion smear in degrees, 0 to the right, 90 down",
        ),
        (
            "center",
            "`x,y` centre of a zoom or arc smear, in percent of the width and height",
        ),
        ("mix", "how much of the smear shows, 0 to 100"),
        ("blend", "`lighten`, `screen` or `normal`"),
    ];

    fn parse(params: &mut Params) -> Result<Drag, Error> {
        let d = Drag::default();
        Ok(Drag {
            kind: params.get("kind", d.kind)?,
            length: params.get("length", d.length)?,
            angle: params.get("angle", d.angle)?,
            center: params.pair("center", d.center)?,
            mix: params.get("mix", d.mix)?,
            blend: params.get("blend", d.blend)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " kind={} length={} angle={} center={},{} mix={} blend={}",
            self.kind, self.length, self.angle, self.center.0, self.center.1, self.mix, self.blend
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(drag(image, *self))
    }
}

impl Spec for Aberration {
    const NAME: &'static str = "aberration";
    const PARAMS: &'static [(&'static str, &'static str)] = &[(
        "amount",
        "how much larger the red is drawn than the green, and the blue smaller; 0.01 is already strong",
    )];

    fn parse(params: &mut Params) -> Result<Aberration, Error> {
        let d = Aberration::default();
        Ok(Aberration {
            amount: params.get("amount", d.amount)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, " amount={}", self.amount)
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(aberration(image, *self))
    }
}

impl Spec for Grain {
    const NAME: &'static str = "grain";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        ("strength", "how strong the grain is, 0 to 100"),
        ("size", "size of a grain, in pixels"),
        (
            "color",
            "grain that colours the picture, or only lightens and darkens it",
        ),
    ];

    fn parse(params: &mut Params) -> Result<Grain, Error> {
        let d = Grain::default();
        Ok(Grain {
            strength: params.get("strength", d.strength)?,
            size: params.get("size", d.size)?,
            color: params.get("color", d.color)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " strength={} size={} color={}",
            self.strength, self.size, self.color
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(grain(image, *self, seed))
    }
}

impl Spec for Fade {
    const NAME: &'static str = "fade";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        ("lift", "how far the blacks are lifted, 0 to 100"),
        (
            "saturation",
            "100 leaves the colour, 0 takes it out, 200 doubles it",
        ),
        ("tint", "colour the picture leans towards"),
        ("tint_amount", "how much it leans, 0 to 100"),
    ];

    fn parse(params: &mut Params) -> Result<Fade, Error> {
        let d = Fade::default();
        Ok(Fade {
            lift: params.get("lift", d.lift)?,
            saturation: params.get("saturation", d.saturation)?,
            tint: params.get("tint", d.tint)?,
            tint_amount: params.get("tint_amount", d.tint_amount)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " lift={} saturation={} tint={} tint_amount={}",
            self.lift, self.saturation, self.tint, self.tint_amount
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(fade(image, *self))
    }
}

impl Spec for Vignette {
    const NAME: &'static str = "vignette";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        ("strength", "how dark the corners get, 0 to 100"),
        (
            "start",
            "where the darkening starts, 0 to 100 of the way from the centre to the corners",
        ),
    ];

    fn parse(params: &mut Params) -> Result<Vignette, Error> {
        let d = Vignette::default();
        Ok(Vignette {
            strength: params.get("strength", d.strength)?,
            start: params.get("start", d.start)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, " strength={} start={}", self.strength, self.start)
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(vignette(image, *self))
    }
}

impl Spec for Leak {
    const NAME: &'static str = "leak";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        ("color", "colour of the light"),
        ("strength", "how strong the leak is, 0 to 100"),
        ("size", "how far it reaches, 0 to 100 of the width"),
        (
            "edge",
            "`random` (a point on an edge, from the seed), `left`, `right`, `top` or `bottom` (the whole side)",
        ),
        (
            "spread",
            "how far the hue wanders round the colour, in degrees either way",
        ),
        (
            "wobble",
            "how uneven the glow is, in reach and strength, 0 to 100",
        ),
    ];

    fn parse(params: &mut Params) -> Result<Leak, Error> {
        let d = Leak::default();
        Ok(Leak {
            color: params.get("color", d.color)?,
            strength: params.get("strength", d.strength)?,
            size: params.get("size", d.size)?,
            edge: params.get("edge", d.edge)?,
            spread: params.get("spread", d.spread)?,
            wobble: params.get("wobble", d.wobble)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " color={} strength={} size={} edge={} spread={} wobble={}",
            self.color, self.strength, self.size, self.edge, self.spread, self.wobble
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(leak(image, *self, seed))
    }
}

impl Spec for Streaks {
    const NAME: &'static str = "streaks";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        ("color", "colour of the streaks where they are half bright"),
        ("strength", "how strong the streaks are, 0 to 100"),
        ("count", "how many streaks, 1 to 8"),
        ("width", "how wide a streak is, 0 to 100 of the width"),
        (
            "spread",
            "how far the hue goes either way between the dim edge and the bright core, in degrees",
        ),
        (
            "wobble",
            "how much the streaks wander and fade from top to bottom, 0 to 100",
        ),
        (
            "softness",
            "how soft the edge is where the light stops short, 0 for a cut, 0 to 100",
        ),
        (
            "position",
            "`random`, or where the first streak is, 0 to 100 of the width (the others stay drawn)",
        ),
    ];

    fn parse(params: &mut Params) -> Result<Streaks, Error> {
        let d = Streaks::default();
        Ok(Streaks {
            color: params.get("color", d.color)?,
            strength: params.get("strength", d.strength)?,
            count: params.get("count", d.count)?,
            width: params.get("width", d.width)?,
            spread: params.get("spread", d.spread)?,
            wobble: params.get("wobble", d.wobble)?,
            softness: params.get("softness", d.softness)?,
            position: params.get("position", d.position)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " color={} strength={} count={} width={} spread={} wobble={} softness={} position={}",
            self.color,
            self.strength,
            self.count,
            self.width,
            self.spread,
            self.wobble,
            self.softness,
            self.position
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(streaks(image, *self, seed))
    }
}

impl Spec for Bloom {
    const NAME: &'static str = "bloom";
    const PARAMS: &'static [(&'static str, &'static str)] = &[
        (
            "threshold",
            "brightness above which a light glows, 0 to 255",
        ),
        ("radius", "how far the glow spreads, in pixels"),
        ("strength", "how bright the glow is, 0 to 100"),
    ];

    fn parse(params: &mut Params) -> Result<Bloom, Error> {
        let d = Bloom::default();
        Ok(Bloom {
            threshold: params.get("threshold", d.threshold)?,
            radius: params.get("radius", d.radius)?,
            strength: params.get("strength", d.strength)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " threshold={} radius={} strength={}",
            self.threshold, self.radius, self.strength
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(bloom(image, *self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark picture with one bright spot in the middle.
    fn spot() -> RgbImage {
        RgbImage::from_fn(200, 120, |x, y| {
            if (95..105).contains(&x) && (55..65).contains(&y) {
                Rgb([250, 250, 250])
            } else {
                Rgb([30, 30, 30])
            }
        })
    }

    fn mean(image: &RgbImage) -> f64 {
        image
            .pixels()
            .map(|p| f64::from(brightness(*p)))
            .sum::<f64>()
            / f64::from(image.width() * image.height())
    }

    #[test]
    fn a_drag_trails_the_light_without_losing_the_spot() {
        let image = spot();
        let dragged = drag(
            &image,
            Drag {
                kind: DragKind::Motion,
                length: Size::new(200), // 40 px at this width
                angle: 0.0,
                mix: Percent::new(100),
                blend: Blend::Lighten,
                ..Drag::default()
            },
        );
        assert_eq!(dragged.get_pixel(100, 60)[0], 250);
        // To the left of the spot, light has trailed; to the right, not.
        assert!(
            dragged.get_pixel(80, 60)[0] > 60,
            "{:?}",
            dragged.get_pixel(80, 60)
        );
        assert_eq!(dragged.get_pixel(120, 60)[0], 30);
        let zoomed = drag(
            &image,
            Drag {
                kind: DragKind::Zoom,
                length: Size::new(100),
                ..Drag::default()
            },
        );
        assert_eq!((zoomed.width(), zoomed.height()), (200, 120));
    }

    #[test]
    fn an_arc_drag_trails_the_light_round_the_centre() {
        // Turning round the left edge, the spot leaves a trail up or down
        // and none along the row.
        let arced = drag(
            &spot(),
            Drag {
                kind: DragKind::Arc,
                length: Size::new(200),
                center: (Percent::new(0), Percent::new(50)),
                mix: Percent::new(100),
                ..Drag::default()
            },
        );
        assert_eq!(arced.get_pixel(100, 60)[0], 250);
        let trail = arced.get_pixel(100, 45)[0].max(arced.get_pixel(100, 75)[0]);
        assert!(trail > 60, "{trail}");
        assert_eq!(arced.get_pixel(140, 60)[0], 30);
    }

    #[test]
    fn aberration_pulls_the_reds_outwards() {
        let image = spot();
        let fringed = aberration(&image, Aberration { amount: 0.1 });
        // Just outside the spot on the right: red reaches, blue does not.
        let pixel = fringed.get_pixel(105, 60);
        assert!(pixel[0] > 100 && pixel[2] < 60, "{pixel:?}");
        assert_eq!(fringed.get_pixel(100, 60), &Rgb([250, 250, 250]));
    }

    #[test]
    fn grain_scatters_around_the_tone_and_follows_the_seed() {
        let flat = RgbImage::from_pixel(1000, 50, Rgb([128; 3]));
        let params = Grain {
            strength: Percent::new(50),
            size: Size::new(1),
            color: false,
        };
        let grained = grain(&flat, params, 3);
        assert!((mean(&grained) - 128.0).abs() < 2.0);
        let spread = grained.pixels().filter(|p| p[0].abs_diff(128) > 10).count();
        assert!(spread > 10_000, "{spread}");
        assert!(grained.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));
        assert_eq!(grain(&flat, params, 3), grained);
        assert_ne!(grain(&flat, params, 4), grained);
        let coloured = grain(
            &flat,
            Grain {
                color: true,
                ..params
            },
            3,
        );
        assert!(coloured.pixels().any(|p| p[0] != p[1]));
    }

    #[test]
    fn a_fade_lifts_the_blacks_and_washes_the_colours() {
        let image = RgbImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgb([0, 0, 0])
            } else {
                Rgb([200, 50, 50])
            }
        });
        let faded = fade(
            &image,
            Fade {
                lift: Percent::new(20),
                saturation: 50,
                tint: Color(Rgb([255, 255, 255])),
                tint_amount: Percent::new(0),
            },
        );
        assert!(faded.get_pixel(0, 0)[0] > 15, "{:?}", faded.get_pixel(0, 0));
        let red = faded.get_pixel(1, 0);
        assert!(red[0] - red[1] < 150 - 20, "{red:?}");
        let plain = fade(
            &image,
            Fade {
                lift: Percent::new(0),
                saturation: 100,
                tint_amount: Percent::new(0),
                ..Fade::default()
            },
        );
        assert_eq!(plain, image);
    }

    #[test]
    fn corners_darken_and_the_centre_stays() {
        let flat = RgbImage::from_pixel(200, 100, Rgb([200; 3]));
        let shaded = vignette(
            &flat,
            Vignette {
                strength: Percent::new(80),
                start: Percent::new(30),
            },
        );
        assert_eq!(shaded.get_pixel(100, 50)[0], 200);
        assert!(
            shaded.get_pixel(0, 0)[0] < 60,
            "{:?}",
            shaded.get_pixel(0, 0)
        );
    }

    #[test]
    fn a_leak_adds_light_from_an_edge() {
        let flat = RgbImage::from_pixel(200, 100, Rgb([40; 3]));
        let params = Leak {
            color: Color(Rgb([255, 150, 60])),
            strength: Percent::new(100),
            size: Percent::new(60),
            ..Leak::default()
        };
        let lit = leak(&flat, params, 1);
        assert!(mean(&lit) > 41.0);
        assert_eq!(leak(&flat, params, 1), lit);
        assert_ne!(leak(&flat, params, 2), lit);
    }

    #[test]
    fn a_leak_from_a_side_lights_the_whole_side() {
        let flat = RgbImage::from_pixel(200, 100, Rgb([40; 3]));
        let params = Leak {
            strength: Percent::new(100),
            size: Percent::new(50),
            edge: LeakEdge::Left,
            ..Leak::default()
        };
        let lit = leak(&flat, params, 1);
        for y in [0, 50, 99] {
            assert!(lit.get_pixel(5, y)[0] > 150, "row {y}");
            assert_eq!(lit.get_pixel(150, y), &Rgb([40; 3]), "row {y}");
        }
    }

    #[test]
    fn a_wobbly_leak_is_uneven_and_its_hue_wanders() {
        let flat = RgbImage::from_pixel(200, 100, Rgb([40; 3]));
        let even = Leak {
            strength: Percent::new(100),
            edge: LeakEdge::Left,
            ..Leak::default()
        };
        let uneven = Leak {
            spread: 60,
            wobble: Percent::new(100),
            ..even
        };
        let (plain, wobbly) = (leak(&flat, even, 1), leak(&flat, uneven, 1));
        assert_ne!(plain, wobbly);
        assert_eq!(leak(&flat, uneven, 1), wobbly, "the same for a seed");
        // On a column of the edge, the colour is not the same everywhere.
        let ratios: Vec<f32> = (0..100)
            .map(|y| {
                let pixel = wobbly.get_pixel(2, y);
                f32::from(pixel[1]) / f32::from(pixel[0]).max(1.0)
            })
            .collect();
        let (low, high) = ratios
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), &r| (l.min(r), h.max(r)));
        assert!(high - low > 0.1, "{low} {high}");
    }

    #[test]
    fn streaks_light_whole_columns_and_leave_the_rest() {
        let flat = RgbImage::from_pixel(300, 100, Rgb([40; 3]));
        let params = Streaks {
            count: 2,
            width: Percent::new(10),
            wobble: Percent::new(0),
            strength: Percent::new(100),
            softness: Percent::new(0),
            ..Streaks::default()
        };
        let lit = streaks(&flat, params, 3);
        assert_eq!(streaks(&flat, params, 3), lit, "the same for a seed");
        assert_ne!(streaks(&flat, params, 4), lit);
        // Without wobble a column is lit the same from top to bottom, and
        // most columns stay as they were.
        let columns: Vec<bool> = (0..300)
            .map(|x| lit.get_pixel(x, 0) != &Rgb([40; 3]))
            .collect();
        assert!(columns.iter().any(|&on| on));
        assert!(columns.iter().filter(|&&on| !on).count() > 150);
        for x in 0..300 {
            assert_eq!(lit.get_pixel(x, 0), lit.get_pixel(x, 99), "column {x}");
        }
    }

    #[test]
    fn the_first_streak_can_be_put_where_asked() {
        let flat = RgbImage::from_pixel(300, 10, Rgb([0; 3]));
        let at = |position| {
            let params = Streaks {
                count: 1,
                width: Percent::new(10),
                wobble: Percent::new(0),
                strength: Percent::new(100),
                position,
                ..Streaks::default()
            };
            let image = streaks(&flat, params, 3);
            let lit: Vec<u32> = (0..300).filter(|&x| image.get_pixel(x, 0)[0] > 0).collect();
            (lit[0], lit[lit.len() - 1])
        };
        let (first, last) = at(StreakPosition::At(Percent::new(10)));
        assert!(first < 30 && last < 60, "{first} {last}");
        let (first, _) = at(StreakPosition::At(Percent::new(90)));
        assert!(first > 240, "{first}");
        assert_eq!("random".parse(), Ok(StreakPosition::Random));
        assert!("far".parse::<StreakPosition>().is_err());
    }

    #[test]
    fn a_soft_edge_fades_where_a_cut_one_stops() {
        let flat = RgbImage::from_pixel(300, 10, Rgb([0; 3]));
        let streak = Streaks {
            count: 1,
            width: Percent::new(30),
            wobble: Percent::new(0),
            strength: Percent::new(100),
            ..Streaks::default()
        };
        let lit = |softness| {
            let image = streaks(
                &flat,
                Streaks {
                    softness: Percent::new(softness),
                    ..streak
                },
                5,
            );
            // Columns with some light but less than the brightest of the streak.
            let brightest = (0..300).map(|x| image.get_pixel(x, 0)[0]).max().unwrap();
            (0..300)
                .filter(|&x| (1..brightest / 2).contains(&image.get_pixel(x, 0)[0]))
                .count()
        };
        assert!(lit(100) > lit(0), "{} {}", lit(100), lit(0));
    }

    #[test]
    fn bloom_spreads_the_lights_only() {
        let image = spot();
        let glowing = bloom(
            &image,
            Bloom {
                threshold: 200,
                radius: Size::new(60),
                strength: Percent::new(100),
            },
        );
        assert!(
            glowing.get_pixel(110, 60)[0] > 30,
            "{:?}",
            glowing.get_pixel(110, 60)
        );
        assert_eq!(glowing.get_pixel(5, 5), &Rgb([30, 30, 30]));
    }
}
