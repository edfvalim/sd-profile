//! The pipeline: find the target, ask the OS about the card and reader, read
//! the disk, build the profile, print it.

use std::io::Write;
use std::path::Path;

use crate::cli::{Args, Format};
use crate::disk::{self, block::AlignedReader};
use crate::error::Error;
use crate::model::{
    BlockSize, DeviceDisk, DeviceName, Field, Layout, Mount, Parts, Profile, Source, Target,
    Unavailable,
};
use crate::platform::{self, Named, PlatformError, SdDevice, TargetKind, UsbScope};
use crate::render;

/// Profiles the target named in `args` and writes the report to `out`.
///
/// # Errors
///
/// When the target can't be found, opened or isn't supported, or the output
/// can't be written. Read failures after that end up in the profile as
/// unavailable fields instead.
pub fn run(args: &Args, out: &mut impl Write) -> Result<(), Error> {
    let profile = match &args.target {
        None => live_device_profile(choose(platform::sd_devices(UsbScope::NamedReaders)?)?),
        Some(path) => match platform::target_kind(path)? {
            TargetKind::DiskImage => image_profile(path)?,
            TargetKind::Device => live_device_profile(named_device(path)?),
        },
    };
    match args.format() {
        Format::Human => render::human::write(&profile, out),
        Format::Json => render::json::write(&profile, out),
    }
    .map_err(Error::Output)
}

/// The reader to profile when none is named: the one with a card in it, or
/// the only one there is.
fn choose(devices: Vec<SdDevice>) -> Result<SdDevice, Error> {
    let (with_card, empty): (Vec<_>, Vec<_>) = devices
        .into_iter()
        .partition(|d| d.card != Field::Unavailable(Unavailable::NoCard));
    let mut candidates = if with_card.is_empty() {
        empty
    } else {
        with_card
    };
    match candidates.len() {
        0 => Err(Error::NoReader),
        1 => Ok(candidates.remove(0)),
        _ => Err(Error::SeveralCards {
            disks: candidates
                .iter()
                .map(|d| d.disk.as_ref().map(|disk| disk.name.clone()))
                .collect(),
        }),
    }
}

/// The reader holding the disk at `path`.
fn named_device(path: &Path) -> Result<SdDevice, Error> {
    let disk = match platform::device_name(path)? {
        Named::WholeDisk(name) => name,
        Named::Partition { whole } => {
            return Err(Error::Partition {
                path: path.to_path_buf(),
                whole: whole.path(),
            });
        }
        Named::Other => {
            return Err(Error::NotADiskDevice {
                path: path.to_path_buf(),
            });
        }
    };
    platform::sd_devices(UsbScope::AllDisks)?
        .into_iter()
        .find(|d| d.disk.as_ref().is_some_and(|d| d.name == disk))
        .ok_or_else(|| Error::NotACard {
            path: path.to_path_buf(),
        })
}

fn live_device_profile(device: SdDevice) -> Profile {
    device_profile(device, platform::mounts_of, platform::is_root())
}

/// The profile of a card in a reader, with the OS's mount table and root
/// state passed in.
fn device_profile(
    device: SdDevice,
    mounts_of: impl FnOnce(&DeviceName) -> Result<Vec<Mount>, PlatformError>,
    is_root: bool,
) -> Profile {
    let SdDevice {
        disk,
        card,
        reader,
        source,
    } = device;
    let reader = Field::known(reader, source);
    let Some(disk) = disk else {
        // No disk: the reader is empty or the card is ejected. The card's
        // identity can still be known.
        let reason = match &card {
            Field::Unavailable(Unavailable::NoCard) => Unavailable::NoCard,
            _ => Unavailable::CardInactive,
        };
        return Profile::new(Parts {
            target: Target::Device { disk: None },
            card,
            reader,
            mounts: Field::Unavailable(reason.clone()),
            layout: Layout::unavailable(&reason),
        });
    };
    let mounts = match mounts_of(&disk.name) {
        Ok(mounts) => Field::known(mounts, Source::MountTable),
        Err(err) => Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
    };
    // Reading the raw device comes in the next version, and needs root.
    let not_read = if is_root {
        Unavailable::NotImplemented
    } else {
        Unavailable::RequiresRoot
    };
    Profile::new(Parts {
        target: Target::Device {
            disk: Some(DeviceDisk {
                name: disk.name,
                raw_path: disk.raw_path,
            }),
        },
        card,
        reader,
        mounts,
        layout: Layout {
            size_bytes: Field::known(disk.size_bytes, source),
            block_size: Field::known(disk.block_size, source),
            partition_table: Field::Unavailable(not_read),
            volumes: Vec::new(),
        },
    })
}

