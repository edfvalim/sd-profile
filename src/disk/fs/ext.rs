//! ext2, ext3 and ext4.
//!
//! The superblock sits 1024 bytes into the volume, whatever the block size.
//! The block group descriptor table follows in the next block and records
//! the free blocks of each group. Sources: Linux `fs/ext4/ext4.h`, e2fsprogs
//! `lib/ext2fs/ext2_fs.h`, util-linux `libblkid/src/superblocks/ext.c`.

use super::{MetadataError, Volume, field};
use crate::disk::block::BlockReader;
use crate::disk::{crc32, le_u16, le_u32};
use crate::model::{Ext, ExtFeatures, Field, Filesystem, FsIssue, Guid, Source, Uuid};

pub const SUPERBLOCK_OFFSET: usize = 1024;
pub const SUPERBLOCK_LEN: usize = 1024;

/// Offsets within the superblock.
mod sb {
    use std::ops::Range;

    pub const INODES_COUNT: usize = 0x00;
    pub const BLOCKS_COUNT_LO: usize = 0x04;
    pub const R_BLOCKS_COUNT_LO: usize = 0x08;
    pub const FREE_BLOCKS_COUNT_LO: usize = 0x0C;
    pub const FREE_INODES_COUNT: usize = 0x10;
    pub const FIRST_DATA_BLOCK: usize = 0x14;
    pub const LOG_BLOCK_SIZE: usize = 0x18;
    pub const BLOCKS_PER_GROUP: usize = 0x20;
    pub const INODES_PER_GROUP: usize = 0x28;
    pub const MAGIC: usize = 0x38;
    pub const STATE: usize = 0x3A;
    pub const REV_LEVEL: usize = 0x4C;
    pub const FEATURE_COMPAT: usize = 0x5C;
    pub const FEATURE_INCOMPAT: usize = 0x60;
    pub const FEATURE_RO_COMPAT: usize = 0x64;
    pub const UUID: Range<usize> = 0x68..0x78;
    pub const VOLUME_NAME: Range<usize> = 0x78..0x88;
    pub const LAST_MOUNTED: Range<usize> = 0x88..0xC8;
    pub const DESC_SIZE: usize = 0xFE;
    pub const BLOCKS_COUNT_HI: usize = 0x150;
    pub const R_BLOCKS_COUNT_HI: usize = 0x154;
    pub const FREE_BLOCKS_COUNT_HI: usize = 0x158;
    pub const CHECKSUM: usize = 0x3FC;
}

const MAGIC: u16 = 0xEF53;

/// The first revision with the fields from `s_first_ino` on: features,
/// UUID, label and last mount point.
const DYNAMIC_REV: u32 = 1;

/// `s_state` bits.
const STATE_VALID: u16 = 0x0001;
const STATE_ERROR: u16 = 0x0002;

/// Feature bits this module acts on.
const COMPAT_HAS_JOURNAL: u32 = 0x0004;
const INCOMPAT_FILETYPE: u32 = 0x0002;
const INCOMPAT_RECOVER: u32 = 0x0004;
const INCOMPAT_JOURNAL_DEV: u32 = 0x0008;
const INCOMPAT_META_BG: u32 = 0x0010;
const INCOMPAT_64BIT: u32 = 0x0080;
const RO_COMPAT_SPARSE_SUPER: u32 = 0x0001;
const RO_COMPAT_LARGE_FILE: u32 = 0x0002;
const RO_COMPAT_BTREE_DIR: u32 = 0x0004;
const RO_COMPAT_METADATA_CSUM: u32 = 0x0400;

/// Features ext3 supports, as `blkid` defines them: a volume with any other
/// incompatible or read-only compatible feature is ext4.
const EXT3_INCOMPAT: u32 = INCOMPAT_FILETYPE | INCOMPAT_RECOVER | INCOMPAT_META_BG;
const EXT3_RO_COMPAT: u32 = RO_COMPAT_SPARSE_SUPER | RO_COMPAT_LARGE_FILE | RO_COMPAT_BTREE_DIR;

/// Group descriptor fields.
mod gd {
    pub const MIN_LEN: usize = 32;
    pub const LEN_64BIT: usize = 64;
    pub const FREE_BLOCKS_COUNT_LO: usize = 0x0C;
    pub const FREE_BLOCKS_COUNT_HI: usize = 0x2C;
}

