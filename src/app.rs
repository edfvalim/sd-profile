//! The pipeline: find the target, ask the OS about the card and reader, read
//! the disk, build the profile, print it.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use crate::cli::{Args, Format};
use crate::disk::{
    self,
    block::{AlignedReader, BlockReader, ReadError},
};
use crate::error::Error;
use crate::model::{
    BlockSize, DeviceDisk, DeviceName, Field, Layout, Mount, PartitionTable, Parts, Profile,
    Source, Target, Unavailable, Volume,
};
use crate::platform::{
    self, Disk, Named, OpenError, PlatformError, SdDevice, TargetKind, UsbScope,
};
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
        None => live_device_profile(choose(platform::sd_devices(UsbScope::NamedReaders)?)?)?,
        Some(path) => match platform::target_kind(path)? {
            TargetKind::DiskImage => image_profile(path)?,
            TargetKind::Device => live_device_profile(named_device(path)?)?,
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

/// The profile of `device`, checked against a second look at the registry
/// so that a card pulled or swapped during the run doesn't pass as one
/// profile.
fn live_device_profile(device: SdDevice) -> Result<Profile, Error> {
    let before = device.clone();
    let profile = device_profile(device, platform::mounts_of, open_raw);
    let Some(disk) = &before.disk else {
        return Ok(profile);
    };
    // After a failed read, give the OS time to notice a pulled card.
    let looks = match profile.layout.partition_table {
        Field::Unavailable(Unavailable::ReadFailed(_)) => SETTLE_LOOKS,
        _ => 1,
    };
    let look = || -> Result<Option<SdDevice>, Error> {
        Ok(platform::sd_devices(UsbScope::AllDisks)?
            .into_iter()
            .find(|device| {
                device
                    .disk
                    .as_ref()
                    .is_some_and(|found| found.name == disk.name)
            }))
    };
    second_look(&before, looks, look, || {
        std::thread::sleep(SETTLE_INTERVAL);
    })?;
    Ok(profile)
}

/// Right after a pull, reads fail while the registry still lists the card.
/// A failed read looks this many times, [`SETTLE_INTERVAL`] apart (2 s in
/// all), for the OS to notice.
const SETTLE_LOOKS: usize = 9;
const SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Looks at the registry again with `look`, `looks` times with `wait`
/// between them, and checks each time that the card is still the one in
/// `before`.
fn second_look(
    before: &SdDevice,
    looks: usize,
    mut look: impl FnMut() -> Result<Option<SdDevice>, Error>,
    mut wait: impl FnMut(),
) -> Result<(), Error> {
    for i in 0..looks {
        if i > 0 {
            wait();
        }
        ensure_same_card(before, look()?.as_ref())?;
    }
    Ok(())
}

/// `Ok` when `after`, the reader found under `before`'s disk name on a
/// second look, still holds the same card on the same disk. A USB reader
/// hides the card, so there only the disk can tell. Any change to the card
/// counts, a flipped lock switch too.
fn ensure_same_card(before: &SdDevice, after: Option<&SdDevice>) -> Result<(), Error> {
    let Some(disk) = &before.disk else {
        return Ok(());
    };
    match after {
        None => Err(Error::CardRemoved {
            disk: disk.name.path(),
        }),
        Some(after) if after.card == Field::Unavailable(Unavailable::NoCard) => {
            Err(Error::CardRemoved {
                disk: disk.name.path(),
            })
        }
        Some(after) if after.disk != before.disk || after.card != before.card => {
            Err(Error::CardChanged {
                disk: disk.name.path(),
            })
        }
        Some(_) => Ok(()),
    }
}

/// Opens the card's unbuffered device node read-only.
fn open_raw(disk: &Disk) -> Result<File, Unavailable> {
    let path = disk.raw_path.clone().unwrap_or_else(|| disk.name.path());
    match platform::open_read_only(&path) {
        Ok(opened) if opened.kind == TargetKind::Device => Ok(opened.file),
        Ok(_) => Err(Unavailable::ReadFailed(format!(
            "{} is not a device",
            path.display()
        ))),
        // Device nodes belong to root, so only their permission bits
        // refusing a user other than root means "use sudo". Any other
        // refusal (a privacy setting, or anything refusing root itself)
        // keeps the OS's message, which says more.
        Err(OpenError::Io { source, .. })
            if !platform::is_root() && platform::is_permission_bits_refusal(&source) =>
        {
            Err(Unavailable::RequiresRoot)
        }
        Err(err) => Err(Unavailable::ReadFailed(err.to_string())),
    }
}

/// The profile of a card in a reader, with the OS's mount table and the
/// opening of the raw device passed in.
fn device_profile<R: Read + Seek>(
    device: SdDevice,
    mounts_of: impl FnOnce(&DeviceName) -> Result<Vec<Mount>, PlatformError>,
    open_disk: impl FnOnce(&Disk) -> Result<R, Unavailable>,
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
        Ok(mut mounts) => {
            // The mount table lists them in mount order; whole disk first,
            // then by partition, reads better.
            mounts
                .sort_by(|a, b| (a.partition, &a.mount_point).cmp(&(b.partition, &b.mount_point)));
            Field::known(mounts, Source::MountTable)
        }
        Err(err) => Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
    };
    // Alignment boundaries follow the card's capacity class. A USB reader
    // hides the card, and then the disk is all there is to go by.
    let capacity = card
        .value()
        .and_then(|card| card.capacity_bytes.value())
        .copied()
        .unwrap_or(disk.size_bytes);
    let (partition_table, mut volumes) = match open_disk(&disk) {
        Ok(raw) => read_layout(
            &mut AlignedReader::new(raw, disk.block_size, disk.size_bytes),
            capacity,
        )
        .unwrap_or_else(|err| (Field::Unavailable(card_read_failure(&err)), Vec::new())),
        Err(reason) => (Field::Unavailable(reason), Vec::new()),
    };
    if let Some(mounts) = mounts.value() {
        settle_mounted(&mut volumes, mounts);
    }
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
            partition_table,
            volumes,
        },
    })
}

