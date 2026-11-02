//! GUID partition table parsing (UEFI specification, chapter 5).
//!
//! The primary header is at LBA 1 and points to an array of partition entries
//! (usually at LBA 2). A backup header and entry array sit at the end of the
//! disk. Both the header and the entry array carry CRC-32 checksums.

use super::{crc32, le_u32, le_u64};
use crate::model::{Extent, GptAttributes, GptHeaderProblem, GptType, Guid, Lba, PartitionNumber};

const SIGNATURE: &[u8; 8] = b"EFI PART";
const MIN_HEADER_SIZE: usize = 92;
const MIN_ENTRY_SIZE: u32 = 128;
/// Far beyond any real table (128 entries of 128 bytes is 16 KiB), small
/// enough that a corrupt header can't make us allocate gigabytes.
const MAX_ENTRY_ARRAY_BYTES: u64 = 1024 * 1024;

/// Header field offsets.
mod header {
    pub const HEADER_SIZE: usize = 12;
    pub const HEADER_CRC: usize = 16;
    pub const MY_LBA: usize = 24;
    pub const ALTERNATE_LBA: usize = 32;
    pub const FIRST_USABLE_LBA: usize = 40;
    pub const LAST_USABLE_LBA: usize = 48;
    pub const DISK_GUID: usize = 56;
    pub const ENTRIES_LBA: usize = 72;
    pub const ENTRY_COUNT: usize = 80;
    pub const ENTRY_SIZE: usize = 84;
    pub const ENTRIES_CRC: usize = 88;
}

/// Entry field offsets.
mod entry {
    pub const TYPE_GUID: usize = 0;
    pub const UNIQUE_GUID: usize = 16;
    pub const FIRST_LBA: usize = 32;
    pub const LAST_LBA: usize = 40;
    pub const ATTRIBUTES: usize = 48;
    /// UTF-16LE, up to 36 code units, padded with zeros.
    pub const NAME: std::ops::Range<usize> = 56..128;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub my_lba: Lba,
    pub alternate_lba: Lba,
    pub first_usable: Lba,
    pub last_usable: Lba,
    pub disk_guid: Guid,
    pub entries_lba: Lba,
    pub entry_count: u32,
    pub entry_size: u32,
    pub entries_checksum: u32,
}

impl Header {
    /// Parses and validates a header read from `lba`.
    ///
    /// # Errors
    ///
    /// The first problem found, checked in the order the specification
    /// lists them: signature, size, checksum, location, entry array shape.
    pub fn parse(block: &[u8], lba: Lba) -> Result<Self, GptHeaderProblem> {
        if block.len() < MIN_HEADER_SIZE || &block[..SIGNATURE.len()] != SIGNATURE {
            return Err(GptHeaderProblem::Missing);
        }
        let header_size = le_u32(block, header::HEADER_SIZE) as usize;
        if !(MIN_HEADER_SIZE..=block.len()).contains(&header_size) {
            return Err(GptHeaderProblem::BadHeaderSize);
        }
        // The checksum covers the header with its own checksum field zeroed.
        let mut bytes = block[..header_size].to_vec();
        bytes[header::HEADER_CRC..header::HEADER_CRC + 4].fill(0);
        if crc32::checksum(&bytes) != le_u32(block, header::HEADER_CRC) {
            return Err(GptHeaderProblem::BadHeaderChecksum);
        }
        if Lba(le_u64(block, header::MY_LBA)) != lba {
            return Err(GptHeaderProblem::WrongLocation);
        }
        let entry_count = le_u32(block, header::ENTRY_COUNT);
        let entry_size = le_u32(block, header::ENTRY_SIZE);
        let array_bytes = u64::from(entry_count) * u64::from(entry_size);
        if entry_size < MIN_ENTRY_SIZE
            || !entry_size.is_power_of_two()
            || array_bytes > MAX_ENTRY_ARRAY_BYTES
        {
            return Err(GptHeaderProblem::BadEntryArray);
        }
        Ok(Self {
            my_lba: lba,
            alternate_lba: Lba(le_u64(block, header::ALTERNATE_LBA)),
            first_usable: Lba(le_u64(block, header::FIRST_USABLE_LBA)),
            last_usable: Lba(le_u64(block, header::LAST_USABLE_LBA)),
            disk_guid: guid_at(block, header::DISK_GUID),
            entries_lba: Lba(le_u64(block, header::ENTRIES_LBA)),
            entry_count,
            entry_size,
            entries_checksum: le_u32(block, header::ENTRIES_CRC),
        })
    }

