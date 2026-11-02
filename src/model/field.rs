//! A profile value together with where it came from, or the reason it is
//! missing.

use std::fmt;

use serde::ser::{Serialize, SerializeMap, Serializer};

/// Where a value was read from.
///
/// Serialized in snake case (`"partition_table"`). Later sources (the I/O
/// Registry, `/sys`, card registers) are added as the parts that read them
/// arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The partition table sectors on the disk: MBR, extended boot records,
    /// GPT headers and entries.
    PartitionTable,
    /// The first sector of a FAT, exFAT or NTFS volume, or the first sectors
    /// of any volume when probing what it holds.
    BootSector,
    /// The root directory of a FAT or exFAT volume (its volume label entry).
    RootDirectory,
    /// The file allocation table of a FAT volume.
    AllocationTable,
    /// The allocation bitmap of an exFAT volume.
    AllocationBitmap,
    /// The superblock of an ext2, ext3 or ext4 volume.
    Superblock,
    /// The block group descriptors of an ext2, ext3 or ext4 volume.
    GroupDescriptors,
}

/// Why a value is missing from the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// The target is a disk image, which has no card and no reader.
    DiskImage,
    /// The operating system does not expose this value.
    NotExposedByOs,
    /// The card sits behind a USB reader, which hides it from the OS.
    HiddenByUsbReader,
    /// The card does not support the feature this value describes.
    NotSupportedByCard,
    /// Reading the value failed; the text says how.
    ReadFailed(String),
}

impl Unavailable {
    /// Stable identifier used in the JSON output.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::DiskImage => "disk_image",
            Self::NotExposedByOs => "not_exposed_by_os",
            Self::HiddenByUsbReader => "hidden_by_usb_reader",
            Self::NotSupportedByCard => "not_supported_by_card",
            Self::ReadFailed(_) => "read_failed",
        }
    }
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiskImage => f.write_str("the target is a disk image"),
            Self::NotExposedByOs => f.write_str("not exposed by this operating system"),
            Self::HiddenByUsbReader => f.write_str("hidden by the USB card reader"),
            Self::NotSupportedByCard => f.write_str("not supported by the card"),
            Self::ReadFailed(detail) => write!(f, "read failed: {detail}"),
        }
    }
}

/// A value with its source, or the reason it is unavailable.
///
/// JSON shape, the same for every field:
///
/// ```json
/// {"value": ..., "source": "partition_table"}
/// {"value": null, "unavailable": "read_failed", "detail": "..."}
/// ```
///
/// `detail` is present only for `read_failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field<T> {
    Known { value: T, source: Source },
    Unavailable(Unavailable),
}

impl<T> Field<T> {
    #[must_use]
    pub fn known(value: T, source: Source) -> Self {
        Self::Known { value, source }
    }
}

impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known { value, source } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("value", value)?;
                map.serialize_entry("source", source)?;
                map.end()
            }
            Self::Unavailable(reason) => {
                let detail = match reason {
                    Unavailable::ReadFailed(detail) => Some(detail),
                    _ => None,
                };
                let mut map = serializer.serialize_map(Some(2 + usize::from(detail.is_some())))?;
                map.serialize_entry("value", &())?;
                map.serialize_entry("unavailable", reason.code())?;
                if let Some(detail) = detail {
                    map.serialize_entry("detail", detail)?;
                }
                map.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_serializes_value_and_source() {
        let field = Field::known(42, Source::PartitionTable);
        assert_eq!(
            serde_json::to_string(&field).unwrap(),
            r#"{"value":42,"source":"partition_table"}"#
        );
    }

    #[test]
    fn unavailable_serializes_null_with_reason() {
        let field: Field<u32> = Field::Unavailable(Unavailable::HiddenByUsbReader);
        assert_eq!(
            serde_json::to_string(&field).unwrap(),
            r#"{"value":null,"unavailable":"hidden_by_usb_reader"}"#
        );
    }

    #[test]
    fn read_failure_carries_detail() {
        let field: Field<u32> = Field::Unavailable(Unavailable::ReadFailed("timeout".into()));
        assert_eq!(
            serde_json::to_string(&field).unwrap(),
            r#"{"value":null,"unavailable":"read_failed","detail":"timeout"}"#
        );
    }
}