fn image_profile(path: &Path) -> Result<Profile, Error> {
    let opened = platform::open_read_only(path)?;
    if opened.kind == TargetKind::Device {
        // Replaced between the two looks at it.
        return Err(Error::Changed {
            path: path.to_path_buf(),
        });
    }

    // Images have no sector size of their own; use the one every SD card has.
    let block_size = BlockSize::SD;
    let mut reader = AlignedReader::new(opened.file, block_size, opened.size);
    let (partition_table, volumes) = match disk::table::read(&mut reader) {
        Ok(table) => {
            // An image has no card; its size stands in for the card's capacity.
            let volumes = disk::volumes::read(&mut reader, &table, opened.size);
            (Field::known(table, Source::PartitionTable), volumes)
        }
        Err(err) => (
            Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
            Vec::new(),
        ),
    };

    Ok(Profile::new(Parts {
        target: Target::DiskImage {
            path: path.to_path_buf(),
        },
        card: Field::Unavailable(Unavailable::DiskImage),
        reader: Field::Unavailable(Unavailable::DiskImage),
        mounts: Field::Unavailable(Unavailable::DiskImage),
        layout: Layout {
            size_bytes: Field::known(opened.size, Source::FileSize),
            block_size: Field::known(block_size, Source::SdSpecification),
            partition_table,
            volumes,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MountOption;
    use crate::render;

    fn mounts(disk: &DeviceName) -> Vec<Mount> {
        vec![Mount {
            device: DeviceName::new(&format!("{disk}s1")).unwrap(),
            mount_point: "/Volumes/BOOTFS".into(),
            os_fs_type: "msdos".into(),
            options: vec![
                MountOption::NoSuid,
                MountOption::NoDev,
                MountOption::NoAtime,
            ],
        }]
    }

    fn profile(fixture: &str, is_root: bool) -> Profile {
        let mut devices = platform::fixture_devices(fixture);
        assert_eq!(devices.len(), 1);
        device_profile(devices.remove(0), |disk| Ok(mounts(disk)), is_root)
    }

    /// Runs `assertions` with snapshots kept in `tests/snapshots/`, next to
    /// the end-to-end ones.
    fn snapshots(assertions: impl FnOnce()) {
        let mut settings = insta::Settings::clone_current();
        settings.set_snapshot_path(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots"));
        settings.bind(assertions);
    }

    fn human(profile: &Profile) -> String {
        let mut out = Vec::new();
        render::human::write(profile, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn json(profile: &Profile) -> String {
        let mut out = Vec::new();
        render::json::write(profile, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn mounted_card_report() {
        let profile = profile("builtin-sdhc-mounted", false);
        snapshots(|| {
            insta::assert_snapshot!("human-builtin-sdhc-mounted", human(&profile));
            insta::assert_snapshot!("json-builtin-sdhc-mounted", json(&profile));
        });
    }

    #[test]
    fn ejected_card_report() {
        let profile = profile("builtin-sdhc-inactive", false);
        snapshots(|| {
            insta::assert_snapshot!("human-builtin-sdhc-inactive", human(&profile));
            insta::assert_snapshot!("json-builtin-sdhc-inactive", json(&profile));
        });
    }

    #[test]
    fn root_is_told_reading_comes_later() {
        let profile = profile("builtin-sdhc-mounted", true);
        assert_eq!(
            profile.layout.partition_table,
            Field::Unavailable(Unavailable::NotImplemented)
        );
    }

    #[test]
    fn mount_table_failure_fails_only_the_mounts() {
        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let profile = device_profile(
            devices.remove(0),
            |_| Err(PlatformError::Unsupported),
            false,
        );
        assert!(matches!(
            profile.mounts,
            Field::Unavailable(Unavailable::ReadFailed(_))
        ));
        assert!(profile.card.value().is_some());
    }
}
