//! Finds the partition table on a disk and turns it into the model.
//!
//! Detection order:
//!
//! 1. GPT, when the MBR announces it with a 0xEE entry, or when sector 0
//!    holds no MBR with partitions that the GPT would hide. That is the rule
//!    Linux and macOS follow, so a stale GPT left behind by a tool that only
//!    rewrote sector 0 doesn't hide the real MBR. A damaged primary header
//!    falls back to the backup copy.
//! 2. A filesystem boot sector at sector 0 (no partition table).
//! 3. An MBR, including the logical partitions of an extended partition.
//! 4. Otherwise none: blank, or unrecognized.

use std::collections::HashSet;

use super::block::{BlockReader, ReadError};
use super::gpt;
use super::mbr::{self, BootRecord, SECTOR_LEN};
use crate::model::{
    BlockSize, DiskSignature, EbrProblem, Extent, GptHeaderProblem, GptPartition, GptTable,
    HeaderCopy, Issue, Lba, MbrPartition, MbrRole, MbrTable, NoTable, NoTableReason,
    PartitionNumber, PartitionTable, ProtectiveMbr,
};

/// More logical partitions than this means the EBR chain is corrupt. Linux
/// stops at 256 partitions in total.
const MAX_LOGICAL_PARTITIONS: usize = 256;

/// Reads the partition table.
///
/// Problems inside the table (bad checksums, partitions past the end, broken
/// chains) are reported as [`Issue`]s in the result, not as errors.
///
/// # Errors
///
/// [`ReadError::Io`] when the disk can't be read, and
/// [`ReadError::OutOfBounds`] when it is smaller than one sector.
pub fn read(disk: &mut impl BlockReader) -> Result<PartitionTable, ReadError> {
    let geometry = Geometry::of(disk);
    let sector0 = read_sector(disk, Lba(0))?;
    let record = BootRecord::parse(&sector0);
    let announces_gpt = record
        .as_ref()
        .is_some_and(|r| r.gpt_protective_entry().is_some());
    let has_mbr_partitions = record
        .as_ref()
        .is_some_and(|r| r.has_valid_status() && r.used_entries().next().is_some());

    let mut issues = Vec::new();
    if announces_gpt || !has_mbr_partitions {
        let backup_candidates = backup_candidates(geometry, record.as_ref());
        match read_gpt(disk, geometry, announces_gpt, &backup_candidates)? {
            GptRead::Found(found) => {
                return Ok(PartitionTable::Gpt(build_gpt(
                    found,
                    record.as_ref(),
                    geometry,
                )));
            }
            GptRead::Absent => {}
            GptRead::Unreadable { primary, backup } => {
                issues.push(Issue::GptUnreadable { primary, backup });
            }
        }
    } else if read_gpt_copy(disk, Lba(1))?.is_ok() {
        issues.push(Issue::StaleGpt);
    }

    if mbr::is_volume_boot_record(&sector0) {
        return Ok(no_table(NoTableReason::WholeDiskFilesystem));
    }
    if let Some(record) = record.filter(BootRecord::has_valid_status) {
        let mut table = build_mbr(disk, &record, geometry)?;
        issues.append(&mut table.issues);
        table.issues = issues;
        return Ok(PartitionTable::Mbr(table));
    }
    let reason = if sector0.iter().all(|&b| b == 0) {
        NoTableReason::Blank
    } else {
        NoTableReason::Unrecognized
    };
    Ok(no_table(reason))
}

#[derive(Debug, Clone, Copy)]
struct Geometry {
    block_size: BlockSize,
    /// Whole sectors on the disk.
    sectors: u64,
}

impl Geometry {
    fn of(disk: &impl BlockReader) -> Self {
        Self {
            block_size: disk.block_size(),
            sectors: disk.size_bytes() / u64::from(disk.block_size().bytes()),
        }
    }

    fn extent(self, start: u64, sectors: u64) -> Extent {
        Extent::new(Lba(start), sectors, self.block_size)
    }

    fn last_lba(self) -> Lba {
        Lba(self.sectors.saturating_sub(1))
    }
}

