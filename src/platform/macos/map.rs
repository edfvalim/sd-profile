//! I/O Registry subtrees to the model. Builds on every OS.
//!
//! The built-in reader is a PCI device with Apple's `AppleSDXC` driver on it.
//! Below that, one `AppleSDXCSlot` per slot carries the card's identity and
//! the bus setup, and the card's disk hangs off the slot:
//!
//! ```text
//! IOPCIDevice                          vendor-id, device-id, link status
//!   AppleSDXC
//!     AppleSDXCSlot                    Card Characteristics, Speed Mode, Bus Width
//!       AppleSDXCBlockStorageDevice    Device Characteristics (reader name)
//!         IOBlockStorageDriver
//!           IOMedia (Whole)            BSD Name, Size, Preferred Block Size
//! ```
//!
//! A USB reader is a USB device with a mass storage driver; it shows the
//! disk but not the card:
//!
//! ```text
//! IOUSBHostDevice                      idVendor, idProduct, names, USBSpeed
//!   ... IOSCSIPeripheralDeviceType00   Protocol Characteristics: USB
//!         IOBlockStorageDriver
//!           IOMedia (Whole)
//! ```
//!
//! Property names and encodings are in
//! `.scratch/docs/research/2026-11-02T1935-macos-iokit-sd.md`.

use std::path::PathBuf;

use super::tree::{Entry, Value};
use crate::model::{
    BlockSize, BusWidth, Card, CardLink, CardType, Connection, DeviceName, Field, HardwareId,
    Manufacturer, ManufacturingDate, OemId, PciDevice, PciRevision, PcieGeneration, PcieLink,
    ProductRevision, Reader, SerialNumber, Source, SpecVersion, SpeedClass, SpeedMode, Unavailable,
    UsbDevice, UsbSpeed, WriteProtect,
};
use crate::platform::{Disk, SdDevice, UsbScope};

/// Why a value couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Problem {
    /// The driver doesn't publish the property (for some cards it leaves
    /// keys out).
    Absent,
    /// The property is there but holds something unexpected; the message
    /// names it.
    Invalid(String),
}

/// A [`Problem::Invalid`] with a formatted message.
macro_rules! invalid {
    ($($arg:tt)*) => {
        Problem::Invalid(format!($($arg)*))
    };
}

type Read<T> = Result<T, Problem>;

fn field<T>(read: Read<T>) -> Field<T> {
    match read {
        Ok(value) => Field::known(value, Source::IoRegistry),
        Err(problem) => Field::Unavailable(unavailable(problem)),
    }
}

fn unavailable(problem: Problem) -> Unavailable {
    match problem {
        Problem::Absent => Unavailable::NotExposedByOs,
        Problem::Invalid(detail) => Unavailable::ReadFailed(detail),
    }
}

/// The SD card readers in subtrees rooted at built-in readers' PCI devices.
/// Each slot is one device.
#[must_use]
pub fn built_in_readers(pci_devices: &[Entry]) -> Vec<SdDevice> {
    pci_devices
        .iter()
        .flat_map(|pci| {
            pci.descendants()
                .filter(|e| e.class == "AppleSDXCSlot")
                .map(move |slot| built_in_slot(pci, slot))
        })
        .collect()
}

fn built_in_slot(pci: &Entry, slot: &Entry) -> SdDevice {
    let state = SlotState::of(slot);
    let storage = slot.find("AppleSDXCBlockStorageDevice");
    let disk = match state {
        SlotState::Active => storage.and_then(whole_disk),
        SlotState::Empty | SlotState::Inactive => None,
    };
    let card = match state {
        SlotState::Empty => Field::Unavailable(Unavailable::NoCard),
        SlotState::Active | SlotState::Inactive => Field::known(card(slot), Source::IoRegistry),
    };
    let card_link = match state {
        SlotState::Empty => Field::Unavailable(Unavailable::NoCard),
        SlotState::Inactive => Field::Unavailable(Unavailable::CardInactive),
        SlotState::Active => field(card_link(slot)),
    };
    let characteristics = storage.and_then(|s| s.get("Device Characteristics"));
    let reader = Reader {
        connection: Connection::BuiltIn {
            pci: pci_device(pci),
        },
        vendor_name: field(str_of(&characteristics, "Vendor Name")),
        product_name: field(str_of(&characteristics, "Product Name")),
        uhs_ii_supported: field(bool_of(slot, "Host Supports UHS-II")),
        card_link,
    };
    SdDevice {
        disk,
        card,
        reader,
        source: Source::IoRegistry,
    }
}

