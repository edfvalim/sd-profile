//! The readable report. Its wording and layout may change between versions;
//! scripts should use `--json`.

mod device;

use std::borrow::Cow;
use std::io::{self, Write};

use crate::model::{
    Alignment, AlignmentCheck, Boundary, CapacityClass, EbrProblem, Exfat, Ext, Extent, Fat, Field,
    Filesystem, FsIssue, GptFlag, GptHeaderProblem, GptPartition, GptTable, HeaderCopy, Issue,
    Layout, MbrPartition, MbrRole, MbrTable, Misaligned, Mount, NoTableReason, PartitionTable,
    Profile, ProtectiveMbr, Source, Target, Volume,
};

const LABEL_WIDTH: usize = 17;
/// Width of a nested label: two spaces of indent less than [`LABEL_WIDTH`],
/// so nested values line up with top-level ones.
const SUB_LABEL_WIDTH: usize = LABEL_WIDTH - 2;
/// Lines that wrap (the ext feature list) stay within this many columns.
const MAX_WIDTH: usize = 80;

/// Writes the profile as a readable report.
///
/// # Errors
///
/// When writing to `out` fails.
pub fn write(profile: &Profile, out: &mut impl Write) -> io::Result<()> {
    match &profile.target {
        Target::DiskImage { path } => {
            // An image has no card, reader or mounts; the JSON says so, the
            // report leaves the sections out.
            line(out, "Target", &format!("{} (disk image)", path.display()))?;
        }
        Target::Device { .. } => device::write(profile, out)?,
    }
    // Images have no mounts to show; a device whose mount table couldn't
    // be read says so in its mounts section.
    let mounts = match &profile.target {
        Target::DiskImage { .. } => None,
        Target::Device { .. } => profile.mounts.value().map(Vec::as_slice),
    };
    write_layout(&profile.layout, mounts, out)
}

/// A field's value through `show`, or why it is unavailable.
fn shown<T>(field: &Field<T>, show: impl FnOnce(&T) -> String) -> String {
    match field {
        Field::Known { value, .. } => show(value),
        Field::Unavailable(reason) => format!("unavailable ({reason})"),
    }
}

fn line(out: &mut impl Write, label: &str, value: &str) -> io::Result<()> {
    let (label, value) = (printable(label), printable(value));
    writeln!(out, "{label:<LABEL_WIDTH$}{value}")
}

/// A line nested under a heading line, its value aligned with top-level
/// values.
fn sub_line(out: &mut impl Write, label: &str, value: &str) -> io::Result<()> {
    let (label, value) = (printable(label), printable(value));
    writeln!(out, "  {label:<SUB_LABEL_WIDTH$}{value}")
}

/// `text` with control characters and bidirectional overrides escaped
/// (`\u{1b}`). Labels, partition names and mount points come from the card
/// and could otherwise move the cursor, recolour the terminal or reorder
/// the line they are on.
fn printable(text: &str) -> Cow<'_, str> {
    let unsafe_char =
        |c: char| c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    if !text.contains(unsafe_char) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| {
                if unsafe_char(c) {
                    c.escape_unicode().to_string()
                } else {
                    c.to_string()
                }
            })
            .collect(),
    )
}

/// The layout sections. `mounts` is `None` when they aren't known, as for
/// a disk image.
fn write_layout(layout: &Layout, mounts: Option<&[Mount]>, out: &mut impl Write) -> io::Result<()> {
    write_table_section(layout, out)?;
    for volume in &layout.volumes {
        writeln!(out)?;
        let state = match mounts {
            None => MountState::Unknown,
            Some(mounts) => match mounts.iter().find(|m| m.partition == volume.partition) {
                Some(mount) => MountState::At(mount),
                None => MountState::NotMounted,
            },
        };
        write_volume(volume, &state, out)?;
    }
    Ok(())
}

/// Whether a volume is mounted, as far as the report knows.
enum MountState<'a> {
    /// No mount table: a disk image, or a table that couldn't be read.
    Unknown,
    NotMounted,
    At(&'a Mount),
}

