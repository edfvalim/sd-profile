//! A copy of part of the I/O Registry as plain Rust values.
//!
//! `iokit.rs` fills it from the live registry; the tests fill it from
//! `ioreg -a` captures. `map.rs` reads it into the model. Builds on every OS.

use std::collections::BTreeMap;

/// One registry entry in the service plane, with its properties and the
/// entries attached below it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    /// The entry's C++ class, like `AppleSDXCSlot` or `IOMedia`.
    pub class: String,
    pub properties: BTreeMap<String, Value>,
    pub children: Vec<Entry>,
}

/// A property value. These are the types the I/O Registry hands out as
/// Core Foundation objects.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    /// Registry numbers are unsigned 64-bit in the kernel. A number Core
    /// Foundation hands back as negative is kept as its bit pattern, which is
    /// how `ioreg` prints it too.
    Integer(u64),
    String(String),
    Data(Vec<u8>),
    Array(Vec<Value>),
    Dictionary(BTreeMap<String, Value>),
    /// Anything else (dates, floating-point numbers), which the mapping
    /// never needs.
    Other,
}

impl Entry {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }

    /// The entry and all entries below it, depth first.
    pub fn descendants(&self) -> impl Iterator<Item = &Entry> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let entry = stack.pop()?;
            stack.extend(entry.children.iter().rev());
            Some(entry)
        })
    }

    /// Like [`Entry::descendants`], but without going below entries (other
    /// than this one) that `stop` picks out.
    pub fn descendants_until(&self, stop: impl Fn(&Entry) -> bool) -> impl Iterator<Item = &Entry> {
        let mut stack = vec![self];
        let root: *const Entry = self;
        std::iter::from_fn(move || {
            let entry = stack.pop()?;
            if std::ptr::eq(entry, root) || !stop(entry) {
                stack.extend(entry.children.iter().rev());
            }
            Some(entry)
        })
    }

    /// The first entry of `class` at or below this one, depth first.
    #[must_use]
    pub fn find(&self, class: &str) -> Option<&Entry> {
        self.descendants().find(|e| e.class == class)
    }
}

impl Value {
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Integer(n) => Some(*n),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Self::Data(d) => Some(d),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_dictionary(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Dictionary(d) => Some(d),
            _ => None,
        }
    }
}

/// Reading `ioreg -a -l -r` captures, the format of the fixtures in
/// `tests/fixtures/macos/`.
#[cfg(test)]
pub mod ioreg {
    use std::path::PathBuf;

    use super::{BTreeMap, Entry, Value};

    /// Loads `tests/fixtures/macos/<name>.ioreg.plist`: an array of subtrees,
    /// each a dictionary of properties plus `ioreg`'s own keys for the class
    /// and the children.
    pub fn fixture(name: &str) -> Vec<Entry> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "fixtures",
            "macos",
            &format!("{name}.ioreg.plist"),
        ]
        .iter()
        .collect();
        let plist = plist::Value::from_file(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        plist
            .as_array()
            .expect("ioreg -a -r writes an array of subtrees")
            .iter()
            .map(entry)
            .collect()
    }

    fn entry(plist: &plist::Value) -> Entry {
        let dict = plist.as_dictionary().expect("each entry is a dictionary");
        let mut entry = Entry::default();
        for (key, value) in dict {
            match key.as_str() {
                "IOObjectClass" => {
                    value
                        .as_string()
                        .expect("class is a string")
                        .clone_into(&mut entry.class);
                }
                "IORegistryEntryChildren" => {
                    entry.children = value
                        .as_array()
                        .expect("children are an array")
                        .iter()
                        .map(self::entry)
                        .collect();
                }
                _ => {
                    entry.properties.insert(key.clone(), self::value(value));
                }
            }
        }
        entry
    }

    fn value(plist: &plist::Value) -> Value {
        match plist {
            plist::Value::Boolean(b) => Value::Bool(*b),
            plist::Value::Integer(n) => Value::Integer(
                n.as_unsigned()
                    .or_else(|| n.as_signed().map(i64::cast_unsigned))
                    .expect("an integer is signed or unsigned"),
            ),
            plist::Value::String(s) => Value::String(s.clone()),
            plist::Value::Data(d) => Value::Data(d.clone()),
            plist::Value::Array(a) => Value::Array(a.iter().map(value).collect()),
            plist::Value::Dictionary(d) => Value::Dictionary(
                d.iter()
                    .map(|(k, v)| (k.clone(), value(v)))
                    .collect::<BTreeMap<_, _>>(),
            ),
            _ => Value::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_loads_the_reader_subtree() {
        let roots = ioreg::fixture("builtin-sdhc-mounted");
        assert_eq!(roots.len(), 1);
        let pci = &roots[0];
        assert_eq!(pci.class, "IOPCIDevice");
        let slot = pci.find("AppleSDXCSlot").expect("slot below the reader");
        assert_eq!(slot.get("Bus Width").and_then(Value::as_u64), Some(4));
        let media = pci.find("IOMedia").expect("the card's disk");
        assert_eq!(media.get("BSD Name").and_then(Value::as_str), Some("disk6"));
    }

    #[test]
    fn descendants_are_depth_first() {
        let leaf = |class: &str| Entry {
            class: class.into(),
            ..Entry::default()
        };
        let tree = Entry {
            class: "root".into(),
            children: vec![
                Entry {
                    class: "a".into(),
                    children: vec![leaf("a1")],
                    ..Entry::default()
                },
                leaf("b"),
            ],
            ..Entry::default()
        };
        let order: Vec<&str> = tree.descendants().map(|e| e.class.as_str()).collect();
        assert_eq!(order, ["root", "a", "a1", "b"]);
        let order: Vec<&str> = tree
            .descendants_until(|e| e.class == "a")
            .map(|e| e.class.as_str())
            .collect();
        assert_eq!(order, ["root", "a", "b"]);
    }
}