/// Whether the slot holds a card, and whether the driver has set it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Empty,
    /// Present but not configured: ejected, or not set up yet. The slot
    /// keeps the card's identity, but the bus values are the last ones used
    /// and the clock reads as the idle 25 MHz.
    Inactive,
    Active,
}

impl SlotState {
    fn of(slot: &Entry) -> Self {
        let flag = |key| slot.get(key).and_then(Value::as_bool).unwrap_or(false);
        match (flag("Card Present"), flag("Card Configured")) {
            (false, _) => Self::Empty,
            (true, false) => Self::Inactive,
            (true, true) => Self::Active,
        }
    }
}

fn card(slot: &Entry) -> Card {
    let c = slot.get("Card Characteristics");
    Card {
        card_type: field(str_of(&c, "Card Type").and_then(|s| card_type(&s))),
        capacity_bytes: field(u64_of(&c, "Block Count").and_then(|blocks| {
            blocks
                .checked_mul(u64::from(BlockSize::SD.bytes()))
                .ok_or_else(|| invalid!("\"Block Count\" {blocks} overflows"))
        })),
        manufacturer: field(
            u64_of(&c, "Manufacturer ID")
                .and_then(|id| narrow::<u8>(id, "Manufacturer ID").map(Manufacturer::new)),
        ),
        oem_id: field(
            u64_of(&c, "Application ID").and_then(|id| narrow(id, "Application ID").map(OemId)),
        ),
        product_name: field(str_of(&c, "Product Name").map(|name| product_name(&name))),
        product_revision: field(
            str_of(&c, "Product Revision Level")
                .and_then(|s| parse_pair(&s, '.', "Product Revision Level"))
                .and_then(|(major, minor)| {
                    ProductRevision::new(major, minor).ok_or_else(|| {
                        invalid!("\"Product Revision Level\" {major}.{minor} is not BCD")
                    })
                }),
        ),
        // Published as a 32-bit number, which comes back negative (sign
        // extended) when the top bit is set; the low 32 bits are the serial.
        serial_number: field(
            u64_of(&c, "Serial Number")
                .map(|n| SerialNumber(u32::try_from(n & 0xffff_ffff).expect("masked to 32 bits"))),
        ),
        manufacturing_date: field(str_of(&c, "Manufacturing Date").and_then(|s| {
            let (year, month) = parse_pair::<u16, u8>(&s, '-', "Manufacturing Date")?;
            ManufacturingDate::new(year, month)
                .ok_or_else(|| invalid!("\"Manufacturing Date\" {s:?} is out of range"))
        })),
        spec_version: field(str_of(&c, "Specification Version").and_then(|s| spec_version(&s))),
        speed_class: field(u64_of(&c, "Speed Class").and_then(|n| {
            u8::try_from(n)
                .ok()
                .and_then(SpeedClass::from_register)
                .ok_or_else(|| invalid!("unrecognized \"Speed Class\" {n}"))
        })),
        uhs_ii_supported: field(bool_of(&c, "UHS-II Supported")),
        write_protect: WriteProtect {
            switch: field(bool_of(&c, "Write Protect Enabled")),
            register: field(bool_of(&c, "CSD Protect Enabled")),
        },
    }
}

