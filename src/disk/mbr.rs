//! Master boot record and extended boot record parsing.
//!
//! The MBR is sector 0 of the disk: boot code, a 32-bit disk signature, four
//! 16-byte partition entries, and the `0x55AA` signature. An extended
//! partition holds a chain of extended boot records (EBRs) with the same
//! layout, each describing one logical partition and pointing to the next
//! record.

use super::le_u32;
use crate::model::{Lba, MbrType};

pub const SECTOR_LEN: usize = 512;

const DISK_SIGNATURE_OFFSET: usize = 440;
const ENTRIES_OFFSET: usize = 446;
const ENTRY_LEN: usize = 16;
const SIGNATURE_OFFSET: usize = 510;
const SIGNATURE: [u8; 2] = [0x55, 0xAA];
const STATUS_BOOTABLE: u8 = 0x80;

/// Offsets within a partition entry.
const ENTRY_STATUS: usize = 0;
const ENTRY_TYPE: usize = 4;
const ENTRY_START: usize = 8;
const ENTRY_SECTORS: usize = 12;

/// A parsed MBR or EBR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootRecord {
    pub disk_signature: u32,
    pub entries: [Entry; 4],
}

/// One partition entry. CHS addresses are ignored: everything since the
/// 1990s uses the LBA fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub status: u8,
    pub partition_type: MbrType,
    /// First sector: absolute in the MBR; in an EBR, relative to the EBR for
    /// the logical partition and to the extended partition for the next EBR.
    pub start: u32,
    pub sectors: u32,
}

impl Entry {
    #[must_use]
    pub fn is_used(&self) -> bool {
        self.partition_type != MbrType::EMPTY
    }

    #[must_use]
    pub fn is_bootable(&self) -> bool {
        self.status == STATUS_BOOTABLE
    }
}

impl BootRecord {
    /// Parses a sector with the `0x55AA` signature; `None` without it.
    #[must_use]
    pub fn parse(sector: &[u8; SECTOR_LEN]) -> Option<Self> {
        if sector[SIGNATURE_OFFSET..] != SIGNATURE {
            return None;
        }
        let entry = |i: usize| {
            let at = ENTRIES_OFFSET + i * ENTRY_LEN;
            Entry {
                status: sector[at + ENTRY_STATUS],
                partition_type: MbrType(sector[at + ENTRY_TYPE]),
                start: le_u32(sector, at + ENTRY_START),
                sectors: le_u32(sector, at + ENTRY_SECTORS),
            }
        };
        Some(Self {
            disk_signature: le_u32(sector, DISK_SIGNATURE_OFFSET),
            entries: [entry(0), entry(1), entry(2), entry(3)],
        })
    }

    /// Whether every status byte is one the specification allows (0x00 or
    /// 0x80). Boot code that merely ends in `0x55AA` usually fails this.
    #[must_use]
    pub fn has_valid_status(&self) -> bool {
        self.entries
            .iter()
            .all(|e| e.status == 0 || e.status == STATUS_BOOTABLE)
    }

    pub fn used_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_used())
    }

    /// The 0xEE entry that announces a GPT, if any.
    #[must_use]
    pub fn gpt_protective_entry(&self) -> Option<&Entry> {
        self.used_entries()
            .find(|e| e.partition_type == MbrType::GPT_PROTECTIVE)
    }

    /// The last sector the 0xEE entry covers: where the backup GPT header
    /// sits on the disk the table was made for.
    #[must_use]
    pub fn gpt_protective_end(&self) -> Option<Lba> {
        self.gpt_protective_entry()
            .filter(|e| e.sectors > 0)
            .map(|e| Lba(u64::from(e.start) + u64::from(e.sectors) - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fixture_image;

    fn sector0(image: &[u8]) -> [u8; SECTOR_LEN] {
        image[..SECTOR_LEN].try_into().unwrap()
    }

    fn sector_with_entry(slot: usize, status: u8, kind: u8, start: u32, sectors: u32) -> [u8; 512] {
        let mut s = [0u8; 512];
        let at = ENTRIES_OFFSET + slot * ENTRY_LEN;
        s[at] = status;
        s[at + 4] = kind;
        s[at + 8..at + 12].copy_from_slice(&start.to_le_bytes());
        s[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
        s[510..].copy_from_slice(&SIGNATURE);
        s
    }

    #[test]
    fn parses_macos_fat32_mbr() {
        let record = BootRecord::parse(&sector0(&fixture_image("mbr-fat32"))).unwrap();
        assert!(record.has_valid_status());
        let [first, rest @ ..] = record.entries;
        assert_eq!(first.partition_type, MbrType(0x0B));
        assert_eq!(first.start, 63);
        assert_eq!(first.sectors, 130_977);
        assert!(!first.is_bootable());
        assert!(rest.iter().all(|e| !e.is_used()));
    }

    #[test]
    fn parses_boot_flag() {
        let record = BootRecord::parse(&sector0(&fixture_image("mbr-sd-aligned"))).unwrap();
        assert!(record.entries[0].is_bootable());
        assert_eq!(record.entries[0].start, 8192);
        assert_eq!(record.entries[1].partition_type, MbrType(0x83));
    }

    #[test]
    fn protective_entry_end() {
        let image = fixture_image("gpt-exfat");
        let record = BootRecord::parse(&sector0(&image)).unwrap();
        assert_eq!(
            record.gpt_protective_end(),
            Some(Lba(image.len() as u64 / 512 - 1))
        );
        let plain = BootRecord::parse(&sector0(&fixture_image("mbr-fat32"))).unwrap();
        assert_eq!(plain.gpt_protective_entry(), None);
    }

    #[test]
    fn no_signature_is_none() {
        assert_eq!(BootRecord::parse(&[0; 512]), None);
    }

    #[test]
    fn invalid_status_detected() {
        let record = BootRecord::parse(&sector_with_entry(1, 0x12, 0x83, 1, 1)).unwrap();
        assert!(!record.has_valid_status());
    }
}