fn write_table_section(layout: &Layout, out: &mut impl Write) -> io::Result<()> {
    line(
        out,
        "Size",
        &shown(&layout.size_bytes, |&bytes| {
            format!("{} ({bytes} bytes)", size(bytes))
        }),
    )?;
    line(
        out,
        "Sector size",
        &shown(&layout.block_size, |block| match layout.sectors() {
            Some(sectors) => format!("{} bytes ({sectors} sectors)", block.bytes()),
            None => format!("{} bytes", block.bytes()),
        }),
    )?;
    writeln!(out)?;

    match &layout.partition_table {
        Field::Unavailable(reason) => {
            line(out, "Partition table", &format!("unavailable ({reason})"))
        }
        Field::Known { value, .. } => match value {
            PartitionTable::Mbr(table) => write_mbr(table, out),
            PartitionTable::Gpt(table) => write_gpt(table, out),
            PartitionTable::None(none) => line(
                out,
                "Partition table",
                match none.reason {
                    NoTableReason::Blank => "none, the disk is blank (sector 0 is all zeros)",
                    NoTableReason::WholeDiskFilesystem => {
                        "none, a filesystem starts at sector 0 and spans the whole disk"
                    }
                    NoTableReason::Unrecognized => {
                        "none, sector 0 holds neither a partition table nor a known boot sector"
                    }
                },
            ),
        },
    }
}

fn write_mbr(table: &MbrTable, out: &mut impl Write) -> io::Result<()> {
    line(
        out,
        "Partition table",
        &format!("MBR, disk signature {:#010x}", table.disk_signature.0),
    )?;
    writeln!(out)?;

    if table.partitions.is_empty() {
        writeln!(out, "  No partitions.")?;
    } else {
        let mut columns = vec![
            Column::right("#"),
            Column::left("Role"),
            Column::left("Boot"),
        ];
        columns.extend(extent_columns());
        columns.push(Column::left("Type"));
        let rows: Vec<Vec<String>> = table.partitions.iter().map(mbr_row).collect();
        write_table(out, &columns, &rows)?;
    }
    write_issues(&table.issues, out)
}

fn mbr_row(p: &MbrPartition) -> Vec<String> {
    let role = match p.role {
        MbrRole::Primary => "primary",
        MbrRole::Extended => "extended",
        MbrRole::Logical => "logical",
    };
    let partition_type = &p.partition_type;
    let type_text = match partition_type.name() {
        Some(name) => format!("{:#04x} {name}", partition_type.0),
        None => format!("{:#04x}", partition_type.0),
    };
    let mut row = vec![
        p.number.to_string(),
        role.to_owned(),
        if p.bootable { "*" } else { "" }.to_owned(),
    ];
    row.extend(extent_cells(&p.extent));
    row.push(type_text);
    row
}

fn write_gpt(table: &GptTable, out: &mut impl Write) -> io::Result<()> {
    line(
        out,
        "Partition table",
        &format!("GPT, disk GUID {}", table.disk_guid),
    )?;
    line(
        out,
        "Header",
        match table.header {
            HeaderCopy::Primary => "primary",
            HeaderCopy::Backup => "backup (the primary is damaged)",
        },
    )?;
    line(
        out,
        "Usable sectors",
        &format!(
            "LBA {} to {}",
            table.first_usable_lba.0, table.last_usable_lba.0
        ),
    )?;
    line(out, "Entry slots", &table.entry_slots.to_string())?;
    line(
        out,
        "Protective MBR",
        match table.protective_mbr {
            ProtectiveMbr::Protective => "yes",
            ProtectiveMbr::Hybrid => "hybrid (MBR entries mirror GPT partitions)",
            ProtectiveMbr::Missing => "missing",
        },
    )?;
    writeln!(out)?;

    if table.partitions.is_empty() {
        writeln!(out, "  No partitions.")?;
    } else {
        let mut columns = vec![Column::right("#")];
        columns.extend(extent_columns());
        columns.extend([
            Column::left("Type"),
            Column::left("Name").hidden_when_empty(),
            Column::left("Flags").hidden_when_empty(),
        ]);
        let rows: Vec<Vec<String>> = table.partitions.iter().map(gpt_row).collect();
        write_table(out, &columns, &rows)?;
    }
    write_issues(&table.issues, out)
}

