//! Helpers for the unit tests: loading the disk image fixtures.

use std::io::Cursor;
use std::path::PathBuf;

use crate::disk::block::AlignedReader;
use crate::model::BlockSize;

/// Decompresses `tests/fixtures/disk/<name>.img.zst`.
pub fn fixture_image(name: &str) -> Vec<u8> {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "disk",
        &format!("{name}.img.zst"),
    ]
    .iter()
    .collect();
    let compressed =
        std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    zstd::decode_all(compressed.as_slice()).expect("fixture decompresses")
}

/// A reader over an in-memory image with 512-byte sectors.
pub fn reader(image: Vec<u8>) -> AlignedReader<Cursor<Vec<u8>>> {
    let size = image.len() as u64;
    AlignedReader::new(Cursor::new(image), BlockSize::SD, size)
}
