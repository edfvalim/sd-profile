//! Globally unique identifiers (GPT disk, partition and type GUIDs).

use std::fmt;

use serde::{Serialize, Serializer};

/// A GUID, stored in the byte order it is written in
/// (`C12A7328-F81F-11D2-BA4B-00A0C93EC93B` is `[0xC1, 0x2A, ...]`).
///
/// GPT stores the first three groups little-endian on disk; use
/// [`Guid::from_mixed_endian`] for those bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid([u8; 16]);

impl Guid {
    pub const NIL: Self = Self([0; 16]);

    /// Builds a GUID from its on-disk GPT form, where the first three groups
    /// are little-endian.
    #[must_use]
    pub const fn from_mixed_endian(bytes: [u8; 16]) -> Self {
        let b = bytes;
        Self([
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ])
    }

    /// Parses the canonical text form at compile time, for tables of known
    /// GUIDs.
    ///
    /// # Panics
    ///
    /// Panics (a compile error in const context) if `text` is not
    /// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` in hex digits.
    #[must_use]
    pub const fn parse_const(text: &str) -> Self {
        const fn nibble(c: u8) -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => panic!("GUID text has a character that is not a hex digit"),
            }
        }
        let text = text.as_bytes();
        assert!(text.len() == 36, "GUID text must be 36 characters long");
        let mut out = [0u8; 16];
        let mut i = 0;
        let mut j = 0;
        while i < text.len() {
            if matches!(i, 8 | 13 | 18 | 23) {
                assert!(text[i] == b'-', "GUID text has no dash where one belongs");
                i += 1;
                continue;
            }
            out[j] = (nibble(text[i]) << 4) | nibble(text[i + 1]);
            i += 2;
            j += 1;
        }
        Self(out)
    }

    #[must_use]
    pub fn is_nil(&self) -> bool {
        *self == Self::NIL
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

impl Serialize for Guid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESP: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

    #[test]
    fn parses_and_formats_round_trip() {
        assert_eq!(Guid::parse_const(ESP).to_string(), ESP);
    }

    #[test]
    fn parse_accepts_lowercase() {
        assert_eq!(
            Guid::parse_const(&ESP.to_lowercase()),
            Guid::parse_const(ESP)
        );
    }

    #[test]
    fn mixed_endian_swaps_first_three_groups() {
        // The EFI System Partition type GUID as GPT stores it on disk.
        let on_disk = [
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B,
        ];
        assert_eq!(Guid::from_mixed_endian(on_disk), Guid::parse_const(ESP));
    }

    #[test]
    fn nil_is_nil() {
        assert!(Guid::NIL.is_nil());
        assert!(!Guid::parse_const(ESP).is_nil());
    }
}