fn gpt_row(p: &GptPartition) -> Vec<String> {
    let flags: Vec<&str> = p
        .attributes
        .flags()
        .iter()
        .map(|flag| match flag {
            GptFlag::Required => "required",
            GptFlag::NoBlockIo => "no block I/O",
            GptFlag::LegacyBiosBootable => "legacy BIOS bootable",
        })
        .collect();
    let mut row = vec![p.number.to_string()];
    row.extend(extent_cells(&p.extent));
    row.extend([
        match p.partition_type.name() {
            Some(name) => name.to_owned(),
            None => p.partition_type.0.to_string(),
        },
        p.name.clone(),
        flags.join(", "),
    ]);
    row
}

fn extent_columns() -> [Column; 4] {
    [
        Column::right("Start LBA"),
        Column::right("End LBA"),
        Column::right("Sectors"),
        Column::right("Size"),
    ]
}

fn extent_cells(extent: &Extent) -> [String; 4] {
    [
        extent.start().0.to_string(),
        extent
            .last()
            .map_or_else(|| "-".to_owned(), |last| last.0.to_string()),
        extent.sectors().to_string(),
        size(extent.size_bytes()),
    ]
}

fn write_issues(issues: &[Issue], out: &mut impl Write) -> io::Result<()> {
    if issues.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for issue in issues {
        writeln!(out, "WARN: {}", issue_text(issue))?;
    }
    Ok(())
}

fn issue_text(issue: &Issue) -> String {
    match issue {
        Issue::PastEndOfDisk {
            partition,
            end_lba,
            disk_sectors,
        } => format!(
            "partition {partition} ends at LBA {}, past the end of the disk ({disk_sectors} sectors)",
            end_lba.0
        ),
        Issue::Overlap { first, second } => format!("partitions {first} and {second} overlap"),
        Issue::EmptyOrReversed { partition } => {
            format!("partition {partition} has no sectors or ends before it starts")
        }
        Issue::OutsideUsableRange { partition } => {
            format!("partition {partition} lies outside the GPT's usable sectors")
        }
        Issue::OutsideExtended {
            partition,
            extended,
        } => format!("logical partition {partition} extends outside extended partition {extended}"),
        Issue::GptHeader { copy, problem } => format!(
            "{} GPT header is unusable: {}",
            copy_name(*copy),
            gpt_problem_text(*problem)
        ),
        Issue::GptHeadersDiffer => {
            "primary and backup GPT headers describe different tables".to_owned()
        }
        Issue::GptBackupNotAtEnd {
            backup_lba,
            last_lba,
        } => format!(
            "backup GPT header is at LBA {}, not at the last LBA {}; the image was likely \
             written to a larger disk",
            backup_lba.0, last_lba.0
        ),
        Issue::GptUnreadable { primary, backup } => format!(
            "the MBR announces a GPT, but neither GPT header is usable (primary: {}; backup: {})",
            gpt_problem_text(*primary),
            gpt_problem_text(*backup)
        ),
        Issue::StaleGpt => "a GPT sits at LBA 1, but the MBR has partitions and no 0xEE entry; \
                             the MBR is the table in use and the GPT is probably left over"
            .to_owned(),
        Issue::EbrChain { lba, problem } => format!(
            "the chain of logical partitions stops at LBA {}: {}",
            lba.0,
            match problem {
                EbrProblem::PastEndOfDisk => "the record is past the end of the disk",
                EbrProblem::NoSignature => "the record has no 0x55AA signature",
                EbrProblem::Loop => "the chain loops back to an earlier record",
                EbrProblem::TooLong => "the chain is longer than any real table",
            }
        ),
    }
}

fn copy_name(copy: HeaderCopy) -> &'static str {
    match copy {
        HeaderCopy::Primary => "primary",
        HeaderCopy::Backup => "backup",
    }
}