/// `AppleSDXC`'s table has four entries, `1.0x`, `1.1`, `2.0` and `3.0`, so
/// `3.0` stands for 3.0 and every later version.
fn spec_version(s: &str) -> Read<SpecVersion> {
    match s {
        "1.0x" => SpecVersion::new(1, 0),
        "1.1" => SpecVersion::new(1, 10),
        "2.0" => SpecVersion::new(2, 0),
        "3.0" => SpecVersion::new(3, 0).map(SpecVersion::or_later),
        _ => None,
    }
    .ok_or_else(|| invalid!("unrecognized \"Specification Version\" {s:?}"))
}

fn card_type(s: &str) -> Read<CardType> {
    Ok(match s {
        "SDSC" => CardType::Sdsc,
        "SDHC" => CardType::Sdhc,
        "SDXC" => CardType::Sdxc,
        "SDUC" => CardType::Sduc,
        _ => return Err(invalid!("unrecognized \"Card Type\" {s:?}")),
    })
}

/// The CID's five-character product name, without the padding some cards
/// fill it out with.
fn product_name(raw: &str) -> String {
    raw.trim_end_matches(['\0', ' ']).to_owned()
}

fn card_link(slot: &Entry) -> Read<CardLink> {
    let mode = str_of(slot, "Speed Mode")?;
    let lines = u64_of(slot, "Bus Width")?;
    // Below 1 MHz (while identifying the card) the driver publishes the
    // clock under a kHz key instead.
    let clock_khz = match u64_of(slot, "Calculated Clock Frequency (MHz)") {
        Ok(mhz) => mhz.saturating_mul(1000),
        Err(Problem::Absent) => u64_of(slot, "Calculated Clock Frequency (KHz)")?,
        Err(problem) => return Err(problem),
    };
    Ok(CardLink {
        speed_mode: speed_mode(&mode)?,
        bus_width: BusWidth::from_lines(lines)
            .ok_or_else(|| invalid!("unrecognized \"Bus Width\" {lines}"))?,
        clock_khz: narrow(clock_khz, "Calculated Clock Frequency")?,
    })
}

/// Apple's `Speed Mode` strings. The driver also writes `UHS: NotSet`,
/// `MMC` and `Unknown`, which name no mode.
fn speed_mode(s: &str) -> Read<SpeedMode> {
    Ok(match s {
        "NormalSpeed" => SpeedMode::DefaultSpeed,
        "HighSpeed" => SpeedMode::HighSpeed,
        "UHS: SDR12" => SpeedMode::Sdr12,
        "UHS: SDR25" => SpeedMode::Sdr25,
        "UHS: SDR50" => SpeedMode::Sdr50,
        "UHS: SDR104" => SpeedMode::Sdr104,
        "UHS: DDR50" => SpeedMode::Ddr50,
        "UHS-II" => SpeedMode::UhsII,
        _ => return Err(invalid!("unrecognized \"Speed Mode\" {s:?}")),
    })
}

/// The PCI function's IDs, which the registry keeps as the little-endian
/// 32-bit cells of the device tree. `None` when they can't be read.
fn pci_device(pci: &Entry) -> Option<PciDevice> {
    let id = |key: &str| {
        cell(pci, key)
            .and_then(|v| u16::try_from(v).ok())
            .map(HardwareId)
    };
    Some(PciDevice {
        vendor_id: id("vendor-id")?,
        device_id: id("device-id")?,
        subsystem_vendor_id: id("subsystem-vendor-id")?,
        subsystem_id: id("subsystem-id")?,
        revision_id: PciRevision(u8::try_from(cell(pci, "revision-id")?).ok()?),
        link: pcie_link(pci),
    })
}

fn cell(entry: &Entry, key: &str) -> Option<u32> {
    let data = entry.get(key)?.as_data()?;
    Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?))
}

