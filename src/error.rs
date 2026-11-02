//! Errors that stop a run. Problems the profile can still describe (a damaged
//! table, an unreadable field) are part of the profile instead.

use std::io;
use std::path::PathBuf;

use crate::platform::OpenError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error(
        "{} is a device; this version profiles disk image files only",
        path.display()
    )]
    DeviceNotSupported { path: PathBuf },
    #[error("cannot write the output: {0}")]
    Output(#[source] io::Error),
}
