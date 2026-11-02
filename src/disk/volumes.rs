//! The volumes of a disk: where its filesystems are, and what each holds.

use super::alignment::{self, StartRule, VolumeStart};
use super::block::BlockReader;
use super::fs;
use crate::model::{
    Extent, Field, Filesystem, Lba, MbrRole, MbrType, NoTableReason, PartitionNumber,
    PartitionTable, Volume,
};

/// Reads the filesystem of every volume the partition table describes and
/// checks its alignment against the boundary for `capacity_bytes`.
///
/// Read failures make the affected filesystem unavailable instead of
/// failing the whole list.
pub fn read(
    disk: &mut impl BlockReader,
    table: &PartitionTable,
    capacity_bytes: u64,
) -> Vec<Volume> {
    locate(table, disk)
        .into_iter()
        .map(|(partition, extent)| {
            let filesystem = fs::read(disk, extent);
            let alignment = alignment::check(start_of(extent, &filesystem), capacity_bytes);
            Volume {
                partition,
                extent,
                filesystem,
                alignment,
            }
        })
        .collect()
}

/// What the alignment check needs from a volume and its filesystem.
fn start_of(extent: Extent, filesystem: &Field<Filesystem>) -> VolumeStart {
    let (data_offset_bytes, rule) = match filesystem {
        Field::Known { value, .. } => (
            value.data_offset_bytes(),
            match value {
                Filesystem::Fat12(_) | Filesystem::Fat16(_) => StartRule::Floating,
                _ => StartRule::OnBoundary,
            },
        ),
        Field::Unavailable(_) => (0, StartRule::OnBoundary),
    };
    VolumeStart {
        lba: extent.start(),
        offset_bytes: extent.offset_bytes(),
        data_offset_bytes,
        rule,
    }
}

/// The partitions that can hold a filesystem, or the whole disk.
fn locate(
    table: &PartitionTable,
    disk: &impl BlockReader,
) -> Vec<(Option<PartitionNumber>, Extent)> {
    let holds_data = |extent: &Extent| extent.sectors() > 0;
    match table {
        PartitionTable::Mbr(mbr) => mbr
            .partitions
            .iter()
            .filter(|p| p.role != MbrRole::Extended)
            .filter(|p| p.partition_type != MbrType::GPT_PROTECTIVE)
            .filter(|p| holds_data(&p.extent))
            .map(|p| (Some(p.number), p.extent))
            .collect(),
        PartitionTable::Gpt(gpt) => gpt
            .partitions
            .iter()
            .filter(|p| holds_data(&p.extent))
            .map(|p| (Some(p.number), p.extent))
            .collect(),
        PartitionTable::None(none) => match none.reason {
            NoTableReason::WholeDiskFilesystem => {
                let block_size = disk.block_size();
                let sectors = disk.size_bytes() / u64::from(block_size.bytes());
                vec![(None, Extent::new(Lba(0), sectors, block_size))]
            }
            NoTableReason::Blank | NoTableReason::Unrecognized => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::table;
    use crate::model::{AlignmentCheck, Misaligned};
    use crate::test_support::{fixture_image, reader};

    fn summary(image: &str) -> Vec<(Option<u32>, u64, &'static str)> {
        let mut disk = reader(fixture_image(image));
        let table = table::read(&mut disk).unwrap();
        let capacity = disk.size_bytes();
        read(&mut disk, &table, capacity)
            .into_iter()
            .map(|v| {
                let kind = match v.filesystem {
                    Field::Known { value, .. } => value.name(),
                    Field::Unavailable(_) => "unavailable",
                };
                (v.partition.map(|p| p.0), v.extent.start().0, kind)
            })
            .collect()
    }

    #[test]
    fn extended_partition_is_not_a_volume() {
        assert_eq!(
            summary("mbr-extended"),
            [
                (Some(1), 63, "FAT32"),
                (Some(2), 78246, "FAT16"),
                (Some(3), 109_557, "exFAT"),
                (Some(5), 140_868, "FAT32"),
            ]
        );
    }

    #[test]
    fn gpt_volumes() {
        assert_eq!(
            summary("gpt-multi"),
            [(Some(1), 2048, "FAT32"), (Some(2), 79872, "exFAT")]
        );
    }

    #[test]
    fn whole_disk_volumes() {
        assert_eq!(summary("superfloppy-fat32"), [(None, 0, "FAT32")]);
        assert_eq!(summary("superfloppy-ext4"), [(None, 0, "ext4")]);
        assert_eq!(summary("blank"), []);
    }

    #[test]
    fn raspberry_pi_layout() {
        assert_eq!(
            summary("mbr-sd-aligned"),
            [(Some(1), 8192, "FAT32"), (Some(2), 139_264, "ext4")]
        );
    }

    #[test]
    fn truncated_partition_still_probes() {
        assert_eq!(summary("mbr-fat32-truncated"), [(Some(1), 63, "FAT32")]);
    }

    #[test]
    fn alignment_of_fixtures() {
        let status = |image: &str| {
            let mut disk = reader(fixture_image(image));
            let table = table::read(&mut disk).unwrap();
            let capacity = disk.size_bytes();
            read(&mut disk, &table, capacity)
                .iter()
                .map(|v| match v.alignment.check {
                    AlignmentCheck::Aligned { .. } => None,
                    AlignmentCheck::Misaligned { misaligned, .. } => Some(misaligned),
                    AlignmentCheck::Unknown => panic!("no boundary for {image}"),
                })
                .collect::<Vec<_>>()
        };
        // Disk Utility's MBR layout starts at LBA 63; its first cluster is at
        // LBA 2111, a multiple of 512 bytes only.
        assert_eq!(status("mbr-fat32"), [Some(Misaligned::StartAndData)]);
        assert_eq!(status("mbr-sd-aligned"), [None, None]);
        // newfs_msdos FAT12 at 1 MiB: the start may float, the data may not.
        assert_eq!(
            status("mbr-fat12-fat16"),
            [Some(Misaligned::Data), Some(Misaligned::Data)]
        );
    }
}