fn no_table(reason: NoTableReason) -> PartitionTable {
    PartitionTable::None(NoTable { reason })
}

/// Reads the first 512 bytes of a block: where MBR and EBR live, whatever the
/// block size.
fn read_sector(disk: &mut impl BlockReader, lba: Lba) -> Result<[u8; SECTOR_LEN], ReadError> {
    let mut sector = [0; SECTOR_LEN];
    disk.read_exact_at(disk.block_size().offset_of(lba), &mut sector)?;
    Ok(sector)
}

/// Turns a read past the end of the disk into `None`, so it can be reported
/// as a table problem; other read errors still propagate.
fn within_disk<T>(result: Result<T, ReadError>) -> Result<Option<T>, ReadError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ReadError::OutOfBounds { .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

/// One GPT header copy and the entries of its array.
struct GptCopy {
    header: gpt::Header,
    entries: Vec<(PartitionNumber, gpt::Entry)>,
}

struct GptFound {
    header_copy: HeaderCopy,
    contents: GptCopy,
    issues: Vec<Issue>,
}

enum GptRead {
    Found(GptFound),
    /// No GPT signature at LBA 1 and no 0xEE entry in the MBR.
    Absent,
    /// A GPT is expected, but neither copy is usable.
    Unreadable {
        primary: GptHeaderProblem,
        backup: GptHeaderProblem,
    },
}

/// Where to look for the backup header when the primary is unusable: the
/// last sector of the disk, then the end of the 0xEE entry, which is the
/// last sector of the disk the table was made for.
fn backup_candidates(geometry: Geometry, record: Option<&BootRecord>) -> Vec<Lba> {
    let mut candidates = vec![geometry.last_lba()];
    if let Some(end) = record.and_then(BootRecord::gpt_protective_end)
        && end != geometry.last_lba()
    {
        candidates.push(end);
    }
    candidates
}

fn read_gpt(
    disk: &mut impl BlockReader,
    geometry: Geometry,
    announced: bool,
    backup_candidates: &[Lba],
) -> Result<GptRead, ReadError> {
    let (header_copy, contents, backup_lba, mut issues) = match read_gpt_copy(disk, Lba(1))? {
        Ok(primary) => {
            let backup_lba = primary.header.alternate_lba;
            let issue = match read_gpt_copy(disk, backup_lba)? {
                Err(problem) => Some(Issue::GptHeader {
                    copy: HeaderCopy::Backup,
                    problem,
                }),
                Ok(backup) => {
                    (!primary.header.matches(&backup.header)).then_some(Issue::GptHeadersDiffer)
                }
            };
            (
                HeaderCopy::Primary,
                primary,
                backup_lba,
                issue.into_iter().collect(),
            )
        }
        Err(GptHeaderProblem::Missing) if !announced => return Ok(GptRead::Absent),
        Err(primary_problem) => {
            let mut backup_problem = None;
            let mut found = None;
            for &lba in backup_candidates {
                match read_gpt_copy(disk, lba)? {
                    Ok(backup) => {
                        found = Some((lba, backup));
                        break;
                    }
                    Err(problem) => {
                        backup_problem.get_or_insert(problem);
                    }
                }
            }
            let Some((lba, backup)) = found else {
                return Ok(GptRead::Unreadable {
                    primary: primary_problem,
                    backup: backup_problem.unwrap_or(GptHeaderProblem::PastEndOfDisk),
                });
            };
            let issue = Issue::GptHeader {
                copy: HeaderCopy::Primary,
                problem: primary_problem,
            };
            (HeaderCopy::Backup, backup, lba, vec![issue])
        }
    };
    if backup_lba < geometry.last_lba() {
        issues.push(Issue::GptBackupNotAtEnd {
            backup_lba,
            last_lba: geometry.last_lba(),
        });
    }
    Ok(GptRead::Found(GptFound {
        header_copy,
        contents,
        issues,
    }))
}

/// Reads and validates one header copy and its entry array.
fn read_gpt_copy(
    disk: &mut impl BlockReader,
    lba: Lba,
) -> Result<Result<GptCopy, GptHeaderProblem>, ReadError> {
    let Some(block) = within_disk(disk.read_blocks(lba, 1))? else {
        return Ok(Err(GptHeaderProblem::PastEndOfDisk));
    };
    let header = match gpt::Header::parse(&block, lba) {
        Ok(header) => header,
        Err(problem) => return Ok(Err(problem)),
    };
    let mut array = vec![0; header.entry_array_len()];
    let offset = disk.block_size().offset_of(header.entries_lba);
    if within_disk(disk.read_exact_at(offset, &mut array))?.is_none() {
        return Ok(Err(GptHeaderProblem::PastEndOfDisk));
    }
    Ok(header
        .parse_entries(&array)
        .map(|entries| GptCopy { header, entries }))
}

fn build_gpt(found: GptFound, mbr: Option<&BootRecord>, geometry: Geometry) -> GptTable {
    let GptFound {
        header_copy,
        contents: GptCopy { header, entries },
        mut issues,
    } = found;

    let partitions: Vec<GptPartition> = entries
        .into_iter()
        .map(|(number, entry)| GptPartition {
            number,
            extent: geometry.extent(entry.first.0, entry.sectors()),
            partition_type: entry.partition_type,
            guid: entry.guid,
            name: entry.name,
            attributes: entry.attributes,
        })
        .collect();

    let extents: Vec<_> = partitions.iter().map(|p| (p.number, p.extent)).collect();
    issues.extend(
        extents
            .iter()
            .filter(|(_, extent)| !header.is_usable(extent))
            .map(|&(partition, _)| Issue::OutsideUsableRange { partition }),
    );
    issues.extend(extent_issues(&extents, geometry));
    issues.extend(overlap_issues(&extents));
    issues.sort_by_key(issue_order);

    GptTable {
        disk_guid: header.disk_guid,
        header: header_copy,
        first_usable_lba: header.first_usable,
        last_usable_lba: header.last_usable,
        entry_slots: header.entry_count,
        protective_mbr: protective_mbr(mbr),
        partitions,
        issues,
    }
}

fn protective_mbr(mbr: Option<&BootRecord>) -> ProtectiveMbr {
    match mbr {
        Some(record) if record.gpt_protective_entry().is_some() => {
            if record.used_entries().count() == 1 {
                ProtectiveMbr::Protective
            } else {
                ProtectiveMbr::Hybrid
            }
        }
        _ => ProtectiveMbr::Missing,
    }
}

/// Partitions and issues collected while reading an MBR and its EBR chains.
struct MbrPartitions {
    partitions: Vec<MbrPartition>,
    issues: Vec<Issue>,
    next_logical: PartitionNumber,
}

fn build_mbr(
    disk: &mut impl BlockReader,
    record: &BootRecord,
    geometry: Geometry,
) -> Result<MbrTable, ReadError> {
    let mut found = MbrPartitions {
        partitions: Vec::new(),
        issues: Vec::new(),
        next_logical: PartitionNumber::FIRST_LOGICAL,
    };

    for (slot, entry) in (1..).zip(&record.entries) {
        if !entry.is_used() {
            continue;
        }
        found.partitions.push(MbrPartition {
            number: PartitionNumber(slot),
            role: if entry.partition_type.is_extended() {
                MbrRole::Extended
            } else {
                MbrRole::Primary
            },
            bootable: entry.is_bootable(),
            partition_type: entry.partition_type,
            extent: geometry.extent(u64::from(entry.start), u64::from(entry.sectors)),
        });
    }

    let containers: Vec<Extent> = found
        .partitions
        .iter()
        .filter(|p| p.role == MbrRole::Extended)
        .map(|p| p.extent)
        .collect();
    for container in containers {
        read_logical(disk, container, geometry, &mut found)?;
    }

    let MbrPartitions {
        partitions,
        mut issues,
        ..
    } = found;
    let extents: Vec<_> = partitions.iter().map(|p| (p.number, p.extent)).collect();
    issues.extend(extent_issues(&extents, geometry));
    // Logical partitions overlap their extended container by design.
    let data: Vec<_> = partitions
        .iter()
        .filter(|p| p.role != MbrRole::Extended)
        .map(|p| (p.number, p.extent))
        .collect();
    issues.extend(overlap_issues(&data));
    issues.sort_by_key(issue_order);

    Ok(MbrTable {
        disk_signature: DiskSignature(record.disk_signature),
        partitions,
        issues,
    })
}

/// Walks the EBR chain of one extended partition, adding its logical
/// partitions to `found`.
fn read_logical(
    disk: &mut impl BlockReader,
    container: Extent,
    geometry: Geometry,
    found: &mut MbrPartitions,
) -> Result<(), ReadError> {
    let base = container.start().0;
    let mut ebr_lba = base;
    let mut seen = HashSet::new();
    loop {
        let stop = |problem| Issue::EbrChain {
            lba: Lba(ebr_lba),
            problem,
        };
        if seen.len() >= MAX_LOGICAL_PARTITIONS {
            found.issues.push(stop(EbrProblem::TooLong));
            return Ok(());
        }
        if !seen.insert(ebr_lba) {
            found.issues.push(stop(EbrProblem::Loop));
            return Ok(());
        }
        let Some(sector) = within_disk(read_sector(disk, Lba(ebr_lba)))? else {
            found.issues.push(stop(EbrProblem::PastEndOfDisk));
            return Ok(());
        };
        let Some(ebr) = BootRecord::parse(&sector) else {
            found.issues.push(stop(EbrProblem::NoSignature));
            return Ok(());
        };

        let [logical, next, ..] = ebr.entries;
        if logical.is_used() {
            found.partitions.push(MbrPartition {
                number: found.next_logical,
                role: MbrRole::Logical,
                bootable: logical.is_bootable(),
                partition_type: logical.partition_type,
                extent: geometry.extent(
                    ebr_lba.saturating_add(u64::from(logical.start)),
                    u64::from(logical.sectors),
                ),
            });
            found.next_logical = found.next_logical.next();
        }
        if !(next.is_used() && next.partition_type.is_extended()) {
            return Ok(());
        }
        ebr_lba = base.saturating_add(u64::from(next.start));
    }
}

/// Partitions with no sectors, and partitions past the end of the disk.
fn extent_issues(partitions: &[(PartitionNumber, Extent)], geometry: Geometry) -> Vec<Issue> {
    partitions
        .iter()
        .filter_map(|&(partition, extent)| match extent.last() {
            None => Some(Issue::EmptyOrReversed { partition }),
            Some(end_lba) => (end_lba.0 >= geometry.sectors).then_some(Issue::PastEndOfDisk {
                partition,
                end_lba,
                disk_sectors: geometry.sectors,
            }),
        })
        .collect()
}

/// Pairs of partitions that share sectors.
fn overlap_issues(partitions: &[(PartitionNumber, Extent)]) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (i, (first, a)) in partitions.iter().enumerate() {
        for (second, b) in &partitions[i + 1..] {
            if a.overlaps(b) {
                issues.push(Issue::Overlap {
                    first: *first,
                    second: *second,
                });
            }
        }
    }
    issues
}