/// Upper bound on the descriptor table read, far above any SD card's
/// (2 TiB with 4 KiB blocks is 16384 groups, 1 MiB of descriptors).
const MAX_DESCRIPTOR_TABLE: u64 = 64 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superblock {
    bytes: Vec<u8>,
    features: ExtFeatures,
    block_size: u32,
    block_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtType {
    Ext2,
    Ext3,
    Ext4,
}

impl Superblock {
    /// Parses the superblock at the start of `bytes`, which holds at least
    /// [`SUPERBLOCK_LEN`] bytes; `None` without the magic number or with
    /// geometry no ext volume has.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let bytes = &bytes[..SUPERBLOCK_LEN];
        if le_u16(bytes, sb::MAGIC) != MAGIC {
            return None;
        }
        // Revision 0 superblocks end before the feature words.
        let features = if le_u32(bytes, sb::REV_LEVEL) >= DYNAMIC_REV {
            ExtFeatures {
                compat: le_u32(bytes, sb::FEATURE_COMPAT),
                incompat: le_u32(bytes, sb::FEATURE_INCOMPAT),
                ro_compat: le_u32(bytes, sb::FEATURE_RO_COMPAT),
            }
        } else {
            ExtFeatures {
                compat: 0,
                incompat: 0,
                ro_compat: 0,
            }
        };
        let log_block_size = le_u32(bytes, sb::LOG_BLOCK_SIZE);
        let wide = features.incompat & INCOMPAT_64BIT != 0;
        let block_count = split_u64(bytes, sb::BLOCKS_COUNT_LO, sb::BLOCKS_COUNT_HI, wide);
        let valid = log_block_size <= 6
            && le_u32(bytes, sb::BLOCKS_PER_GROUP) > 0
            && le_u32(bytes, sb::INODES_PER_GROUP) > 0
            && block_count > u64::from(le_u32(bytes, sb::FIRST_DATA_BLOCK));
        valid.then(|| Self {
            bytes: bytes.to_vec(),
            features,
            block_size: 1024 << log_block_size,
            block_count,
        })
    }

    fn is_dynamic(&self) -> bool {
        self.u32_at(sb::REV_LEVEL) >= DYNAMIC_REV
    }

    /// A string field of a dynamic-revision superblock.
    fn dynamic_string(&self, range: std::ops::Range<usize>) -> Option<String> {
        self.is_dynamic()
            .then(|| c_string(&self.bytes[range]))
            .flatten()
    }

    fn u32_at(&self, at: usize) -> u32 {
        le_u32(&self.bytes, at)
    }

    fn is_64bit(&self) -> bool {
        self.features.incompat & INCOMPAT_64BIT != 0
    }

    fn wide(&self, lo: usize, hi: usize) -> u64 {
        split_u64(&self.bytes, lo, hi, self.is_64bit())
    }

    /// ext2, ext3 or ext4, decided as `blkid` decides; `None` for an
    /// external journal device, which holds no files.
    fn ext_type(&self) -> Option<ExtType> {
        let f = self.features;
        if f.incompat & INCOMPAT_JOURNAL_DEV != 0 {
            None
        } else if f.incompat & !EXT3_INCOMPAT != 0 || f.ro_compat & !EXT3_RO_COMPAT != 0 {
            Some(ExtType::Ext4)
        } else if f.compat & COMPAT_HAS_JOURNAL != 0 {
            Some(ExtType::Ext3)
        } else {
            Some(ExtType::Ext2)
        }
    }

    /// Whether the stored checksum matches, `None` without `metadata_csum`.
    /// The kernel computes CRC-32C seeded with all ones and stores it without
    /// the final inversion.
    fn checksum_matches(&self) -> Option<bool> {
        (self.features.ro_compat & RO_COMPAT_METADATA_CSUM != 0)
            .then(|| !crc32::castagnoli(&self.bytes[..sb::CHECKSUM]) == self.u32_at(sb::CHECKSUM))
    }
}

