//! exFAT.
//!
//! The volume starts with the main boot region: the boot sector, 8 extended
//! boot sectors, OEM parameters, a reserved sector and a checksum sector (12
//! sectors), followed by a backup copy. Then come the FAT and the cluster
//! heap. The root directory, in the heap, holds the volume label and the
//! allocation bitmap, which records which clusters are in use. Source:
//! Microsoft, "exFAT file system specification".

use super::{MetadataError, Volume, field};
use crate::disk::block::{BlockReader, ReadError};
use crate::disk::{le_u16, le_u32, le_u64};
use crate::model::{
    Exfat, Field, Filesystem, FsIssue, Revision, Source, Unavailable, VolumeSerial,
};

/// Offsets within the main boot sector.
mod boot {
    use std::ops::Range;

    pub const FILE_SYSTEM_NAME: Range<usize> = 3..11;
    pub const MUST_BE_ZERO: Range<usize> = 11..64;
    pub const VOLUME_LENGTH: usize = 72;
    pub const FAT_OFFSET: usize = 80;
    pub const FAT_LENGTH: usize = 84;
    pub const CLUSTER_HEAP_OFFSET: usize = 88;
    pub const CLUSTER_COUNT: usize = 92;
    pub const ROOT_CLUSTER: usize = 96;
    pub const SERIAL: usize = 100;
    pub const REVISION: usize = 104;
    pub const VOLUME_FLAGS: usize = 106;
    pub const BYTES_PER_SECTOR_SHIFT: usize = 108;
    pub const SECTORS_PER_CLUSTER_SHIFT: usize = 109;
    pub const FAT_COUNT: usize = 110;
    pub const PERCENT_IN_USE: usize = 112;
    pub const SIGNATURE: usize = 510;
}

const NAME: &[u8; 8] = b"EXFAT   ";

/// `VolumeFlags` bits.
const ACTIVE_FAT: u16 = 0x0001;
const VOLUME_DIRTY: u16 = 0x0002;
const MEDIA_FAILURE: u16 = 0x0004;

const PERCENT_UNKNOWN: u8 = 0xFF;

/// Sectors covered by the boot checksum; the checksum sector follows them.
const CHECKSUMMED_SECTORS: usize = 11;

/// Directory entry types and fields.
mod entry {
    pub const LEN: usize = 32;
    pub const END_OF_DIRECTORY: u8 = 0x00;
    pub const ALLOCATION_BITMAP: u8 = 0x81;
    pub const VOLUME_LABEL: u8 = 0x83;
    /// Allocation bitmap: bit 0 says which FAT the bitmap belongs to.
    pub const BITMAP_FLAGS: usize = 1;
    pub const FIRST_CLUSTER: usize = 20;
    pub const DATA_LENGTH: usize = 24;
    /// Volume label: character count, then up to 11 UTF-16 characters.
    pub const CHARACTER_COUNT: usize = 1;
    pub const LABEL: usize = 2;
    pub const MAX_LABEL_CHARS: usize = 11;
}

/// Root directory entries scanned for the label and the bitmap, which
/// formatters write first. Only bounds the read.
const MAX_ROOT_ENTRIES_SCANNED: usize = 65536;

/// FAT entry value ending a cluster chain.
const END_OF_CHAIN: u32 = 0xFFFF_FFFF;

