//! CRC-32 (IEEE 802.3, the variant GPT uses for its header and entry array
//! checksums).

const POLYNOMIAL: u32 = 0xEDB8_8320;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)] // i < 256, so it fits in u32.
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

pub fn checksum(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &byte| {
        TABLE[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(checksum(b""), 0);
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            checksum(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }
}
