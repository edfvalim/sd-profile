//! macOS: the I/O Registry for readers and cards, `getfsstat` for mounts.
//!
//! `iokit.rs` and `mounts.rs` call the OS and build on macOS only.
//! `tree.rs` and `map.rs` build everywhere, so the mapping can be tested on
//! any OS against the `ioreg` captures in `tests/fixtures/macos/`.

// On other OSes nothing calls the mapping outside its tests.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
mod iokit;
mod map;
mod mount_table;
#[cfg(target_os = "macos")]
mod mounts;
pub mod names;
mod tree;

#[cfg(target_os = "macos")]
pub use iokit::sd_devices;

#[cfg(test)]
pub fn fixture_devices(name: &str) -> Vec<crate::platform::SdDevice> {
    map::built_in_readers(&tree::ioreg::fixture(name))
}
#[cfg(target_os = "macos")]
pub use mounts::mounts_of;