fn gpt_problem_text(problem: GptHeaderProblem) -> &'static str {
    match problem {
        GptHeaderProblem::PastEndOfDisk => "it lies past the end of the disk",
        GptHeaderProblem::Missing => "no GPT signature",
        GptHeaderProblem::BadHeaderSize => "invalid header size",
        GptHeaderProblem::BadHeaderChecksum => "header checksum mismatch",
        GptHeaderProblem::WrongLocation => "it records a different location than where it is",
        GptHeaderProblem::BadEntryArray => "invalid entry array size",
        GptHeaderProblem::BadEntriesChecksum => "entry array checksum mismatch",
    }
}

fn write_volume(volume: &Volume, mount: &MountState<'_>, out: &mut impl Write) -> io::Result<()> {
    let name = match volume.partition {
        Some(number) => format!("Partition {number}"),
        None => "Whole disk".to_owned(),
    };
    let issues = match &volume.filesystem {
        Field::Unavailable(reason) => {
            line(out, &name, &format!("filesystem unavailable ({reason})"))?;
            &[]
        }
        Field::Known { value, .. } => {
            let heading = match value {
                Filesystem::Ntfs => "NTFS (not profiled)",
                Filesystem::Blank => "no filesystem, its first sectors are all zeros",
                Filesystem::Unrecognized => "unrecognized filesystem",
                profiled => profiled.name(),
            };
            line(out, &name, heading)?;
            match value {
                Filesystem::Fat12(fat) | Filesystem::Fat16(fat) | Filesystem::Fat32(fat) => {
                    write_fat(fat, out)?;
                }
                Filesystem::Exfat(exfat) => write_exfat(exfat, out)?,
                Filesystem::Ext2(ext) | Filesystem::Ext3(ext) | Filesystem::Ext4(ext) => {
                    write_ext(ext, out)?;
                }
                Filesystem::Ntfs | Filesystem::Blank | Filesystem::Unrecognized => {}
            }
            value.issues()
        }
    };
    match mount {
        MountState::Unknown => {}
        MountState::NotMounted => sub_line(out, "Mounted", "no")?,
        MountState::At(mount) => {
            let access = if mount.is_read_only() {
                "read-only"
            } else {
                "read-write"
            };
            sub_line(
                out,
                "Mounted",
                &format!("{} ({access})", mount.mount_point.display()),
            )?;
        }
    }
    sub_line(out, "Start", &start_text(volume))?;
    sub_line(out, "Alignment", &alignment_text(&volume.alignment))?;

    let prefix = name.to_lowercase();
    let mut warnings: Vec<String> = issues.iter().map(fs_issue_text).collect();
    if volume.alignment.dos_cylinder_start {
        warnings.push(
            "it starts at LBA 63, the old DOS cylinder layout that macOS Disk Utility writes \
             for MBR"
                .to_owned(),
        );
    }
    if let AlignmentCheck::Misaligned { boundary, .. } = volume.alignment.check {
        warnings.push(format!(
            "its clusters straddle the {} boundaries the SD specification recommends for this \
             capacity, which slows writes and wears the card",
            exact_size(boundary.bytes)
        ));
    }
    if !warnings.is_empty() {
        writeln!(out)?;
    }
    for warning in warnings {
        writeln!(out, "WARN: {prefix}: {warning}")?;
    }
    Ok(())
}

fn start_text(volume: &Volume) -> String {
    let lba = volume.extent.start().0;
    match volume.alignment.start.disk_offset_bytes {
        0 => format!("LBA {lba}, the start of the disk"),
        _ => format!(
            "LBA {lba}, a multiple of {}",
            exact_size(volume.alignment.start.aligned_to_bytes)
        ),
    }
}