/// A 64-bit count split in two 32-bit fields; the high half counts only on
/// 64bit volumes.
fn split_u64(bytes: &[u8], lo: usize, hi: usize, wide: bool) -> u64 {
    let high = if wide {
        u64::from(le_u32(bytes, hi))
    } else {
        0
    };
    high << 32 | u64::from(le_u32(bytes, lo))
}

/// Reads the volume the superblock describes.
pub(super) fn read<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    superblock: &Superblock,
) -> Filesystem {
    let Some(ext_type) = superblock.ext_type() else {
        return Filesystem::Unrecognized;
    };
    let block_size = u64::from(superblock.block_size);
    let mut issues = Vec::new();

    let state = le_u16(&superblock.bytes, sb::STATE);
    if state & STATE_VALID == 0 || superblock.features.incompat & INCOMPAT_RECOVER != 0 {
        issues.push(FsIssue::NotCleanlyUnmounted);
    }
    if state & STATE_ERROR != 0 {
        issues.push(FsIssue::ErrorsRecorded);
    }
    if superblock.checksum_matches() == Some(false) {
        issues.push(FsIssue::SuperblockChecksumMismatch);
    }

    let recorded_free = superblock.wide(sb::FREE_BLOCKS_COUNT_LO, sb::FREE_BLOCKS_COUNT_HI);
    let free_bytes = if superblock.features.incompat & INCOMPAT_META_BG == 0 {
        let counted = free_blocks_in_groups(volume, superblock);
        if let Ok(counted) = counted
            && counted != recorded_free
        {
            issues.push(FsIssue::FreeCountStale {
                recorded_bytes: recorded_free * block_size,
                counted_bytes: counted * block_size,
            });
        }
        field(counted.map(|n| n * block_size), Source::GroupDescriptors)
    } else {
        // With meta_bg the descriptors are spread over the volume; the
        // superblock's count is the one value at hand.
        Field::known(recorded_free * block_size, Source::Superblock)
    };

    let filesystem_bytes = superblock.block_count * block_size;
    issues.extend(volume.size_issue(filesystem_bytes));

    let ext = Ext {
        label: Field::known(
            superblock.dynamic_string(sb::VOLUME_NAME),
            Source::Superblock,
        ),
        uuid: superblock.is_dynamic().then(|| {
            Uuid(Guid::from_bytes(
                superblock.bytes[sb::UUID]
                    .try_into()
                    .expect("the UUID range is 16 bytes"),
            ))
        }),
        block_size_bytes: superblock.block_size,
        block_count: superblock.block_count,
        size_bytes: filesystem_bytes,
        free_bytes,
        reserved_bytes: superblock.wide(sb::R_BLOCKS_COUNT_LO, sb::R_BLOCKS_COUNT_HI) * block_size,
        inode_count: superblock.u32_at(sb::INODES_COUNT),
        free_inodes: superblock.u32_at(sb::FREE_INODES_COUNT),
        last_mounted: superblock.dynamic_string(sb::LAST_MOUNTED),
        revision: superblock.u32_at(sb::REV_LEVEL),
        features: superblock.features,
        issues,
    };
    match ext_type {
        ExtType::Ext2 => Filesystem::Ext2(ext),
        ExtType::Ext3 => Filesystem::Ext3(ext),
        ExtType::Ext4 => Filesystem::Ext4(ext),
    }
}

/// Sums the free block counts of all group descriptors. The kernel keeps
/// these current; the superblock's total is refreshed less often.
fn free_blocks_in_groups<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    superblock: &Superblock,
) -> Result<u64, MetadataError> {
    let first_data_block = u64::from(superblock.u32_at(sb::FIRST_DATA_BLOCK));
    let per_group = u64::from(superblock.u32_at(sb::BLOCKS_PER_GROUP));
    let groups = (superblock.block_count - first_data_block).div_ceil(per_group);
    let desc_len = if superblock.is_64bit() {
        usize::from(le_u16(&superblock.bytes, sb::DESC_SIZE)).max(gd::MIN_LEN)
    } else {
        gd::MIN_LEN
    };
    let table_len = groups * desc_len as u64;
    if table_len > MAX_DESCRIPTOR_TABLE {
        return Err(MetadataError::Inconsistent(
            "the block group count implies a descriptor table over 64 MiB",
        ));
    }
    let offset = (first_data_block + 1) * u64::from(superblock.block_size);
    let table = volume.read(offset, usize::try_from(table_len).expect("bounded above"))?;
    Ok(table
        .chunks_exact(desc_len)
        .map(|d| {
            let hi = if desc_len >= gd::LEN_64BIT {
                u64::from(le_u16(d, gd::FREE_BLOCKS_COUNT_HI))
            } else {
                0
            };
            hi << 16 | u64::from(le_u16(d, gd::FREE_BLOCKS_COUNT_LO))
        })
        .sum())
}

