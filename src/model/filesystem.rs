//! What a volume holds: the filesystem and the details read from its
//! metadata.

use std::fmt;

use serde::Serialize;
use serde::ser::Serializer;

use super::field::Field;
use super::guid::Guid;

/// The filesystem found at the start of a volume, tagged by `type`.
///
/// Fields every profiled filesystem has, under the same keys: `label`,
/// `size_bytes`, `free_bytes` and `issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filesystem {
    Fat12(Fat),
    Fat16(Fat),
    Fat32(Fat),
    Exfat(Exfat),
    Ext2(Ext),
    Ext3(Ext),
    Ext4(Ext),
    /// An NTFS boot sector. Recognized, so it isn't mistaken for exFAT (MBR
    /// type `0x07` covers both), but not profiled.
    Ntfs,
    /// The first sectors of the volume are all zeros.
    Blank,
    /// No filesystem this tool knows.
    Unrecognized,
}

impl Filesystem {
    /// Name as `blkid` and `mount` spell it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Fat12(_) => "FAT12",
            Self::Fat16(_) => "FAT16",
            Self::Fat32(_) => "FAT32",
            Self::Exfat(_) => "exFAT",
            Self::Ext2(_) => "ext2",
            Self::Ext3(_) => "ext3",
            Self::Ext4(_) => "ext4",
            Self::Ntfs => "NTFS",
            Self::Blank => "blank",
            Self::Unrecognized => "unrecognized",
        }
    }

    /// Problems found in the metadata; none for filesystems not profiled.
    #[must_use]
    pub fn issues(&self) -> &[FsIssue] {
        match self {
            Self::Fat12(fat) | Self::Fat16(fat) | Self::Fat32(fat) => &fat.issues,
            Self::Exfat(exfat) => &exfat.issues,
            Self::Ext2(ext) | Self::Ext3(ext) | Self::Ext4(ext) => &ext.issues,
            Self::Ntfs | Self::Blank | Self::Unrecognized => &[],
        }
    }

    /// Drops [`FsIssue::NotCleanlyUnmounted`], for a filesystem that is
    /// mounted read-write: it keeps the flag set while mounted.
    pub fn forget_mounted_dirty_flag(&mut self) {
        let issues = match self {
            Self::Fat12(fat) | Self::Fat16(fat) | Self::Fat32(fat) => &mut fat.issues,
            Self::Exfat(exfat) => &mut exfat.issues,
            Self::Ext2(ext) | Self::Ext3(ext) | Self::Ext4(ext) => &mut ext.issues,
            Self::Ntfs | Self::Blank | Self::Unrecognized => return,
        };
        issues.retain(|issue| *issue != FsIssue::NotCleanlyUnmounted);
    }

    /// Offset of the data area from the volume start: the first cluster on
    /// FAT and exFAT. ext block groups, and anything not profiled, start
    /// with the volume.
    #[must_use]
    pub fn data_offset_bytes(&self) -> u64 {
        match self {
            Self::Fat12(fat) | Self::Fat16(fat) | Self::Fat32(fat) => fat.data_offset_bytes,
            Self::Exfat(exfat) => exfat.data_offset_bytes,
            _ => 0,
        }
    }
}

/// A FAT12, FAT16 or FAT32 volume. The variant of [`Filesystem`] says which:
/// the FAT type follows from the cluster count alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fat {
    /// Volume label: the root directory's label entry, which is what Windows,
    /// macOS and Linux show, or the boot sector's copy when the root
    /// directory has none. `null` when neither has one.
    pub label: Field<Option<String>>,
    /// Volume serial number, `null` when the boot sector has no extended
    /// boot signature.
    pub volume_id: Option<VolumeSerial>,
    /// Name of the formatter that wrote the boot sector (`MSDOS5.0`,
    /// `BSD  4.4`, `mkfs.fat`).
    pub oem_name: String,
    pub sector_size_bytes: u32,
    pub cluster_size_bytes: u32,
    pub cluster_count: u32,
    /// Size of the data area: clusters times cluster size.
    pub size_bytes: u64,
    /// Free clusters counted in the allocation table, in bytes.
    pub free_bytes: Field<u64>,
    /// Offset of the first cluster from the start of the volume.
    pub data_offset_bytes: u64,
    pub reserved_sectors: u16,
    pub fat_count: u8,
    /// Sectors only the FAT32 layout has; `null` on FAT12 and FAT16.
    pub fat32: Option<Fat32Sectors>,
    pub issues: Vec<FsIssue>,
}