fn alignment_text(alignment: &Alignment) -> String {
    let data = match alignment.data.disk_offset_bytes {
        0 => "data area at the start of the disk".to_owned(),
        offset => format!("data area at {}", exact_size(offset)),
    };
    let data_multiple = format!(
        "a multiple of {}",
        exact_size(alignment.data.aligned_to_bytes)
    );
    match alignment.check {
        AlignmentCheck::Aligned { boundary } => format!(
            "ok, {data}, on the {} boundary for {}",
            exact_size(boundary.bytes),
            capacity_text(&boundary)
        ),
        AlignmentCheck::Misaligned {
            boundary,
            misaligned,
        } => {
            let what = match misaligned {
                Misaligned::Start => "the start is",
                Misaligned::Data => "the data area is",
                Misaligned::StartAndData => "the start and the data area are",
            };
            format!(
                "off, {what} not on the {} boundary for {}; {data}, {data_multiple}",
                exact_size(boundary.bytes),
                capacity_text(&boundary)
            )
        }
        AlignmentCheck::Unknown => {
            format!("{data}, {data_multiple}; no boundary is known for cards over 2 TiB")
        }
    }
}

fn capacity_text(boundary: &Boundary) -> String {
    let class = match boundary.capacity_class {
        CapacityClass::Sdsc => "SDSC",
        CapacityClass::Sdhc => "SDHC",
        CapacityClass::Sdxc => "SDXC",
    };
    format!("{} {class}", size(boundary.capacity_bytes))
}

/// An offset or size in the largest binary unit that divides it exactly,
/// like `4 MiB` or `1080832 bytes`.
fn exact_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GiB"), (1 << 20, "MiB"), (1 << 10, "KiB")];
    UNITS
        .iter()
        .find(|(unit, _)| bytes > 0 && bytes.is_multiple_of(*unit))
        .map_or_else(
            || format!("{bytes} bytes"),
            |(unit, name)| format!("{} {name}", bytes / unit),
        )
}

fn write_fat(fat: &Fat, out: &mut impl Write) -> io::Result<()> {
    write_label(&fat.label, out)?;
    sub_line(
        out,
        "Volume ID",
        &fat.volume_id
            .map_or_else(|| "none".to_owned(), |id| id.to_string()),
    )?;
    write_size(
        fat.size_bytes,
        fat.cluster_count.into(),
        "clusters",
        fat.cluster_size_bytes,
        out,
    )?;
    write_free(&fat.free_bytes, fat.size_bytes, None, out)?;
    sub_line(out, "Formatter", &fat.oem_name)
}

fn write_exfat(exfat: &Exfat, out: &mut impl Write) -> io::Result<()> {
    write_label(&exfat.label, out)?;
    sub_line(out, "Volume ID", &exfat.volume_id.to_string())?;
    write_size(
        exfat.size_bytes,
        exfat.cluster_count.into(),
        "clusters",
        exfat.cluster_size_bytes,
        out,
    )?;
    write_free(&exfat.free_bytes, exfat.size_bytes, None, out)?;
    sub_line(out, "Revision", &exfat.revision.to_string())
}

fn write_ext(ext: &Ext, out: &mut impl Write) -> io::Result<()> {
    write_label(&ext.label, out)?;
    sub_line(
        out,
        "UUID",
        &ext.uuid
            .map_or_else(|| "none (revision 0)".to_owned(), |uuid| uuid.to_string()),
    )?;
    write_size(
        ext.size_bytes,
        ext.block_count,
        "blocks",
        ext.block_size_bytes,
        out,
    )?;
    if ext.cluster_size_bytes != ext.block_size_bytes {
        sub_line(
            out,
            "Clusters",
            &format!("{} (bigalloc)", size(u64::from(ext.cluster_size_bytes))),
        )?;
    }
    write_free(
        &ext.free_bytes,
        ext.size_bytes,
        Some(ext.reserved_bytes),
        out,
    )?;
    sub_line(
        out,
        "Inodes",
        &format!("{} of {} free", ext.free_inodes, ext.inode_count),
    )?;
    write_wrapped("Features", &ext.features.names(), out)?;
    if let Some(path) = &ext.last_mounted {
        sub_line(out, "Last mounted", path)?;
    }
    Ok(())
}

