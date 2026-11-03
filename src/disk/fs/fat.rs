//! FAT12, FAT16 and FAT32.
//!
//! A FAT volume starts with a boot sector holding the BIOS parameter block
//! (BPB), followed by the reserved sectors (on FAT32 these hold the `FSInfo`
//! sector and a backup boot sector), the file allocation tables, the fixed
//! root directory (FAT12 and FAT16 only), and the data area of clusters
//! numbered from 2. Source: Microsoft, "FAT: General Overview of On-Disk
//! Format", version 1.03.

use super::{Volume, field};
use crate::disk::block::{BlockReader, ReadError};
use crate::disk::{le_u16, le_u32};
use crate::model::{Fat, Fat32Sectors, Field, Filesystem, FsIssue, Source, VolumeSerial};

/// Offsets within the boot sector.
mod bpb {
    use std::ops::Range;

    pub const OEM_NAME: Range<usize> = 3..11;
    pub const BYTES_PER_SECTOR: usize = 11;
    pub const SECTORS_PER_CLUSTER: usize = 13;
    pub const RESERVED_SECTORS: usize = 14;
    pub const FAT_COUNT: usize = 16;
    pub const ROOT_ENTRIES: usize = 17;
    pub const TOTAL_SECTORS_16: usize = 19;
    pub const MEDIA: usize = 21;
    pub const FAT_SECTORS_16: usize = 22;
    pub const TOTAL_SECTORS_32: usize = 32;

    /// Extended BPB of FAT12 and FAT16.
    pub const BOOT_SIGNATURE_16: usize = 38;
    /// The FAT32 BPB inserts 28 bytes before the extended BPB fields.
    pub const FAT32_SHIFT: usize = 28;
    pub const FAT_SECTORS_32: usize = 36;
    pub const EXT_FLAGS: usize = 40;
    pub const ROOT_CLUSTER: usize = 44;
    pub const FSINFO_SECTOR: usize = 48;
    pub const BACKUP_BOOT_SECTOR: usize = 50;

    /// Relative to the boot signature byte.
    pub const VOLUME_ID: usize = 1;
    pub const VOLUME_LABEL: Range<usize> = 5..16;
    pub const FS_TYPE: Range<usize> = 16..24;
}

/// Extended boot signature: volume ID, label and type string follow.
const EXTENDED_BOOT_SIGNATURE: u8 = 0x29;
/// Older extended boot signature: only the volume ID follows.
const EXTENDED_BOOT_SIGNATURE_ID_ONLY: u8 = 0x28;

/// Volumes with the FAT12/16 BPB layout and fewer clusters than this are
/// FAT12, the rest FAT16 (the specification's cluster count rule).
const MAX_FAT12_CLUSTERS: u32 = 4085;

const DIR_ENTRY_LEN: usize = 32;
/// Root directory entries scanned for the volume label. Formatters write the
/// label first; this only bounds the read on a huge FAT32 root directory.
const MAX_ROOT_ENTRIES_SCANNED: usize = 65536;
/// Bytes of the allocation table read at once while counting free clusters.
const FAT_CHUNK: usize = 1 << 20;

/// The label formatters write when the volume has none.
const NO_NAME: &[u8; 11] = b"NO NAME    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

impl FatType {
    /// Bits per allocation table entry.
    fn entry_bits(self) -> u64 {
        match self {
            Self::Fat12 => 12,
            Self::Fat16 => 16,
            Self::Fat32 => 32,
        }
    }

    /// Bytes to read for one entry. A FAT12 entry spans two bytes it shares
    /// with its neighbour.
    fn read_width(self) -> usize {
        match self {
            Self::Fat12 | Self::Fat16 => 2,
            Self::Fat32 => 4,
        }
    }
}

/// FAT32 fields of the BPB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fat32Fields {
    ext_flags: u16,
    root_cluster: u32,
    fsinfo_sector: u16,
    backup_boot_sector: u16,
}