/// Where FAT32 keeps its extra structures, in sectors from the volume start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Fat32Sectors {
    /// The `FSInfo` sector, which holds the free cluster hint.
    pub fsinfo_sector: u16,
    pub backup_boot_sector: u16,
}

/// An exFAT volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Exfat {
    /// From the root directory's volume label entry; `null` when it has none.
    pub label: Field<Option<String>>,
    pub volume_id: VolumeSerial,
    pub revision: Revision,
    pub sector_size_bytes: u32,
    pub cluster_size_bytes: u32,
    pub cluster_count: u32,
    /// Size of the cluster heap: clusters times cluster size.
    pub size_bytes: u64,
    /// Free clusters counted in the allocation bitmap, in bytes.
    pub free_bytes: Field<u64>,
    /// Offset of the cluster heap from the start of the volume.
    pub data_offset_bytes: u64,
    /// 2 on `TexFAT` volumes, 1 otherwise.
    pub fat_count: u8,
    /// The percentage of clusters in use the boot sector records; `null` when
    /// the formatter left it unset (`0xFF`).
    pub percent_in_use: Option<u8>,
    pub issues: Vec<FsIssue>,
}

/// An ext2, ext3 or ext4 volume. The variant of [`Filesystem`] says which,
/// decided by the feature flags the way `blkid` decides it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ext {
    /// From the superblock; `null` when empty.
    pub label: Field<Option<String>>,
    /// `null` on revision 0 volumes, which predate UUIDs.
    pub uuid: Option<Uuid>,
    pub block_size_bytes: u32,
    pub block_count: u64,
    /// Blocks times block size.
    pub size_bytes: u64,
    /// Free blocks summed over the block group descriptors, in bytes.
    pub free_bytes: Field<u64>,
    /// Blocks reserved for root, in bytes.
    pub reserved_bytes: u64,
    pub inode_count: u32,
    pub free_inodes: u32,
    /// The directory it was last mounted on, `null` when never recorded.
    pub last_mounted: Option<String>,
    pub revision: u32,
    pub features: ExtFeatures,
    pub issues: Vec<FsIssue>,
}

/// A 32-bit FAT or exFAT volume serial number. Serialized the way Windows and
/// `blkid` show it: `"1A2B-3C4D"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeSerial(pub u32);

impl fmt::Display for VolumeSerial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}-{:04X}", self.0 >> 16, self.0 & 0xFFFF)
    }
}

impl Serialize for VolumeSerial {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// An exFAT revision, `major.minor`. Serialized as `"1.00"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    pub major: u8,
    pub minor: u8,
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.major, self.minor)
    }
}

impl Serialize for Revision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// An RFC 4122 UUID as ext stores it. Serialized in lowercase, as `blkid`,
/// `/dev/disk/by-uuid` and `fstab` show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uuid(pub Guid);

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_string().to_ascii_lowercase())
    }
}

impl Serialize for Uuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The three ext feature words. Serialized as one list of names in the order
/// and spelling `dumpe2fs` and `tune2fs -l` print: compatible, incompatible,
/// then read-only compatible features. Bits without a name come out as
/// e2fsprogs writes them, `FEATURE_C12`, `FEATURE_I31` or `FEATURE_R16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtFeatures {
    pub compat: u32,
    pub incompat: u32,
    pub ro_compat: u32,
}

/// Feature names from e2fsprogs `lib/e2p/feature.c`, by bit. Bits defined
/// there without a name (`btree_dir`, `has_snapshot`) print as unknown.
const COMPAT_NAMES: &[(u32, &str)] = &[
    (0x0001, "dir_prealloc"),
    (0x0002, "imagic_inodes"),
    (0x0004, "has_journal"),
    (0x0008, "ext_attr"),
    (0x0010, "resize_inode"),
    (0x0020, "dir_index"),
    (0x0040, "lazy_bg"),
    (0x0100, "snapshot_bitmap"),
    (0x0200, "sparse_super2"),
    (0x0400, "fast_commit"),
    (0x0800, "stable_inodes"),
    (0x1000, "orphan_file"),
];

const INCOMPAT_NAMES: &[(u32, &str)] = &[
    (0x0001, "compression"),
    (0x0002, "filetype"),
    (0x0004, "needs_recovery"),
    (0x0008, "journal_dev"),
    (0x0010, "meta_bg"),
    (0x0040, "extent"),
    (0x0080, "64bit"),
    (0x0100, "mmp"),
    (0x0200, "flex_bg"),
    (0x0400, "ea_inode"),
    (0x1000, "dirdata"),
    (0x2000, "metadata_csum_seed"),
    (0x4000, "large_dir"),
    (0x8000, "inline_data"),
    (0x1_0000, "encrypt"),
    (0x2_0000, "casefold"),
];

