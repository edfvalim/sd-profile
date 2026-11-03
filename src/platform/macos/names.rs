//! macOS device naming: `/dev/disk6`, the raw `/dev/rdisk6`, and partitions
//! `disk6s1`. Builds on every OS.

use std::path::Path;

use crate::model::DeviceName;
use crate::platform::Named;

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
