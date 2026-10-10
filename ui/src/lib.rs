//! What the terminal interfaces of corrode share: jobs run in background
//! threads, pictures turned into terminal graphics, and text laid out to
//! the width of the terminal. Nothing here knows what a shot or an
//! effect is.

pub mod encoder;
pub mod loader;
pub mod text;