    /// Length of the entry array in bytes.
    #[must_use]
    pub fn entry_array_len(&self) -> usize {
        // Bounded by MAX_ENTRY_ARRAY_BYTES in `parse`.
        self.entry_count as usize * self.entry_size as usize
    }

    /// Whether two header copies describe the same table. The fields that
    /// differ by design (own and alternate LBA, entry array location) are
    /// left out.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.disk_guid == other.disk_guid
            && self.first_usable == other.first_usable
            && self.last_usable == other.last_usable
            && self.entry_count == other.entry_count
            && self.entry_size == other.entry_size
            && self.entries_checksum == other.entries_checksum
    }

    /// Whether the extent lies within the usable sectors. Empty extents
    /// count as inside; they are reported on their own.
    #[must_use]
    pub fn is_usable(&self, extent: &Extent) -> bool {
        extent
            .last()
            .is_none_or(|last| extent.start() >= self.first_usable && last <= self.last_usable)
    }

    /// Parses the used entries of the entry array, with their slot numbers.
    ///
    /// # Errors
    ///
    /// [`GptHeaderProblem::BadEntriesChecksum`] if the array doesn't match the
    /// header's checksum.
    pub fn parse_entries(
        &self,
        array: &[u8],
    ) -> Result<Vec<(PartitionNumber, Entry)>, GptHeaderProblem> {
        if array.len() != self.entry_array_len() || crc32::checksum(array) != self.entries_checksum
        {
            return Err(GptHeaderProblem::BadEntriesChecksum);
        }
        Ok(array
            .chunks_exact(self.entry_size as usize)
            .zip(1..)
            .filter_map(|(raw, slot)| Entry::parse(raw).map(|e| (PartitionNumber(slot), e)))
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub partition_type: GptType,
    pub guid: Guid,
    pub first: Lba,
    /// Inclusive.
    pub last: Lba,
    pub attributes: GptAttributes,
    pub name: String,
}

impl Entry {
    /// Parses one entry; `None` for an unused slot (nil type GUID).
    fn parse(raw: &[u8]) -> Option<Self> {
        let partition_type = guid_at(raw, entry::TYPE_GUID);
        if partition_type.is_nil() {
            return None;
        }
        let name: Vec<u16> = raw[entry::NAME]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&unit| u16::from_le_bytes(unit))
            .take_while(|&unit| unit != 0)
            .collect();
        Some(Self {
            partition_type: GptType(partition_type),
            guid: guid_at(raw, entry::UNIQUE_GUID),
            first: Lba(le_u64(raw, entry::FIRST_LBA)),
            last: Lba(le_u64(raw, entry::LAST_LBA)),
            attributes: GptAttributes(le_u64(raw, entry::ATTRIBUTES)),
            name: String::from_utf16_lossy(&name),
        })
    }

    /// Number of sectors from `first` to `last` inclusive; 0 when `last`
    /// comes before `first`.
    #[must_use]
    pub fn sectors(&self) -> u64 {
        if self.last >= self.first {
            self.last.0 - self.first.0 + 1
        } else {
            0
        }
    }
}

