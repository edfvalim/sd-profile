//! The live I/O Registry, copied into a [`tree`](super::tree) for
//! [`map`](super::map). macOS only. All the `IOKit` calls, and so all the
//! `unsafe`, are in this file.

use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};

use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFDictionary, CFMutableDictionary, CFNumber, CFRetained, CFString,
    CFType,
};
use objc2_io_kit::{
    IOIteratorNext, IOObjectGetClass, IOObjectRelease, IORegistryEntryCreateCFProperties,
    IORegistryEntryGetChildIterator, IORegistryEntryGetParentEntry, IOServiceGetMatchingServices,
    IOServiceMatching, io_object_t, kIOMainPortDefault,
};

use super::map;
use super::tree::{Entry, Value};
use crate::platform::{PlatformError, SdDevice, UsbScope};

/// `KERN_SUCCESS` from `<mach/kern_return.h>`.
const KERN_SUCCESS: i32 = 0;
/// `KERN_RESOURCE_SHORTAGE`, what a failed allocation in `IOKitLib` means.
const KERN_RESOURCE_SHORTAGE: i32 = 6;

/// `Ok` for `KERN_SUCCESS`, otherwise the error naming `call`.
fn check(call: &'static str, status: i32) -> Result<(), PlatformError> {
    if status == KERN_SUCCESS {
        Ok(())
    } else {
        Err(PlatformError::Registry { call, status })
    }
}

/// Built-in readers first, then USB ones.
///
/// # Errors
///
/// When the registry can't be searched.
pub fn sd_devices(usb: UsbScope) -> Result<Vec<SdDevice>, PlatformError> {
    // Apple's driver sits on the reader's PCI device, which holds the IDs
    // and the link; take the subtree from there.
    let pci_devices: Vec<Entry> = services(c"AppleSDXC")?
        .iter()
        .filter_map(Object::parent)
        .map(|pci| subtree(&pci))
        .collect::<Result<_, _>>()?;
    // A USB device unplugged during the walk is left out rather than failing
    // the run; it holds no card anymore.
    let usb_devices: Vec<Entry> = services(c"IOUSBHostDevice")?
        .iter()
        .filter_map(|device| subtree(device).ok())
        .collect();
    let mut devices = map::built_in_readers(&pci_devices);
    devices.extend(map::usb_readers(&usb_devices, usb));
    Ok(devices)
}

/// An I/O Registry object handle, released when dropped.
struct Object(io_object_t);

impl Drop for Object {
    fn drop(&mut self) {
        // The handle came from IOKit with a reference this value owns, and
        // nothing uses it after the drop.
        IOObjectRelease(self.0);
    }
}

impl Object {
    /// The provider in the service plane.
    fn parent(&self) -> Option<Object> {
        let mut plane = service_plane();
        let mut parent: io_object_t = 0;
        // SAFETY: `self.0` is a live handle, `plane` a NUL-terminated name in
        // a buffer of the size IOKit expects, and `parent` a valid place for
        // the result.
        let status =
            unsafe { IORegistryEntryGetParentEntry(self.0, &raw mut plane, &raw mut parent) };
        (status == KERN_SUCCESS && parent != 0).then_some(Object(parent))
    }

    /// The clients attached below in the service plane.
    fn children(&self) -> Result<Vec<Object>, PlatformError> {
        let mut plane = service_plane();
        let mut iterator: io_object_t = 0;
        // SAFETY: as in `parent`; on success `iterator` is a new handle that
        // `Object` takes over.
        let status =
            unsafe { IORegistryEntryGetChildIterator(self.0, &raw mut plane, &raw mut iterator) };
        check("IORegistryEntryGetChildIterator", status)?;
        Ok(drain(&Object(iterator)))
    }

    fn class(&self) -> Result<String, PlatformError> {
        let mut name: [c_char; 128] = [0; 128];
        // SAFETY: `name` is the 128-byte buffer `io_name_t` stands for;
        // IOKit writes a NUL-terminated class name into it.
        let status = unsafe { IOObjectGetClass(self.0, &raw mut name) };
        check("IOObjectGetClass", status)?;
        // SAFETY: on success the buffer holds a NUL-terminated string.
        Ok(unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned())
    }

    fn properties(&self) -> Result<BTreeMap<String, Value>, PlatformError> {
        let mut dict: *mut CFMutableDictionary = std::ptr::null_mut();
        // SAFETY: `self.0` is a live handle and `dict` a valid place for the
        // result. Options 0 asks for all properties.
        let status = unsafe { IORegistryEntryCreateCFProperties(self.0, &raw mut dict, None, 0) };
        check("IORegistryEntryCreateCFProperties", status)?;
        let dict = std::ptr::NonNull::new(dict).ok_or(PlatformError::Registry {
            call: "IORegistryEntryCreateCFProperties",
            status: KERN_RESOURCE_SHORTAGE,
        })?;
        // SAFETY: IORegistryEntryCreateCFProperties follows the Create rule,
        // so the caller owns the one reference the dictionary comes with.
        let dict: CFRetained<CFMutableDictionary> = unsafe { CFRetained::from_raw(dict) };
        Ok(dictionary(&dict))
    }
}