/// A NUL-padded string field; `None` when empty.
fn c_string(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]);
    (!text.is_empty()).then(|| text.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::fs::tests::{extent, read_volume, sector_of};
    use crate::test_support::{fixture_image, reader};

    fn ext(field: Field<Filesystem>) -> (&'static str, Ext) {
        match field {
            Field::Known {
                value,
                source: Source::Superblock,
            } => {
                let name = value.name();
                match value {
                    Filesystem::Ext2(e) | Filesystem::Ext3(e) | Filesystem::Ext4(e) => (name, e),
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    fn free(e: &Ext) -> (u64, Source) {
        match &e.free_bytes {
            Field::Known { value, source } => (*value, *source),
            Field::Unavailable(reason) => panic!("unavailable: {reason}"),
        }
    }

    fn label(e: &Ext) -> Option<String> {
        match &e.label {
            Field::Known { value, .. } => value.clone(),
            Field::Unavailable(reason) => panic!("unavailable: {reason}"),
        }
    }

    // Expected values come from dumpe2fs -h on the fixtures; see the phase 2
    // oracle notes.

    #[test]
    fn ext2() {
        let (kind, e) = ext(read_volume("mbr-ext", 2048, 40960));
        assert_eq!(kind, "ext2");
        assert_eq!(label(&e).as_deref(), Some("two"));
        assert_eq!(
            e.uuid.map(|u| u.to_string()).as_deref(),
            Some("7a8913ea-ff85-4a3f-83be-f57eb5e34921")
        );
        assert_eq!(e.block_size_bytes, 1024);
        assert_eq!(e.block_count, 20480);
        assert_eq!(free(&e), (15930 * 1024, Source::GroupDescriptors));
        assert_eq!(e.reserved_bytes, 1024 * 1024);
        assert_eq!((e.inode_count, e.free_inodes), (5112, 5096));
        assert_eq!(
            e.features.names().join(" "),
            "ext_attr resize_inode dir_index filetype sparse_super large_file"
        );
        assert_eq!(e.last_mounted, None);
        assert_eq!(e.revision, 1);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn ext3() {
        let (kind, e) = ext(read_volume("mbr-ext", 43008, 40960));
        assert_eq!(kind, "ext3");
        assert_eq!(label(&e).as_deref(), Some("three"));
        assert_eq!(free(&e).0, 14901 * 1024);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn ext4_with_checksums() {
        let (kind, e) = ext(read_volume("mbr-ext", 83968, 40960));
        assert_eq!(kind, "ext4");
        assert_eq!(label(&e).as_deref(), Some("four"));
        assert_eq!(free(&e).0, 14727 * 1024);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn ext4_with_4k_blocks() {
        let (kind, e) = ext(read_volume("mbr-sd-aligned", 139_264, 122_880));
        assert_eq!(kind, "ext4");
        assert_eq!(label(&e).as_deref(), Some("rootfs"));
        assert_eq!(
            e.uuid.map(|u| u.to_string()).as_deref(),
            Some("d97db754-2e18-42d0-95c1-c9dd9ae14f64")
        );
        assert_eq!(e.block_size_bytes, 4096);
        assert_eq!(free(&e).0, 12555 * 4096);
        assert_eq!(e.reserved_bytes, 768 * 4096);
        assert!(e.issues.is_empty(), "{:?}", e.issues);
    }

    #[test]
    fn ext4_on_whole_disk() {
        let (_, e) = ext(read_volume("superfloppy-ext4", 0, 16384));
        assert_eq!(label(&e).as_deref(), Some("FLOPPY"));
        assert_eq!(free(&e).0, 3466 * 1024);
    }

    /// The ext4 partition of `mbr-ext` with its superblock changed by `edit`.
    fn edited_ext4(edit: impl FnOnce(&mut [u8])) -> Ext {
        let mut image = fixture_image("mbr-ext");
        let at = sector_of(83968) + SUPERBLOCK_OFFSET;
        edit(&mut image[at..at + SUPERBLOCK_LEN]);
        ext(crate::disk::fs::read(
            &mut reader(image),
            extent(83968, 40960),
        ))
        .1
    }

    #[test]
    fn edited_superblock_fails_its_checksum() {
        let e = edited_ext4(|s| s[sb::VOLUME_NAME.start] = b'X');
        assert_eq!(e.issues, [FsIssue::SuperblockChecksumMismatch]);
    }

    #[test]
    fn state_and_stale_count_are_reported() {
        let e = edited_ext4(|s| {
            s[sb::STATE] = 0; // not clean
            s[sb::STATE] |= u8::try_from(STATE_ERROR).unwrap();
            s[sb::FREE_BLOCKS_COUNT_LO..sb::FREE_BLOCKS_COUNT_LO + 4]
                .copy_from_slice(&7u32.to_le_bytes());
            let sum = !crc32::castagnoli(&s[..sb::CHECKSUM]);
            s[sb::CHECKSUM..sb::CHECKSUM + 4].copy_from_slice(&sum.to_le_bytes());
        });
        assert_eq!(
            e.issues,
            [
                FsIssue::NotCleanlyUnmounted,
                FsIssue::ErrorsRecorded,
                FsIssue::FreeCountStale {
                    recorded_bytes: 7 * 1024,
                    counted_bytes: 14727 * 1024
                }
            ]
        );
    }

    #[test]
    fn classification_follows_blkid() {
        let with = |compat, incompat, ro_compat| {
            let mut bytes = fixture_image("mbr-ext")[sector_of(2048) + SUPERBLOCK_OFFSET..]
                [..SUPERBLOCK_LEN]
                .to_vec();
            bytes[sb::FEATURE_COMPAT..sb::FEATURE_COMPAT + 4]
                .copy_from_slice(&u32::to_le_bytes(compat));
            bytes[sb::FEATURE_INCOMPAT..sb::FEATURE_INCOMPAT + 4]
                .copy_from_slice(&u32::to_le_bytes(incompat));
            bytes[sb::FEATURE_RO_COMPAT..sb::FEATURE_RO_COMPAT + 4]
                .copy_from_slice(&u32::to_le_bytes(ro_compat));
            Superblock::parse(&bytes).unwrap().ext_type()
        };
        assert_eq!(
            with(0, INCOMPAT_FILETYPE, RO_COMPAT_SPARSE_SUPER),
            Some(ExtType::Ext2)
        );
        assert_eq!(
            with(COMPAT_HAS_JOURNAL, INCOMPAT_RECOVER, 0),
            Some(ExtType::Ext3)
        );
        // Extents without a journal are still ext4.
        assert_eq!(with(0, 0x0040, 0), Some(ExtType::Ext4));
        assert_eq!(
            with(COMPAT_HAS_JOURNAL, 0, RO_COMPAT_METADATA_CSUM),
            Some(ExtType::Ext4)
        );
        assert_eq!(with(0, INCOMPAT_JOURNAL_DEV, 0), None);
    }

    #[test]
    fn revision_0_has_no_dynamic_fields() {
        // ext2 has no metadata_csum, so the edit leaves no checksum issue.
        let mut image = fixture_image("mbr-ext");
        let at = sector_of(2048) + SUPERBLOCK_OFFSET;
        image[at + sb::REV_LEVEL..at + sb::REV_LEVEL + 4].copy_from_slice(&0u32.to_le_bytes());
        let (kind, e) = ext(crate::disk::fs::read(
            &mut reader(image),
            extent(2048, 40960),
        ));
        assert_eq!(kind, "ext2");
        assert_eq!(e.uuid, None);
        assert_eq!(label(&e), None);
        assert!(e.features.names().is_empty());
    }

    #[test]
    fn no_magic_is_none() {
        assert_eq!(Superblock::parse(&[0; SUPERBLOCK_LEN]), None);
    }
}
