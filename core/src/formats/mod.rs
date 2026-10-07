//! The file formats read without a decoder: what a TIFF structure, a JPEG
//! file and a Panasonic RW2 file hold, and where, so that a preview or a
//! thumbnail can be found from the head of a file.

pub mod jpeg;
pub mod rw2;
pub mod tiff;

#[cfg(test)]
pub(crate) mod testing;