/// A filesystem mounted read-write keeps its dirty flag set for as long as
/// it is mounted (Linux and macOS both set it), and its free count hint
/// lags until unmount, so on those volumes neither says anything about
/// how it was last unmounted.
fn settle_mounted(volumes: &mut [Volume], mounts: &[Mount]) {
    let read_write = mounts.iter().filter(|m| !m.is_read_only());
    for mount in read_write {
        let volume = volumes.iter_mut().find(|v| v.partition == mount.partition);
        if let Some(Field::Known { value, .. }) = volume.map(|v| &mut v.filesystem) {
            value.forget_mounted_state();
        }
    }
}

/// The partition table and the volumes it describes, with alignment checked
/// against the boundary for `capacity_bytes`.
fn read_layout(
    disk: &mut impl BlockReader,
    capacity_bytes: u64,
) -> Result<(Field<PartitionTable>, Vec<Volume>), ReadError> {
    let table = disk::table::read(disk)?;
    let volumes = disk::volumes::read(disk, &table, capacity_bytes);
    Ok((Field::known(table, Source::PartitionTable), volumes))
}

/// A failed read of a card, with a hint when the device itself failed the
/// read: the card was pulled out, or it is faulty.
fn card_read_failure(err: &ReadError) -> Unavailable {
    let device_failed =
        matches!(err, ReadError::Io { source, .. } if platform::is_device_io_error(source));
    let text = read_failure_text(err);
    Unavailable::ReadFailed(if device_failed {
        format!("{text}; the card may have been pulled out, or it is faulty")
    } else {
        text
    })
}