/// The PCI Express Link Status register: current speed in bits 0-3,
/// negotiated width in bits 4-9. A powered-down reader reads as all ones.
fn pcie_link(pci: &Entry) -> Field<PcieLink> {
    let status = match u64_of(pci, "IOPCIExpressLinkStatus") {
        Ok(status) => status,
        Err(problem) => return Field::Unavailable(unavailable(problem)),
    };
    // system_profiler treats a low byte of all ones as "Off".
    if status & 0xff == 0xff {
        return Field::Unavailable(Unavailable::PoweredDown);
    }
    let speed = u8::try_from(status & 0xf).expect("a 4-bit field fits in u8");
    let width = u8::try_from((status >> 4) & 0x3f).expect("a 6-bit field fits in u8");
    field(
        PcieGeneration::from_link_speed(speed)
            .map(|generation| PcieLink::new(generation, width))
            .ok_or_else(|| {
                invalid!("unrecognized link speed {speed} in \"IOPCIExpressLinkStatus\"")
            }),
    )
}

/// The whole-disk media below a storage device, once the OS has made one.
fn whole_disk(storage: &Entry) -> Option<Disk> {
    let media = storage
        .descendants()
        .find(|e| e.class == "IOMedia" && e.get("Whole").and_then(Value::as_bool) == Some(true))?;
    let name = DeviceName::new(media.get("BSD Name")?.as_str()?)?;
    Some(Disk {
        raw_path: Some(PathBuf::from(format!("/dev/r{name}"))),
        size_bytes: media.get("Size")?.as_u64()?,
        block_size: media
            .get("Preferred Block Size")
            .and_then(Value::as_u64)
            .and_then(|b| u32::try_from(b).ok())
            .and_then(BlockSize::new)
            .unwrap_or(BlockSize::SD),
        name,
    })
}

/// USB storage devices that look like card readers: the subtree of each
/// `IOUSBHostDevice` that has a whole-disk media below it.
///
/// A USB reader hides the card, so the only sign of one is its name. A
/// device counts when its USB product name, or the SCSI product name its
/// storage driver reports, mentions a card format or a card reader.
/// [`UsbScope::AllDisks`] drops that test, for a disk the user named.
#[must_use]
pub fn usb_readers(usb_devices: &[Entry], scope: UsbScope) -> Vec<SdDevice> {
    usb_devices
        .iter()
        .filter_map(|usb| {
            // A hub's subtree holds the devices plugged into it; each of
            // those is handled on its own.
            let storage = usb.descendants_until(is_usb_device).find(|e| {
                e.get("Protocol Characteristics")
                    .and_then(Value::as_dictionary)
                    .and_then(|p| p.get("Physical Interconnect"))
                    .and_then(Value::as_str)
                    == Some("USB")
            })?;
            let disk = whole_disk(storage);
            // The SCSI INQUIRY names, on the peripheral device below the
            // mass storage driver, padded with spaces.
            let characteristics = storage
                .descendants()
                .find_map(|e| e.get("Device Characteristics"));
            let scsi = |key| str_of(&characteristics, key).map(|s| s.trim().to_owned());
            // `kUSB*String` are the descriptor strings as the device sends
            // them; the older `USB * Name` keys are sanitized copies.
            let product = str_of(usb, "kUSBProductString")
                .or_else(|_| str_of(usb, "USB Product Name"))
                .or_else(|_| scsi("Product Name"));
            let vendor = str_of(usb, "kUSBVendorString")
                .or_else(|_| str_of(usb, "USB Vendor Name"))
                .or_else(|_| scsi("Vendor Name"));
            let names_a_reader = [&product, &scsi("Product Name")]
                .into_iter()
                .any(|name| name.as_deref().is_ok_and(looks_like_card_reader));
            if scope == UsbScope::NamedReaders && !names_a_reader {
                return None;
            }
            let card = if disk.is_some() {
                hidden()
            } else {
                Field::Unavailable(Unavailable::NoCard)
            };
            let id = |key| {
                u64_of(usb, key)
                    .ok()
                    .and_then(|v| u16::try_from(v).ok())
                    .map(HardwareId)
            };
            let reader = Reader {
                // Every USB device has its IDs; an entry without them isn't
                // one.
                connection: Connection::Usb(UsbDevice {
                    vendor_id: id("idVendor")?,
                    product_id: id("idProduct")?,
                    speed: field(u64_of(usb, "USBSpeed").and_then(usb_speed)),
                }),
                vendor_name: field(vendor),
                product_name: field(product),
                uhs_ii_supported: hidden(),
                card_link: hidden(),
            };
            Some(SdDevice {
                disk,
                card,
                reader,
                source: Source::IoRegistry,
            })
        })
        .collect()
}

