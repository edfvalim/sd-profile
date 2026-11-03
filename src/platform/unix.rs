//! Opening targets, shared by macOS and Linux.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

/// What kind of file a target path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// A regular file: a disk image.
    DiskImage,
    /// A block or character device node, like `/dev/disk4` or `/dev/rdisk4`.
    Device,
}

#[derive(Debug)]
pub struct OpenedTarget {
    pub file: File,
    pub kind: TargetKind,
    /// Size in bytes. For devices this comes later from the OS; a device node's
    /// file size is 0.
    pub size: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("cannot open {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not a disk image file or a device", path.display())]
    NotADisk { path: PathBuf },
}

/// The process runs as root, so it can open device nodes.
#[must_use]
pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The open was refused by the file's permissions (`EACCES`), as device
/// nodes refuse anyone but root. `EPERM` is a different refusal, by a
/// policy such as a macOS privacy setting, that root doesn't get past.
#[must_use]
pub fn is_permission_bits_refusal(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EACCES)
}

/// The device failed the I/O itself (`EIO`), as a card pulled out mid-read
/// or a failing card does.
#[must_use]
pub fn is_device_io_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EIO)
}

/// Says what kind of target `path` names, without opening it. Device nodes
/// can be looked at without root, but not opened.
///
/// # Errors
///
/// [`OpenError::Io`] if the path can't be inspected,
/// [`OpenError::NotADisk`] for directories, sockets and the like.
pub fn target_kind(path: &Path) -> Result<TargetKind, OpenError> {
    let metadata = std::fs::metadata(path).map_err(|source| OpenError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    kind_of(metadata.file_type(), path)
}

fn kind_of(file_type: std::fs::FileType, path: &Path) -> Result<TargetKind, OpenError> {
    if file_type.is_file() {
        Ok(TargetKind::DiskImage)
    } else if file_type.is_block_device() || file_type.is_char_device() {
        Ok(TargetKind::Device)
    } else {
        Err(OpenError::NotADisk {
            path: path.to_path_buf(),
        })
    }
}

/// Opens `path` read-only and says what kind of target it is.
///
/// Read-only is the whole point: `File::open` uses `O_RDONLY`, and nothing in
/// the crate opens a target any other way.
///
/// # Errors
///
/// [`OpenError::Io`] if the path can't be opened or inspected,
/// [`OpenError::NotADisk`] for directories, sockets and the like.
pub fn open_read_only(path: &Path) -> Result<OpenedTarget, OpenError> {
    let io_error = |source| OpenError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = File::open(path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    let kind = kind_of(metadata.file_type(), path)?;
    Ok(OpenedTarget {
        file,
        kind,
        size: metadata.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_eacces_is_a_permission_bits_refusal() {
        assert!(is_permission_bits_refusal(&io::Error::from_raw_os_error(
            libc::EACCES
        )));
        assert!(!is_permission_bits_refusal(&io::Error::from_raw_os_error(
            libc::EPERM
        )));
    }

    #[test]
    fn regular_file_is_disk_image() {
        let opened = open_read_only(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("Cargo.toml")
                .as_path(),
        )
        .unwrap();
        assert_eq!(opened.kind, TargetKind::DiskImage);
        assert!(opened.size > 0);
    }

    #[test]
    fn dev_null_is_device() {
        let opened = open_read_only(Path::new("/dev/null")).unwrap();
        assert_eq!(opened.kind, TargetKind::Device);
    }

    #[test]
    fn device_kind_needs_no_open() {
        assert_eq!(
            target_kind(Path::new("/dev/null")).unwrap(),
            TargetKind::Device
        );
    }

    #[test]
    fn directory_is_rejected() {
        assert!(matches!(
            open_read_only(Path::new(env!("CARGO_MANIFEST_DIR"))),
            Err(OpenError::NotADisk { .. })
        ));
    }

    #[test]
    fn missing_file_is_io_error() {
        assert!(matches!(
            open_read_only(Path::new("/nonexistent/card.img")),
            Err(OpenError::Io { .. })
        ));
    }
}
