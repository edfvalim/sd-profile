//! Mounted filesystems on the card, and the names the OS gives the card's
//! disk and partitions.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use super::layout::PartitionNumber;

/// The OS's name for a disk or a partition on it: `disk6` and `disk6s1` on
/// macOS, `mmcblk0` and `mmcblk0p1` or `sdb` and `sdb1` on Linux. The device
/// nodes are under `/dev`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceName(String);

impl DeviceName {
    /// `None` for an empty name or one with a `/` in it.
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        (!name.is_empty() && !name.contains('/')).then(|| Self(name.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The device node, `/dev/<name>`.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        PathBuf::from("/dev").join(&self.0)
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for DeviceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// The whole disk the OS made for a card.
///
/// Serialized as `{"name": "disk6", "path": "/dev/disk6", "raw_path":
/// "/dev/rdisk6"}`, with `raw_path` `null` where the OS has no separate
/// unbuffered node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDisk {
    pub name: DeviceName,
    pub raw_path: Option<PathBuf>,
}

impl Serialize for DeviceDisk {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("path", &self.name.path().display().to_string())?;
        map.serialize_entry(
            "raw_path",
            &self.raw_path.as_ref().map(|p| p.display().to_string()),
        )?;
        map.end()
    }
}

/// A filesystem mounted from the card's disk or one of its partitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mount {
    pub device: DeviceName,
    /// The partition the filesystem is on, numbered as in the partition
    /// table; `null` for a filesystem on the whole disk.
    pub partition: Option<PartitionNumber>,
    #[serde(serialize_with = "super::serialize_path")]
    pub mount_point: PathBuf,
    /// The filesystem type as the OS names it (`msdos`, `exfat`, `vfat`).
    /// The type read from the disk is in the layout.
    pub os_fs_type: String,
    pub options: Vec<MountOption>,
}

impl Mount {
    /// Mounted read-only, so the OS doesn't write to the filesystem.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.options.contains(&MountOption::ReadOnly)
    }
}

/// Mount options that change how the card can be used. Each OS has more;
/// these are the ones both macOS and Linux have. Serialized as `mount`
/// writes them, except `read_only`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum MountOption {
    /// Mounted read-only.
    #[serde(rename = "read_only")]
    ReadOnly,
    /// Writes go to the card synchronously.
    #[serde(rename = "synchronous")]
    Synchronous,
    /// Programs on it can't be run.
    #[serde(rename = "noexec")]
    NoExec,
    /// Set-user-ID bits are ignored.
    #[serde(rename = "nosuid")]
    NoSuid,
    /// Device files on it are not interpreted.
    #[serde(rename = "nodev")]
    NoDev,
    /// Access times are not updated.
    #[serde(rename = "noatime")]
    NoAtime,
}

impl fmt::Display for MountOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::Synchronous => "synchronous",
            Self::NoExec => "noexec",
            Self::NoSuid => "nosuid",
            Self::NoDev => "nodev",
            Self::NoAtime => "noatime",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_rejects_paths() {
        assert!(DeviceName::new("disk6s1").is_some());
        assert!(DeviceName::new("/dev/disk6").is_none());
        assert!(DeviceName::new("").is_none());
        assert_eq!(
            DeviceName::new("disk6").unwrap().path(),
            PathBuf::from("/dev/disk6")
        );
    }
}
