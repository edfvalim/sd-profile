//! The profile: everything the tool reports, shaped the same on every OS.
//!
//! JSON keys never depend on the platform. A value the platform or the
//! target can't provide is a [`Field`] that says why it is missing, so adding
//! a platform fills fields in without changing the shape.

mod card;
mod field;
mod filesystem;
mod guid;
mod layout;
mod mount;
mod partition_type;
mod reader;
mod volume;

use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

pub use card::{
    Card, CardType, Manufacturer, ManufacturingDate, OemId, ProductRevision, SerialNumber,
    SpecVersion, SpeedClass, WriteProtect,
};
pub use field::{Field, Source, Unavailable};
pub use filesystem::{
    Exfat, Ext, ExtFeatures, Fat, Fat32Sectors, Filesystem, FsIssue, Revision, Uuid, VolumeSerial,
};
pub use guid::Guid;
pub use layout::{
    BlockSize, DiskSignature, EbrProblem, Extent, GptAttributes, GptFlag, GptHeaderProblem,
    GptPartition, GptTable, HeaderCopy, Issue, Layout, Lba, MbrPartition, MbrRole, MbrTable,
    NoTable, NoTableReason, PartitionNumber, PartitionTable, ProtectiveMbr,
};
pub use mount::{DeviceDisk, DeviceName, Mount, MountOption};
pub use partition_type::{GptType, MbrType};
pub use reader::{
    BusWidth, CardLink, Connection, HardwareId, PciDevice, PciRevision, PcieGeneration, PcieLink,
    Reader, SpeedMode, UsbDevice, UsbSpeed,
};
pub use volume::{
    Alignment, AlignmentCheck, Boundary, CapacityClass, Misaligned, Placement, Volume,
};

/// Version of the JSON output's shape. Bumped when a key is removed, renamed
/// or changes type; adding keys does not bump it.
pub const SCHEMA_VERSION: u32 = 1;

/// The full report on one target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Profile {
    schema_version: u32,
    pub target: Target,
    pub card: Field<Card>,
    pub reader: Field<Reader>,
    /// Filesystems mounted from the card, in the OS's mount order.
    pub mounts: Field<Vec<Mount>>,
    pub layout: Layout,
}

/// The parts of a profile, gathered separately and joined by
/// [`Profile::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    pub target: Target,
    pub card: Field<Card>,
    pub reader: Field<Reader>,
    pub mounts: Field<Vec<Mount>>,
    pub layout: Layout,
}

impl Profile {
    #[must_use]
    pub fn new(parts: Parts) -> Self {
        let Parts {
            target,
            card,
            reader,
            mounts,
            layout,
        } = parts;
        Self {
            schema_version: SCHEMA_VERSION,
            target,
            card,
            reader,
            mounts,
            layout,
        }
    }
}

/// What was profiled. Serialized tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A file holding a copy of a disk, such as a `dd` backup of a card.
    DiskImage {
        #[serde(serialize_with = "serialize_path")]
        path: PathBuf,
    },
    /// A card in a reader. `disk` is the card's whole disk, `null` while
    /// the card is inactive or the reader is empty.
    Device { disk: Option<DeviceDisk> },
}

fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&path.display())
}
