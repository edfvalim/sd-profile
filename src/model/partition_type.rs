//! Partition type identifiers and their well-known names.

use serde::ser::{Serialize, SerializeMap, Serializer};

use super::guid::Guid;

/// An MBR partition type ID (the "system ID" byte of a partition entry).
///
/// Serialized as `{"id": "0x0c", "name": "FAT32 (LBA)"}`; `name` is `null`
/// for IDs not in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbrType(pub u8);

impl MbrType {
    pub const EMPTY: Self = Self(0x00);
    pub const GPT_PROTECTIVE: Self = Self(0xEE);

    /// Whether this type marks an extended partition, which holds logical
    /// partitions instead of data.
    #[must_use]
    pub fn is_extended(self) -> bool {
        matches!(self.0, 0x05 | 0x0F | 0x85)
    }

    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        Some(match self.0 {
            0x01 => "FAT12",
            0x04 => "FAT16 (under 32 MiB)",
            0x05 => "Extended (CHS)",
            0x06 => "FAT16",
            0x07 => "NTFS or exFAT",
            0x0B => "FAT32 (CHS)",
            0x0C => "FAT32 (LBA)",
            0x0E => "FAT16 (LBA)",
            0x0F => "Extended (LBA)",
            0x11 => "Hidden FAT12",
            0x14 => "Hidden FAT16 (under 32 MiB)",
            0x16 => "Hidden FAT16",
            0x17 => "Hidden NTFS or exFAT",
            0x1B => "Hidden FAT32 (CHS)",
            0x1C => "Hidden FAT32 (LBA)",
            0x1E => "Hidden FAT16 (LBA)",
            0x82 => "Linux swap",
            0x83 => "Linux",
            0x85 => "Linux extended",
            0x8E => "Linux LVM",
            0xA5 => "FreeBSD",
            0xA6 => "OpenBSD",
            0xA9 => "NetBSD",
            0xAB => "Apple boot",
            0xAF => "Apple HFS+",
            0xEE => "GPT protective",
            0xEF => "EFI system",
            0xFD => "Linux RAID",
            _ => return None,
        })
    }
}

impl Serialize for MbrType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("id", &format!("{:#04x}", self.0))?;
        map.serialize_entry("name", &self.name())?;
        map.end()
    }
}

/// A GPT partition type GUID.
///
/// Serialized as `{"guid": "...", "name": "EFI system"}`; `name` is `null`
/// for GUIDs not in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GptType(pub Guid);

/// Known GPT partition type GUIDs. Sources: the UEFI specification, the
/// Discoverable Partitions Specification, and Apple, Microsoft and FreeBSD
/// documentation.
const GPT_TYPES: &[(Guid, &str)] = &[
    (
        Guid::parse_const("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"),
        "EFI system",
    ),
    (
        Guid::parse_const("024DEE41-33E7-11D3-9D69-0008C781F39F"),
        "MBR partition scheme",
    ),
    (
        Guid::parse_const("21686148-6449-6E6F-744E-656564454649"),
        "BIOS boot",
    ),
    (
        Guid::parse_const("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"),
        "Microsoft basic data",
    ),
    (
        Guid::parse_const("E3C9E316-0B5C-4DB8-817D-F92DF00215AE"),
        "Microsoft reserved",
    ),
    (
        Guid::parse_const("DE94BBA4-06D1-4D40-A16A-BFD50179D6AC"),
        "Windows recovery",
    ),
    (
        Guid::parse_const("0FC63DAF-8483-4772-8E79-3D69D8477DE4"),
        "Linux filesystem",
    ),
    (
        Guid::parse_const("0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"),
        "Linux swap",
    ),
    (
        Guid::parse_const("E6D6D379-F507-44C2-A23C-238F2A3DF928"),
        "Linux LVM",
    ),
    (
        Guid::parse_const("A19D880F-05FC-4D3B-A006-743F0F84911E"),
        "Linux RAID",
    ),
    (
        Guid::parse_const("933AC7E1-2EB4-4F13-B844-0E14E2AEF915"),
        "Linux home",
    ),
    (
        Guid::parse_const("BC13C2FF-59E6-4262-A352-B275FD6F7172"),
        "Linux extended boot",
    ),
    (
        Guid::parse_const("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709"),
        "Linux root (x86-64)",
    ),
    (
        Guid::parse_const("B921B045-1DF0-41C3-AF44-4C6F280D3FAE"),
        "Linux root (ARM64)",
    ),
    (
        Guid::parse_const("69DAD710-2CE4-4E3C-B16C-21A1D49ABED3"),
        "Linux root (ARM)",
    ),
    (
        Guid::parse_const("48465300-0000-11AA-AA11-00306543ECAC"),
        "Apple HFS+",
    ),
    (
        Guid::parse_const("7C3457EF-0000-11AA-AA11-00306543ECAC"),
        "Apple APFS",
    ),
    (
        Guid::parse_const("426F6F74-0000-11AA-AA11-00306543ECAC"),
        "Apple boot",
    ),
    (
        Guid::parse_const("516E7CB4-6ECF-11D6-8FF8-00022D09712B"),
        "FreeBSD data",
    ),
    (
        Guid::parse_const("516E7CBA-6ECF-11D6-8FF8-00022D09712B"),
        "FreeBSD ZFS",
    ),
    (
        Guid::parse_const("6A898CC3-1DD2-11B2-99A6-080020736631"),
        "ZFS (Solaris or Apple)",
    ),
    (
        Guid::parse_const("FE3A2A5D-4F32-41A7-B725-ACCC3285A309"),
        "ChromeOS kernel",
    ),
    (
        Guid::parse_const("3CB8E202-3B7E-47DD-8A3C-7FF2A13CFCEC"),
        "ChromeOS root",
    ),
];

impl GptType {
    pub const MICROSOFT_BASIC_DATA: Self =
        Self(Guid::parse_const("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"));

    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        GPT_TYPES
            .iter()
            .find(|(guid, _)| *guid == self.0)
            .map(|(_, name)| *name)
    }
}

impl Serialize for GptType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("guid", &self.0)?;
        map.serialize_entry("name", &self.name())?;
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mbr_type_serializes_id_and_name() {
        assert_eq!(
            serde_json::to_string(&MbrType(0x0C)).unwrap(),
            r#"{"id":"0x0c","name":"FAT32 (LBA)"}"#
        );
        assert_eq!(
            serde_json::to_string(&MbrType(0x42)).unwrap(),
            r#"{"id":"0x42","name":null}"#
        );
    }

    #[test]
    fn extended_types() {
        for id in [0x05, 0x0F, 0x85] {
            assert!(MbrType(id).is_extended());
        }
        assert!(!MbrType(0x0C).is_extended());
    }

    #[test]
    fn gpt_type_names() {
        assert_eq!(
            GptType::MICROSOFT_BASIC_DATA.name(),
            Some("Microsoft basic data")
        );
        assert_eq!(GptType(Guid::NIL).name(), None);
    }

    #[test]
    fn gpt_type_table_has_no_duplicates() {
        for (i, (a, _)) in GPT_TYPES.iter().enumerate() {
            assert!(
                GPT_TYPES[i + 1..].iter().all(|(b, _)| a != b),
                "duplicate GPT type {a}"
            );
        }
    }
}