fn is_usb_device(entry: &Entry) -> bool {
    entry.class == "IOUSBHostDevice"
}

/// Words in a product name that point to a card reader rather than a flash
/// drive or a disk.
const READER_WORDS: &[&str] = &[
    "card",
    "reader",
    "cardreader",
    "sd",
    "sdhc",
    "sdxc",
    "microsd",
    "mmc",
    "tf",
];

fn looks_like_card_reader(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| READER_WORDS.contains(&word))
}

/// `USBSpeed`, the `tIOUSBHostConnectionSpeed` values from
/// `IOUSBHostFamilyDefinitions.h`. Note that full speed comes before low.
fn usb_speed(value: u64) -> Read<UsbSpeed> {
    Ok(match value {
        1 => UsbSpeed::Full,
        2 => UsbSpeed::Low,
        3 => UsbSpeed::High,
        4 => UsbSpeed::Super,
        5 => UsbSpeed::SuperPlus,
        6 => UsbSpeed::SuperPlusX2,
        _ => return Err(invalid!("unrecognized \"USBSpeed\" {value}")),
    })
}

fn hidden<T>() -> Field<T> {
    Field::Unavailable(Unavailable::HiddenByUsbReader)
}

fn narrow<T: TryFrom<u64>>(value: u64, key: &str) -> Read<T> {
    T::try_from(value).map_err(|_| invalid!("\"{key}\" {value} is out of range"))
}

/// Splits `"2.0"` or `"2020-10"` into its two numbers.
fn parse_pair<A: std::str::FromStr, B: std::str::FromStr>(
    s: &str,
    separator: char,
    key: &str,
) -> Read<(A, B)> {
    let bad = || invalid!("\"{key}\" {s:?} is not two numbers");
    let (a, b) = s.split_once(separator).ok_or_else(bad)?;
    Ok((a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?))
}

/// `key`'s value through `convert`: absent when there is no such key,
/// invalid when it holds another type.
fn typed<'a, T>(
    props: &'a impl Properties,
    key: &str,
    convert: impl FnOnce(&'a Value) -> Option<T>,
    kind: &str,
) -> Read<T> {
    let value = props.property(key).ok_or(Problem::Absent)?;
    convert(value).ok_or_else(|| invalid!("\"{key}\" is not {kind}"))
}

/// Something properties can be read from: a registry entry, or a
/// dictionary property (absent when the entry doesn't have it).
trait Properties {
    fn property(&self, key: &str) -> Option<&Value>;
}

impl Properties for Entry {
    fn property(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
}

impl Properties for Option<&Value> {
    fn property(&self, key: &str) -> Option<&Value> {
        self.and_then(Value::as_dictionary)?.get(key)
    }
}

fn u64_of(props: &impl Properties, key: &str) -> Read<u64> {
    typed(props, key, Value::as_u64, "a number")
}

fn str_of(props: &impl Properties, key: &str) -> Read<String> {
    typed(props, key, |v| v.as_str().map(str::to_owned), "a string")
}

fn bool_of(props: &impl Properties, key: &str) -> Read<bool> {
    typed(props, key, Value::as_bool, "a boolean")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value as Json;

    use super::super::tree::ioreg;
    use super::*;

    const FIXTURES: &[&str] = &["builtin-sdhc-mounted", "builtin-sdhc-inactive"];

    fn system_profiler(name: &str) -> Json {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "macos",
            &format!("{name}.system_profiler.json"),
        ]
        .iter()
        .collect();
        serde_json::from_slice(&std::fs::read(&path).expect("oracle is readable"))
            .expect("oracle is JSON")
    }

