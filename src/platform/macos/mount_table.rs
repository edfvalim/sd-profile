//! Entries of the macOS mount table to the model. Builds on every OS.

use std::path::PathBuf;

use crate::model::{DeviceName, Mount, MountOption};

/// One `statfs` record from `getfsstat`, copied out of the C struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMount {
    /// `f_mntfromname`, like `/dev/disk6s1`.
    pub from: String,
    /// `f_mntonname`, like `/Volumes/BOOTFS`.
    pub on: PathBuf,
    /// `f_fstypename`, like `msdos`.
    pub fs_type: String,
    /// `f_flags`, the `MNT_*` bits.
    pub flags: u32,
}

/// The `MNT_*` bits from `<sys/mount.h>` that map to a [`MountOption`].
pub const OPTION_BITS: &[(u32, MountOption)] = &[
    (0x0000_0001, MountOption::ReadOnly),    // MNT_RDONLY
    (0x0000_0002, MountOption::Synchronous), // MNT_SYNCHRONOUS
    (0x0000_0004, MountOption::NoExec),      // MNT_NOEXEC
    (0x0000_0008, MountOption::NoSuid),      // MNT_NOSUID
    (0x0000_0010, MountOption::NoDev),       // MNT_NODEV
    (0x1000_0000, MountOption::NoAtime),     // MNT_NOATIME
];

/// The mounts of `disk` and its partitions (`disk6`, `disk6s1`, ...), in
/// table order.
#[must_use]
pub fn mounts_of(disk: &DeviceName, table: &[RawMount]) -> Vec<Mount> {
    table
        .iter()
        .filter_map(|raw| {
            let name = DeviceName::new(raw.from.strip_prefix("/dev/")?)?;
            is_on_disk(&name, disk).then(|| Mount {
                device: name,
                mount_point: raw.on.clone(),
                os_fs_type: raw.fs_type.clone(),
                options: OPTION_BITS
                    .iter()
                    .filter(|(bit, _)| raw.flags & bit != 0)
                    .map(|&(_, option)| option)
                    .collect(),
            })
        })
        .collect()
}

/// `name` is `disk` itself or one of its partitions, `<disk>s<n>`. A
/// partition's own slices (`disk6s1s2`, APFS snapshots) count too.
fn is_on_disk(name: &DeviceName, disk: &DeviceName) -> bool {
    match name.as_str().strip_prefix(disk.as_str()) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('s')
            .is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit())),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(from: &str, on: &str, flags: u32) -> RawMount {
        RawMount {
            from: from.into(),
            on: on.into(),
            fs_type: "msdos".into(),
            flags,
        }
    }

    #[test]
    fn keeps_only_the_disk_and_its_partitions() {
        let disk = DeviceName::new("disk6").unwrap();
        let table = [
            raw("/dev/disk3s1s1", "/", 0x1),
            raw("/dev/disk6s1", "/Volumes/BOOTFS", 0x1000_0018),
            raw("/dev/disk60s1", "/Volumes/OTHER", 0),
            raw("map auto_home", "/System/Volumes/Data/home", 0),
            raw("/dev/disk6", "/Volumes/WHOLE", 0x1),
        ];
        let mounts = mounts_of(&disk, &table);
        let devices: Vec<&str> = mounts.iter().map(|m| m.device.as_str()).collect();
        assert_eq!(devices, ["disk6s1", "disk6"]);
        assert_eq!(
            mounts[0].options,
            [
                MountOption::NoSuid,
                MountOption::NoDev,
                MountOption::NoAtime
            ]
        );
        assert_eq!(mounts[1].options, [MountOption::ReadOnly]);
    }
}
