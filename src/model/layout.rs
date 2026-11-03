//! The disk's layout: size, sector size, partition table and volumes.

use std::fmt;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use super::field::{Field, Unavailable};
use super::guid::Guid;
use super::partition_type::{GptType, MbrType};
use super::volume::Volume;

/// Logical block (sector) size in bytes: a power of two, at least 512.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BlockSize(u32);

impl BlockSize {
    /// 512 bytes, the sector size the SD specification fixes for every card.
    pub const SD: Self = Self(512);

    #[must_use]
    pub fn new(bytes: u32) -> Option<Self> {
        (bytes >= 512 && bytes.is_power_of_two()).then_some(Self(bytes))
    }

    #[must_use]
    pub fn bytes(self) -> u32 {
        self.0
    }
}

impl BlockSize {
    /// Byte offset of the start of block `lba`.
    #[must_use]
    pub fn offset_of(self, lba: Lba) -> u64 {
        lba.0.saturating_mul(u64::from(self.0))
    }
}

/// A logical block address: a sector number counted from the start of the
/// disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Lba(pub u64);

/// A run of sectors on the disk.
///
/// Serialized as `start_lba`, `sectors`, `offset_bytes` and `size_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    start: Lba,
    sectors: u64,
    block_size: BlockSize,
}

impl Extent {
    #[must_use]
    pub fn new(start: Lba, sectors: u64, block_size: BlockSize) -> Self {
        Self {
            start,
            sectors,
            block_size,
        }
    }

    #[must_use]
    pub fn start(&self) -> Lba {
        self.start
    }

    #[must_use]
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// The first sector after the extent.
    #[must_use]
    pub fn end(&self) -> Lba {
        Lba(self.start.0.saturating_add(self.sectors))
    }

    /// The extent's last sector; `None` when it has no sectors.
    #[must_use]
    pub fn last(&self) -> Option<Lba> {
        (self.sectors > 0).then(|| Lba(self.end().0 - 1))
    }

    #[must_use]
    pub fn offset_bytes(&self) -> u64 {
        self.block_size.offset_of(self.start)
    }

    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.sectors.saturating_mul(u64::from(self.block_size.0))
    }

    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.sectors > 0
            && other.sectors > 0
            && self.start < other.end()
            && other.start < self.end()
    }
}

impl Serialize for Extent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("start_lba", &self.start)?;
        map.serialize_entry("sectors", &self.sectors)?;
        map.serialize_entry("offset_bytes", &self.offset_bytes())?;
        map.serialize_entry("size_bytes", &self.size_bytes())?;
        map.end()
    }
}

/// A partition's number, counted from 1. Device names are built from it
/// (`disk6s1`, `mmcblk0p1`, `sdb1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PartitionNumber(pub u32);

impl PartitionNumber {
    /// Logical partitions are numbered from 5, after the four MBR slots.
    pub const FIRST_LOGICAL: Self = Self(5);

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for PartitionNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Size, sector size, partition table and volumes of the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub size_bytes: Field<u64>,
    pub block_size: Field<BlockSize>,
    pub partition_table: Field<PartitionTable>,
    /// The data partitions in table order (extended partitions hold no data
    /// and are left out), or the whole disk when a filesystem spans it.
    /// Empty when the partition table is unavailable.
    pub volumes: Vec<Volume>,
}

impl Layout {
    /// A layout nothing could be read for, all for the same reason.
    #[must_use]
    pub fn unavailable(reason: &Unavailable) -> Self {
        Self {
            size_bytes: Field::Unavailable(reason.clone()),
            block_size: Field::Unavailable(reason.clone()),
            partition_table: Field::Unavailable(reason.clone()),
            volumes: Vec::new(),
        }
    }

    /// Number of whole sectors on the disk, when both sizes are known.
    #[must_use]
    pub fn sectors(&self) -> Option<u64> {
        match (&self.size_bytes, &self.block_size) {
            (Field::Known { value: size, .. }, Field::Known { value: block, .. }) => {
                Some(size / u64::from(block.0))
            }
            _ => None,
        }
    }
}