/// The volume geometry and identity a FAT boot sector describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootSector {
    oem_name: [u8; 8],
    bytes_per_sector: u32,
    sectors_per_cluster: u32,
    reserved_sectors: u16,
    fat_count: u8,
    root_entries: u16,
    total_sectors: u32,
    fat_sectors: u32,
    /// Present when `BPB_FATSz16` is zero: the BPB has the FAT32 layout.
    fat32: Option<Fat32Fields>,
    volume_id: Option<u32>,
    label: Option<[u8; 11]>,
    type_string: Option<[u8; 8]>,
    /// Bit 0 of `BS_Reserved1`, the byte before the boot signature: the
    /// dirty flag Linux sets while mounted read-write and Windows also uses.
    boot_dirty: bool,
}

impl BootSector {
    /// Parses a boot sector whose BPB describes a usable volume; `None` when
    /// it doesn't. `sector` holds at least 512 bytes.
    #[must_use]
    pub fn parse(sector: &[u8]) -> Option<Self> {
        let jump = (sector[0] == 0xEB && sector[2] == 0x90) || sector[0] == 0xE9;
        let bytes_per_sector = u32::from(le_u16(sector, bpb::BYTES_PER_SECTOR));
        let sectors_per_cluster = u32::from(sector[bpb::SECTORS_PER_CLUSTER]);
        let reserved_sectors = le_u16(sector, bpb::RESERVED_SECTORS);
        let fat_count = sector[bpb::FAT_COUNT];
        let media = sector[bpb::MEDIA];
        let total_sectors = match le_u16(sector, bpb::TOTAL_SECTORS_16) {
            0 => le_u32(sector, bpb::TOTAL_SECTORS_32),
            n => u32::from(n),
        };
        let fat_sectors_16 = le_u16(sector, bpb::FAT_SECTORS_16);
        let fat_sectors = match fat_sectors_16 {
            0 => le_u32(sector, bpb::FAT_SECTORS_32),
            n => u32::from(n),
        };
        let valid = jump
            && matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096)
            && sectors_per_cluster.is_power_of_two()
            && sectors_per_cluster <= 128
            && reserved_sectors >= 1
            && (1..=4).contains(&fat_count)
            && (media == 0xF0 || media >= 0xF8)
            && total_sectors > 0
            && fat_sectors > 0;
        if !valid {
            return None;
        }

        let fat32 = (fat_sectors_16 == 0).then(|| Fat32Fields {
            ext_flags: le_u16(sector, bpb::EXT_FLAGS),
            root_cluster: le_u32(sector, bpb::ROOT_CLUSTER),
            fsinfo_sector: le_u16(sector, bpb::FSINFO_SECTOR),
            backup_boot_sector: le_u16(sector, bpb::BACKUP_BOOT_SECTOR),
        });
        let signature_at =
            bpb::BOOT_SIGNATURE_16 + if fat32.is_some() { bpb::FAT32_SHIFT } else { 0 };
        let field = |range: std::ops::Range<usize>| {
            &sector[signature_at + range.start..signature_at + range.end]
        };
        let signature = sector[signature_at];
        let has_id = matches!(
            signature,
            EXTENDED_BOOT_SIGNATURE | EXTENDED_BOOT_SIGNATURE_ID_ONLY
        );
        let has_label = signature == EXTENDED_BOOT_SIGNATURE;

