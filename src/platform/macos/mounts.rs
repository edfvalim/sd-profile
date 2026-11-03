//! The mount table, from `getfsstat`. macOS only.

use std::ffi::{CStr, c_char};
use std::io;
use std::path::PathBuf;

use super::mount_table::{self, RawMount};
use crate::model::{DeviceName, Mount};
use crate::platform::PlatformError;

/// The filesystems mounted from `disk` or its partitions.
///
/// # Errors
///
/// When `getfsstat` fails.
pub fn mounts_of(disk: &DeviceName) -> Result<Vec<Mount>, PlatformError> {
    Ok(mount_table::mounts_of(
        disk,
        &table().map_err(PlatformError::Mounts)?,
    ))
}

fn table() -> io::Result<Vec<RawMount>> {
    // Mounts can come and go between the two calls; a few spare records
    // keep a new one from being cut off, and the second call says how many
    // it filled.
    // SAFETY: a null buffer asks only for the number of mounted filesystems.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    let count = usize::try_from(count).map_err(|_| io::Error::last_os_error())? + 8;
    // SAFETY: `statfs` is plain data; all zeros is a valid value.
    let mut records = vec![unsafe { std::mem::zeroed::<libc::statfs>() }; count];
    let bytes = libc::c_int::try_from(count * size_of::<libc::statfs>())
        .map_err(|_| io::Error::other("mount table too large"))?;
    // SAFETY: the buffer holds `count` records and `bytes` is its size, so
    // getfsstat writes within it.
    let filled = unsafe { libc::getfsstat(records.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    let filled = usize::try_from(filled).map_err(|_| io::Error::last_os_error())?;
    records.truncate(filled);
    Ok(records.iter().map(raw).collect())
}

fn raw(record: &libc::statfs) -> RawMount {
    RawMount {
        from: text(&record.f_mntfromname),
        on: PathBuf::from(text(&record.f_mntonname)),
        fs_type: text(&record.f_fstypename),
        flags: record.f_flags,
    }
}

/// A fixed-size C string field, which the kernel always terminates.
fn text(field: &[c_char]) -> String {
    // SAFETY: `c_char` and `u8` have the same size and alignment, and the
    // slice stays borrowed for the length of the view.
    let bytes = unsafe { std::slice::from_raw_parts(field.as_ptr().cast::<u8>(), field.len()) };
    CStr::from_bytes_until_nul(bytes)
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MountOption;

    #[test]
    fn option_bits_match_the_system_headers() {
        let expected = [
            (libc::MNT_RDONLY, MountOption::ReadOnly),
            (libc::MNT_SYNCHRONOUS, MountOption::Synchronous),
            (libc::MNT_NOEXEC, MountOption::NoExec),
            (libc::MNT_NOSUID, MountOption::NoSuid),
            (libc::MNT_NODEV, MountOption::NoDev),
            (libc::MNT_NOATIME, MountOption::NoAtime),
        ];
        assert_eq!(mount_table::OPTION_BITS.len(), expected.len());
        for ((bit, option), (libc_bit, libc_option)) in
            mount_table::OPTION_BITS.iter().zip(expected)
        {
            assert_eq!(i64::from(*bit), i64::from(libc_bit), "{option:?}");
            assert_eq!(*option, libc_option);
        }
    }

    #[test]
    fn root_filesystem_is_in_the_table() {
        let table = table().unwrap();
        assert!(table.iter().any(|m| m.on.as_os_str() == "/"));
    }
}
