//! Filesystems: what a volume holds, and the details its metadata gives.
//!
//! Every read goes through a [`Volume`], a window on the disk at the
//! partition's offset. The volume's first sector says what it holds; the
//! filesystem's own parser then reads what else it needs. A read that fails
//! there (past the end of a truncated image, or an I/O error) makes only the
//! affected value unavailable, not the whole filesystem.

mod exfat;
mod ext;
mod fat;

use super::block::{BlockReader, ReadError};
use crate::model::{Extent, Field, Filesystem, FsIssue, Source, Unavailable};

/// Bytes read from the start of a volume to tell what it holds: the boot
/// sector (FAT, exFAT, NTFS) and the ext superblock at byte 1024.
const PROBE_LEN: usize = 2048;

/// A byte range of the disk holding one filesystem.
struct Volume<'a, R> {
    disk: &'a mut R,
    /// Byte offset on the disk.
    start: u64,
    /// Size of the partition in bytes. Reads are not limited to it: a
    /// filesystem larger than its partition is reported as an issue, and its
    /// metadata is still read where the disk has it.
    len: u64,
}

impl<R: BlockReader> Volume<'_, R> {
    /// Reads `len` bytes at `offset` from the volume start.
    fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, ReadError> {
        let mut buf = vec![0; len];
        // An offset that overflows is past the end of any disk; the reader
        // reports it as such.
        self.disk
            .read_exact_at(self.start.saturating_add(offset), &mut buf)?;
        Ok(buf)
    }

    /// The issue to report when the filesystem says it spans more bytes than
    /// the partition holds.
    fn size_issue(&self, filesystem_bytes: u64) -> Option<FsIssue> {
        (filesystem_bytes > self.len).then_some(FsIssue::LargerThanPartition {
            filesystem_bytes,
            partition_bytes: self.len,
        })
    }
}

