//! What a camera and film do when pushed: a flash over a slow shutter,
//! a lens that does not keep the colours together, grain, a faded print,
//! dark corners, a leak of light, lights that glow.
//!
//! Sizes are in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide, as in the
//! glitch effects. The blurs are made on a reduced copy of the picture,
//! which they hide anyway, and blended back at full size.

use std::fmt;

use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

use super::Error;
use super::parallel;
use super::print::Color;
use super::random::Random;
use super::recipe::{Params, Spec};
use super::sort::brightness;
use super::units::{Percent, Size};

/// Width the blurs are made at: enough for a blur, a fraction of the
/// work.
const BLUR_WIDTH: u32 = 1000;
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
}

impl std::str::FromStr for DragKind {
    type Err = String;

    fn from_str(text: &str) -> Result<DragKind, String> {
        match text {
            "motion" => Ok(DragKind::Motion),
            "zoom" => Ok(DragKind::Zoom),
            other => Err(format!("unknown kind '{other}' (motion or zoom)")),
        }
    }
}

impl std::fmt::Display for DragKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DragKind::Motion => "motion",
            DragKind::Zoom => "zoom",
        })
    }
}

/// Parameters of the drag: a flash over a slow shutter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub kind: DragKind,
    /// How far the smear goes, in pixels for a picture
    /// [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide: the length of a motion smear, or how far
    /// the edge of the picture moves in a zoom smear.
    pub length: Size,
    /// Direction of a motion smear, in degrees, 0 being to the right
    /// and 90 downwards.
    pub angle: f32,
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
    let length = params.length.on_f32(small.width());
    let samples = (length.ceil() as usize).clamp(1, SMEAR_SAMPLES);
    let (sw, sh) = (small.width() as f32, small.height() as f32);
    let (cx, cy) = (sw / 2.0, sh / 2.0);
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
    /// Size of a grain, in pixels for a picture [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
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
    let dot = params.size.on(width).max(1);
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

/// Parameters of the light leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leak {
    pub color: Color,
    /// How strong the leak is, 0 to 100.
    pub strength: Percent,
    /// How far it reaches into the picture, 0 to 100 of the width.
    pub size: Percent,
}

impl Default for Leak {
    fn default() -> Leak {
        Leak {
            color: Color(Rgb([255, 150, 60])),
            strength: Percent::new(70),
            size: Percent::new(50),
        }
    }
}

/// Light that got into the camera: a soft glow of colour from a point
/// on an edge, chosen by the seed, screened over the picture.
pub fn leak(image: &RgbImage, params: Leak, seed: u64) -> RgbImage {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let mut random = Random::new(seed);
    // A point on one of the four edges.
    let along = random.below(1001) as f32 / 1000.0;
    let (ox, oy) = match random.below(4) {
        0 => (0.0, height * along),
        1 => (width, height * along),
        2 => (width * along, 0.0),
        _ => (width * along, height),
    };
    let reach = width * f32::from(params.size.value()) / 100.0;
    let strength = params.strength.fraction();
    let color = params.color.0.0.map(f32::from);
    let mut lit = image.clone();
    parallel::for_each_pixel(&mut lit, |x, y, pixel| {
        let (dx, dy) = (x as f32 - ox, y as f32 - oy);
        let distance = (dx * dx + dy * dy).sqrt() / reach.max(1.0);
        let glow = (1.0 - distance).clamp(0.0, 1.0);
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

/// Parameters of the bloom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bloom {
    /// Brightness, 0 to 255, above which a light glows.
    pub threshold: u8,
    /// How far the glow spreads, in pixels for a picture
    /// [`REFERENCE_WIDTH`](super::units::REFERENCE_WIDTH) wide.
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
    let sigma = (params.radius.on_f32(small.width()) / 2.0).max(0.5);
    let glow = image::imageops::fast_blur(&lights, sigma);
    blend_over(
        image,
        &enlarged(&glow, width, height),
        Blend::Screen,
        params.strength,
    )
}

/// A copy of the picture no wider than [`BLUR_WIDTH`].
fn reduced(image: &RgbImage) -> RgbImage {
    if image.width() <= BLUR_WIDTH {
        return image.clone();
    }
    let height =
        (u64::from(image.height()) * u64::from(BLUR_WIDTH) / u64::from(image.width())) as u32;
    DynamicImage::ImageRgb8(image.clone())
        .resize_exact(BLUR_WIDTH, height.max(1), FilterType::Triangle)
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

    fn parse(params: &mut Params) -> Result<Drag, Error> {
        let d = Drag::default();
        Ok(Drag {
            kind: params.get("kind", d.kind)?,
            length: params.get("length", d.length)?,
            angle: params.get("angle", d.angle)?,
            mix: params.get("mix", d.mix)?,
            blend: params.get("blend", d.blend)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " kind={} length={} angle={} mix={} blend={}",
            self.kind, self.length, self.angle, self.mix, self.blend
        )
    }

    fn apply(&self, image: &RgbImage, _seed: u64) -> Result<RgbImage, Error> {
        Ok(drag(image, *self))
    }
}

impl Spec for Aberration {
    const NAME: &'static str = "aberration";

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

    fn parse(params: &mut Params) -> Result<Leak, Error> {
        let d = Leak::default();
        Ok(Leak {
            color: params.get("color", d.color)?,
            strength: params.get("strength", d.strength)?,
            size: params.get("size", d.size)?,
        })
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            " color={} strength={} size={}",
            self.color, self.strength, self.size
        )
    }

    fn apply(&self, image: &RgbImage, seed: u64) -> Result<RgbImage, Error> {
        Ok(leak(image, *self, seed))
    }
}

impl Spec for Bloom {
    const NAME: &'static str = "bloom";

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
        };
        let lit = leak(&flat, params, 1);
        assert!(mean(&lit) > 41.0);
        assert_eq!(leak(&flat, params, 1), lit);
        assert_ne!(leak(&flat, params, 2), lit);
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