/// `kIOServicePlane` in the fixed-size buffer `io_name_t` stands for.
fn service_plane() -> [c_char; 128] {
    let mut buffer = [0; 128];
    for (slot, &byte) in buffer.iter_mut().zip(c"IOService".to_bytes()) {
        *slot = byte.cast_signed();
    }
    buffer
}

/// The registered services of `class` and its subclasses.
fn services(class: &CStr) -> Result<Vec<Object>, PlatformError> {
    // SAFETY: `class` is a NUL-terminated string.
    let matching = unsafe { IOServiceMatching(class.as_ptr()) }.ok_or(PlatformError::Registry {
        call: "IOServiceMatching",
        status: KERN_RESOURCE_SHORTAGE,
    })?;
    let mut iterator: io_object_t = 0;
    // SAFETY: the call consumes the matching dictionary's reference, which
    // the wrapper hands over, and `iterator` is a valid place for the result.
    let status = unsafe {
        IOServiceGetMatchingServices(
            kIOMainPortDefault,
            Some(CFRetained::<CFDictionary>::from(&*matching)),
            &raw mut iterator,
        )
    };
    check("IOServiceGetMatchingServices", status)?;
    Ok(drain(&Object(iterator)))
}

fn drain(iterator: &Object) -> Vec<Object> {
    // Each object returned is a new reference that `Object` takes over; 0
    // ends the iteration.
    std::iter::from_fn(|| match IOIteratorNext(iterator.0) {
        0 => None,
        object => Some(Object(object)),
    })
    .collect()
}

/// The entry with its properties and everything below it. A child whose
/// walk fails is left out: the status codes don't tell an entry that went
/// away during the walk (a card pulled out) from one that can't be read,
/// and either way the entry reads as it is without it. Only a failure on
/// `object` itself is an error.
fn subtree(object: &Object) -> Result<Entry, PlatformError> {
    Ok(Entry {
        class: object.class()?,
        properties: object.properties()?,
        children: object
            .children()?
            .iter()
            .filter_map(|child| subtree(child).ok())
            .collect(),
    })
}

fn dictionary(dict: &CFDictionary) -> BTreeMap<String, Value> {
    // SAFETY: registry property dictionaries are keyed by strings and hold
    // property-list objects, so viewing them as CFType to CFType is sound.
    let dict: &CFDictionary<CFType, CFType> = unsafe { dict.cast_unchecked() };
    let (keys, values) = dict.to_vecs();
    keys.iter()
        .zip(&values)
        .filter_map(|(key, value)| {
            let key = key.downcast_ref::<CFString>()?.to_string();
            Some((key, self::value(value)))
        })
        .collect()
}

fn value(object: &CFType) -> Value {
    if let Some(b) = object.downcast_ref::<CFBoolean>() {
        Value::Bool(b.as_bool())
    } else if let Some(n) = object.downcast_ref::<CFNumber>() {
        // Registry numbers are unsigned 64-bit; Core Foundation hands them
        // out as signed, so keep the bit pattern.
        n.as_i64()
            .map_or(Value::Other, |n| Value::Integer(n.cast_unsigned()))
    } else if let Some(s) = object.downcast_ref::<CFString>() {
        Value::String(s.to_string())
    } else if let Some(d) = object.downcast_ref::<CFData>() {
        Value::Data(d.to_vec())
    } else if let Some(a) = object.downcast_ref::<CFArray>() {
        // SAFETY: registry arrays hold property-list objects.
        let a: &CFArray<CFType> = unsafe { a.cast_unchecked() };
        Value::Array(a.to_vec().iter().map(|v| value(v)).collect())
    } else if let Some(d) = object.downcast_ref::<CFDictionary>() {
        Value::Dictionary(dictionary(d))
    } else {
        Value::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Mac has a disk; this checks the walk and the conversions
    /// against the live registry without needing a card.
    #[test]
    fn walks_the_live_registry() {
        let media = services(c"IOMedia").unwrap();
        let disk = media.first().expect("every Mac has a disk");
        let entry = subtree(disk).unwrap();
        assert_eq!(entry.class, "IOMedia");
        assert!(entry.get("BSD Name").and_then(Value::as_str).is_some());
        assert!(entry.get("Size").and_then(Value::as_u64).is_some());
        assert!(entry.get("Whole").and_then(Value::as_bool).is_some());
        let parent = disk.parent().expect("a disk has a provider");
        assert!(!parent.class().unwrap().is_empty());
    }
}
