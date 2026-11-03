//! The card itself: identity (the CID register), capacity and speed class.
//!
//! Values use the SD specification's types, whatever form the source gives
//! them in: Apple reports the revision as `"2.0"` and the serial as a decimal
//! number, Linux as hex strings, the card's CID register as bits.

use std::fmt;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use super::field::Field;

/// The card in the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Card {
    #[serde(rename = "type")]
    pub card_type: Field<CardType>,
    pub capacity_bytes: Field<u64>,
    pub manufacturer: Field<Manufacturer>,
    pub oem_id: Field<OemId>,
    pub product_name: Field<String>,
    pub product_revision: Field<ProductRevision>,
    pub serial_number: Field<SerialNumber>,
    pub manufacturing_date: Field<ManufacturingDate>,
    /// The version of the SD Physical Layer specification the card
    /// implements (its SCR register).
    pub spec_version: Field<SpecVersion>,
    pub speed_class: Field<SpeedClass>,
    pub uhs_ii_supported: Field<bool>,
    pub write_protect: WriteProtect,
}

/// The card's capacity type, which also fixes how it addresses blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CardType {
    /// Standard capacity, up to 2 GB, byte addressed.
    Sdsc,
    /// High capacity, over 2 GB up to 32 GB.
    Sdhc,
    /// Extended capacity, over 32 GB up to 2 TB.
    Sdxc,
    /// Ultra capacity, over 2 TB up to 128 TB.
    Sduc,
}

impl fmt::Display for CardType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sdsc => "SDSC",
            Self::Sdhc => "SDHC",
            Self::Sdxc => "SDXC",
            Self::Sduc => "SDUC",
        })
    }
}

/// The manufacturer ID (CID `MID`), assigned by the SD Association, and the
/// name it is known to stand for. The SD Association doesn't publish the
/// list, so `name` is `null` for IDs nobody has matched to a company.
///
/// Serialized as `{"id": "0x03", "name": "SanDisk"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manufacturer {
    id: u8,
}

impl Manufacturer {
    #[must_use]
    pub fn new(id: u8) -> Self {
        Self { id }
    }

    #[must_use]
    pub fn id(self) -> u8 {
        self.id
    }

    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        crate::sd::manufacturers::name(self.id)
    }
}

impl fmt::Display for Manufacturer {
    /// `SanDisk (0x03)`, or the ID alone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{name} ({:#04x})", self.id),
            None => write!(f, "{:#04x}", self.id),
        }
    }
}

impl Serialize for Manufacturer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("id", &format!("{:#04x}", self.id))?;
        map.serialize_entry("name", &self.name())?;
        map.end()
    }
}

/// The OEM/application ID (CID `OID`): two bytes, by convention two ASCII
/// characters such as `"SD"` or `"PT"`.
///
/// Serialized as `{"id": "0x5344", "text": "SD"}`, with `text` `null` when
/// the bytes aren't printable ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OemId(pub u16);

impl OemId {
    /// The two bytes as text, if both are printable ASCII.
    #[must_use]
    pub fn text(self) -> Option<String> {
        let bytes = self.0.to_be_bytes();
        bytes
            .iter()
            .all(|b| b.is_ascii_graphic() || *b == b' ')
            .then(|| bytes.iter().map(|&b| char::from(b)).collect())
    }
}

impl Serialize for OemId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("id", &format!("{:#06x}", self.0))?;
        map.serialize_entry("text", &self.text())?;
        map.end()
    }
}

/// The product revision (CID `PRV`): two BCD digits, `n.m`.
///
/// Serialized as `"2.0"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductRevision {
    major: u8,
    minor: u8,
}

impl ProductRevision {
    /// `None` unless both parts are single decimal digits, as the BCD
    /// encoding requires.
    #[must_use]
    pub fn new(major: u8, minor: u8) -> Option<Self> {
        (major <= 9 && minor <= 9).then_some(Self { major, minor })
    }
}

impl fmt::Display for ProductRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl Serialize for ProductRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The product serial number (CID `PSN`), 32 bits.
///
/// Serialized as `"0x0000039e"`, the way Linux shows it in
/// `/sys/block/mmcblk0/device/serial`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialNumber(pub u32);

impl fmt::Display for SerialNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

impl Serialize for SerialNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The manufacturing date (CID `MDT`): year and month, 2000 to 2255.
///
/// Serialized as `"2020-10"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManufacturingDate {
    year: u16,
    month: u8,
}

impl ManufacturingDate {
    /// `None` outside the years the CID can encode or for a month outside
    /// 1 to 12.
    #[must_use]
    pub fn new(year: u16, month: u8) -> Option<Self> {
        ((2000..=2255).contains(&year) && (1..=12).contains(&month)).then_some(Self { year, month })
    }
}

