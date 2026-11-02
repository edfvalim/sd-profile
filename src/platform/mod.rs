//! Access to the operating system.
//!
//! Only the code that calls the OS is OS-specific, and all conditional
//! compilation stays in this module. Parsing and mapping code elsewhere builds
//! on every OS, so it can be tested anywhere against captured fixtures.

#[cfg(not(unix))]
compile_error!("sd-profile supports macOS and Linux only");

mod unix;

pub use unix::{OpenError, TargetKind, open_read_only};