/// Sort key for issues: table-wide ones first, then by the partition they
/// concern, so the report reads in order. The sort is stable.
fn issue_order(issue: &Issue) -> PartitionNumber {
    match issue {
        Issue::PastEndOfDisk { partition, .. }
        | Issue::EmptyOrReversed { partition }
        | Issue::OutsideUsableRange { partition }
        | Issue::Overlap {
            first: partition, ..
        } => *partition,
        _ => PartitionNumber(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::crc32;
    use crate::model::{GptType, MbrType};
    use crate::test_support::{fixture_image, reader};

    fn table(image: Vec<u8>) -> PartitionTable {
        read(&mut reader(image)).unwrap()
    }

    fn mbr_table(image: Vec<u8>) -> MbrTable {
        match table(image) {
            PartitionTable::Mbr(t) => t,
            other => panic!("expected MBR, got {other:?}"),
        }
    }

    fn gpt_table(image: Vec<u8>) -> GptTable {
        match table(image) {
            PartitionTable::Gpt(t) => t,
            other => panic!("expected GPT, got {other:?}"),
        }
    }

    fn summary(table: &MbrTable) -> Vec<(u32, MbrRole, u8, u64, u64)> {
        table
            .partitions
            .iter()
            .map(|p| {
                (
                    p.number.0,
                    p.role,
                    p.partition_type.0,
                    p.extent.start().0,
                    p.extent.sectors(),
                )
            })
            .collect()
    }

    /// Writes an MBR partition entry into a 512-byte sector and signs it.
    fn entry(sector: &mut [u8], slot: usize, kind: u8, start: u32, sectors: u32) {
        let at = 446 + slot * 16;
        sector[at + 4] = kind;
        sector[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
        sector[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
        sector[510] = 0x55;
        sector[511] = 0xAA;
    }

    /// Recomputes the primary GPT header's checksums after editing its
    /// entries (128 entries of 128 bytes at LBA 2, as diskutil writes them).
    fn reseal_primary_gpt(image: &mut [u8]) {
        let entries = crc32::checksum(&image[1024..1024 + 128 * 128]);
        image[512 + 88..512 + 92].copy_from_slice(&entries.to_le_bytes());
        image[512 + 16..512 + 20].fill(0);
        let header = crc32::checksum(&image[512..512 + 92]);
        image[512 + 16..512 + 20].copy_from_slice(&header.to_le_bytes());
    }

    fn p(number: u32) -> PartitionNumber {
        PartitionNumber(number)
    }

    #[test]
    fn mbr_single_fat32() {
        let t = mbr_table(fixture_image("mbr-fat32"));
        assert_eq!(summary(&t), [(1, MbrRole::Primary, 0x0B, 63, 130_977)]);
        assert!(t.issues.is_empty(), "{:?}", t.issues);
    }

    #[test]
    fn mbr_aligned_with_boot_flag() {
        let t = mbr_table(fixture_image("mbr-sd-aligned"));
        assert_eq!(
            summary(&t),
            [
                (1, MbrRole::Primary, 0x0C, 8192, 131_072),
                (2, MbrRole::Primary, 0x83, 139_264, 122_880),
            ]
        );
        assert!(t.partitions[0].bootable);
        assert!(!t.partitions[1].bootable);
        assert!(t.issues.is_empty(), "{:?}", t.issues);
    }

    #[test]
    fn mbr_extended_and_logical() {
        let t = mbr_table(fixture_image("mbr-extended"));
        let roles: Vec<_> = t.partitions.iter().map(|p| (p.number.0, p.role)).collect();
        assert_eq!(
            roles,
            [
                (1, MbrRole::Primary),
                (2, MbrRole::Primary),
                (3, MbrRole::Primary),
                (4, MbrRole::Extended),
                (5, MbrRole::Logical),
            ]
        );
        let extended = t.partitions[3].extent;
        let logical = t.partitions[4].extent;
        assert!(logical.start() > extended.start());
        assert!(logical.end() <= extended.end());
        assert!(t.issues.is_empty(), "{:?}", t.issues);
    }

    #[test]
    fn mbr_truncated_image_reports_past_end() {
        let t = mbr_table(fixture_image("mbr-fat32-truncated"));
        assert_eq!(
            t.issues,
            [Issue::PastEndOfDisk {
                partition: p(1),
                end_lba: Lba(63 + 130_977 - 1),
                disk_sectors: 2048,
            }]
        );
    }

    #[test]
    fn gpt_multi() {
        let t = gpt_table(fixture_image("gpt-multi"));
        assert_eq!(t.header, HeaderCopy::Primary);
        assert_eq!(t.protective_mbr, ProtectiveMbr::Protective);
        assert_eq!(t.entry_slots, 128);
        assert_eq!(t.partitions.len(), 2);
        assert!(
            t.partitions
                .iter()
                .all(|p| p.partition_type == GptType::MICROSOFT_BASIC_DATA)
        );
        assert!(t.issues.is_empty(), "{:?}", t.issues);
    }

    #[test]
    fn gpt_falls_back_to_backup_header() {
        let mut image = fixture_image("gpt-exfat");
        image[512 + 60] ^= 0xFF; // corrupt the primary header's disk GUID
        let t = gpt_table(image);
        assert_eq!(t.header, HeaderCopy::Backup);
        assert_eq!(
            t.issues,
            [Issue::GptHeader {
                copy: HeaderCopy::Primary,
                problem: GptHeaderProblem::BadHeaderChecksum,
            }]
        );
        assert_eq!(t.partitions.len(), 1);
    }

    #[test]
    fn gpt_backup_found_through_protective_entry() {
        // An image written to a larger disk, with a damaged primary header:
        // the backup is not at the last LBA but where the 0xEE entry ends.
        let mut image = fixture_image("gpt-exfat");
        let original_last = Lba(image.len() as u64 / 512 - 1);
        image.resize(image.len() * 2, 0);
        image[512 + 60] ^= 0xFF;
        let last = Lba(image.len() as u64 / 512 - 1);
        let t = gpt_table(image);
        assert_eq!(t.header, HeaderCopy::Backup);
        assert_eq!(
            t.issues,
            [
                Issue::GptHeader {
                    copy: HeaderCopy::Primary,
                    problem: GptHeaderProblem::BadHeaderChecksum,
                },
                Issue::GptBackupNotAtEnd {
                    backup_lba: original_last,
                    last_lba: last,
                },
            ]
        );
    }

    #[test]
    fn gpt_reports_damaged_backup() {
        let mut image = fixture_image("gpt-exfat");
        let last = image.len() - 512;
        image[last..].fill(0);
        let t = gpt_table(image);
        assert_eq!(t.header, HeaderCopy::Primary);
        assert_eq!(
            t.issues,
            [Issue::GptHeader {
                copy: HeaderCopy::Backup,
                problem: GptHeaderProblem::Missing,
            }]
        );
    }

    #[test]
    fn gpt_image_written_to_larger_disk() {
        let mut image = fixture_image("gpt-exfat");
        let original_last = Lba(image.len() as u64 / 512 - 1);
        image.resize(image.len() * 2, 0);
        let last = Lba(image.len() as u64 / 512 - 1);
        assert_eq!(
            gpt_table(image).issues,
            [Issue::GptBackupNotAtEnd {
                backup_lba: original_last,
                last_lba: last,
            }]
        );
    }

    #[test]
    fn gpt_reversed_entry_is_reported() {
        let mut image = fixture_image("gpt-multi");
        // Entry 1's last LBA before its first (2048).
        image[1024 + 40..1024 + 48].copy_from_slice(&100u64.to_le_bytes());
        reseal_primary_gpt(&mut image);
        let t = gpt_table(image);
        assert_eq!(t.partitions[0].extent.sectors(), 0);
        assert_eq!(
            t.issues,
            [
                Issue::GptHeadersDiffer,
                Issue::EmptyOrReversed { partition: p(1) },
            ]
        );
    }

    #[test]
    fn gpt_unreadable_falls_back_to_mbr() {
        let mut image = fixture_image("gpt-exfat");
        image[512..1024].fill(0);
        let last = image.len() - 512;
        image[last..].fill(0);
        let t = mbr_table(image);
        assert_eq!(t.partitions[0].partition_type, MbrType::GPT_PROTECTIVE);
        assert_eq!(
            t.issues,
            [Issue::GptUnreadable {
                primary: GptHeaderProblem::Missing,
                backup: GptHeaderProblem::Missing,
            }]
        );
    }

    #[test]
    fn stale_gpt_does_not_hide_mbr() {
        // A tool rewrote sector 0 as a plain MBR and left the old GPT behind.
        let mut image = fixture_image("gpt-multi");
        image[446..510].fill(0);
        entry(&mut image, 0, 0x0C, 2048, 100_000);
        let t = mbr_table(image);
        assert_eq!(summary(&t), [(1, MbrRole::Primary, 0x0C, 2048, 100_000)]);
        assert_eq!(t.issues, [Issue::StaleGpt]);
    }

    #[test]
    fn gpt_without_protective_mbr() {
        let mut image = fixture_image("gpt-multi");
        image[..512].fill(0);
        let t = gpt_table(image);
        assert_eq!(t.protective_mbr, ProtectiveMbr::Missing);
        assert_eq!(t.partitions.len(), 2);
    }

    #[test]
    fn hybrid_mbr_detected() {
        let mut image = fixture_image("gpt-multi");
        // Mirror GPT partition 1 into MBR slot 2, as hybrid-MBR tools do.
        entry(&mut image, 1, 0x0C, 2048, 1000);
        assert_eq!(gpt_table(image).protective_mbr, ProtectiveMbr::Hybrid);
    }

    #[test]
    fn no_table_reasons() {
        for (name, reason) in [
            ("blank", NoTableReason::Blank),
            ("superfloppy-fat32", NoTableReason::WholeDiskFilesystem),
        ] {
            assert_eq!(
                table(fixture_image(name)),
                PartitionTable::None(NoTable { reason }),
                "{name}"
            );
        }
        let mut junk = vec![0; 4096];
        junk[0] = 0x42;
        assert_eq!(
            table(junk),
            PartitionTable::None(NoTable {
                reason: NoTableReason::Unrecognized
            })
        );
    }

    #[test]
    fn image_smaller_than_a_sector_is_an_error() {
        assert!(matches!(
            read(&mut reader(vec![0; 100])),
            Err(ReadError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn overlapping_and_empty_partitions() {
        let mut image = vec![0; 1024 * 512];
        entry(&mut image, 0, 0x83, 100, 200);
        entry(&mut image, 1, 0x83, 250, 100);
        entry(&mut image, 2, 0x83, 500, 0);
        let t = mbr_table(image);
        assert_eq!(
            t.issues,
            [
                Issue::Overlap {
                    first: p(1),
                    second: p(2)
                },
                Issue::EmptyOrReversed { partition: p(3) },
            ]
        );
    }

    #[test]
    fn ebr_chain_with_two_logical_partitions() {
        let mut image = vec![0; 4096 * 512];
        entry(&mut image, 0, 0x0F, 1000, 3000);
        // First EBR at 1000: logical at +10, next EBR at extended + 1500.
        let ebr1 = 1000 * 512;
        entry(&mut image[ebr1..ebr1 + 512], 0, 0x83, 10, 100);
        entry(&mut image[ebr1..ebr1 + 512], 1, 0x05, 1500, 500);
        // Second EBR at 2500: logical at +10, end of chain.
        let ebr2 = 2500 * 512;
        entry(&mut image[ebr2..ebr2 + 512], 0, 0x0C, 10, 200);
        let t = mbr_table(image);
        assert_eq!(
            summary(&t),
            [
                (1, MbrRole::Extended, 0x0F, 1000, 3000),
                (5, MbrRole::Logical, 0x83, 1010, 100),
                (6, MbrRole::Logical, 0x0C, 2510, 200),
            ]
        );
        assert!(t.issues.is_empty(), "{:?}", t.issues);
    }

    #[test]
    fn ebr_loop_is_stopped() {
        let mut image = vec![0; 4096 * 512];
        entry(&mut image, 0, 0x05, 1000, 3000);
        let ebr = 1000 * 512;
        entry(&mut image[ebr..ebr + 512], 0, 0x83, 10, 100);
        entry(&mut image[ebr..ebr + 512], 1, 0x05, 0, 500); // points to itself
        let t = mbr_table(image);
        assert_eq!(t.partitions.len(), 2);
        assert_eq!(
            t.issues,
            [Issue::EbrChain {
                lba: Lba(1000),
                problem: EbrProblem::Loop
            }]
        );
    }

    #[test]
    fn ebr_without_signature() {
        let mut image = vec![0; 4096 * 512];
        entry(&mut image, 0, 0x05, 1000, 3000);
        let t = mbr_table(image);
        assert_eq!(
            t.issues,
            [Issue::EbrChain {
                lba: Lba(1000),
                problem: EbrProblem::NoSignature
            }]
        );
    }
}