    fn device(name: &str) -> SdDevice {
        let mut devices = built_in_readers(&ioreg::fixture(name));
        assert_eq!(devices.len(), 1, "{name}: one slot");
        devices.remove(0)
    }

    fn known<T>(field: &Field<T>) -> &T {
        field.value().expect("field is known")
    }

    /// The done-when of the macOS card phase: what we report matches what
    /// `system_profiler SPCardReaderDataType` shows, captured at the same
    /// moment as the registry.
    #[test]
    fn matches_system_profiler() {
        for &name in FIXTURES {
            let oracle = &system_profiler(name)["SPCardReaderDataType"][0];
            let item = &oracle["_items"][0];
            let device = device(name);
            let card = known(&device.card);
            let reader = &device.reader;
            let text = |v: &Json| v.as_str().expect("oracle value is a string").to_owned();

            let manufacturer = known(&card.manufacturer);
            assert_eq!(
                format!("{:#x}", manufacturer.id()),
                text(&item["spcardreader_card_manufacturer-id"]),
                "{name}"
            );
            // system_profiler joins the OEM ID's two characters and the
            // product name.
            assert_eq!(
                format!(
                    "{}{}",
                    known(&card.oem_id).text().unwrap(),
                    known(&card.product_name)
                ),
                text(&item["spcardreader_card_productname"]),
                "{name}"
            );
            assert_eq!(
                known(&card.product_revision).to_string(),
                text(&item["spcardreader_card_productrevision"])
            );
            assert_eq!(
                format!("{:#x}", known(&card.serial_number).0),
                text(&item["spcardreader_card_serialnumber"])
            );
            assert_eq!(
                known(&card.manufacturing_date).to_string(),
                text(&item["spcardreader_card_manufacturing_date"])
            );
            // Apple's "3.0" covers every later version too, so the model
            // marks it as a lower bound.
            let spec = known(&card.spec_version);
            assert_eq!(
                spec.to_string().trim_end_matches('+'),
                text(&item["spcardreader_card_specversion"])
            );
            assert!(spec.is_lower_bound());
            assert_eq!(
                format!(
                    "{} Card (Class {})",
                    known(&card.card_type),
                    known(&card.speed_class).number()
                ),
                text(&item["_name"]),
                "{name}"
            );

            let Connection::BuiltIn { pci: Some(pci) } = &reader.connection else {
                panic!("{name}: built-in reader with PCI IDs");
            };
            assert_eq!(
                pci.vendor_id.to_string(),
                text(&oracle["spcardreader_vendor-id"])
            );
            assert_eq!(
                pci.device_id.to_string(),
                text(&oracle["spcardreader_device-id"])
            );
            assert_eq!(
                pci.subsystem_vendor_id.to_string(),
                text(&oracle["spcardreader_subsystem_vendor-id"])
            );
            assert_eq!(
                pci.subsystem_id.to_string(),
                text(&oracle["spcardreader_subsystem-id"])
            );
            // system_profiler pads the revision to four digits.
            assert_eq!(
                format!("{:#06x}", pci.revision_id.0),
                text(&oracle["spcardreader_revision-id"])
            );
            match &pci.link {
                Field::Known { value: link, .. } => {
                    assert_eq!(
                        link.generation().transfer_rate(),
                        text(&oracle["spcardreader_link-speed"])
                    );
                    assert_eq!(
                        format!("x{}", link.width()),
                        text(&oracle["spcardreader_link-width"])
                    );
                }
                Field::Unavailable(reason) => {
                    assert_eq!(*reason, Unavailable::PoweredDown);
                    assert_eq!(text(&oracle["spcardreader_link-speed"]), "Off");
                }
            }

            match &device.disk {
                Some(disk) => {
                    assert_eq!(disk.name.as_str(), text(&item["bsd_name"]));
                    assert_eq!(Some(disk.size_bytes), item["size_in_bytes"].as_u64());
                    assert_eq!(
                        Some(*known(&card.capacity_bytes)),
                        item["size_in_bytes"].as_u64()
                    );
                }
                None => assert!(item.get("bsd_name").is_none(), "{name}"),
            }
        }
    }

