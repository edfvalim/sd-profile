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
    let file_type = metadata.file_type();
    let kind = if file_type.is_file() {
        TargetKind::DiskImage
    } else if file_type.is_block_device() || file_type.is_char_device() {
        TargetKind::Device
    } else {
        return Err(OpenError::NotADisk {
            path: path.to_path_buf(),
        });
    };
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
