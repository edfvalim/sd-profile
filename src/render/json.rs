//! The JSON output: the [`Profile`] as serialized by the model. Its shape is
//! versioned by `schema_version`.

use std::io::{self, Write};

use crate::model::Profile;

/// Writes the profile as pretty-printed JSON followed by a newline.
///
/// # Errors
///
/// When writing to `out` fails.
pub fn write(profile: &Profile, out: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, profile)?;
    writeln!(out)
}