/// Whether the boot sector names exFAT.
pub fn is_exfat(sector: &[u8]) -> bool {
    sector[boot::FILE_SYSTEM_NAME] == *NAME
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootSector {
    volume_length: u64,
    fat_offset: u32,
    fat_length: u32,
    cluster_heap_offset: u32,
    cluster_count: u32,
    root_cluster: u32,
    serial: u32,
    revision: u16,
    flags: u16,
    sector_shift: u8,
    cluster_shift: u8,
    fat_count: u8,
    percent_in_use: u8,
}

impl BootSector {
    /// Parses a main boot sector whose fields are within the ranges the
    /// specification allows; `None` otherwise.
    pub fn parse(sector: &[u8]) -> Option<Self> {
        let parsed = Self {
            volume_length: le_u64(sector, boot::VOLUME_LENGTH),
            fat_offset: le_u32(sector, boot::FAT_OFFSET),
            fat_length: le_u32(sector, boot::FAT_LENGTH),
            cluster_heap_offset: le_u32(sector, boot::CLUSTER_HEAP_OFFSET),
            cluster_count: le_u32(sector, boot::CLUSTER_COUNT),
            root_cluster: le_u32(sector, boot::ROOT_CLUSTER),
            serial: le_u32(sector, boot::SERIAL),
            revision: le_u16(sector, boot::REVISION),
            flags: le_u16(sector, boot::VOLUME_FLAGS),
            sector_shift: sector[boot::BYTES_PER_SECTOR_SHIFT],
            cluster_shift: sector[boot::SECTORS_PER_CLUSTER_SHIFT],
            fat_count: sector[boot::FAT_COUNT],
            percent_in_use: sector[boot::PERCENT_IN_USE],
        };
        let fats_end = u64::from(parsed.fat_offset)
            + u64::from(parsed.fat_length) * u64::from(parsed.fat_count);
        let valid = is_exfat(sector)
            && sector[boot::MUST_BE_ZERO].iter().all(|&b| b == 0)
            && sector[boot::SIGNATURE..boot::SIGNATURE + 2] == [0x55, 0xAA]
            && (9..=12).contains(&parsed.sector_shift)
            && parsed.cluster_shift <= 25 - parsed.sector_shift
            && matches!(parsed.fat_count, 1 | 2)
            && parsed.cluster_count > 0
            && u64::from(parsed.cluster_heap_offset) >= fats_end
            && (2..=parsed.last_cluster()).contains(&parsed.root_cluster);
        valid.then_some(parsed)
    }

    fn sector_bytes(&self) -> u64 {
        1 << self.sector_shift
    }

    fn cluster_bytes(&self) -> u64 {
        self.sector_bytes() << self.cluster_shift
    }

    fn last_cluster(&self) -> u32 {
        self.cluster_count.saturating_add(1)
    }

    /// Byte offset of the FAT in use from the volume start. On `TexFAT`
    /// volumes (two FATs) the `ActiveFat` flag picks one.
    /// Index of the FAT in use (and of its allocation bitmap): the
    /// `ActiveFat` flag on `TexFAT` volumes, which have two, else 0.
    fn active_fat(&self) -> u8 {
        u8::from(self.flags & ACTIVE_FAT != 0 && self.fat_count == 2)
    }

    fn active_fat_offset(&self) -> u64 {
        (u64::from(self.fat_offset) + u64::from(self.active_fat()) * u64::from(self.fat_length))
            * self.sector_bytes()
    }

    fn cluster_offset(&self, cluster: u32) -> u64 {
        u64::from(self.cluster_heap_offset) * self.sector_bytes()
            + u64::from(cluster - 2) * self.cluster_bytes()
    }
}

/// Reads the volume the boot sector describes.
pub(super) fn read<R: BlockReader>(volume: &mut Volume<'_, R>, boot: &BootSector) -> Filesystem {
    let mut issues = Vec::new();
    if boot_checksum_matches(volume, boot) == Some(false) {
        issues.push(FsIssue::BootChecksumMismatch);
    }
    if boot.flags & VOLUME_DIRTY != 0 {
        issues.push(FsIssue::NotCleanlyUnmounted);
    }
    if boot.flags & MEDIA_FAILURE != 0 {
        issues.push(FsIssue::ErrorsRecorded);
    }
    issues.extend(volume.size_issue(boot.volume_length.saturating_mul(boot.sector_bytes())));

    let (label, free_bytes) = match root_entries(volume, boot) {
        Err(err) => {
            let reason = Unavailable::ReadFailed(err.to_string());
            (
                Field::Unavailable(reason.clone()),
                Field::Unavailable(reason),
            )
        }
        Ok(root) => {
            let label = Field::known(root.label, Source::RootDirectory);
            let used = root
                .bitmap
                .ok_or(MetadataError::Inconsistent(
                    "the root directory has no allocation bitmap entry",
                ))
                .and_then(|bitmap| count_used(volume, boot, bitmap));
            let free = field(
                used.map(|used| u64::from(boot.cluster_count - used) * boot.cluster_bytes()),
                Source::AllocationBitmap,
            );
            (label, free)
        }
    };

    Filesystem::Exfat(Exfat {
        label,
        volume_id: VolumeSerial(boot.serial),
        revision: Revision {
            major: boot.revision.to_le_bytes()[1],
            minor: boot.revision.to_le_bytes()[0],
        },
        sector_size_bytes: u32::try_from(boot.sector_bytes()).expect("at most 4096"),
        cluster_size_bytes: u32::try_from(boot.cluster_bytes()).expect("at most 32 MiB"),
        cluster_count: boot.cluster_count,
        size_bytes: u64::from(boot.cluster_count) * boot.cluster_bytes(),
        free_bytes,
        data_offset_bytes: u64::from(boot.cluster_heap_offset) * boot.sector_bytes(),
        fat_count: boot.fat_count,
        percent_in_use: (boot.percent_in_use != PERCENT_UNKNOWN).then_some(boot.percent_in_use),
        issues,
    })
}

/// Checks the main boot region against its checksum sector. `None` when the
/// region can't be read.
///
/// The checksum covers the first 11 sectors, except `VolumeFlags` and
/// `PercentInUse`, which change without the region being rewritten. The
/// 12th sector repeats it in every 4-byte slot; the first one is checked.
fn boot_checksum_matches<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
) -> Option<bool> {
    let sector = usize::try_from(boot.sector_bytes()).expect("at most 4096");
    let region = volume.read(0, sector * (CHECKSUMMED_SECTORS + 1)).ok()?;
    let (covered, checksum_sector) = region.split_at(sector * CHECKSUMMED_SECTORS);
    Some(boot_checksum(covered) == le_u32(checksum_sector, 0))
}

