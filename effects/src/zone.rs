//! Zones of hues: what an effect acts on is the colours near a hue, whole
//! within a few degrees of it and fading away beyond, and never the greys.

use super::colour::{Hue, to_hsv};

/// 0 up to `from`, 1 from `to` on, smooth in between.
pub(super) fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Under this saturation a colour is a grey, which has no hue to be near,
/// and from `COLOURED_FROM` on it has a hue to be judged by.
const GREY_BELOW: f32 = 0.02;
const COLOURED_FROM: f32 = 0.06;

/// How much a colour has a hue: 0 for a grey, 1 once it has some colour,
/// from its saturation.
pub(super) fn colourfulness(saturation: f32) -> f32 {
    smoothstep(GREY_BELOW, COLOURED_FROM, saturation)
}

/// The colours near a hue: whole within `width` degrees of it either side,
/// fading to nothing over `soft` more degrees.
#[derive(Debug, Clone, Copy)]
pub(super) struct HueZone {
    centre: Hue,
    width: f32,
    soft: f32,
}

impl HueZone {
    pub(super) fn new(centre: Hue, width: u16, soft: u16) -> HueZone {
        HueZone {
            centre,
            width: f32::from(width),
            // A fade of nothing would be a step.
            soft: f32::from(soft).max(1.0),
        }
    }

    /// How near a hue is to the zone, from 0 to 1, whatever the colour it
    /// belongs to.
    pub(super) fn nearness(&self, hue: Hue) -> f32 {
        1.0 - smoothstep(
            self.width,
            self.width + self.soft,
            hue.distance(self.centre),
        )
    }

    /// How much of the zone a colour is, channels from 0 to 255: how near
    /// its hue is, and none for a grey.
    pub(super) fn weight(&self, rgb: [f32; 3]) -> f32 {
        let [hue, saturation, _] = to_hsv(rgb.map(|channel| channel / 255.0));
        self.nearness(Hue::new(hue)) * colourfulness(saturation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> HueZone {
        // Red, whole within 20 degrees, gone 40 degrees further.
        HueZone::new(Hue::new(0.0), 20, 40)
    }

    #[test]
    fn a_zone_is_whole_near_its_hue_fades_and_ends() {
        let zone = zone();
        assert_eq!(zone.nearness(Hue::new(0.0)), 1.0);
        assert_eq!(zone.nearness(Hue::new(20.0)), 1.0);
        assert_eq!(zone.nearness(Hue::new(340.0)), 1.0, "round the circle");
        let half = zone.nearness(Hue::new(40.0));
        assert!((half - 0.5).abs() < 1e-6, "{half}");
        assert_eq!(zone.nearness(Hue::new(60.0)), 0.0);
        assert_eq!(zone.nearness(Hue::new(180.0)), 0.0);
    }

    #[test]
    fn a_grey_is_in_no_zone_whatever_its_hue() {
        let zone = zone();
        assert_eq!(zone.weight([200.0, 30.0, 30.0]), 1.0);
        assert_eq!(zone.weight([128.0, 128.0, 128.0]), 0.0);
        assert_eq!(zone.weight([0.0, 0.0, 0.0]), 0.0);
        // A pale red, nearly grey, counts for less than a strong one.
        let pale = zone.weight([130.0, 125.0, 125.0]);
        assert!(pale > 0.0 && pale < 1.0, "{pale}");
        assert_eq!(colourfulness(0.0), 0.0);
        assert_eq!(colourfulness(1.0), 1.0);
    }

    #[test]
    fn no_fade_is_a_step_not_a_division_by_zero() {
        let step = HueZone::new(Hue::new(0.0), 30, 0);
        assert_eq!(step.nearness(Hue::new(30.0)), 1.0);
        assert!(step.nearness(Hue::new(32.0)) < 0.5);
        assert!(step.nearness(Hue::new(32.0)).is_finite());
    }
}
