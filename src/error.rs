//! Errors that stop a run. Problems the profile can still describe (a damaged
//! table, an unreadable field) are part of the profile instead.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::model::DeviceName;
use crate::platform::{OpenError, PlatformError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error("no SD card reader found")]
    NoReader,
    #[error(
        "found more than one card ({}); name the one to profile, like `sd-profile /dev/disk6`",
        DiskList(disks)
    )]
    SeveralCards { disks: Vec<Option<DeviceName>> },
    #[error(
        "{} is a partition; name the whole disk, {}",
        path.display(),
        whole.display()
    )]
    Partition { path: PathBuf, whole: PathBuf },
    #[error("{} is not a disk device", path.display())]
    NotADiskDevice { path: PathBuf },
    #[error("{} is not a card in an SD card reader or a USB disk", path.display())]
    NotACard { path: PathBuf },
    #[error("{} changed while it was being opened", path.display())]
    Changed { path: PathBuf },
    #[error("cannot write the output: {0}")]
    Output(#[source] io::Error),
}

/// The cards' disks for a message, with `inactive` for cards that have none.
struct DiskList<'a>(&'a [Option<DeviceName>]);

impl fmt::Display for DiskList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, disk) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            match disk {
                Some(name) => write!(f, "{}", name.path().display())?,
                None => f.write_str("an inactive card")?,
            }
        }
        Ok(())
    }
}