fn boot_checksum(covered: &[u8]) -> u32 {
    covered
        .iter()
        .enumerate()
        .filter(|&(i, _)| {
            i != boot::VOLUME_FLAGS && i != boot::VOLUME_FLAGS + 1 && i != boot::PERCENT_IN_USE
        })
        .fold(0u32, |sum, (_, &byte)| {
            sum.rotate_right(1).wrapping_add(u32::from(byte))
        })
}

/// Where an allocation bitmap lives.
#[derive(Debug, Clone, Copy)]
struct Bitmap {
    first_cluster: u32,
    length: u64,
}

struct RootEntries {
    label: Option<String>,
    bitmap: Option<Bitmap>,
}

/// Follows a cluster chain through the FAT in use.
struct Chain {
    next: Option<u32>,
    steps: u32,
}

impl Chain {
    fn new(first: u32) -> Self {
        Self {
            next: Some(first),
            steps: 0,
        }
    }

    /// The next cluster, `None` at the end of the chain or on an entry that
    /// points outside the heap. A chain longer than the heap has a loop and
    /// stops too.
    fn advance<R: BlockReader>(
        &mut self,
        volume: &mut Volume<'_, R>,
        boot: &BootSector,
    ) -> Result<Option<u32>, ReadError> {
        let Some(current) = self.next.filter(|c| (2..=boot.last_cluster()).contains(c)) else {
            return Ok(None);
        };
        self.steps += 1;
        if self.steps > boot.cluster_count {
            self.next = None;
            return Ok(Some(current));
        }
        let entry = volume.read(boot.active_fat_offset() + u64::from(current) * 4, 4)?;
        let value = le_u32(&entry, 0);
        self.next = (value != END_OF_CHAIN).then_some(value);
        Ok(Some(current))
    }
}

/// Scans the root directory for the volume label and the allocation bitmap
/// of the FAT in use.
fn root_entries<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
) -> Result<RootEntries, ReadError> {
    let active = boot.active_fat();
    let cluster_len = usize::try_from(boot.cluster_bytes()).expect("at most 32 MiB");
    let mut found = RootEntries {
        label: None,
        bitmap: None,
    };
    let mut scanned = 0;
    let mut chain = Chain::new(boot.root_cluster);
    while let Some(cluster) = chain.advance(volume, boot)? {
        let bytes = volume.read(boot.cluster_offset(cluster), cluster_len)?;
        for e in bytes.as_chunks::<{ entry::LEN }>().0 {
            scanned += 1;
            if e[0] == entry::END_OF_DIRECTORY || scanned > MAX_ROOT_ENTRIES_SCANNED {
                return Ok(found);
            }
            match e[0] {
                entry::VOLUME_LABEL => found.label = decode_label(e),
                entry::ALLOCATION_BITMAP if e[entry::BITMAP_FLAGS] & 1 == active => {
                    found.bitmap = Some(Bitmap {
                        first_cluster: le_u32(e, entry::FIRST_CLUSTER),
                        length: le_u64(e, entry::DATA_LENGTH),
                    });
                }
                _ => {}
            }
        }
    }
    Ok(found)
}

fn decode_label(e: &[u8]) -> Option<String> {
    let count = usize::from(e[entry::CHARACTER_COUNT]).min(entry::MAX_LABEL_CHARS);
    let units: Vec<u16> = (0..count)
        .map(|i| le_u16(e, entry::LABEL + 2 * i))
        .collect();
    let label = String::from_utf16_lossy(&units);
    (!label.is_empty()).then_some(label)
}

