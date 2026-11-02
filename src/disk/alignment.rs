//! Checks where a volume's data area sits against the boundary the SD
//! specification recommends for the card's capacity.
//!
//! Boundary units come from the SD Association's Part 2 File System
//! Specification 3.00, tables A-3 (SDSC), A-7 (SDHC) and A-15 (SDXC). The
//! specification places the first cluster on a boundary unit in every class.
//! For FAT32 and exFAT the partition starts on one too. For FAT12 and FAT16
//! it is deliberately off the boundary so the data area lands on it, so
//! there only the data area is checked. Filesystems the specification
//! doesn't cover (ext) start their data with the volume, which is checked as
//! the start.
//!
//! The card's real allocation unit can be larger than the boundary unit
//! (SDXC cards report up to 64 MiB). macOS can't read it, so the
//! specification's value stands in for it.

use crate::model::{
    Alignment, AlignmentCheck, Boundary, CapacityClass, Lba, Misaligned, Placement,
};

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

/// Alignments are reported up to this power of two.
const MAX_REPORTED: u64 = GIB;

/// Capacity ranges, upper bound included, and their boundary units. The
/// specification leaves 2048 to 2088.5 MiB and 32768 to 32896 MiB undefined;
/// those fall to the next class here.
const BOUNDARIES: &[(u64, CapacityClass, u64)] = &[
    (8 * MIB, CapacityClass::Sdsc, 8 * KIB),
    (64 * MIB, CapacityClass::Sdsc, 16 * KIB),
    (256 * MIB, CapacityClass::Sdsc, 32 * KIB),
    (2 * GIB, CapacityClass::Sdsc, 64 * KIB),
    (32 * GIB, CapacityClass::Sdhc, 4 * MIB),
    (128 * GIB, CapacityClass::Sdxc, 16 * MIB),
    (512 * GIB, CapacityClass::Sdxc, 32 * MIB),
    (2 * TIB, CapacityClass::Sdxc, 64 * MIB),
];

/// The start LBA of the first partition in the cylinder-aligned DOS layout.
const DOS_CYLINDER_START: Lba = Lba(63);

/// Whether the specification puts the volume's start on a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRule {
    /// FAT32, exFAT, and anything whose data starts with the volume.
    OnBoundary,
    /// FAT12 and FAT16: the start floats so the data area lands on one.
    Floating,
}

/// What the check needs to know about the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeStart {
    pub lba: Lba,
    pub offset_bytes: u64,
    /// Offset of the data area from the volume start.
    pub data_offset_bytes: u64,
    pub rule: StartRule,
}

/// The recommended boundary for a card of `capacity_bytes`; `None` above
/// 2 TiB (SDUC).
#[must_use]
pub fn recommended_boundary(capacity_bytes: u64) -> Option<Boundary> {
    BOUNDARIES
        .iter()
        .find(|(limit, _, _)| capacity_bytes <= *limit)
        .map(|&(_, capacity_class, bytes)| Boundary {
            bytes,
            capacity_class,
            capacity_bytes,
        })
}

/// Largest power of two, up to [`MAX_REPORTED`], that `offset` is a
/// multiple of.
fn aligned_to(offset: u64) -> u64 {
    if offset == 0 {
        MAX_REPORTED
    } else {
        (1u64 << offset.trailing_zeros()).min(MAX_REPORTED)
    }
}

fn placement(disk_offset_bytes: u64) -> Placement {
    Placement {
        disk_offset_bytes,
        aligned_to_bytes: aligned_to(disk_offset_bytes),
    }
}