    #[test]
    fn mounted_card_is_active() {
        let device = device("builtin-sdhc-mounted");
        let disk = device.disk.expect("mounted card has a disk");
        assert_eq!(disk.raw_path, Some(PathBuf::from("/dev/rdisk6")));
        assert_eq!(disk.block_size, BlockSize::SD);
        let link = known(&device.reader.card_link);
        assert_eq!(link.speed_mode, SpeedMode::Sdr104);
        assert_eq!(link.bus_width, BusWidth::Four);
        assert_eq!(link.clock_khz, 200_000);
        assert_eq!(known(&device.reader.vendor_name), "Apple");
        assert_eq!(known(&device.reader.product_name), "Built In SDXC Reader");
        assert_eq!(known(&device.reader.uhs_ii_supported), &true);
    }

    /// An ejected card keeps its identity, but its bus values are stale and
    /// there is no disk.
    #[test]
    fn ejected_card_is_inactive() {
        let device = device("builtin-sdhc-inactive");
        assert!(device.disk.is_none());
        assert!(device.card.value().is_some());
        assert_eq!(
            device.reader.card_link,
            Field::Unavailable(Unavailable::CardInactive)
        );
    }

    #[test]
    fn empty_slot_has_no_card() {
        let mut roots = ioreg::fixture("builtin-sdhc-mounted");
        let slot = slot_mut(&mut roots[0]);
        slot.properties
            .insert("Card Present".into(), Value::Bool(false));
        let device = &built_in_readers(&roots)[0];
        assert_eq!(device.card, Field::Unavailable(Unavailable::NoCard));
        assert!(device.disk.is_none());
    }

    #[test]
    fn unknown_property_values_fail_only_their_field() {
        let mut roots = ioreg::fixture("builtin-sdhc-mounted");
        let slot = slot_mut(&mut roots[0]);
        slot.properties
            .insert("Speed Mode".into(), Value::String("UHS: SDR999".into()));
        let device = &built_in_readers(&roots)[0];
        assert_eq!(
            device.reader.card_link,
            Field::Unavailable(Unavailable::ReadFailed(
                "unrecognized \"Speed Mode\" \"UHS: SDR999\"".into()
            ))
        );
        assert!(device.card.value().is_some());
    }

    fn slot_mut(entry: &mut Entry) -> &mut Entry {
        if entry.class == "AppleSDXCSlot" {
            return entry;
        }
        entry
            .children
            .iter_mut()
            .find(|c| c.descendants().any(|e| e.class == "AppleSDXCSlot"))
            .map(slot_mut)
            .expect("fixture has a slot")
    }