        let boot = Self {
            oem_name: sector[bpb::OEM_NAME]
                .try_into()
                .expect("the OEM name range is 8 bytes"),
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            fat_count,
            root_entries: le_u16(sector, bpb::ROOT_ENTRIES),
            total_sectors,
            fat_sectors,
            fat32,
            volume_id: has_id.then(|| le_u32(sector, signature_at + bpb::VOLUME_ID)),
            label: has_label.then(|| {
                field(bpb::VOLUME_LABEL)
                    .try_into()
                    .expect("the label range is 11 bytes")
            }),
            boot_dirty: has_id && sector[signature_at - 1] & 0x01 != 0,
            type_string: has_label.then(|| {
                field(bpb::FS_TYPE)
                    .try_into()
                    .expect("the type range is 8 bytes")
            }),
        };
        boot.cluster_count().filter(|&n| n > 0)?;
        Some(boot)
    }

    /// Whether the boot sector names its FAT type (`FAT12   `, `FAT16   `,
    /// `FAT32   `, or just `FAT     `). Used to tell a FAT boot sector at
    /// sector 0 from an MBR, where a sane-looking BPB alone is weak evidence.
    #[must_use]
    pub fn has_type_string(&self) -> bool {
        self.type_string.is_some_and(|t| t.starts_with(b"FAT"))
    }

    fn root_dir_sectors(&self) -> u32 {
        (u32::from(self.root_entries) * 32).div_ceil(self.bytes_per_sector)
    }

    /// First sector of the data area (cluster 2), from the volume start.
    fn data_start_sector(&self) -> u64 {
        u64::from(self.reserved_sectors)
            + u64::from(self.fat_count) * u64::from(self.fat_sectors)
            + u64::from(self.root_dir_sectors())
    }

    fn cluster_count(&self) -> Option<u32> {
        let data_sectors = u64::from(self.total_sectors).checked_sub(self.data_start_sector())?;
        u32::try_from(data_sectors / u64::from(self.sectors_per_cluster)).ok()
    }

    /// The FAT type. The FAT32 layout of the BPB (`BPB_FATSz16` zero) means
    /// FAT32, as Linux, macOS and `blkid` decide; otherwise the cluster count
    /// tells FAT12 from FAT16, as the specification says.
    #[must_use]
    pub fn fat_type(&self) -> FatType {
        if self.fat32.is_some() {
            return FatType::Fat32;
        }
        match self.cluster_count() {
            Some(n) if n < MAX_FAT12_CLUSTERS => FatType::Fat12,
            _ => FatType::Fat16,
        }
    }

    fn cluster_bytes(&self) -> u32 {
        self.bytes_per_sector * self.sectors_per_cluster
    }

    fn sector_offset(&self, sector: u64) -> u64 {
        sector * u64::from(self.bytes_per_sector)
    }
}

/// The view of one allocation table needed to read entries.
struct Table {
    fat_type: FatType,
    /// Byte offset of the table from the volume start.
    offset: u64,
    /// Entries the table holds, including the two reserved ones.
    entries: u32,
}

impl Table {
    /// The table in use: on FAT32 with mirroring turned off, the one
    /// `BPB_ExtFlags` names; otherwise the first.
    fn active(boot: &BootSector, cluster_count: u32) -> Self {
        let fat_type = boot.fat_type();
        let active = boot
            .fat32
            .filter(|f| f.ext_flags & 0x80 != 0)
            .map_or(0, |f| u64::from(f.ext_flags & 0x0F))
            .min(u64::from(boot.fat_count) - 1);
        let first_sector = u64::from(boot.reserved_sectors) + active * u64::from(boot.fat_sectors);
        let capacity = boot.sector_offset(u64::from(boot.fat_sectors)) * 8 / fat_type.entry_bits();
        let entries = u64::from(cluster_count + 2).min(capacity);
        Self {
            fat_type,
            offset: boot.sector_offset(first_sector),
            entries: u32::try_from(entries).expect("entries is at most cluster_count + 2"),
        }
    }

    fn entry_offset(&self, n: u32) -> u64 {
        match self.fat_type {
            FatType::Fat12 => u64::from(n) + u64::from(n / 2),
            FatType::Fat16 => u64::from(n) * 2,
            FatType::Fat32 => u64::from(n) * 4,
        }
    }

    fn len_bytes(&self) -> u64 {
        match self.fat_type {
            FatType::Fat12 => (u64::from(self.entries) * 3).div_ceil(2),
            _ => self.entry_offset(self.entries),
        }
    }

