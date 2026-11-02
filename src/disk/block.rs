//! Reading bytes from a disk or disk image.
//!
//! Every raw read in the crate goes through [`BlockReader`]. macOS raw devices
//! (`/dev/rdiskN`) reject reads that don't start and end on a block boundary;
//! [`AlignedReader`] widens each read to whole blocks, so callers can ask for
//! any byte range on any platform.

use std::io::{self, Read, Seek, SeekFrom};

use crate::model::{BlockSize, Lba};

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("read of {len} bytes at offset {offset} is past the end of the disk ({size} bytes)")]
    OutOfBounds { offset: u64, len: usize, size: u64 },
    #[error("read at offset {offset} failed: {source}")]
    Io {
        offset: u64,
        #[source]
        source: io::Error,
    },
}

/// Random-access reads from a disk of known size.
pub trait BlockReader {
    /// Logical block size, used to turn LBAs into byte offsets.
    fn block_size(&self) -> BlockSize;

    /// Size of the disk in bytes.
    fn size_bytes(&self) -> u64;

    /// Fills `buf` with the bytes starting at `offset`.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfBounds`] if the range extends past the end of the
    /// disk, [`ReadError::Io`] if the underlying read fails.
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError>;

    /// Reads `count` blocks starting at `lba`.
    ///
    /// # Errors
    ///
    /// As [`BlockReader::read_exact_at`].
    fn read_blocks(&mut self, lba: Lba, count: usize) -> Result<Vec<u8>, ReadError> {
        let block = self.block_size().bytes() as usize;
        let len = block.saturating_mul(count);
        let offset = lba.0.saturating_mul(u64::from(self.block_size().bytes()));
        let mut buf = vec![0; len];
        self.read_exact_at(offset, &mut buf)?;
        Ok(buf)
    }
}

/// A [`BlockReader`] over anything seekable that only issues reads aligned to
/// whole blocks.
pub struct AlignedReader<R> {
    inner: R,
    block_size: BlockSize,
    size: u64,
    scratch: Vec<u8>,
}

impl<R: Read + Seek> AlignedReader<R> {
    /// `size` is the disk size in bytes; reads past it fail without touching
    /// `inner`.
    pub fn new(inner: R, block_size: BlockSize, size: u64) -> Self {
        Self {
            inner,
            block_size,
            size,
            scratch: Vec::new(),
        }
    }
}

impl<R: Read + Seek> BlockReader for AlignedReader<R> {
    fn block_size(&self) -> BlockSize {
        self.block_size
    }

    fn size_bytes(&self) -> u64 {
        self.size
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        let out_of_bounds = || ReadError::OutOfBounds {
            offset,
            len: buf.len(),
            size: self.size,
        };
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= self.size)
            .ok_or_else(out_of_bounds)?;
        if buf.is_empty() {
            return Ok(());
        }

        let block = u64::from(self.block_size.bytes());
        let aligned_start = offset - offset % block;
        // A disk image's size need not be a whole number of blocks; its last
        // partial block can only be read up to the end of the file.
        let aligned_end = end.div_ceil(block).saturating_mul(block).min(self.size);
        let io_error = |source| ReadError::Io {
            offset: aligned_start,
            source,
        };

        let span = usize::try_from(aligned_end - aligned_start).map_err(|_| out_of_bounds())?;
        self.scratch.resize(span, 0);
        self.inner
            .seek(SeekFrom::Start(aligned_start))
            .map_err(io_error)?;
        self.inner.read_exact(&mut self.scratch).map_err(io_error)?;

        let skip = usize::try_from(offset - aligned_start)
            .expect("the skip is less than one block, which fits in usize");
        buf.copy_from_slice(&self.scratch[skip..skip + buf.len()]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A device that, like a macOS raw device, fails any read or seek that
    /// isn't aligned to 512 bytes.
    struct StrictDevice {
        data: Cursor<Vec<u8>>,
    }

    impl Read for StrictDevice {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !buf.len().is_multiple_of(512) {
                return Err(io::Error::other("unaligned read length"));
            }
            self.data.read(buf)
        }
    }

    impl Seek for StrictDevice {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            let to = self.data.seek(pos)?;
            if to % 512 != 0 {
                return Err(io::Error::other("unaligned seek"));
            }
            Ok(to)
        }
    }

    /// The byte at `offset` in the test images: a pattern that doesn't repeat
    /// on block boundaries, so misplaced copies show up.
    fn pattern(offset: usize) -> u8 {
        u8::try_from(offset % 251).unwrap()
    }

    fn strict(len: usize) -> AlignedReader<StrictDevice> {
        let bytes: Vec<u8> = (0..len).map(pattern).collect();
        AlignedReader::new(
            StrictDevice {
                data: Cursor::new(bytes),
            },
            BlockSize::SD,
            len as u64,
        )
    }

    #[test]
    fn unaligned_read_within_one_block() {
        let mut reader = strict(4096);
        let mut buf = [0; 4];
        reader.read_exact_at(510, &mut buf).unwrap();
        let expected: Vec<u8> = (510..514).map(pattern).collect();
        assert_eq!(buf.to_vec(), expected);
    }

    #[test]
    fn unaligned_read_across_blocks() {
        let mut reader = strict(4096);
        let mut buf = vec![0; 1000];
        reader.read_exact_at(100, &mut buf).unwrap();
        let expected: Vec<u8> = (100..1100).map(pattern).collect();
        assert_eq!(buf, expected);
    }

    #[test]
    fn read_blocks_by_lba() {
        let mut reader = strict(4096);
        let block = reader.read_blocks(Lba(2), 1).unwrap();
        assert_eq!(block.len(), 512);
        assert_eq!(block[0], pattern(1024));
    }

    #[test]
    fn read_past_end_is_out_of_bounds() {
        let mut reader = strict(1024);
        let mut buf = [0; 16];
        assert!(matches!(
            reader.read_exact_at(1020, &mut buf),
            Err(ReadError::OutOfBounds {
                offset: 1020,
                len: 16,
                size: 1024
            })
        ));
    }

    #[test]
    fn image_with_partial_last_block() {
        let bytes: Vec<u8> = (0..700).map(pattern).collect();
        let mut reader = AlignedReader::new(Cursor::new(bytes), BlockSize::SD, 700);
        let mut buf = [0; 10];
        reader.read_exact_at(690, &mut buf).unwrap();
        assert_eq!(buf[9], pattern(699));
    }

    #[test]
    fn io_failure_is_reported() {
        let mut reader = strict(4096);
        // Shrink the device behind the reader's back so the read hits EOF.
        reader.inner.data.get_mut().truncate(100);
        let mut buf = [0; 4];
        assert!(matches!(
            reader.read_exact_at(0, &mut buf),
            Err(ReadError::Io { offset: 0, .. })
        ));
    }
}