fn write_label(label: &Field<Option<String>>, out: &mut impl Write) -> io::Result<()> {
    let text = match label {
        Field::Unavailable(reason) => format!("unavailable ({reason})"),
        Field::Known { value: None, .. } => "none".to_owned(),
        Field::Known {
            value: Some(label),
            source: Source::BootSector,
        } => format!("{label} (from the boot sector; the root directory has no label)"),
        Field::Known {
            value: Some(label), ..
        } => label.clone(),
    };
    sub_line(out, "Label", &text)
}

fn write_size(
    bytes: u64,
    units: u64,
    unit_name: &str,
    unit_bytes: u32,
    out: &mut impl Write,
) -> io::Result<()> {
    sub_line(
        out,
        "Size",
        &format!(
            "{} in {units} {unit_name} of {}",
            size(bytes),
            size(unit_bytes.into())
        ),
    )
}

/// Free space and its share of the total; `reserved` is the part only root
/// may use (ext).
fn write_free(
    free: &Field<u64>,
    total: u64,
    reserved: Option<u64>,
    out: &mut impl Write,
) -> io::Result<()> {
    let text = match free {
        Field::Unavailable(reason) => format!("unavailable ({reason})"),
        Field::Known { value, .. } => {
            let mut text = size(*value);
            if total > 0 {
                let tenths =
                    (u128::from(*value) * 1000 + u128::from(total) / 2) / u128::from(total);
                text = format!("{text} ({}.{}%)", tenths / 10, tenths % 10);
            }
            if let Some(reserved) = reserved.filter(|&r| r > 0) {
                text = format!("{text}, {} of it reserved for root", size(reserved));
            }
            text
        }
    };
    sub_line(out, "Free", &text)
}

/// A nested line whose words wrap within [`MAX_WIDTH`], continuation lines
/// aligned with the first value.
fn write_wrapped(label: &str, words: &[String], out: &mut impl Write) -> io::Result<()> {
    let mut lines: Vec<String> = Vec::new();
    for word in words {
        match lines.last_mut() {
            Some(current) if LABEL_WIDTH + current.len() + 1 + word.len() <= MAX_WIDTH => {
                current.push(' ');
                current.push_str(word);
            }
            _ => lines.push(word.clone()),
        }
    }
    if lines.is_empty() {
        return sub_line(out, label, "none");
    }
    for (i, text) in lines.iter().enumerate() {
        sub_line(out, if i == 0 { label } else { "" }, text)?;
    }
    Ok(())
}

fn fs_issue_text(issue: &FsIssue) -> String {
    match issue {
        FsIssue::NotCleanlyUnmounted => {
            "the filesystem was not cleanly unmounted; check it with fsck".to_owned()
        }
        FsIssue::ErrorsRecorded => {
            "the filesystem has recorded errors; check it with fsck".to_owned()
        }
        FsIssue::FreeCountStale {
            recorded_bytes,
            counted_bytes,
        } => format!(
            "the stored free space hint is {} {} the allocation count ({} free)",
            size(recorded_bytes.abs_diff(*counted_bytes)),
            if recorded_bytes < counted_bytes {
                "below"
            } else {
                "above"
            },
            size(*counted_bytes)
        ),
        FsIssue::BootChecksumMismatch => "the boot region checksum does not match".to_owned(),
        FsIssue::SuperblockChecksumMismatch => "the superblock checksum does not match".to_owned(),
        FsIssue::LargerThanPartition {
            filesystem_bytes,
            partition_bytes,
        } => format!(
            "the filesystem ({}) is larger than its partition ({})",
            size(*filesystem_bytes),
            size(*partition_bytes)
        ),
    }
}

/// Size in binary units with one decimal, like `64.0 MiB`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut unit = 1u128;
    let mut index = 0;
    while index + 1 < UNITS.len() && u128::from(bytes) >= unit * 1024 {
        unit *= 1024;
        index += 1;
    }
    let tenths = (u128::from(bytes) * 10 + unit / 2) / unit;
    format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[index])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WhenEmpty {
    Show,
    /// Leave the column out when no row has a value for it.
    Hide,
}

struct Column {
    title: &'static str,
    align: Align,
    when_empty: WhenEmpty,
}

