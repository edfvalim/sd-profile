//! On-disk formats: partition tables and filesystems. Everything here reads
//! through [`block::BlockReader`] and has no OS dependencies.

pub mod alignment;
pub mod block;
mod crc32;
pub mod fs;
mod gpt;
mod mbr;
pub mod table;
pub mod volumes;

/// Little-endian integer at `at`. Callers pass slices they have already sized
/// (sectors, headers, entries), so a short slice is a bug.
fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(
        bytes[at..at + 2]
            .try_into()
            .expect("a 2-byte slice converts to [u8; 2]"),
    )
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        bytes[at..at + 4]
            .try_into()
            .expect("a 4-byte slice converts to [u8; 4]"),
    )
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("a 8-byte slice converts to [u8; 8]"),
    )
}
