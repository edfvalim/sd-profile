//! The report's device sections: target, card, reader and mounts.

use std::io::{self, Write};

use super::{line, shown, size, sub_line};
use crate::model::{
    Card, CardLink, Connection, DeviceDisk, Field, Mount, PciDevice, PcieLink, Profile, Reader,
    Target, Unavailable, UsbSpeed, WriteProtect,
};

/// Writes the target line and the card, reader and mounts sections, each
/// followed by a blank line.
pub(super) fn write(profile: &Profile, out: &mut impl Write) -> io::Result<()> {
    let Target::Device { disk } = &profile.target else {
        return Ok(());
    };
    let connection = profile.reader.value().map(|r| &r.connection);
    let target = target_text(disk.as_ref(), &profile.card, connection);
    line(out, "Target", &target)?;
    writeln!(out)?;
    section(out, "Card", &profile.card, write_card)?;
    section(out, "Reader", &profile.reader, write_reader)?;
    section(out, "Mounts", &profile.mounts, |mounts, out| {
        write_mounts(mounts, out)
    })
}

/// The target line: the card's disk and the reader it is in, or why there
/// is no disk.
fn target_text(
    disk: Option<&DeviceDisk>,
    card: &Field<Card>,
    connection: Option<&Connection>,
) -> String {
    let reader_kind = match connection {
        Some(Connection::BuiltIn { .. }) => "built-in reader",
        Some(Connection::Usb(_)) => "USB reader",
        None => "reader",
    };
    match (disk, card) {
        (Some(disk), _) => format!("{} (card in the {reader_kind})", disk.name.path().display()),
        (None, Field::Unavailable(Unavailable::NoCard)) => {
            format!("the {reader_kind}, no card in it")
        }
        (None, _) => {
            format!("card in the {reader_kind}, no disk (ejected or not set up yet)")
        }
    }
}

/// A section written by `write` when its field is known, or one line saying
/// why it isn't, followed by a blank line.
fn section<T, W: Write>(
    out: &mut W,
    label: &str,
    field: &Field<T>,
    write: impl FnOnce(&T, &mut W) -> io::Result<()>,
) -> io::Result<()> {
    match field {
        Field::Known { value, .. } => write(value, out)?,
        Field::Unavailable(reason) => line(out, label, &format!("unavailable ({reason})"))?,
    }
    writeln!(out)
}

fn write_card(card: &Card, out: &mut impl Write) -> io::Result<()> {
    let card_type = shown(&card.card_type, ToString::to_string);
    let capacity = shown(&card.capacity_bytes, |&bytes| {
        format!("{} ({bytes} bytes)", size(bytes))
    });
    line(out, "Card", &format!("{card_type}, {capacity}"))?;
    sub_line(
        out,
        "Manufacturer",
        &shown(&card.manufacturer, |m| match m.name() {
            Some(_) => m.to_string(),
            None => format!("{m} (not a known manufacturer ID)"),
        }),
    )?;
    sub_line(
        out,
        "OEM ID",
        &shown(&card.oem_id, |oem| match oem.text() {
            Some(text) => format!("{:#06x} ({text:?})", oem.0),
            None => format!("{:#06x}", oem.0),
        }),
    )?;
    sub_line(
        out,
        "Product",
        &shown(&card.product_name, |name| format!("{name:?}")),
    )?;
    sub_line(
        out,
        "Revision",
        &shown(&card.product_revision, ToString::to_string),
    )?;
    sub_line(
        out,
        "Serial number",
        &shown(&card.serial_number, ToString::to_string),
    )?;
    sub_line(
        out,
        "Manufactured",
        &shown(&card.manufacturing_date, ToString::to_string),
    )?;
    sub_line(
        out,
        "SD spec",
        &shown(&card.spec_version, |v| {
            if v.is_lower_bound() {
                format!("{v} (the OS reports no version above this one)")
            } else {
                v.to_string()
            }
        }),
    )?;
    sub_line(
        out,
        "Speed class",
        &shown(&card.speed_class, |class| {
            format!("Class {}", class.number())
        }),
    )?;
    sub_line(
        out,
        "UHS-II",
        &shown(&card.uhs_ii_supported, |&s| supported(s)),
    )?;
    sub_line(
        out,
        "Write protect",
        &write_protect_text(&card.write_protect),
    )
}

fn write_protect_text(wp: &WriteProtect) -> String {
    let mut on = Vec::new();
    let mut unknown = Vec::new();
    for (field, what) in [(&wp.switch, "lock switch"), (&wp.register, "card register")] {
        match field {
            Field::Known { value: true, .. } => on.push(what.to_owned()),
            Field::Known { value: false, .. } => {}
            Field::Unavailable(reason) => unknown.push(format!("{what} unavailable ({reason})")),
        }
    }
    let state = if on.is_empty() {
        "off".to_owned()
    } else {
        format!("on ({})", on.join(", "))
    };
    if unknown.is_empty() {
        state
    } else {
        format!("{state}; {}", unknown.join(", "))
    }
}

