//! Creative effects: what the compression, the wear and the failures of
//! images do to them, done on purpose. Each effect is a function of an
//! image and its parameters to a new image; a [`Recipe`] chains them, with
//! one seed for whatever they draw at random, so that a result can be
//! made again from its text.

mod film;
mod glitch;
mod jpeg;
mod print;
mod random;
mod recipe;
mod sort;

pub use film::{
    Aberration, Blend, Bloom, Drag, DragKind, Fade, Grain, Leak, Vignette, aberration, bloom, drag,
    fade, grain, leak, vignette,
};
pub use glitch::{
    ChannelSplit, PixelStretch, REFERENCE_WIDTH, SliceShift, channel_split, pixel_stretch,
    slice_shift,
};
pub use jpeg::{Databend, GenerationLoss, databend, generation_loss};
pub use print::{Color, Dither, DitherMethod, Duotone, Scanlines, dither, duotone, scanlines};
pub use recipe::{Effect, NAMES, Recipe};
pub use sort::{Direction, PixelSort, pixel_sort};

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
}
