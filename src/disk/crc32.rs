//! CRC-32 in the two variants the on-disk formats use: IEEE 802.3 (GPT
//! header and entry array) and Castagnoli, CRC-32C (ext4 metadata checksums).

/// A reflected CRC-32 with its byte lookup table.
struct Crc32 {
    table: [u32; 256],
}

impl Crc32 {
    /// Builds the table for a reflected polynomial.
    const fn new(polynomial: u32) -> Self {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            #[allow(clippy::cast_possible_truncation)] // i < 256, so it fits in u32.
            let mut crc = i as u32;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ polynomial
                } else {
                    crc >> 1
                };
                bit += 1;
            }
            table[i] = crc;
            i += 1;
        }
        Self { table }
    }

    /// The standard checksum: initial value and final XOR both all ones.
    fn checksum(&self, bytes: &[u8]) -> u32 {
        !bytes.iter().fold(!0u32, |crc, &byte| {
            self.table[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8)
        })
    }
}

const IEEE: Crc32 = Crc32::new(0xEDB8_8320);
const CASTAGNOLI: Crc32 = Crc32::new(0x82F6_3B78);

/// CRC-32 (IEEE 802.3), as GPT uses it.
pub fn checksum(bytes: &[u8]) -> u32 {
    IEEE.checksum(bytes)
}

/// CRC-32C (Castagnoli), the standard form with the final XOR.
pub fn castagnoli(bytes: &[u8]) -> u32 {
    CASTAGNOLI.checksum(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ieee_known_values() {
        assert_eq!(checksum(b""), 0);
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            checksum(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn castagnoli_known_values() {
        // Check value from the CRC catalogue (CRC-32/ISCSI) and RFC 3720.
        assert_eq!(castagnoli(b"123456789"), 0xE306_9283);
        assert_eq!(castagnoli(&[0u8; 32]), 0x8A91_36AA);
    }
}