/// Checks a volume on a card of `capacity_bytes`.
#[must_use]
pub fn check(volume: VolumeStart, capacity_bytes: u64) -> Alignment {
    let start = placement(volume.offset_bytes);
    let data = placement(volume.offset_bytes.saturating_add(volume.data_offset_bytes));
    let check = match recommended_boundary(capacity_bytes) {
        None => AlignmentCheck::Unknown,
        Some(boundary) => {
            let off = |p: Placement| !p.disk_offset_bytes.is_multiple_of(boundary.bytes);
            let start_off = volume.rule == StartRule::OnBoundary && off(start);
            match (start_off, off(data)) {
                (false, false) => AlignmentCheck::Aligned { boundary },
                (true, false) => AlignmentCheck::Misaligned {
                    boundary,
                    misaligned: Misaligned::Start,
                },
                (false, true) => AlignmentCheck::Misaligned {
                    boundary,
                    misaligned: Misaligned::Data,
                },
                (true, true) => AlignmentCheck::Misaligned {
                    boundary,
                    misaligned: Misaligned::StartAndData,
                },
            }
        }
    };
    Alignment {
        start,
        data,
        dos_cylinder_start: volume.lba == DOS_CYLINDER_START,
        check,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn boundaries_by_capacity() {
        let bytes = |capacity| recommended_boundary(capacity).map(|b| b.bytes);
        assert_eq!(bytes(64 * MIB), Some(16 * KIB));
        assert_eq!(bytes(128 * MIB), Some(32 * KIB));
        assert_eq!(bytes(2 * GIB), Some(64 * KIB));
        // A "32 GB" card holds about 29.7 GiB.
        assert_eq!(bytes(31_914_983_424), Some(4 * MIB));
        assert_eq!(bytes(64 * GB), Some(16 * MIB));
        assert_eq!(bytes(256 * GB), Some(32 * MIB));
        assert_eq!(bytes(1000 * GB), Some(64 * MIB));
        assert_eq!(bytes(4 * TIB), None);
    }

    fn volume(lba: u64, data_sectors: u64, rule: StartRule) -> VolumeStart {
        VolumeStart {
            lba: Lba(lba),
            offset_bytes: lba * 512,
            data_offset_bytes: data_sectors * 512,
            rule,
        }
    }

    fn misaligned(a: &Alignment) -> Option<Misaligned> {
        match a.check {
            AlignmentCheck::Misaligned { misaligned, .. } => Some(misaligned),
            _ => None,
        }
    }

    #[test]
    fn spec_layouts_are_aligned() {
        // SDHC example from the specification: partition at LBA 8192, first
        // cluster at LBA 16384 on an 8 GB card.
        let a = check(volume(8192, 8192, StartRule::OnBoundary), 8 * GB);
        assert!(matches!(a.check, AlignmentCheck::Aligned { .. }));
        assert_eq!(a.start.aligned_to_bytes, 4 * MIB);
        // SDSC FAT12 example: partition at LBA 39, data at LBA 96. The start
        // is off the boundary on purpose.
        let a = check(volume(39, 57, StartRule::Floating), 63 * MIB);
        assert!(matches!(a.check, AlignmentCheck::Aligned { .. }));
        assert_eq!(a.start.aligned_to_bytes, 512);
    }

    #[test]
    fn disk_utility_mbr_is_misaligned() {
        // mbr-fat32: partition at LBA 63, first cluster 2048 sectors later.
        let a = check(volume(63, 2048, StartRule::OnBoundary), 32 * GB);
        assert_eq!(misaligned(&a), Some(Misaligned::StartAndData));
        assert!(a.dos_cylinder_start);
        assert_eq!(a.data.disk_offset_bytes, 2111 * 512);
    }

    #[test]
    fn fat32_start_off_the_boundary() {
        // Partition at 1 MiB whose first cluster happens to land on 4 MiB.
        let a = check(volume(2048, 6144, StartRule::OnBoundary), 16 * GB);
        assert_eq!(misaligned(&a), Some(Misaligned::Start));
        assert!(!a.dos_cylinder_start);
    }

    #[test]
    fn one_mib_is_not_enough_for_sdhc() {
        let a = check(volume(2048, 0, StartRule::OnBoundary), 16 * GB);
        assert_eq!(misaligned(&a), Some(Misaligned::StartAndData));
        assert_eq!(a.data.aligned_to_bytes, MIB);
    }

    #[test]
    fn start_of_disk_counts_as_fully_aligned() {
        let a = check(volume(0, 0, StartRule::OnBoundary), 16 * GB);
        assert_eq!(a.start.aligned_to_bytes, GIB);
    }

    #[test]
    fn no_boundary_over_2_tib() {
        let a = check(volume(2048, 0, StartRule::OnBoundary), 4 * TIB);
        assert_eq!(a.check, AlignmentCheck::Unknown);
    }

    #[test]
    fn check_serializes_flat() {
        let a = check(volume(2048, 0, StartRule::OnBoundary), 64 * MIB);
        let json = serde_json::to_value(a).unwrap();
        assert_eq!(json["status"], "aligned");
        assert_eq!(json["boundary"]["bytes"], 16 * KIB);
        assert_eq!(json["dos_cylinder_start"], false);
    }
}