    /// Entry `n`, decoded from table bytes that start at entry `first`.
    /// FAT32 entries keep only their low 28 bits.
    fn decode(&self, bytes: &[u8], first: u32, n: u32) -> u32 {
        let at = usize::try_from(self.entry_offset(n) - self.entry_offset(first))
            .expect("callers pass entries within the bytes they read");
        match self.fat_type {
            FatType::Fat12 => {
                let pair = u32::from(le_u16(bytes, at));
                if n.is_multiple_of(2) {
                    pair & 0x0FFF
                } else {
                    pair >> 4
                }
            }
            FatType::Fat16 => u32::from(le_u16(bytes, at)),
            FatType::Fat32 => le_u32(bytes, at) & 0x0FFF_FFFF,
        }
    }

    fn read_entry<R: BlockReader>(
        &self,
        volume: &mut Volume<'_, R>,
        n: u32,
    ) -> Result<u32, ReadError> {
        let bytes = volume.read(
            self.offset + self.entry_offset(n),
            self.fat_type.read_width(),
        )?;
        Ok(self.decode(&bytes, n, n))
    }

    /// Counts the clusters marked free (entry value 0).
    fn count_free<R: BlockReader>(&self, volume: &mut Volume<'_, R>) -> Result<u32, ReadError> {
        if self.fat_type == FatType::Fat12 {
            // At most 4086 entries, about 6 KiB: read it whole.
            let len = usize::try_from(self.len_bytes()).expect("a FAT12 table is a few KiB");
            let bytes = volume.read(self.offset, len)?;
            return Ok(u32::try_from(
                (2..self.entries)
                    .filter(|&n| self.decode(&bytes, 0, n) == 0)
                    .count(),
            )
            .expect("the count is at most the number of entries"));
        }
        let width = self.fat_type.read_width();
        let per_chunk = u32::try_from(FAT_CHUNK / width).expect("a chunk holds under 2^32 entries");
        let mut free = 0;
        let mut first = 2;
        while first < self.entries {
            let count = per_chunk.min(self.entries - first);
            let len = usize::try_from(count).expect("count fits a chunk") * width;
            let bytes = volume.read(self.offset + self.entry_offset(first), len)?;
            free += (first..first + count)
                .filter(|&n| self.decode(&bytes, first, n) == 0)
                .count();
            first += count;
        }
        Ok(u32::try_from(free).expect("the count is at most the number of entries"))
    }

    /// Whether `value` links to another cluster of the volume, rather than
    /// ending the chain or marking a free, reserved or bad cluster.
    fn next_cluster(&self, value: u32) -> Option<u32> {
        (2..self.entries).contains(&value).then_some(value)
    }
}

/// Reads the volume the boot sector describes.
pub(super) fn read<R: BlockReader>(volume: &mut Volume<'_, R>, boot: &BootSector) -> Filesystem {
    let cluster_count = boot
        .cluster_count()
        .expect("BootSector::parse checks there are clusters");
    let cluster_bytes = u64::from(boot.cluster_bytes());
    let table = Table::active(boot, cluster_count);
    let mut issues = Vec::new();

    let free_clusters = table.count_free(volume);
    let counted = free_clusters.as_ref().ok().copied();
    let free_bytes = field(
        free_clusters.map(|n| u64::from(n) * cluster_bytes),
        Source::AllocationTable,
    );
    if let (Some(counted), Some(recorded)) = (counted, fsinfo_free(volume, boot, cluster_count))
        && recorded != counted
    {
        issues.push(FsIssue::FreeCountStale {
            recorded_bytes: u64::from(recorded) * cluster_bytes,
            counted_bytes: u64::from(counted) * cluster_bytes,
        });
    }
    let mut state = state_issues(volume, &table);
    if boot.boot_dirty && !state.contains(&FsIssue::NotCleanlyUnmounted) {
        state.insert(0, FsIssue::NotCleanlyUnmounted);
    }
    issues.extend(state);

    issues.extend(volume.size_issue(boot.sector_offset(u64::from(boot.total_sectors))));

    let fat = Fat {
        label: label(volume, boot, &table),
        volume_id: boot.volume_id.map(VolumeSerial),
        oem_name: decode_oem(boot.oem_name),
        sector_size_bytes: boot.bytes_per_sector,
        cluster_size_bytes: boot.cluster_bytes(),
        cluster_count,
        size_bytes: u64::from(cluster_count) * cluster_bytes,
        free_bytes,
        data_offset_bytes: boot.sector_offset(boot.data_start_sector()),
        reserved_sectors: boot.reserved_sectors,
        fat_count: boot.fat_count,
        fat32: boot.fat32.map(|f| Fat32Sectors {
            fsinfo_sector: f.fsinfo_sector,
            backup_boot_sector: f.backup_boot_sector,
        }),
        issues,
    };
    match boot.fat_type() {
        FatType::Fat12 => Filesystem::Fat12(fat),
        FatType::Fat16 => Filesystem::Fat16(fat),
        FatType::Fat32 => Filesystem::Fat32(fat),
    }
}