/// Counts the clusters the bitmap marks in use. Bit `n` stands for cluster
/// `n + 2`; bits past the cluster count are ignored.
fn count_used<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
    bitmap: Bitmap,
) -> Result<u32, MetadataError> {
    if bitmap.length < u64::from(boot.cluster_count).div_ceil(8) {
        return Err(MetadataError::Inconsistent(
            "the allocation bitmap is shorter than the cluster count needs",
        ));
    }
    let cluster_len = usize::try_from(boot.cluster_bytes()).expect("at most 32 MiB");
    let mut remaining_bits = u64::from(boot.cluster_count);
    let mut used = 0u64;
    let mut chain = Chain::new(bitmap.first_cluster);
    while remaining_bits > 0 {
        let Some(cluster) = chain.advance(volume, boot)? else {
            break;
        };
        let bytes = volume.read(boot.cluster_offset(cluster), cluster_len)?;
        for &byte in &bytes {
            if remaining_bits == 0 {
                break;
            }
            let bits = remaining_bits.min(8);
            let mask = u8::try_from((1u16 << bits) - 1).expect("bits is at most 8");
            used += u64::from((byte & mask).count_ones());
            remaining_bits -= bits;
        }
    }
    Ok(u32::try_from(used).expect("used is at most the cluster count"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::fs::tests::{extent, read_volume, sector_of};
    use crate::test_support::{fixture_image, reader};

    fn exfat(image: &str, start: u64, sectors: u64) -> Exfat {
        exfat_of(read_volume(image, start, sectors))
    }

    fn exfat_of(field: Field<Filesystem>) -> Exfat {
        match field {
            Field::Known {
                value: Filesystem::Exfat(e),
                source: Source::BootSector,
            } => e,
            other => panic!("{other:?}"),
        }
    }

    fn known<T: Clone + std::fmt::Debug>(field: &Field<T>) -> T {
        match field {
            Field::Known { value, .. } => value.clone(),
            Field::Unavailable(reason) => panic!("unavailable: {reason}"),
        }
    }

    // Free space comes from diskutil info with the volumes mounted
    // read-only; see the phase 2 oracle notes. diskutil's total is one
    // cluster more than ClusterCount, which the specification defines as the
    // number of clusters in the heap; the sizes here follow the
    // specification.

    #[test]
    fn exfat_with_files() {
        let e = exfat("gpt-exfat", 2048, 126_976);
        assert_eq!(known(&e.label).as_deref(), Some("CARD"));
        assert_eq!(e.revision.to_string(), "1.00");
        assert_eq!(e.cluster_size_bytes, 4096);
        assert_eq!(e.cluster_count, 15840);
        assert_eq!(e.size_bytes, 15840 * 4096);
        assert_eq!(known(&e.free_bytes), 61_661_184);
        assert_eq!(e.fat_count, 1);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn exfat_from_disk_utility_mbr() {
        let e = exfat("mbr-extended", 109_557, 31256);
        assert_eq!(known(&e.label).as_deref(), Some("THREE"));
        assert_eq!(e.size_bytes, 3875 * 4096);
        assert_eq!(known(&e.free_bytes), 15_839_232);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn exfat_second_gpt_partition() {
        let e = exfat("gpt-multi", 79872, 92160);
        assert_eq!(known(&e.label).as_deref(), Some("DATA"));
        assert_eq!(known(&e.free_bytes), 47_022_080);
    }

    #[test]
    fn checksum_and_flags_are_checked() {
        let mut image = fixture_image("gpt-exfat");
        let base = sector_of(2048);
        // A changed serial breaks the checksum; the flags are outside it.
        image[base + boot::SERIAL] ^= 0xFF;
        image[base + boot::VOLUME_FLAGS] |= u8::try_from(VOLUME_DIRTY | MEDIA_FAILURE).unwrap();
        let e = exfat_of(crate::disk::fs::read(
            &mut reader(image),
            extent(2048, 126_976),
        ));
        assert_eq!(
            e.issues,
            [
                FsIssue::BootChecksumMismatch,
                FsIssue::NotCleanlyUnmounted,
                FsIssue::ErrorsRecorded
            ]
        );
    }

    #[test]
    fn flags_alone_keep_the_checksum() {
        let mut image = fixture_image("gpt-exfat");
        image[sector_of(2048) + boot::PERCENT_IN_USE] = 42;
        let e = exfat_of(crate::disk::fs::read(
            &mut reader(image),
            extent(2048, 126_976),
        ));
        assert_eq!(e.percent_in_use, Some(42));
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn rejects_bad_shifts() {
        let mut sector = fixture_image("gpt-exfat")[sector_of(2048)..sector_of(2049)].to_vec();
        assert!(BootSector::parse(&sector).is_some());
        sector[boot::BYTES_PER_SECTOR_SHIFT] = 13;
        assert_eq!(BootSector::parse(&sector), None);
    }
}
