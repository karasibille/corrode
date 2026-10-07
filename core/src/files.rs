//! Reading parts of files: the metadata of a photo sits in its first
//! kilobytes, and a preview at a known place, so whole files are rarely
//! needed. This matters on a spinning disk, where a RAW file takes a
//! large part of a second to read.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;

/// How much of a file is read for its metadata: a RW2 has it in its first
/// 64 KB, a JPEG in a segment of at most 64 KB near the start.
const HEAD: usize = 256 * 1024;

/// The first bytes of a file, where its metadata is.
pub(crate) fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(HEAD);
    File::open(path)?.take(HEAD as u64).read_to_end(&mut data)?;
    Ok(data)
}

/// Reads a part of a file.
pub(crate) fn read_range(path: &Path, range: Range<usize>) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(range.start as u64))?;
    let mut data = Vec::with_capacity(range.len());
    file.take(range.len() as u64).read_to_end(&mut data)?;
    Ok(data)
}