/// A failed read of a disk or image, for the profile.
fn read_failure_text(err: &ReadError) -> String {
    match err {
        // The partition table read starts with sector 0.
        ReadError::OutOfBounds {
            offset: 0, size, ..
        } => {
            format!("the disk ({size} bytes) is smaller than one sector")
        }
        _ => err.to_string(),
    }
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
    // An image has no card; its size stands in for the card's capacity.
    let (partition_table, volumes) = read_layout(
        &mut AlignedReader::new(opened.file, block_size, opened.size),
        opened.size,
    )
    .unwrap_or_else(|err| {
        (
            Field::Unavailable(Unavailable::ReadFailed(read_failure_text(&err))),
            Vec::new(),
        )
    });

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
    use std::io::Cursor;

    use crate::model::{
        AlignmentCheck, CapacityClass, Filesystem, FsIssue, MountOption, PartitionNumber,
    };
    use crate::render;
    use crate::test_support::fixture_image;

    fn mounts(disk: &DeviceName) -> Vec<Mount> {
        vec![Mount {
            device: DeviceName::new(&format!("{disk}s1")).unwrap(),
            partition: Some(PartitionNumber(1)),
            mount_point: "/Volumes/BOOTFS".into(),
            os_fs_type: "msdos".into(),
            options: vec![
                MountOption::NoSuid,
                MountOption::NoDev,
                MountOption::NoAtime,
            ],
        }]
    }

    /// The card in `fixture` with `open` standing in for opening its raw
    /// device.
    fn profile_with<R: Read + Seek>(
        fixture: &str,
        open: impl FnOnce(&Disk) -> Result<R, Unavailable>,
    ) -> Profile {
        let mut devices = platform::fixture_devices(fixture);
        assert_eq!(devices.len(), 1);
        device_profile(devices.remove(0), |disk| Ok(mounts(disk)), open)
    }

    /// The card in `fixture`, run without root.
    fn profile(fixture: &str) -> Profile {
        profile_with(fixture, |_| {
            Err::<Cursor<Vec<u8>>, _>(Unavailable::RequiresRoot)
        })
    }

    /// The mounted card from the registry capture, its raw device holding
    /// `image`. The capture's disk is 16 GB; the image stands in for its
    /// first 128 MiB, and nothing past those is read.
    fn card_holding(image: Vec<u8>) -> Profile {
        profile_with("builtin-sdhc-mounted", move |disk| {
            assert_eq!(disk.raw_path.as_deref(), Some(Path::new("/dev/rdisk6")));
            Ok(Cursor::new(image))
        })
    }

    fn sd_aligned_image() -> Vec<u8> {
        fixture_image("mbr-sd-aligned")
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
        let profile = profile("builtin-sdhc-mounted");
        snapshots(|| {
            insta::assert_snapshot!("human-builtin-sdhc-mounted", human(&profile));
            insta::assert_snapshot!("json-builtin-sdhc-mounted", json(&profile));
        });
    }

    #[test]
    fn ejected_card_report() {
        let profile = profile("builtin-sdhc-inactive");
        snapshots(|| {
            insta::assert_snapshot!("human-builtin-sdhc-inactive", human(&profile));
            insta::assert_snapshot!("json-builtin-sdhc-inactive", json(&profile));
        });
    }

    #[test]
    fn root_reads_the_raw_device() {
        let profile = card_holding(sd_aligned_image());
        assert!(profile.layout.partition_table.value().is_some());
        let partitions: Vec<_> = profile
            .layout
            .volumes
            .iter()
            .map(|v| {
                (
                    v.partition.map(|p| p.0),
                    v.filesystem.value().map(Filesystem::name),
                )
            })
            .collect();
        assert_eq!(
            partitions,
            [(Some(1), Some("FAT32")), (Some(2), Some("ext4"))]
        );
        snapshots(|| {
            insta::assert_snapshot!("human-builtin-sdhc-mounted-root", human(&profile));
            insta::assert_snapshot!("json-builtin-sdhc-mounted-root", json(&profile));
        });
    }

    #[test]
    fn boundary_follows_the_card_not_the_disk() {
        // The image is 128 MiB, an SDSC size; the card is a 16 GB SDHC.
        let profile = card_holding(sd_aligned_image());
        let card_capacity = *profile
            .card
            .value()
            .and_then(|card| card.capacity_bytes.value())
            .unwrap();
        for volume in &profile.layout.volumes {
            let boundary = match volume.alignment.check {
                AlignmentCheck::Aligned { boundary }
                | AlignmentCheck::Misaligned { boundary, .. } => boundary,
                AlignmentCheck::Unknown => panic!("no boundary for {volume:?}"),
            };
            assert_eq!(boundary.capacity_class, CapacityClass::Sdhc);
            assert_eq!(boundary.bytes, 4 << 20);
            assert_eq!(boundary.capacity_bytes, card_capacity);
        }
    }

    #[test]
    fn without_root_the_layout_says_so() {
        let profile = profile("builtin-sdhc-mounted");
        assert_eq!(
            profile.layout.partition_table,
            Field::Unavailable(Unavailable::RequiresRoot)
        );
        assert!(profile.layout.size_bytes.value().is_some());
        assert!(profile.card.value().is_some());
    }

    #[test]
    fn open_failure_fails_only_the_layout() {
        let profile = profile_with("builtin-sdhc-mounted", |_| {
            Err::<Cursor<Vec<u8>>, _>(Unavailable::ReadFailed("busy".into()))
        });
        assert_eq!(
            profile.layout.partition_table,
            Field::Unavailable(Unavailable::ReadFailed("busy".into()))
        );
        assert!(profile.mounts.value().is_some());
    }

    /// The Raspberry Pi image with the FAT32 boot sector's dirty flag set,
    /// as a read-write mount leaves it.
    fn dirty_image() -> Vec<u8> {
        let mut image = sd_aligned_image();
        // Partition 1 starts at LBA 8192; the FAT32 dirty flag is bit 0 of
        // byte 0x41 of its boot sector.
        image[8192 * 512 + 0x41] |= 0x01;
        image
    }

    fn first_volume_issues(profile: &Profile) -> Vec<FsIssue> {
        profile.layout.volumes[0]
            .filesystem
            .value()
            .unwrap()
            .issues()
            .to_vec()
    }

    /// The Raspberry Pi image with the FAT32 `FSInfo` free count one
    /// cluster short, as macOS leaves it while mounted read-write.
    fn stale_free_count_image() -> Vec<u8> {
        let mut image = sd_aligned_image();
        let boot = 8192 * 512;
        let fsinfo =
            boot + 512 * usize::from(u16::from_le_bytes([image[boot + 0x30], image[boot + 0x31]]));
        let at = fsinfo + 0x1e8;
        let free = u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
        image[at..at + 4].copy_from_slice(&(free - 1).to_le_bytes());
        image
    }

    #[test]
    fn stale_free_count_of_a_read_write_mount_is_not_an_issue() {
        let profile = card_holding(stale_free_count_image());
        assert_eq!(first_volume_issues(&profile), []);

        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let profile = device_profile(
            devices.remove(0),
            |_| Ok(Vec::new()),
            |_| Ok(Cursor::new(stale_free_count_image())),
        );
        assert!(matches!(
            first_volume_issues(&profile)[..],
            [FsIssue::FreeCountStale { .. }]
        ));
    }

    #[test]
    fn dirty_flag_of_a_read_write_mount_is_not_an_issue() {
        let profile = card_holding(dirty_image());
        assert_eq!(first_volume_issues(&profile), []);
    }

    #[test]
    fn dirty_flag_stays_when_not_mounted_read_write() {
        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let read_only = |disk: &DeviceName| {
            let mut mounts = mounts(disk);
            mounts[0].options.push(MountOption::ReadOnly);
            Ok(mounts)
        };
        let profile = device_profile(devices.remove(0), read_only, |_| {
            Ok(Cursor::new(dirty_image()))
        });
        assert_eq!(
            first_volume_issues(&profile),
            [FsIssue::NotCleanlyUnmounted]
        );

        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let profile = device_profile(
            devices.remove(0),
            |_| Ok(Vec::new()),
            |_| Ok(Cursor::new(dirty_image())),
        );
        assert_eq!(
            first_volume_issues(&profile),
            [FsIssue::NotCleanlyUnmounted]
        );
    }

    fn builtin_card() -> SdDevice {
        platform::fixture_devices("builtin-sdhc-mounted").remove(0)
    }

    #[test]
    fn same_card_on_the_second_look_passes() {
        let card = builtin_card();
        assert!(ensure_same_card(&card, Some(&card)).is_ok());
    }

    #[test]
    fn card_gone_on_the_second_look_is_removed() {
        assert!(matches!(
            ensure_same_card(&builtin_card(), None),
            Err(Error::CardRemoved { disk }) if disk == Path::new("/dev/disk6")
        ));
    }

    #[test]
    fn empty_slot_on_the_second_look_is_removed() {
        let before = builtin_card();
        let empty = SdDevice {
            card: Field::Unavailable(Unavailable::NoCard),
            ..before.clone()
        };
        assert!(matches!(
            ensure_same_card(&before, Some(&empty)),
            Err(Error::CardRemoved { .. })
        ));
    }

    #[test]
    fn other_card_on_the_second_look_is_changed() {
        let before = builtin_card();
        let other_size = SdDevice {
            disk: before.disk.clone().map(|disk| Disk {
                size_bytes: disk.size_bytes * 2,
                ..disk
            }),
            ..before.clone()
        };
        assert!(matches!(
            ensure_same_card(&before, Some(&other_size)),
            Err(Error::CardChanged { .. })
        ));
        let other_card = SdDevice {
            card: Field::Unavailable(Unavailable::CardInactive),
            ..before.clone()
        };
        assert!(matches!(
            ensure_same_card(&before, Some(&other_card)),
            Err(Error::CardChanged { .. })
        ));
    }

    /// How many times `second_look` asks the registry, `looks` allowed,
    /// when the card stays put.
    fn looks_taken(looks: usize) -> usize {
        let card = builtin_card();
        let mut taken = 0;
        let result = second_look(
            &card,
            looks,
            || {
                taken += 1;
                Ok(Some(card.clone()))
            },
            || {},
        );
        assert!(result.is_ok());
        taken
    }

    #[test]
    fn a_card_that_stays_put_passes_every_look() {
        assert_eq!(looks_taken(1), 1);
        assert_eq!(looks_taken(SETTLE_LOOKS), SETTLE_LOOKS);
    }

    #[test]
    fn a_failed_read_waits_for_the_os_to_notice_a_pull() {
        let card = builtin_card();
        // Listed on the first two looks, gone on the third.
        let mut answers = vec![None, Some(card.clone()), Some(card.clone())];
        let mut waits = 0;
        let result = second_look(
            &card,
            SETTLE_LOOKS,
            || Ok(answers.pop().unwrap()),
            || waits += 1,
        );
        assert!(matches!(result, Err(Error::CardRemoved { .. })));
        assert_eq!(waits, 2);
    }

    #[test]
    fn a_device_io_error_hints_at_a_pulled_card() {
        let eio = ReadError::Io {
            offset: 0,
            source: std::io::Error::from_raw_os_error(libc::EIO),
        };
        assert_eq!(
            card_read_failure(&eio),
            Unavailable::ReadFailed(
                "read at offset 0 failed: Input/output error (os error 5); \
                 the card may have been pulled out, or it is faulty"
                    .into()
            )
        );
        let other = ReadError::Io {
            offset: 0,
            source: std::io::Error::from_raw_os_error(libc::EBUSY),
        };
        let Unavailable::ReadFailed(text) = card_read_failure(&other) else {
            panic!("not a read failure");
        };
        assert!(!text.contains("pulled"));
    }

    #[test]
    fn a_disk_smaller_than_a_sector_says_so() {
        let short = ReadError::OutOfBounds {
            offset: 0,
            len: 512,
            size: 100,
        };
        assert_eq!(
            read_failure_text(&short),
            "the disk (100 bytes) is smaller than one sector"
        );
    }

    #[test]
    fn mounts_are_listed_by_partition() {
        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let two_mounts = |disk: &DeviceName| {
            let mut second = mounts(disk)[0].clone();
            second.device = DeviceName::new(&format!("{disk}s4")).unwrap();
            second.partition = Some(PartitionNumber(4));
            let mut first = mounts(disk)[0].clone();
            first.device = DeviceName::new(&format!("{disk}s2")).unwrap();
            first.partition = Some(PartitionNumber(2));
            Ok(vec![second, first])
        };
        let profile = device_profile(devices.remove(0), two_mounts, |_| {
            Err::<Cursor<Vec<u8>>, _>(Unavailable::RequiresRoot)
        });
        let order: Vec<_> = profile
            .mounts
            .value()
            .unwrap()
            .iter()
            .map(|m| m.partition.map(|p| p.0))
            .collect();
        assert_eq!(order, [Some(2), Some(4)]);
    }

    #[test]
    fn mount_table_failure_fails_only_the_mounts() {
        let mut devices = platform::fixture_devices("builtin-sdhc-mounted");
        let profile = device_profile(
            devices.remove(0),
            |_| Err(PlatformError::Unsupported),
            |_| Err::<Cursor<Vec<u8>>, _>(Unavailable::RequiresRoot),
        );
        assert!(matches!(
            profile.mounts,
            Field::Unavailable(Unavailable::ReadFailed(_))
        ));
        assert!(profile.card.value().is_some());
    }
}