impl Column {
    fn left(title: &'static str) -> Self {
        Self {
            title,
            align: Align::Left,
            when_empty: WhenEmpty::Show,
        }
    }

    fn right(title: &'static str) -> Self {
        Self {
            title,
            align: Align::Right,
            when_empty: WhenEmpty::Show,
        }
    }

    fn hidden_when_empty(self) -> Self {
        Self {
            when_empty: WhenEmpty::Hide,
            ..self
        }
    }
}

/// Writes rows under column titles, indented two spaces, columns two spaces
/// apart.
fn write_table(out: &mut impl Write, columns: &[Column], rows: &[Vec<String>]) -> io::Result<()> {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| printable(cell).into_owned())
                .collect()
        })
        .collect();
    let rows = rows.as_slice();
    let shown: Vec<(usize, &Column, usize)> = columns
        .iter()
        .enumerate()
        .filter(|(i, column)| {
            column.when_empty == WhenEmpty::Show || rows.iter().any(|row| !row[*i].is_empty())
        })
        .map(|(i, column)| {
            let width = rows
                .iter()
                .map(|row| row[i].chars().count())
                .chain([column.title.len()])
                .max()
                .unwrap_or(0);
            (i, column, width)
        })
        .collect();
    let titles: Vec<String> = columns.iter().map(|c| c.title.to_owned()).collect();
    for row in std::iter::once(&titles).chain(rows) {
        let cells: Vec<String> = shown
            .iter()
            .map(|&(i, column, width)| match column.align {
                Align::Right => format!("{:>width$}", row[i]),
                Align::Left => format!("{:<width$}", row[i]),
            })
            .collect();
        writeln!(out, "  {}", cells.join("  ").trim_end())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_free_count_says_how_far_off_it_is() {
        let issue = FsIssue::FreeCountStale {
            recorded_bytes: 3_990_335_488,
            counted_bytes: 3_990_339_584,
        };
        assert_eq!(
            fs_issue_text(&issue),
            "the stored free space hint is 4.0 KiB below the allocation count (3.7 GiB free)"
        );
        let issue = FsIssue::FreeCountStale {
            recorded_bytes: 449_553_408,
            counted_bytes: 449_357_824,
        };
        assert_eq!(
            fs_issue_text(&issue),
            "the stored free space hint is 191.0 KiB above the allocation count (428.5 MiB free)"
        );
    }

    #[test]
    fn card_text_cannot_drive_the_terminal() {
        assert_eq!(printable("BOOTFS"), "BOOTFS");
        assert_eq!(printable("caf\u{e9}"), "caf\u{e9}");
        assert_eq!(printable("A\u{1b}[2JB"), "A\\u{1b}[2JB");
        assert_eq!(printable("a\nb"), "a\\u{a}b");
        assert_eq!(printable("x\u{202e}y"), "x\\u{202e}y");
    }

    #[test]
    fn table_cells_are_escaped() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &[Column::left("Name")],
            &[vec!["a\u{7}b".to_owned()]],
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "  Name\n  a\\u{7}b\n");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 KiB");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(64 * 1024 * 1024), "64.0 MiB");
        assert_eq!(size(31_914_983_424), "29.7 GiB");
        assert_eq!(size(u64::MAX), "16384.0 PiB");
    }

    #[test]
    fn exact_sizes() {
        assert_eq!(exact_size(4 << 20), "4 MiB");
        assert_eq!(exact_size(16 << 10), "16 KiB");
        assert_eq!(exact_size(1_080_832), "1080832 bytes");
        assert_eq!(exact_size(0), "0 bytes");
    }

    #[test]
    fn table_alignment() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &[Column::right("#"), Column::left("Name")],
            &[
                vec!["1".into(), "a".into()],
                vec!["10".into(), String::new()],
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "   #  Name\n   1  a\n  10\n"
        );
    }

    #[test]
    fn empty_optional_column_is_hidden() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &[Column::right("#"), Column::left("Name").hidden_when_empty()],
            &[vec!["1".into(), String::new()]],
        )
        .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "  #\n  1\n");
    }
}
