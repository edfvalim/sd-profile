//! The card reader: its hardware, how it connects to the computer, and the
//! bus between it and the card.

use std::fmt;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use super::field::Field;

/// The reader the card sits in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reader {
    pub connection: Connection,
    pub vendor_name: Field<String>,
    pub product_name: Field<String>,
    /// The reader can run UHS-II, the second row of pins on newer cards.
    pub uhs_ii_supported: Field<bool>,
    /// The bus to the card as the reader last set it up.
    pub card_link: Field<CardLink>,
}

/// How the reader is attached to the computer, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Connection {
    /// A reader wired into the computer, which talks to the card directly
    /// and so can report the card's registers.
    BuiltIn {
        /// The reader's PCI function, `null` for readers on a bus without
        /// PCI IDs (the SD host controllers built into ARM chips) or when the
        /// OS doesn't publish them.
        pci: Option<PciDevice>,
    },
    /// A USB adapter, which hides the card behind a USB mass storage device.
    Usb(UsbDevice),
}

/// A PCI function's IDs and its PCI Express link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PciDevice {
    pub vendor_id: HardwareId,
    pub device_id: HardwareId,
    pub subsystem_vendor_id: HardwareId,
    pub subsystem_id: HardwareId,
    pub revision_id: PciRevision,
    pub link: Field<PcieLink>,
}

/// A 16-bit PCI or USB vendor, device, product or subsystem ID. Serialized
/// as `"0x17a0"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareId(pub u16);

impl fmt::Display for HardwareId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#06x}", self.0)
    }
}

impl Serialize for HardwareId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// An 8-bit PCI revision ID. Serialized as `"0x01"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciRevision(pub u8);

impl fmt::Display for PciRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#04x}", self.0)
    }
}

impl Serialize for PciRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The negotiated PCI Express link, from the Link Status register.
///
/// Serialized as `{"generation": 2, "transfer_rate": "5.0 GT/s",
/// "width": 1}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcieLink {
    generation: PcieGeneration,
    width: u8,
}

impl PcieLink {
    #[must_use]
    pub fn new(generation: PcieGeneration, width: u8) -> Self {
        Self { generation, width }
    }

    #[must_use]
    pub fn generation(self) -> PcieGeneration {
        self.generation
    }

    /// Number of lanes.
    #[must_use]
    pub fn width(self) -> u8 {
        self.width
    }
}

impl Serialize for PcieLink {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("generation", &self.generation.number())?;
        map.serialize_entry("transfer_rate", self.generation.transfer_rate())?;
        map.serialize_entry("width", &self.width)?;
        map.end()
    }
}

/// A PCI Express generation, which fixes the per-lane transfer rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcieGeneration {
    Gen1,
    Gen2,
    Gen3,
    Gen4,
    Gen5,
    Gen6,
}

impl PcieGeneration {
    /// Decodes the Current Link Speed field of the Link Status register.
    #[must_use]
    pub fn from_link_speed(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Gen1,
            2 => Self::Gen2,
            3 => Self::Gen3,
            4 => Self::Gen4,
            5 => Self::Gen5,
            6 => Self::Gen6,
            _ => return None,
        })
    }

    #[must_use]
    pub fn number(self) -> u8 {
        match self {
            Self::Gen1 => 1,
            Self::Gen2 => 2,
            Self::Gen3 => 3,
            Self::Gen4 => 4,
            Self::Gen5 => 5,
            Self::Gen6 => 6,
        }
    }

    /// Per-lane transfer rate, written as `lspci` and `system_profiler`
    /// write it.
    #[must_use]
    pub fn transfer_rate(self) -> &'static str {
        match self {
            Self::Gen1 => "2.5 GT/s",
            Self::Gen2 => "5.0 GT/s",
            Self::Gen3 => "8.0 GT/s",
            Self::Gen4 => "16.0 GT/s",
            Self::Gen5 => "32.0 GT/s",
            Self::Gen6 => "64.0 GT/s",
        }
    }
}

/// A USB device's IDs and names, as its descriptors give them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsbDevice {
    pub vendor_id: HardwareId,
    pub product_id: HardwareId,
    pub speed: Field<UsbSpeed>,
}

/// The USB signalling rate the device runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbSpeed {
    /// USB 1.x, 1.5 Mbit/s.
    Low,
    /// USB 1.x, 12 Mbit/s.
    Full,
    /// USB 2.0, 480 Mbit/s.
    High,
    /// USB 3.x Gen 1, 5 Gbit/s.
    Super,
    /// USB 3.x Gen 2, 10 Gbit/s.
    SuperPlus,
    /// USB 3.2 Gen 2x2, 20 Gbit/s.
    SuperPlusX2,
}

/// The SD bus between the reader and the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CardLink {
    pub speed_mode: SpeedMode,
    pub bus_width: BusWidth,
    /// The SD clock the reader drives, in kHz. It drops to 400 kHz or less
    /// while the card is being identified.
    pub clock_khz: u32,
}

/// The SD bus speed mode. The UHS-I modes need 1.8 V signalling and a card
/// and reader that both support them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedMode {
    /// Up to 25 MHz, 12.5 MB/s.
    DefaultSpeed,
    /// Up to 50 MHz, 25 MB/s.
    HighSpeed,
    /// UHS-I, up to 25 MHz, 12.5 MB/s.
    Sdr12,
    /// UHS-I, up to 50 MHz, 25 MB/s.
    Sdr25,
    /// UHS-I, up to 100 MHz, 50 MB/s.
    Sdr50,
    /// UHS-I, up to 208 MHz, 104 MB/s.
    Sdr104,
    /// UHS-I, up to 50 MHz on both clock edges, 50 MB/s.
    Ddr50,
    /// UHS-II, the second row of pins: serial lanes at 156 or 312 MB/s.
    UhsII,
    /// An MMC on its own bus timing, which the driver doesn't
    /// break down further.
    Mmc,
}

impl fmt::Display for SpeedMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DefaultSpeed => "Default Speed",
            Self::HighSpeed => "High Speed",
            Self::Sdr12 => "UHS-I SDR12",
            Self::Sdr25 => "UHS-I SDR25",
            Self::Sdr50 => "UHS-I SDR50",
            Self::Sdr104 => "UHS-I SDR104",
            Self::Ddr50 => "UHS-I DDR50",
            Self::UhsII => "UHS-II",
            Self::Mmc => "MMC",
        })
    }
}

/// Number of data lines on the SD bus.
///
/// Serialized as the number of lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusWidth {
    One,
    Four,
    /// eMMC and SD Express only; listed so a reader reporting it isn't
    /// rejected.
    Eight,
}

impl BusWidth {
    #[must_use]
    pub fn from_lines(lines: u64) -> Option<Self> {
        Some(match lines {
            1 => Self::One,
            4 => Self::Four,
            8 => Self::Eight,
            _ => return None,
        })
    }

    #[must_use]
    pub fn lines(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Four => 4,
            Self::Eight => 8,
        }
    }
}

impl Serialize for BusWidth {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.lines())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcie_link_serializes_generation_rate_and_width() {
        let link = PcieLink::new(PcieGeneration::Gen2, 1);
        assert_eq!(
            serde_json::to_string(&link).unwrap(),
            r#"{"generation":2,"transfer_rate":"5.0 GT/s","width":1}"#
        );
    }

    #[test]
    fn ids_serialize_as_hex() {
        assert_eq!(
            serde_json::to_string(&HardwareId(0x17a0)).unwrap(),
            r#""0x17a0""#
        );
        assert_eq!(serde_json::to_string(&PciRevision(1)).unwrap(), r#""0x01""#);
    }
}