/// The partition table found on the disk, tagged by `scheme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "scheme", rename_all = "snake_case")]
pub enum PartitionTable {
    Mbr(MbrTable),
    Gpt(GptTable),
    /// No partition table: the reason says what sector 0 holds instead.
    None(NoTable),
}

/// Master boot record table, including logical partitions inside an extended
/// partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MbrTable {
    pub disk_signature: DiskSignature,
    pub partitions: Vec<MbrPartition>,
    pub issues: Vec<Issue>,
}

/// The 32-bit disk signature at offset 440 of the MBR. Serialized as hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSignature(pub u32);

impl serde::Serialize for DiskSignature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{:#010x}", self.0))
    }
}

/// One MBR partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MbrPartition {
    /// Partition number as Linux counts them: the table slot (1 to 4) for
    /// primary and extended partitions, 5 and up for logical partitions.
    pub number: PartitionNumber,
    pub role: MbrRole,
    pub bootable: bool,
    #[serde(rename = "type")]
    pub partition_type: MbrType,
    #[serde(flatten)]
    pub extent: Extent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MbrRole {
    /// An entry in the MBR itself.
    Primary,
    /// An MBR entry that holds logical partitions instead of data.
    Extended,
    /// A partition inside the extended partition.
    Logical,
}

/// GUID partition table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GptTable {
    pub disk_guid: Guid,
    /// Which header copy the table was read from.
    pub header: HeaderCopy,
    pub first_usable_lba: Lba,
    pub last_usable_lba: Lba,
    /// Number of entry slots in the table, used or not.
    pub entry_slots: u32,
    pub protective_mbr: ProtectiveMbr,
    pub partitions: Vec<GptPartition>,
    pub issues: Vec<Issue>,
}

/// One GPT partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GptPartition {
    /// Slot in the entry array, counted from 1.
    pub number: PartitionNumber,
    #[serde(rename = "type")]
    pub partition_type: GptType,
    pub guid: Guid,
    pub name: String,
    pub attributes: GptAttributes,
    #[serde(flatten)]
    pub extent: Extent,
}

/// The 64-bit attribute field of a GPT entry.
///
/// Serialized as `{"raw": "0x...", "flags": [...]}`. `flags` lists the bits
/// the UEFI specification defines for every partition type; bits 48 to 63
/// mean different things per type and are only in `raw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GptAttributes(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GptFlag {
    /// Bit 0: the platform needs this partition to function.
    Required,
    /// Bit 1: firmware must not read the partition's blocks.
    NoBlockIo,
    /// Bit 2: legacy BIOS bootable.
    LegacyBiosBootable,
}

impl GptAttributes {
    #[must_use]
    pub fn flags(self) -> Vec<GptFlag> {
        [
            (0, GptFlag::Required),
            (1, GptFlag::NoBlockIo),
            (2, GptFlag::LegacyBiosBootable),
        ]
        .into_iter()
        .filter(|(bit, _)| self.0 & (1 << bit) != 0)
        .map(|(_, flag)| flag)
        .collect()
    }
}

impl Serialize for GptAttributes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("raw", &format!("{:#018x}", self.0))?;
        map.serialize_entry("flags", &self.flags())?;
        map.end()
    }
}

/// GPT keeps a primary header at LBA 1 and a backup copy at the last LBA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderCopy {
    Primary,
    Backup,
}