/// `FSInfo` structure signatures and the free count field.
mod fsinfo {
    pub const LEAD_SIGNATURE: (usize, u32) = (0, 0x4161_5252);
    pub const STRUCT_SIGNATURE: (usize, u32) = (484, 0x6141_7272);
    pub const TRAIL_SIGNATURE: (usize, u32) = (508, 0xAA55_0000);
    pub const FREE_COUNT: usize = 488;
    pub const UNKNOWN: u32 = 0xFFFF_FFFF;
}

/// The free cluster count FAT32 keeps in its `FSInfo` sector, when present and
/// set. It is a hint the driver updates; the table itself is the truth.
fn fsinfo_free<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
    cluster_count: u32,
) -> Option<u32> {
    let sector = boot.fat32?.fsinfo_sector;
    if sector == 0 || sector >= boot.reserved_sectors {
        return None;
    }
    let bytes = volume
        .read(boot.sector_offset(u64::from(sector)), 512)
        .ok()?;
    let signed = [
        fsinfo::LEAD_SIGNATURE,
        fsinfo::STRUCT_SIGNATURE,
        fsinfo::TRAIL_SIGNATURE,
    ]
    .iter()
    .all(|&(at, value)| le_u32(&bytes, at) == value);
    let free = le_u32(&bytes, fsinfo::FREE_COUNT);
    (signed && free != fsinfo::UNKNOWN && free <= cluster_count).then_some(free)
}

/// The clean-shutdown and hard-error bits FAT16 and FAT32 keep in entry 1
/// of the table, which Windows and macOS maintain. A set bit means clean, or no
/// error. FAT12 has neither.
fn state_issues<R: BlockReader>(volume: &mut Volume<'_, R>, table: &Table) -> Vec<FsIssue> {
    let (clean, no_error) = match table.fat_type {
        FatType::Fat12 => return Vec::new(),
        FatType::Fat16 => (0x8000, 0x4000),
        FatType::Fat32 => (0x0800_0000, 0x0400_0000),
    };
    let Ok(entry) = table.read_entry(volume, 1) else {
        return Vec::new();
    };
    let mut issues = Vec::new();
    if entry & clean == 0 {
        issues.push(FsIssue::NotCleanlyUnmounted);
    }
    if entry & no_error == 0 {
        issues.push(FsIssue::ErrorsRecorded);
    }
    issues
}

/// The volume label: the root directory's label entry, else the boot
/// sector's copy, as `blkid` reads it.
fn label<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
    table: &Table,
) -> Field<Option<String>> {
    match root_dir_label(volume, boot, table) {
        Err(err) => field(Err(err), Source::RootDirectory),
        Ok(Some(label)) => Field::known(Some(label), Source::RootDirectory),
        Ok(None) => Field::known(
            boot.label
                .filter(|l| l != NO_NAME)
                .and_then(|l| decode_name(&l)),
            Source::BootSector,
        ),
    }
}

