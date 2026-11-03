//! Access to the operating system.
//!
//! Only the code that calls the OS is OS-specific, and all conditional
//! compilation stays in this module and its children. Parsing and mapping
//! code builds on every OS, so it can be tested anywhere against captured
//! fixtures: `macos/map.rs` turns an I/O Registry capture into the model
//! whichever OS runs the tests.

#[cfg(not(unix))]
compile_error!("sd-profile supports macOS and Linux only");

mod macos;
mod unix;

use std::path::PathBuf;

use crate::model::{BlockSize, Card, DeviceName, Field, Mount, PartitionNumber, Reader, Source};

pub use unix::{
    OpenError, TargetKind, is_device_io_error, is_permission_bits_refusal, is_root, open_read_only,
    target_kind,
};

/// A card reader the OS knows about, and what it says about the card in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdDevice {
    /// The card's whole disk, `None` while the reader is empty or the card
    /// is inactive.
    pub disk: Option<Disk>,
    pub card: Field<Card>,
    pub reader: Reader,
    /// Where the OS published the reader and the disk sizes.
    pub source: Source,
}

/// The disk the OS made for a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub name: DeviceName,
    /// The unbuffered device node, where the OS has a separate one
    /// (`/dev/rdisk6` on macOS).
    pub raw_path: Option<PathBuf>,
    pub size_bytes: u64,
    pub block_size: BlockSize,
}

/// Failure to ask the OS about readers or mounts.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("cannot read the I/O Registry: {call} failed with kern_return_t {status:#x}")]
    Registry { call: &'static str, status: i32 },
    #[error("cannot list mounted filesystems: {0}")]
    Mounts(#[source] std::io::Error),
    #[error("finding SD card readers is supported on macOS only for now")]
    Unsupported,
}

/// The readers in a capture under `tests/fixtures/macos/`, mapped as the
/// live registry would be.
#[cfg(test)]
pub fn fixture_devices(name: &str) -> Vec<SdDevice> {
    macos::fixture_devices(name)
}

/// What a device path names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    WholeDisk(DeviceName),
    /// A partition, and the whole disk it is on.
    Partition {
        whole: DeviceName,
    },
    /// Not a disk device at all, like `/dev/null`.
    Other,
}

/// Which part of a disk a device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskPart {
    /// The whole disk, as when a filesystem spans it without a partition
    /// table.
    WholeDisk,
    /// One partition, numbered as in the partition table.
    Partition(PartitionNumber),
}

/// Which USB storage devices count as card readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbScope {
    /// Only those whose name says they are card readers.
    NamedReaders,
    /// Every USB disk, for a disk the user named as a card.
    AllDisks,
}

/// The card readers on this computer, built-in ones first.
///
/// # Errors
///
/// When the OS can't be asked, or on an OS this version doesn't support.
pub fn sd_devices(usb: UsbScope) -> Result<Vec<SdDevice>, PlatformError> {
    #[cfg(target_os = "macos")]
    return macos::sd_devices(usb);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = usb;
        return Err(PlatformError::Unsupported);
    }
}

/// What a device path names, in this OS's device naming.
///
/// # Errors
///
/// On an OS this version doesn't support.
#[cfg_attr(
    target_os = "macos",
    expect(
        clippy::unnecessary_wraps,
        reason = "only macOS naming is known; other OSes return the error"
    )
)]
pub fn device_name(path: &std::path::Path) -> Result<Named, PlatformError> {
    #[cfg(target_os = "macos")]
    return Ok(macos::names::parse(path));
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        return Err(PlatformError::Unsupported);
    }
}

/// The filesystems mounted from `disk` or its partitions.
///
/// # Errors
///
/// When the OS's mount table can't be read, or on an OS this version
/// doesn't support.
pub fn mounts_of(disk: &DeviceName) -> Result<Vec<Mount>, PlatformError> {
    #[cfg(target_os = "macos")]
    return macos::mounts_of(disk);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = disk;
        return Err(PlatformError::Unsupported);
    }
}