impl fmt::Display for ManufacturingDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:02}", self.year, self.month)
    }
}

impl Serialize for ManufacturingDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A version of the SD Physical Layer specification, exact or as a lower
/// bound.
///
/// The card's SCR register tells versions apart only down to the major
/// number from 3.0 on, so a 3.01 card reports 3.0. macOS goes further and
/// reports every card from 3.0 up as 3.0, which makes its value a lower
/// bound. Serialized as `"3.0"`, or `"3.0+"` for a lower bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecVersion {
    major: u8,
    minor: u8,
    bound: VersionBound,
}

/// Whether a [`SpecVersion`] is the card's own or a lower bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionBound {
    Exact,
    OrLater,
}

impl SpecVersion {
    /// `None` for a major version the SCR can't express (only 1 to 9).
    #[must_use]
    pub fn new(major: u8, minor: u8) -> Option<Self> {
        (1..=9).contains(&major).then_some(Self {
            major,
            minor,
            bound: VersionBound::Exact,
        })
    }

    /// This version or any later one.
    #[must_use]
    pub fn or_later(self) -> Self {
        Self {
            bound: VersionBound::OrLater,
            ..self
        }
    }

    #[must_use]
    pub fn is_lower_bound(self) -> bool {
        self.bound == VersionBound::OrLater
    }
}

impl fmt::Display for SpecVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if self.is_lower_bound() {
            f.write_str("+")?;
        }
        Ok(())
    }
}

impl Serialize for SpecVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The speed class from the card's SD Status register: the minimum
/// sequential write speed it promises, in MB/s. Class 0 makes no promise.
///
/// Serialized as the class number: `0`, `2`, `4`, `6` or `10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedClass {
    Class0,
    Class2,
    Class4,
    Class6,
    Class10,
}

impl SpeedClass {
    /// Decodes the SD Status register's `SPEED_CLASS` field.
    #[must_use]
    pub fn from_register(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Class0,
            1 => Self::Class2,
            2 => Self::Class4,
            3 => Self::Class6,
            4 => Self::Class10,
            _ => return None,
        })
    }

    /// The class number, which is also the promised minimum write speed in
    /// MB/s.
    #[must_use]
    pub fn number(self) -> u8 {
        match self {
            Self::Class0 => 0,
            Self::Class2 => 2,
            Self::Class4 => 4,
            Self::Class6 => 6,
            Self::Class10 => 10,
        }
    }
}

impl Serialize for SpeedClass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.number())
    }
}

/// Whether writes to the card are blocked, and by what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteProtect {
    /// The lock switch on the side of the card (or its adapter) is in the
    /// locked position.
    pub switch: Field<bool>,
    /// The card's CSD register has its temporary or permanent write
    /// protection bit set.
    pub register: Field<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oem_id_text_only_when_printable() {
        assert_eq!(OemId(0x5344).text().as_deref(), Some("SD"));
        assert_eq!(OemId(0x3428).text().as_deref(), Some("4("));
        assert_eq!(OemId(0x0000).text(), None);
    }

    #[test]
    fn identity_values_serialize_like_linux_shows_them() {
        assert_eq!(
            serde_json::to_string(&SerialNumber(926)).unwrap(),
            r#""0x0000039e""#
        );
        assert_eq!(
            serde_json::to_string(&ManufacturingDate::new(2020, 10).unwrap()).unwrap(),
            r#""2020-10""#
        );
        assert_eq!(
            serde_json::to_string(&Manufacturer::new(3)).unwrap(),
            r#"{"id":"0x03","name":"SanDisk"}"#
        );
        assert_eq!(
            serde_json::to_string(&OemId(0x3428)).unwrap(),
            r#"{"id":"0x3428","text":"4("}"#
        );
    }

    #[test]
    fn invalid_values_are_rejected() {
        assert!(ProductRevision::new(10, 0).is_none());
        assert!(ManufacturingDate::new(2020, 13).is_none());
        assert!(ManufacturingDate::new(1999, 1).is_none());
        assert!(SpecVersion::new(0, 0).is_none());
    }

    #[test]
    fn spec_version_lower_bound_has_a_plus() {
        let v3 = SpecVersion::new(3, 0).unwrap();
        assert_eq!(serde_json::to_string(&v3).unwrap(), r#""3.0""#);
        assert_eq!(serde_json::to_string(&v3.or_later()).unwrap(), r#""3.0+""#);
    }

    #[test]
    fn speed_class_follows_the_sd_status_encoding() {
        assert_eq!(SpeedClass::from_register(4), Some(SpeedClass::Class10));
        assert_eq!(SpeedClass::from_register(1), Some(SpeedClass::Class2));
        assert_eq!(SpeedClass::from_register(5), None);
        assert_eq!(serde_json::to_string(&SpeedClass::Class10).unwrap(), "10");
    }
}