/// Directory entry fields.
mod dir {
    pub const ATTRIBUTES: usize = 11;
    pub const END: u8 = 0x00;
    pub const DELETED: u8 = 0xE5;
    pub const ATTR_VOLUME_ID: u8 = 0x08;
    pub const ATTR_DIRECTORY: u8 = 0x10;
    /// Long file name entries set read-only, hidden, system and volume ID.
    pub const ATTR_LONG_NAME: u8 = 0x0F;
}

fn root_dir_label<R: BlockReader>(
    volume: &mut Volume<'_, R>,
    boot: &BootSector,
    table: &Table,
) -> Result<Option<String>, ReadError> {
    let mut scanned = 0;
    let mut scan = |bytes: &[u8]| -> Option<Option<String>> {
        for entry in bytes.as_chunks::<DIR_ENTRY_LEN>().0 {
            scanned += 1;
            if entry[0] == dir::END || scanned > MAX_ROOT_ENTRIES_SCANNED {
                return Some(None);
            }
            let attributes = entry[dir::ATTRIBUTES];
            let is_label = attributes & dir::ATTR_LONG_NAME != dir::ATTR_LONG_NAME
                && attributes & (dir::ATTR_VOLUME_ID | dir::ATTR_DIRECTORY) == dir::ATTR_VOLUME_ID;
            if entry[0] != dir::DELETED && is_label {
                let name: &[u8; 11] = entry[..11]
                    .try_into()
                    .expect("an entry starts with 11 name bytes");
                return Some(decode_name(name));
            }
        }
        None
    };

    match boot.fat32 {
        None => {
            let start =
                boot.sector_offset(boot.data_start_sector() - u64::from(boot.root_dir_sectors()));
            let len = usize::from(boot.root_entries) * DIR_ENTRY_LEN;
            let bytes = volume.read(start, len)?;
            Ok(scan(&bytes).flatten())
        }
        Some(fat32) => {
            let cluster_bytes = u64::from(boot.cluster_bytes());
            let data_start = boot.sector_offset(boot.data_start_sector());
            let mut cluster = table.next_cluster(fat32.root_cluster);
            let mut steps = 0;
            while let Some(n) = cluster {
                let len = usize::try_from(cluster_bytes).expect("a cluster is at most 512 KiB");
                let bytes = volume.read(data_start + u64::from(n - 2) * cluster_bytes, len)?;
                if let Some(found) = scan(&bytes) {
                    return Ok(found);
                }
                steps += 1;
                if steps > table.entries {
                    break; // A loop in the chain.
                }
                cluster = table.next_cluster(table.read_entry(volume, n)?);
            }
            Ok(None)
        }
    }
}

/// A short (8.3) name or label, trimmed of trailing spaces. `None` when
/// blank.
fn decode_name(name: &[u8; 11]) -> Option<String> {
    let mut bytes = *name;
    if bytes[0] == 0x05 {
        // 0x05 stands for a leading 0xE5, which would mean "deleted".
        bytes[0] = 0xE5;
    }
    // macOS's newfs_msdos writes the label it is given as UTF-8. Bytes in
    // an OEM code page are almost never valid UTF-8, so valid UTF-8 beyond
    // ASCII is taken as such.
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) if !text.is_ascii() && !text.chars().any(char::is_control) => {
            text.trim_end_matches([' ', '\0']).to_owned()
        }
        _ => decode_ascii(&bytes),
    };
    (!text.is_empty()).then_some(text)
}

fn decode_oem(name: [u8; 8]) -> String {
    decode_ascii(&name)
}