/// Why a value that needs more of the filesystem's metadata is unavailable.
#[derive(Debug, thiserror::Error)]
enum MetadataError {
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The metadata contradicts itself or lacks a structure it must have.
    #[error("{0}")]
    Inconsistent(&'static str),
}

/// A value read from `source`, or unavailable with the reason.
fn field<T, E: std::fmt::Display>(result: Result<T, E>, source: Source) -> Field<T> {
    match result {
        Ok(value) => Field::known(value, source),
        Err(err) => Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
    }
}

/// Reads the filesystem of the volume at `extent`.
pub fn read(disk: &mut impl BlockReader, extent: Extent) -> Field<Filesystem> {
    let start = extent.offset_bytes();
    let available = disk
        .size_bytes()
        .saturating_sub(start)
        .min(extent.size_bytes());
    let probe_len = usize::try_from(available).map_or(PROBE_LEN, |n| n.min(PROBE_LEN));
    if probe_len == 0 {
        return Field::Unavailable(Unavailable::ReadFailed(
            "the volume starts past the end of the disk".to_owned(),
        ));
    }
    let mut volume = Volume {
        disk,
        start,
        len: extent.size_bytes(),
    };
    let mut head = match volume.read(0, probe_len) {
        Ok(head) => head,
        Err(err) => return Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
    };
    // A volume smaller than the probe reads as zeros past its end.
    head.resize(PROBE_LEN, 0);

    match probe(&head) {
        Probe::Exfat(boot) => Field::known(exfat::read(&mut volume, &boot), Source::BootSector),
        Probe::Fat(boot) => Field::known(fat::read(&mut volume, &boot), Source::BootSector),
        Probe::Ext(superblock) => {
            Field::known(ext::read(&mut volume, &superblock), Source::Superblock)
        }
        Probe::Ntfs => Field::known(Filesystem::Ntfs, Source::BootSector),
        Probe::Blank => Field::known(Filesystem::Blank, Source::BootSector),
        Probe::Unrecognized => Field::known(Filesystem::Unrecognized, Source::BootSector),
    }
}

enum Probe {
    Exfat(exfat::BootSector),
    Fat(fat::BootSector),
    Ext(ext::Superblock),
    Ntfs,
    Blank,
    Unrecognized,
}

const NTFS_OEM_NAME: &[u8; 8] = b"NTFS    ";

/// What the first [`PROBE_LEN`] bytes of a volume hold. exFAT and NTFS are
/// named in the OEM field; FAT has only its BPB to go by, so it comes after
/// them; ext keeps its superblock at byte 1024, past any boot sector.
fn probe(head: &[u8]) -> Probe {
    let sector = &head[..512];
    if &sector[3..11] == NTFS_OEM_NAME {
        return Probe::Ntfs;
    }
    if exfat::is_exfat(sector) {
        return exfat::BootSector::parse(sector).map_or(Probe::Unrecognized, Probe::Exfat);
    }
    if let Some(boot) = fat::BootSector::parse(sector) {
        return Probe::Fat(boot);
    }
    if let Some(superblock) = ext::Superblock::parse(&head[ext::SUPERBLOCK_OFFSET..]) {
        return Probe::Ext(superblock);
    }
    if head.iter().all(|&b| b == 0) {
        Probe::Blank
    } else {
        Probe::Unrecognized
    }
}

/// Whether sector 0 of a disk is a filesystem boot sector rather than an
/// MBR. Both end in `0x55AA`, so this asks for more than [`probe`] does:
/// exFAT and NTFS by name, FAT only with a sane BPB together with its type
/// string.
#[must_use]
pub fn is_boot_sector(sector: &[u8]) -> bool {
    &sector[3..11] == NTFS_OEM_NAME
        || exfat::is_exfat(sector)
        || fat::BootSector::parse(sector).is_some_and(|boot| boot.has_type_string())
}

/// Whether the disk holds an ext superblock at byte 1024: an ext filesystem
/// written to the whole disk, whose first sectors may be all zeros.
///
/// # Errors
///
/// When the disk can't be read; a disk too small to hold a superblock is
/// `false`, not an error.
pub fn has_ext_superblock(disk: &mut impl BlockReader) -> Result<bool, ReadError> {
    let mut superblock = vec![0; ext::SUPERBLOCK_LEN];
    match disk.read_exact_at(ext::SUPERBLOCK_OFFSET as u64, &mut superblock) {
        Ok(()) => Ok(ext::Superblock::parse(&superblock).is_some()),
        Err(ReadError::OutOfBounds { .. }) => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{BlockSize, Lba};
    use crate::test_support::{fixture_image, reader};

    pub fn extent(start: u64, sectors: u64) -> Extent {
        Extent::new(Lba(start), sectors, BlockSize::SD)
    }

    /// Byte offset of sector `lba`.
    pub fn sector_of(lba: u64) -> usize {
        usize::try_from(lba).unwrap() * 512
    }

    /// Reads the filesystem of the volume at `start` in fixture `image`.
    pub fn read_volume(image: &str, start: u64, sectors: u64) -> Field<Filesystem> {
        let mut disk = reader(fixture_image(image));
        read(&mut disk, extent(start, sectors))
    }

    fn kind(field: &Field<Filesystem>) -> &'static str {
        match field {
            Field::Known { value, .. } => value.name(),
            Field::Unavailable(reason) => panic!("unavailable: {reason}"),
        }
    }

    #[test]
    fn probes_each_fixture_volume() {
        for (image, start, sectors, expected) in [
            ("mbr-fat12-fat16", 2048, 8192, "FAT12"),
            ("mbr-fat12-fat16", 10240, 55296, "FAT16"),
            ("mbr-fat32", 63, 130_977, "FAT32"),
            ("mbr-extended", 109_557, 31256, "exFAT"),
            ("gpt-exfat", 2048, 126_976, "exFAT"),
            ("mbr-ext", 2048, 40960, "ext2"),
            ("mbr-ext", 43008, 40960, "ext3"),
            ("mbr-ext", 83968, 40960, "ext4"),
            ("superfloppy-ext4", 0, 16384, "ext4"),
            ("blank", 0, 2048, "blank"),
        ] {
            assert_eq!(
                kind(&read_volume(image, start, sectors)),
                expected,
                "{image} at {start}"
            );
        }
    }

    #[test]
    fn mbr_sector_is_unrecognized_not_blank() {
        assert_eq!(kind(&read_volume("mbr-fat32", 0, 63)), "unrecognized");
    }

    #[test]
    fn ntfs_is_named() {
        let mut image = vec![0u8; 4096];
        image[3..11].copy_from_slice(NTFS_OEM_NAME);
        let mut disk = reader(image);
        assert_eq!(kind(&read(&mut disk, extent(0, 8))), "NTFS");
    }

    #[test]
    fn volume_past_end_of_disk_is_unavailable() {
        let mut disk = reader(vec![0; 4096]);
        assert!(matches!(
            read(&mut disk, extent(100, 8)),
            Field::Unavailable(Unavailable::ReadFailed(_))
        ));
    }

    #[test]
    fn boot_sectors_at_sector_zero() {
        let sector0 = |name: &str| fixture_image(name)[..512].to_vec();
        assert!(is_boot_sector(&sector0("superfloppy-fat32")));
        for name in [
            "mbr-fat32",
            "mbr-sd-aligned",
            "mbr-extended",
            "gpt-exfat",
            "superfloppy-ext4",
        ] {
            assert!(!is_boot_sector(&sector0(name)), "{name}");
        }
        let mut exfat = [0u8; 512];
        exfat[3..11].copy_from_slice(b"EXFAT   ");
        assert!(is_boot_sector(&exfat));
    }

    #[test]
    fn ext_superblock_on_whole_disk() {
        assert!(has_ext_superblock(&mut reader(fixture_image("superfloppy-ext4"))).unwrap());
        assert!(!has_ext_superblock(&mut reader(fixture_image("mbr-ext"))).unwrap());
        assert!(!has_ext_superblock(&mut reader(vec![0; 512])).unwrap());
    }
}
