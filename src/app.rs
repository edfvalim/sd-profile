//! The pipeline: open the target, read the disk, build the profile, print it.

use std::io::Write;

use crate::cli::{Args, Format};
use crate::disk::{self, block::AlignedReader};
use crate::error::Error;
use crate::model::{BlockSize, Field, Layout, Profile, Source, Target, Unavailable};
use crate::platform::{self, TargetKind};
use crate::render;

/// Profiles the target named in `args` and writes the report to `out`.
///
/// # Errors
///
/// When the target can't be opened or isn't supported, or the output can't
/// be written. Read failures after opening end up in the profile as
/// unavailable fields instead.
pub fn run(args: &Args, out: &mut impl Write) -> Result<(), Error> {
    let profile = profile(args)?;
    match args.format() {
        Format::Human => render::human::write(&profile, out),
        Format::Json => render::json::write(&profile, out),
    }
    .map_err(Error::Output)
}

fn profile(args: &Args) -> Result<Profile, Error> {
    let opened = platform::open_read_only(&args.target)?;
    if opened.kind == TargetKind::Device {
        return Err(Error::DeviceNotSupported {
            path: args.target.clone(),
        });
    }

    // Images have no sector size of their own; use the one every SD card has.
    let block_size = BlockSize::SD;
    let mut reader = AlignedReader::new(opened.file, block_size, opened.size);
    let partition_table = match disk::table::read(&mut reader) {
        Ok(table) => Field::known(table, Source::PartitionTable),
        Err(err) => Field::Unavailable(Unavailable::ReadFailed(err.to_string())),
    };

    Ok(Profile::new(
        Target::DiskImage {
            path: args.target.clone(),
        },
        Layout {
            size_bytes: opened.size,
            block_size,
            partition_table,
        },
    ))
}