/// Text from a padded byte field. The bytes are in an OEM code page the
/// volume doesn't record: printable ASCII is decoded, NUL counts as padding,
/// anything else becomes U+FFFD. Trailing padding is trimmed.
fn decode_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0 => ' ',
            b if b.is_ascii() && !b.is_ascii_control() => char::from(b),
            _ => '\u{FFFD}',
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::fs::tests::{read_volume, sector_of};
    use crate::model::{Fat, Unavailable};

    fn fat(image: &str, start: u64, sectors: u64) -> (FatType, Fat) {
        match read_volume(image, start, sectors) {
            Field::Known {
                value: Filesystem::Fat12(f),
                ..
            } => (FatType::Fat12, f),
            Field::Known {
                value: Filesystem::Fat16(f),
                ..
            } => (FatType::Fat16, f),
            Field::Known {
                value: Filesystem::Fat32(f),
                ..
            } => (FatType::Fat32, f),
            other => panic!("{image} at {start}: {other:?}"),
        }
    }

    fn known<T: std::fmt::Debug>(field: &Field<T>) -> (&T, Source) {
        match field {
            Field::Known { value, source } => (value, *source),
            Field::Unavailable(reason) => panic!("unavailable: {reason}"),
        }
    }

    // Expected values come from fsck_msdos -n and file -s on the fixtures;
    // see the phase 2 oracle notes.

    #[test]
    fn fat12_from_newfs_msdos() {
        let (kind, f) = fat("mbr-fat12-fat16", 2048, 8192);
        assert_eq!(kind, FatType::Fat12);
        assert_eq!(
            known(&f.label),
            (&Some("SMALL".to_owned()), Source::RootDirectory)
        );
        assert_eq!(f.volume_id, Some(VolumeSerial(0x001E_19ED)));
        assert_eq!(f.oem_name, "BSD  4.4");
        assert_eq!(f.cluster_size_bytes, 1024);
        assert_eq!(known(&f.free_bytes).0, &(4067 * 1024));
        assert_eq!(f.fat32, None);
        assert!(f.issues.is_empty(), "{:?}", f.issues);
    }

    #[test]
    fn fat16_from_newfs_msdos() {
        let (kind, f) = fat("mbr-fat12-fat16", 10240, 55296);
        assert_eq!(kind, FatType::Fat16);
        assert_eq!(known(&f.label).0.as_deref(), Some("MEDIUM"));
        assert_eq!(f.volume_id, Some(VolumeSerial(0x40BC_19ED)));
        assert_eq!(f.cluster_size_bytes, 2048);
        assert_eq!(known(&f.free_bytes).0, &(13788 * 2048));
        assert!(f.issues.is_empty(), "{:?}", f.issues);
    }

    #[test]
    fn fat16_from_disk_utility() {
        let (kind, f) = fat("mbr-extended", 78246, 31256);
        assert_eq!(kind, FatType::Fat16);
        assert_eq!(known(&f.label).0.as_deref(), Some("TWO"));
        assert_eq!(f.volume_id, Some(VolumeSerial(0x8A69_19ED)));
        assert_eq!(known(&f.free_bytes).0, &(15546 * 1024));
    }

    #[test]
    fn fat32_with_files() {
        let (kind, f) = fat("mbr-fat32", 63, 130_977);
        assert_eq!(kind, FatType::Fat32);
        assert_eq!(known(&f.label).0.as_deref(), Some("SDTEST"));
        assert_eq!(f.volume_id, Some(VolumeSerial(0x4642_19ED)));
        assert_eq!(f.cluster_size_bytes, 512);
        assert_eq!(known(&f.free_bytes).0, &(122_728 * 512));
        assert_eq!(
            f.fat32,
            Some(Fat32Sectors {
                fsinfo_sector: 1,
                backup_boot_sector: 6
            })
        );
        assert!(f.issues.is_empty(), "{:?}", f.issues);
    }

    #[test]
    fn fat32_on_whole_disk() {
        let (kind, f) = fat("superfloppy-fat32", 0, 131_072);
        assert_eq!(kind, FatType::Fat32);
        assert_eq!(known(&f.label).0.as_deref(), Some("FLOPPY"));
        assert_eq!(f.volume_id, Some(VolumeSerial(0xF26A_19EE)));
        assert_eq!(known(&f.free_bytes).0, &(129_021 * 512));
    }

    #[test]
    fn truncated_image_keeps_what_it_can_read() {
        // The FAT lies inside the first MiB, the root directory beyond it.
        let (_, f) = fat("mbr-fat32-truncated", 63, 130_977);
        assert_eq!(known(&f.free_bytes).0, &(122_728 * 512));
        assert!(matches!(
            f.label,
            Field::Unavailable(Unavailable::ReadFailed(_))
        ));
    }

    #[test]
    fn dirty_bits_are_reported() {
        let mut image = crate::test_support::fixture_image("mbr-fat12-fat16");
        // Clear both bits in FAT16 entry 1 of partition 2. The table starts
        // after the reserved sectors.
        let base = sector_of(10240);
        let reserved = usize::from(le_u16(&image[base..], bpb::RESERVED_SECTORS));
        let entry1 = base + reserved * 512 + 2;
        image[entry1 + 1] &= !0xC0;
        let mut disk = crate::test_support::reader(image);
        let f = match crate::disk::fs::read(&mut disk, crate::disk::fs::tests::extent(10240, 55296))
        {
            Field::Known {
                value: Filesystem::Fat16(f),
                ..
            } => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            f.issues,
            [FsIssue::NotCleanlyUnmounted, FsIssue::ErrorsRecorded]
        );
    }

    #[test]
    fn linux_dirty_flag_is_reported() {
        let mut image = crate::test_support::fixture_image("mbr-fat32");
        // BS_Reserved1 of the FAT32 extended BPB.
        image[sector_of(63) + bpb::BOOT_SIGNATURE_16 + bpb::FAT32_SHIFT - 1] |= 0x01;
        let mut disk = crate::test_support::reader(image);
        let f = match crate::disk::fs::read(&mut disk, crate::disk::fs::tests::extent(63, 130_977))
        {
            Field::Known {
                value: Filesystem::Fat32(f),
                ..
            } => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(f.issues, [FsIssue::NotCleanlyUnmounted]);
    }

    #[test]
    fn stale_fsinfo_is_reported() {
        let mut image = crate::test_support::fixture_image("mbr-fat32");
        let free_count = sector_of(63 + 1) + fsinfo::FREE_COUNT;
        image[free_count..free_count + 4].copy_from_slice(&100u32.to_le_bytes());
        let mut disk = crate::test_support::reader(image);
        let f = match crate::disk::fs::read(&mut disk, crate::disk::fs::tests::extent(63, 130_977))
        {
            Field::Known {
                value: Filesystem::Fat32(f),
                ..
            } => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            f.issues,
            [FsIssue::FreeCountStale {
                recorded_bytes: 100 * 512,
                counted_bytes: 122_728 * 512
            }]
        );
    }

    #[test]
    fn names_decode_and_trim() {
        assert_eq!(decode_name(b"SDTEST     ").as_deref(), Some("SDTEST"));
        assert_eq!(decode_name(b"           "), None);
        assert_eq!(decode_name(b"AB\0\0\0\0\0\0\0\0\0").as_deref(), Some("AB"));
        assert_eq!(
            decode_name(b"AB\xC4        ").as_deref(),
            Some("AB\u{FFFD}")
        );
        assert_eq!(
            decode_name(b"\x05BC        ").as_deref(),
            Some("\u{FFFD}BC")
        );
    }

    #[test]
    fn utf8_labels_decode() {
        // What `newfs_msdos -v CAFÉ` writes on macOS.
        assert_eq!(
            decode_name(b"CAF\xC3\x89      ").as_deref(),
            Some("CAF\u{C9}")
        );
        // cp850 0x90 is É, but alone it isn't UTF-8.
        assert_eq!(
            decode_name(b"CAF\x90       ").as_deref(),
            Some("CAF\u{FFFD}")
        );
    }

    #[test]
    fn mbr_is_not_a_fat_boot_sector() {
        let image = crate::test_support::fixture_image("mbr-fat32");
        assert_eq!(BootSector::parse(&image[..512]), None);
    }
}
