//! SD card manufacturer IDs (CID `MID`) to names.
//!
//! The SD Association assigns these IDs and doesn't publish the list, so
//! every table is pieced together from cards people have read. This one
//! keeps only the IDs that a primary source names with reasonable
//! confidence:
//!
//! - `linux`: the `CID_MANFID_*` constants in the Linux kernel's
//!   `drivers/mmc/core/card.h`, used by SD quirks in `quirks.h`.
//! - `mmc-utils`: the `sdcard.ids` file that `lsmmc` reads.
//!
//! Where one ID is sold under several brands, the name is the company that
//! makes the cards (for 0x27, the controller vendor Phison). Sources and
//! confidence per ID: `.scratch/docs/research/2026-11-02T1935-macos-iokit-sd.md`.
//! Unbranded and counterfeit cards often carry IDs nobody has matched; those
//! stay unnamed rather than guessed.

/// The company a manufacturer ID is known to stand for.
#[must_use]
pub fn name(id: u8) -> Option<&'static str> {
    MANUFACTURERS
        .binary_search_by_key(&id, |&(mid, _)| mid)
        .ok()
        .map(|i| MANUFACTURERS[i].1)
}

/// Sorted by ID.
const MANUFACTURERS: &[(u8, &str)] = &[
    (0x01, "Panasonic"),     // mmc-utils
    (0x02, "Toshiba"),       // mmc-utils: "Toshiba/Kingston/Viking"
    (0x03, "SanDisk"),       // linux CID_MANFID_SANDISK_SD, mmc-utils
    (0x08, "Silicon Power"), // mmc-utils
    (0x09, "ATP"),           // linux CID_MANFID_ATP, mmc-utils
    (0x12, "Gigastone"),     // linux CID_MANFID_GIGASTONE
    (0x1b, "Samsung"),       // mmc-utils: "Transcend/Samsung"
    (0x1c, "Transcend"),     // mmc-utils
    (0x1d, "ADATA"),         // mmc-utils: "Corsair/AData"
    (0x1f, "Kingston"),      // mmc-utils
    (0x27, "Phison"),        // linux CID_MANFID_APACER, mmc-utils: "Delkin/Phison"
    (0x28, "Lexar"),         // mmc-utils
    (0x41, "Kingston"),      // mmc-utils
    (0x5d, "Swissbit"),      // linux CID_MANFID_SWISSBIT
    (0x74, "Transcend"),     // mmc-utils
    (0x76, "Patriot"),       // mmc-utils
    (0x82, "Sony"),          // mmc-utils: "Gobe/Sony"
    (0x9f, "Kingston"),      // linux CID_MANFID_KINGSTON_SD
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_for_binary_search() {
        assert!(MANUFACTURERS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn known_and_unknown_ids() {
        assert_eq!(name(0x03), Some("SanDisk"));
        assert_eq!(name(0x9f), Some("Kingston"));
        assert_eq!(name(0xdf), None);
    }
}
