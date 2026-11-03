//! macOS device naming: `/dev/disk6`, the raw `/dev/rdisk6`, and partitions
//! `disk6s1`. Builds on every OS.

use std::path::Path;

use crate::model::{DeviceName, PartitionNumber};
use crate::platform::{DiskPart, Named};

/// Reads `/dev/diskN`, `/dev/rdiskN` and their partitions `...sM`.
#[must_use]
pub fn parse(path: &Path) -> Named {
    let Some(name) = path
        .to_str()
        .and_then(|p| p.strip_prefix("/dev/"))
        .map(|n| n.strip_prefix('r').unwrap_or(n))
    else {
        return Named::Other;
    };
    let Some(rest) = name.strip_prefix("disk") else {
        return Named::Other;
    };
    let unit_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if unit_len == 0 {
        return Named::Other;
    }
    let whole = DeviceName::new(&name[.."disk".len() + unit_len])
        .expect("disk plus digits is a valid name");
    match &rest[unit_len..] {
        "" => Named::WholeDisk(whole),
        slice if slice.starts_with('s') => Named::Partition { whole },
        _ => Named::Other,
    }
}

/// Which part of `disk` the device `name` is, `None` when it's on another
/// disk. Slice `sN` is partition N of the partition table (MBR logical
/// partitions from 5, GPT by entry slot). A slice's own slices
/// (`disk6s1s2`) are on that partition.
#[must_use]
pub fn part_of(disk: &DeviceName, name: &DeviceName) -> Option<DiskPart> {
    let rest = name.as_str().strip_prefix(disk.as_str())?;
    if rest.is_empty() {
        return Some(DiskPart::WholeDisk);
    }
    let slice = rest.strip_prefix('s')?;
    let digits_len = slice.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, after) = slice.split_at(digits_len);
    // macOS writes slice numbers without leading zeros, and numbers them
    // from 1; anything else (`disk61` is not a slice of `disk6`) is not.
    if digits.is_empty() || digits.starts_with('0') || !(after.is_empty() || after.starts_with('s'))
    {
        return None;
    }
    digits
        .parse()
        .ok()
        .map(|n| DiskPart::Partition(PartitionNumber(n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(name: &str) -> Named {
        Named::WholeDisk(DeviceName::new(name).unwrap())
    }

    #[test]
    fn whole_disks_buffered_and_raw() {
        assert_eq!(parse(Path::new("/dev/disk6")), whole("disk6"));
        assert_eq!(parse(Path::new("/dev/rdisk12")), whole("disk12"));
    }

    #[test]
    fn partitions_name_their_disk() {
        assert_eq!(
            parse(Path::new("/dev/disk6s1")),
            Named::Partition {
                whole: DeviceName::new("disk6").unwrap()
            }
        );
    }

    fn part(disk: &str, name: &str) -> Option<DiskPart> {
        part_of(
            &DeviceName::new(disk).unwrap(),
            &DeviceName::new(name).unwrap(),
        )
    }

    #[test]
    fn slices_are_partition_numbers() {
        assert_eq!(part("disk6", "disk6"), Some(DiskPart::WholeDisk));
        assert_eq!(
            part("disk6", "disk6s1"),
            Some(DiskPart::Partition(PartitionNumber(1)))
        );
        assert_eq!(
            part("disk6", "disk6s12"),
            Some(DiskPart::Partition(PartitionNumber(12)))
        );
        assert_eq!(
            part("disk6", "disk6s1s2"),
            Some(DiskPart::Partition(PartitionNumber(1)))
        );
    }

    #[test]
    fn other_disks_and_odd_slices_are_not_parts() {
        for name in [
            "disk61", "disk61s1", "disk7s1", "disk6s", "disk6s0", "disk6s1x", "disk6s01",
        ] {
            assert_eq!(part("disk6", name), None, "{name}");
        }
    }

    #[test]
    fn other_devices() {
        for path in [
            "/dev/null",
            "/dev/disk",
            "disk6",
            "/dev/diskx",
            "/dev/disk6x",
        ] {
            assert_eq!(parse(Path::new(path)), Named::Other, "{path}");
        }
    }
}