/// What the MBR in sector 0 of a GPT disk looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectiveMbr {
    /// A single 0xEE entry covering the disk, as the UEFI specification asks.
    Protective,
    /// A 0xEE entry plus other entries that mirror GPT partitions for tools
    /// that only read MBR.
    Hybrid,
    /// No 0xEE entry: tools that check for it may not see the GPT.
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NoTable {
    pub reason: NoTableReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoTableReason {
    /// Sector 0 is all zeros.
    Blank,
    /// Sector 0 is a filesystem boot sector: the filesystem spans the whole
    /// disk with no partition table ("superfloppy").
    WholeDiskFilesystem,
    /// Sector 0 is neither a partition table nor a known boot sector.
    Unrecognized,
}

/// A problem found in the partition table. Serialized tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Issue {
    /// The partition ends after the last sector of the disk.
    PastEndOfDisk {
        partition: PartitionNumber,
        end_lba: Lba,
        disk_sectors: u64,
    },
    /// Two partitions share sectors.
    Overlap {
        first: PartitionNumber,
        second: PartitionNumber,
    },
    /// The entry's end comes before its start, or it has no sectors.
    EmptyOrReversed { partition: PartitionNumber },
    /// A GPT partition lies outside the header's usable LBA range.
    OutsideUsableRange { partition: PartitionNumber },
    /// One GPT header copy could not be used.
    GptHeader {
        copy: HeaderCopy,
        problem: GptHeaderProblem,
    },
    /// Both GPT header copies are valid but describe different tables.
    GptHeadersDiffer,
    /// The backup GPT header is not on the last sector, as when an image is
    /// written to a larger card.
    GptBackupNotAtEnd { backup_lba: Lba, last_lba: Lba },
    /// The MBR has a 0xEE entry announcing a GPT, but neither GPT header
    /// copy could be used.
    GptUnreadable {
        primary: GptHeaderProblem,
        backup: GptHeaderProblem,
    },
    /// A valid GPT sits at LBA 1, but the MBR has partitions and no 0xEE
    /// entry. The MBR is the table in use, as on Linux and macOS; the GPT is
    /// probably left over from an earlier format.
    StaleGpt,
    /// The chain of logical partitions stops early.
    EbrChain { lba: Lba, problem: EbrProblem },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GptHeaderProblem {
    /// The header or its entry array lies past the end of the disk.
    PastEndOfDisk,
    /// No `EFI PART` signature.
    Missing,
    /// The header size field is outside what the specification allows.
    BadHeaderSize,
    /// The header checksum does not match.
    BadHeaderChecksum,
    /// The header says it is at a different LBA than where it was read.
    WrongLocation,
    /// The entry size or count is outside what the specification allows.
    BadEntryArray,
    /// The checksum of the entry array does not match.
    BadEntriesChecksum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EbrProblem {
    /// The next extended boot record lies past the end of the disk.
    PastEndOfDisk,
    /// The extended boot record has no 0x55AA signature.
    NoSignature,
    /// The chain points back to a record already read.
    Loop,
    /// The chain is longer than any real table.
    TooLong,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(start: u64, sectors: u64) -> Extent {
        Extent::new(Lba(start), sectors, BlockSize::SD)
    }

    #[test]
    fn block_size_rejects_invalid() {
        assert_eq!(BlockSize::new(512), Some(BlockSize::SD));
        assert!(BlockSize::new(4096).is_some());
        assert_eq!(BlockSize::new(256), None);
        assert_eq!(BlockSize::new(1000), None);
    }

    #[test]
    fn extent_bytes() {
        let e = extent(2048, 4096);
        assert_eq!(e.offset_bytes(), 1024 * 1024);
        assert_eq!(e.size_bytes(), 2 * 1024 * 1024);
        assert_eq!(e.end(), Lba(6144));
        assert_eq!(e.last(), Some(Lba(6143)));
        assert_eq!(extent(10, 0).last(), None);
    }

    #[test]
    fn extent_overlap() {
        assert!(extent(0, 10).overlaps(&extent(9, 5)));
        assert!(!extent(0, 10).overlaps(&extent(10, 5)));
        assert!(!extent(0, 10).overlaps(&extent(5, 0)));
    }

    #[test]
    fn extent_serializes_lba_and_bytes() {
        assert_eq!(
            serde_json::to_string(&extent(1, 2)).unwrap(),
            r#"{"start_lba":1,"sectors":2,"offset_bytes":512,"size_bytes":1024}"#
        );
    }

    #[test]
    fn gpt_attributes_flags() {
        let attrs = GptAttributes(0b101 | 1 << 63);
        assert_eq!(
            attrs.flags(),
            [GptFlag::Required, GptFlag::LegacyBiosBootable]
        );
        assert_eq!(
            serde_json::to_string(&attrs).unwrap(),
            r#"{"raw":"0x8000000000000005","flags":["required","legacy_bios_bootable"]}"#
        );
    }

    #[test]
    fn disk_signature_is_hex() {
        assert_eq!(
            serde_json::to_string(&DiskSignature(0xAB)).unwrap(),
            r#""0x000000ab""#
        );
    }
}