    /// A USB reader built from the chain in Apple's headers and kext
    /// personalities; no capture of a real one exists yet.
    fn usb_device(product: &str, speed: u64, media: bool) -> Entry {
        fn entry(class: &str, properties: Vec<(&str, Value)>, children: Vec<Entry>) -> Entry {
            Entry {
                class: class.into(),
                properties: properties
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
                children,
            }
        }
        let dict = |pairs: &[(&str, &str)]| {
            Value::Dictionary(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned())))
                    .collect(),
            )
        };
        let disk = entry(
            "IOMedia",
            vec![
                ("Whole", Value::Bool(true)),
                ("BSD Name", Value::String("disk8".into())),
                ("Size", Value::Integer(63_864_569_856)),
                ("Preferred Block Size", Value::Integer(512)),
            ],
            vec![],
        );
        let peripheral = entry(
            "IOSCSIPeripheralDeviceType00",
            vec![(
                "Device Characteristics",
                dict(&[
                    ("Vendor Name", "Generic "),
                    ("Product Name", "STORAGE DEVICE  "),
                ]),
            )],
            vec![entry(
                "IOBlockStorageServices",
                vec![],
                vec![entry(
                    "IOBlockStorageDriver",
                    vec![],
                    if media { vec![disk] } else { vec![] },
                )],
            )],
        );
        entry(
            "IOUSBHostDevice",
            vec![
                ("idVendor", Value::Integer(0x05e3)),
                ("idProduct", Value::Integer(0x0749)),
                ("kUSBProductString", Value::String(product.into())),
                ("kUSBVendorString", Value::String("Generic".into())),
                ("USBSpeed", Value::Integer(speed)),
            ],
            vec![entry(
                "IOUSBHostInterface",
                vec![],
                vec![entry(
                    "IOUSBMassStorageDriver",
                    vec![(
                        "Protocol Characteristics",
                        dict(&[("Physical Interconnect", "USB")]),
                    )],
                    vec![peripheral],
                )],
            )],
        )
    }

    #[test]
    fn usb_reader_hides_the_card() {
        let devices = usb_readers(
            &[usb_device("USB3.0 Card Reader", 4, true)],
            UsbScope::NamedReaders,
        );
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert_eq!(device.disk.as_ref().unwrap().name.as_str(), "disk8");
        assert_eq!(
            device.card,
            Field::Unavailable(Unavailable::HiddenByUsbReader)
        );
        assert_eq!(
            device.reader.card_link,
            Field::Unavailable(Unavailable::HiddenByUsbReader)
        );
        let Connection::Usb(usb) = &device.reader.connection else {
            panic!("USB connection");
        };
        assert_eq!(usb.vendor_id, HardwareId(0x05e3));
        assert_eq!(known(&usb.speed), &UsbSpeed::Super);
        assert_eq!(known(&device.reader.product_name), "USB3.0 Card Reader");
    }

    #[test]
    fn usb_disks_count_only_by_name_or_when_named() {
        let stick = [usb_device("Ultra Fit", 3, true)];
        assert!(usb_readers(&stick, UsbScope::NamedReaders).is_empty());
        assert_eq!(usb_readers(&stick, UsbScope::AllDisks).len(), 1);
        // The SCSI name counts too, trimmed of its padding.
        let generic = [usb_device("Mass Storage", 3, true)];
        assert!(usb_readers(&generic, UsbScope::NamedReaders).is_empty());
    }

    #[test]
    fn empty_usb_reader_has_no_card() {
        let devices = usb_readers(&[usb_device("SD Reader", 3, false)], UsbScope::NamedReaders);
        assert!(devices[0].disk.is_none());
        assert_eq!(devices[0].card, Field::Unavailable(Unavailable::NoCard));
    }

    #[test]
    fn hub_does_not_claim_the_disks_behind_it() {
        let hub = Entry {
            class: "IOUSBHostDevice".into(),
            properties: [
                ("idVendor".to_owned(), Value::Integer(0x2109)),
                ("idProduct".to_owned(), Value::Integer(0x0817)),
                (
                    "kUSBProductString".to_owned(),
                    Value::String("USB3.0 Hub".into()),
                ),
            ]
            .into(),
            children: vec![usb_device("SD Card Reader", 4, true)],
        };
        assert!(usb_readers(&[hub], UsbScope::AllDisks).is_empty());
    }

    #[test]
    fn reader_names() {
        for name in [
            "USB3.0 Card Reader",
            "SD/MMC",
            "Multi-Card",
            "STORAGE DEVICE SD",
        ] {
            assert!(looks_like_card_reader(name), "{name}");
        }
        for name in [
            "Ultra Fit",
            "DataTraveler 3.0",
            "Samsung Portable SSD T7",
            "SDDR",
        ] {
            assert!(!looks_like_card_reader(name), "{name}");
        }
    }
}