fn write_reader(reader: &Reader, out: &mut impl Write) -> io::Result<()> {
    let name = match (reader.vendor_name.value(), reader.product_name.value()) {
        (Some(vendor), Some(product)) => format!("{vendor} {product}"),
        (None, Some(product)) => product.clone(),
        (Some(vendor), None) => vendor.clone(),
        (None, None) => shown(&reader.product_name, String::clone),
    };
    line(out, "Reader", &name)?;
    match &reader.connection {
        Connection::BuiltIn { pci: Some(pci) } => write_pci(pci, out)?,
        Connection::BuiltIn { pci: None } => sub_line(out, "Connection", "built-in")?,
        Connection::Usb(usb) => {
            let speed = shown(&usb.speed, |&s| usb_speed_text(s).to_owned());
            sub_line(
                out,
                "Connection",
                &format!(
                    "USB {}:{}, {speed}",
                    hex4(usb.vendor_id.0),
                    hex4(usb.product_id.0)
                ),
            )?;
        }
    }
    sub_line(
        out,
        "UHS-II",
        &shown(&reader.uhs_ii_supported, |&s| supported(s)),
    )?;
    sub_line(
        out,
        "Card bus",
        &shown(&reader.card_link, |&link| card_link_text(link)),
    )
}

fn write_pci(pci: &PciDevice, out: &mut impl Write) -> io::Result<()> {
    sub_line(
        out,
        "Connection",
        &format!(
            "built-in, PCI {}:{} (subsystem {}:{}, revision {})",
            hex4(pci.vendor_id.0),
            hex4(pci.device_id.0),
            hex4(pci.subsystem_vendor_id.0),
            hex4(pci.subsystem_id.0),
            pci.revision_id
        ),
    )?;
    sub_line(
        out,
        "PCIe link",
        &shown(&pci.link, |&link| pcie_link_text(link)),
    )
}

/// IDs as `lspci` and `system_profiler` pair them: `17a0:9755`.
fn hex4(id: u16) -> String {
    format!("{id:04x}")
}

fn pcie_link_text(link: PcieLink) -> String {
    format!(
        "{} x{} (PCIe gen {})",
        link.generation().transfer_rate(),
        link.width(),
        link.generation().number()
    )
}

fn card_link_text(link: CardLink) -> String {
    let lines = link.bus_width.lines();
    format!(
        "{}, {lines} data line{}, {}",
        link.speed_mode,
        if lines == 1 { "" } else { "s" },
        clock_text(link.clock_khz)
    )
}

/// A clock in MHz, or kHz below 1 MHz.
fn clock_text(khz: u32) -> String {
    match (khz / 1000, khz % 1000) {
        (0, _) => format!("{khz} kHz"),
        (mhz, 0) => format!("{mhz} MHz"),
        (mhz, rest) => format!("{mhz}.{} MHz", format!("{rest:03}").trim_end_matches('0')),
    }
}

fn usb_speed_text(speed: UsbSpeed) -> &'static str {
    match speed {
        UsbSpeed::Low => "low speed (1.5 Mbit/s)",
        UsbSpeed::Full => "full speed (12 Mbit/s)",
        UsbSpeed::High => "high speed (480 Mbit/s)",
        UsbSpeed::Super => "SuperSpeed (5 Gbit/s)",
        UsbSpeed::SuperPlus => "SuperSpeed+ (10 Gbit/s)",
        UsbSpeed::SuperPlusX2 => "SuperSpeed+ 20Gbps (20 Gbit/s)",
    }
}

fn supported(yes: bool) -> String {
    if yes { "supported" } else { "not supported" }.to_owned()
}

fn write_mounts(mounts: &[Mount], out: &mut impl Write) -> io::Result<()> {
    if mounts.is_empty() {
        return line(out, "Mounts", "none");
    }
    writeln!(out, "Mounts")?;
    for mount in mounts {
        let mut details = vec![mount.os_fs_type.clone()];
        details.extend(mount.options.iter().map(ToString::to_string));
        sub_line(
            out,
            mount.device.as_str(),
            &format!("{} ({})", mount.mount_point.display(), details.join(", ")),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_says_why_there_is_no_disk() {
        let empty = Field::Unavailable(Unavailable::NoCard);
        assert_eq!(target_text(None, &empty, None), "the reader, no card in it");
        let ejected = Field::Unavailable(Unavailable::CardInactive);
        assert_eq!(
            target_text(None, &ejected, None),
            "card in the reader, no disk (ejected or not set up yet)"
        );
    }

    #[test]
    fn clocks() {
        assert_eq!(clock_text(400), "400 kHz");
        assert_eq!(clock_text(25_000), "25 MHz");
        assert_eq!(clock_text(208_000), "208 MHz");
        assert_eq!(clock_text(52_500), "52.5 MHz");
    }
}
