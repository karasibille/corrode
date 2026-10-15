//! Creative effects: what the compression, the wear and the failures of
//! images do to them, done on purpose. Each effect is a function of an
//! image and its parameters to a new image; a [`Recipe`] chains them, with
//! one seed for whatever they draw at random, so that a result can be
//! made again from its text.

mod colour;
mod film;
mod ghost;
mod glitch;
mod grade;
mod jpeg;
mod mask;
mod parallel;
mod print;
mod random;
mod recipe;
mod register;
mod remap;
mod sort;
mod spin;
mod units;
mod warp;
mod zone;

pub use colour::{Color, Hue};
pub use film::{
    Aberration, Blend, Bloom, Drag, DragKind, Fade, Grain, Leak, LeakEdge, StreakPosition, Streaks,
    Vignette, aberration, bloom, drag, fade, grain, leak, streaks, vignette,
};
pub use ghost::{Ghost, GhostBlend, ghost, track};
pub use glitch::{
    ChannelSplit, PixelStretch, SliceShift, channel_split, pixel_stretch, slice_shift,
};
pub use grade::{
    ChannelOrder, Channels, FalseColor, MaybeHue, OnePoint, Tritone, false_color, one_point,
    tritone,
};
pub use jpeg::{Databend, GenerationLoss, databend, generation_loss};
pub use print::{Dither, DitherMethod, Duotone, Scanlines, dither, duotone, scanlines};
pub use recipe::{Effect, NAMES, ParamInfo, Recipe, describe};
pub use register::{register, shifted};
pub use remap::{HueMap, MAX_MOVES, Remap, remap};
pub use sort::{Direction, PixelSort, pixel_sort};
pub use spin::{Spin, spin};
pub use units::{Percent, Point, REFERENCE_WIDTH, Size, scale_of};
pub use warp::{Liquid, liquid};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("unknown effect '{0}'")]
    UnknownEffect(String),
    #[error("{effect}: unknown parameter '{name}'")]
    UnknownParameter { effect: String, name: String },
    #[error("{effect}: {name}: cannot read '{value}'")]
    BadValue {
        effect: String,
        name: String,
        value: String,
    },
    #[error("no effect given")]
    Empty,
    #[error("{0}")]
    Jpeg(String),
    #[error("ghost: {0}")]
    Frames(String),
}
