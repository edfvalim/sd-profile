//! Volumes: the data partitions of the disk, or the whole disk when a
//! filesystem spans it without a partition table.

use serde::Serialize;

use super::field::Field;
use super::filesystem::Filesystem;
use super::layout::{Extent, PartitionNumber};

/// One volume and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Volume {
    /// The partition the volume is, `null` for a filesystem on the whole disk.
    pub partition: Option<PartitionNumber>,
    #[serde(flatten)]
    pub extent: Extent,
    pub filesystem: Field<Filesystem>,
    pub alignment: Alignment,
}

/// How a volume sits against the card's erase blocks.
///
/// Flash cards erase and program in large units. Writes to a cluster that
/// straddles two units cost two units' worth of work, which slows writes and
/// wears the card. The SD Association's File System Specification therefore
/// puts the data area (the first cluster) on a boundary unit sized by
/// capacity, and for FAT32 and exFAT the partition start too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Alignment {
    /// The volume's first byte.
    pub start: Placement,
    /// The data area's first byte: the first cluster on FAT and exFAT, the
    /// volume start on other filesystems.
    pub data: Placement,
    /// The volume starts at LBA 63, the cylinder-aligned DOS layout that
    /// macOS Disk Utility still writes for MBR.
    pub dos_cylinder_start: bool,
    #[serde(flatten)]
    pub check: AlignmentCheck,
}

/// Where something starts on the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Placement {
    /// Byte offset from the start of the disk.
    pub disk_offset_bytes: u64,
    /// Largest power of two, up to 1 GiB, the offset is a multiple of. An
    /// offset of 0 counts as 1 GiB.
    pub aligned_to_bytes: u64,
}

/// The comparison with the recommended boundary, tagged by `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AlignmentCheck {
    /// Everything the specification aligns is on the boundary.
    Aligned { boundary: Boundary },
    /// Something isn't: clusters straddle erase blocks.
    Misaligned {
        boundary: Boundary,
        misaligned: Misaligned,
    },
    /// The specification gives no boundary we could verify for this
    /// capacity (over 2 TiB, SDUC).
    Unknown,
}

/// What lies off the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Misaligned {
    Start,
    Data,
    StartAndData,
}

/// The SD specification's boundary unit for one capacity range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Boundary {
    pub bytes: u64,
    pub capacity_class: CapacityClass,
    /// The capacity the boundary was chosen for. For a disk image this is
    /// the image's size.
    pub capacity_bytes: u64,
}

/// SD card capacity classes that have a published boundary unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityClass {
    /// Standard capacity, up to 2 GiB (FAT12/FAT16).
    Sdsc,
    /// High capacity, up to 32 GiB (FAT32).
    Sdhc,
    /// Extended capacity, up to 2 TiB (exFAT).
    Sdxc,
}