const RO_COMPAT_NAMES: &[(u32, &str)] = &[
    (0x0001, "sparse_super"),
    (0x0002, "large_file"),
    (0x0008, "huge_file"),
    (0x0010, "uninit_bg"),
    (0x0020, "dir_nlink"),
    (0x0040, "extra_isize"),
    (0x0100, "quota"),
    (0x0200, "bigalloc"),
    (0x0400, "metadata_csum"),
    (0x0800, "replica"),
    (0x1000, "read-only"),
    (0x2000, "project"),
    (0x4000, "shared_blocks"),
    (0x8000, "verity"),
    (0x1_0000, "orphan_present"),
];

impl ExtFeatures {
    /// Feature names, as described on the type.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for (word, table, prefix) in [
            (self.compat, COMPAT_NAMES, 'C'),
            (self.incompat, INCOMPAT_NAMES, 'I'),
            (self.ro_compat, RO_COMPAT_NAMES, 'R'),
        ] {
            for bit in (0..32).filter(|bit| word & (1 << bit) != 0) {
                let mask = 1u32 << bit;
                names.push(match table.iter().find(|(m, _)| *m == mask) {
                    Some((_, name)) => (*name).to_owned(),
                    None => format!("FEATURE_{prefix}{bit}"),
                });
            }
        }
        names
    }
}

impl Serialize for ExtFeatures {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.names())
    }
}

/// A problem found in a filesystem's metadata. Serialized tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsIssue {
    /// The filesystem was not cleanly unmounted: the card was pulled, or the
    /// system lost power, while it was mounted.
    NotCleanlyUnmounted,
    /// The filesystem recorded I/O or consistency errors.
    ErrorsRecorded,
    /// The free count kept as a hint (FAT32 `FSInfo`, the ext superblock)
    /// disagrees with the count from the allocation structures.
    FreeCountStale {
        recorded_bytes: u64,
        counted_bytes: u64,
    },
    /// The exFAT boot region checksum does not match.
    BootChecksumMismatch,
    /// The ext superblock checksum (`metadata_csum`) does not match.
    SuperblockChecksumMismatch,
    /// The filesystem says it is larger than the partition that holds it.
    LargerThanPartition {
        filesystem_bytes: u64,
        partition_bytes: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_serial_is_two_hex_groups() {
        assert_eq!(VolumeSerial(0x001E_19ED).to_string(), "001E-19ED");
        assert_eq!(
            serde_json::to_string(&VolumeSerial(0xF26A_19EE)).unwrap(),
            r#""F26A-19EE""#
        );
    }

    #[test]
    fn revision_has_two_minor_digits() {
        assert_eq!(Revision { major: 1, minor: 0 }.to_string(), "1.00");
    }

    #[test]
    fn uuid_is_lowercase() {
        let uuid = Uuid(Guid::parse_const("7A8913EA-FF85-4A3F-83BE-F57EB5E34921"));
        assert_eq!(uuid.to_string(), "7a8913ea-ff85-4a3f-83be-f57eb5e34921");
    }

    #[test]
    fn ext_feature_names_follow_dumpe2fs() {
        // The ext4 fixtures, as `dumpe2fs -h` lists them.
        let features = ExtFeatures {
            compat: 0x0004 | 0x0008 | 0x0010 | 0x0020 | 0x1000,
            incompat: 0x0002 | 0x0040 | 0x0080 | 0x0200 | 0x2000,
            ro_compat: 0x0001 | 0x0002 | 0x0008 | 0x0020 | 0x0040 | 0x0400,
        };
        assert_eq!(
            features.names().join(" "),
            "has_journal ext_attr resize_inode dir_index orphan_file filetype extent 64bit \
             flex_bg metadata_csum_seed sparse_super large_file huge_file dir_nlink \
             extra_isize metadata_csum"
        );
    }

    #[test]
    fn unknown_ext_features_use_e2fsprogs_names() {
        let features = ExtFeatures {
            compat: 1 << 12 | 1 << 13,
            incompat: 1 << 31,
            ro_compat: 1 << 20,
        };
        assert_eq!(
            features.names(),
            ["orphan_file", "FEATURE_C13", "FEATURE_I31", "FEATURE_R20"]
        );
    }

    #[test]
    fn unit_variants_serialize_as_type_only() {
        assert_eq!(
            serde_json::to_string(&Filesystem::Blank).unwrap(),
            r#"{"type":"blank"}"#
        );
    }
}