fn guid_at(bytes: &[u8], at: usize) -> Guid {
    Guid::from_mixed_endian(
        bytes[at..at + 16]
            .try_into()
            .expect("a 16-byte slice converts to [u8; 16]"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture_image;

    fn primary(image: &[u8]) -> Header {
        Header::parse(&image[512..1024], Lba(1)).unwrap()
    }

    #[test]
    fn parses_primary_header() {
        let image = fixture_image("gpt-multi");
        let header = primary(&image);
        let last = Lba(image.len() as u64 / 512 - 1);
        assert_eq!(header.alternate_lba, last);
        assert_eq!(header.first_usable, Lba(34));
        assert_eq!(header.last_usable, Lba(last.0 - 33));
        assert_eq!(header.entries_lba, Lba(2));
        assert_eq!(header.entry_count, 128);
        assert_eq!(header.entry_size, 128);
    }

    #[test]
    fn parses_entries() {
        let image = fixture_image("gpt-multi");
        let header = primary(&image);
        let array = &image[1024..1024 + header.entry_array_len()];
        let entries = header.parse_entries(array).unwrap();
        assert_eq!(entries.len(), 2);
        let (number, first) = &entries[0];
        assert_eq!(*number, PartitionNumber(1));
        assert_eq!(first.partition_type, GptType::MICROSOFT_BASIC_DATA);
        assert_eq!(first.first, Lba(2048));
        assert!(first.last > first.first);
        assert_eq!(entries[1].0, PartitionNumber(2));
        assert!(entries[1].1.first > first.last);
    }

    #[test]
    fn backup_header_matches_primary() {
        let image = fixture_image("gpt-multi");
        let header = primary(&image);
        let at = usize::try_from(header.alternate_lba.0).unwrap() * 512;
        let backup = Header::parse(&image[at..at + 512], header.alternate_lba).unwrap();
        assert!(header.matches(&backup));
        assert_eq!(backup.alternate_lba, Lba(1));
    }

    #[test]
    fn rejects_corrupt_header() {
        let image = fixture_image("gpt-exfat");
        let mut block = image[512..1024].to_vec();
        block[60] ^= 0xFF;
        assert_eq!(
            Header::parse(&block, Lba(1)),
            Err(GptHeaderProblem::BadHeaderChecksum)
        );
    }

    #[test]
    fn rejects_wrong_location() {
        let image = fixture_image("gpt-exfat");
        assert_eq!(
            Header::parse(&image[512..1024], Lba(2)),
            Err(GptHeaderProblem::WrongLocation)
        );
    }

    #[test]
    fn rejects_missing_signature() {
        assert_eq!(
            Header::parse(&[0; 512], Lba(1)),
            Err(GptHeaderProblem::Missing)
        );
    }

    #[test]
    fn rejects_corrupt_entries() {
        let image = fixture_image("gpt-exfat");
        let header = primary(&image);
        let mut array = image[1024..1024 + header.entry_array_len()].to_vec();
        array[40] ^= 1;
        assert_eq!(
            header.parse_entries(&array),
            Err(GptHeaderProblem::BadEntriesChecksum)
        );
    }

    #[test]
    fn reversed_entry_has_no_sectors() {
        let mut raw = [0u8; 128];
        raw[0] = 1; // non-nil type
        raw[entry::FIRST_LBA] = 10;
        raw[entry::LAST_LBA] = 5;
        assert_eq!(Entry::parse(&raw).unwrap().sectors(), 0);
        raw[entry::LAST_LBA] = 10;
        assert_eq!(Entry::parse(&raw).unwrap().sectors(), 1);
    }

    #[test]
    fn usable_range() {
        let image = fixture_image("gpt-exfat");
        let header = primary(&image);
        let extent = |start, sectors| Extent::new(Lba(start), sectors, crate::model::BlockSize::SD);
        assert!(header.is_usable(&extent(34, 10)));
        assert!(!header.is_usable(&extent(33, 10)));
        assert!(!header.is_usable(&extent(header.last_usable.0, 2)));
        assert!(header.is_usable(&extent(0, 0)));
    }

    #[test]
    fn decodes_utf16_name() {
        let mut raw = [0u8; 128];
        raw[0] = 1; // non-nil type
        for (i, unit) in "Bóot".encode_utf16().enumerate() {
            raw[56 + 2 * i..58 + 2 * i].copy_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(Entry::parse(&raw).unwrap().name, "Bóot");
    }
}
