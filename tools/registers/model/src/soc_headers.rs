//! The pinned ESP-IDF register headers as the source of what a vendor
//! register does.
//!
//! The vendor SVD names fields and an access, but not what a write or a read
//! does beyond that: a write-one-to-clear status, a field the hardware
//! changes under a read-modify-write, a status a read clears and a reserved
//! lane all read `read-write` there. Each `*_reg.h` header of the pinned
//! ESP-IDF states every field's access class (`R/W`, `RO`, `WT`,
//! `R/WTC/SS`, ...) and description, and each `*_struct.h` the device
//! layout. The overlay's `[[headers]]` table names the header of each
//! vendor peripheral; this module finds every vendor register in it and
//! derives the register's API from the header's classes:
//!
//! - `R/W` is a plain field, `RO`/`R`/`HRO` a read-only one, `WT` a write
//!   trigger (a wider one written with its value);
//! - a one-bit `R/W/SC` or `R/W/WTC` field is a command: software sets it to
//!   start, the hardware clears it when done;
//! - a register whose every field is write-one-to-clear (`WTC`, `W1C`) is
//!   sampled and acknowledged, never modified;
//! - any other writable field the hardware also changes (`R/W/SS`, a wider
//!   `R/W/SC`, ...), a set-only one (`W1S`, `WTS`, `WS`), a class this
//!   module does not know and a write-one-to-clear field beside others
//!   leave the register read-only, unless the overlay makes it a command
//!   port's data word;
//! - a field the header describes as `Reserved` is no field: its bits are
//!   reserved and kept by every write;
//! - a field a read clears ("cleared by a read", `R_SS_RC`) or a port read
//!   and written in sequence gives its register no API;
//! - a register the header does not define is read-only, and none at all
//!   when the device layout reserves its word.

use std::collections::{BTreeMap, BTreeSet};

use svd_rs::{Device, MaybeArray, RegisterCluster};

use crate::soc_overlay::{
    HeaderField, HeaderRegister, ReservedWords, parse_layout_reserved, parse_reg_header,
};
use crate::{Error, HeaderTree, Result, StrictnessOverlay};

/// What a header access class lets a field do.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Class {
    /// `R/W`: read and written.
    Plain,
    /// `RO`, `R`, `HRO`, `RO/SS`, ...: only read.
    ReadOnly,
    /// `WT`: a write starts an action.
    Trigger,
    /// `R/W/SC`, `R/W/WTC`: software sets it to start a command, and the
    /// hardware clears it when the command is done.
    Command,
    /// `WTC`, `W1C` without `W`: a one written clears it.
    OneToClear,
    /// `R/W` the hardware also changes (`SC`, `SS`, `WTC`): a
    /// read-modify-write races the hardware.
    HardwareWritable,
    /// `W1S`, `WTS`, `WS`, `R/W1`: a one written sets it.
    SetOnly,
    /// `WO`: only written.
    WriteOnly,
    /// A class this module does not know.
    Unknown,
}

/// The class of a header's access text.
pub(crate) fn class(access: &str) -> Class {
    let tokens = access
        .split('/')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let has = |name: &str| tokens.contains(&name);
    match tokens.as_slice() {
        ["R", "W"] | ["RW"] | ["W", "R"] => Class::Plain,
        ["RO" | "R" | "HRO"] => Class::ReadOnly,
        ["WT"] => Class::Trigger,
        ["R", "W", "SC" | "WTC"] => Class::Command,
        ["WO"] => Class::WriteOnly,
        _ if tokens
            .iter()
            .any(|token| matches!(*token, "W1S" | "WTS" | "WS" | "W1" | "W1S_SC")) =>
        {
            Class::SetOnly
        }
        _ if has("W") && (has("SC") || has("SS") || has("WTC")) => Class::HardwareWritable,
        _ if has("WTC") || has("W1C") => Class::OneToClear,
        _ if tokens
            .iter()
            .all(|token| matches!(*token, "R" | "RO" | "SS" | "SC")) =>
        {
            Class::ReadOnly
        }
        _ => Class::Unknown,
    }
}

/// Whether a description calls the field reserved: `Reserved`,
/// `reserved.`, `Reserved, it was created ...`.
pub(crate) fn reserved_description(description: &str) -> bool {
    let lower = description.trim().to_ascii_lowercase();
    lower.strip_prefix("reserved").is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|next| !next.is_ascii_alphanumeric())
    })
}

fn words(description: &str) -> Vec<String> {
    description
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether a description says a read clears the field: `cleared` followed
/// within six words by a word starting with `read` ("cleared by a Read",
/// "cleared when read", "cleared on reading"), or the class `R_SS_RC`.
pub(crate) fn read_clears(description: &str) -> bool {
    let words = words(description);
    words.iter().any(|word| word == "r_ss_rc")
        || words.iter().enumerate().any(|(index, word)| {
            word == "cleared"
                && words
                    .iter()
                    .skip(index + 1)
                    .take(6)
                    .any(|next| next.starts_with("read"))
        })
}

/// Whether a description makes the register a port read or written in
/// sequence ("eight sequential reads from this address").
pub(crate) fn sequential_port(description: &str) -> bool {
    let words = words(description);
    words.iter().any(|word| word.starts_with("sequential"))
        && words
            .iter()
            .any(|word| word.starts_with("read") || word.starts_with("write"))
}

/// One vendor peripheral's header.
struct PeripheralHeader {
    path: String,
    registers: Vec<HeaderRegister>,
    /// The prefix the header's register names put before the SVD's.
    prefix: String,
    layout: Option<(String, Vec<ReservedWords>)>,
}

/// Every mapped peripheral's header, loaded once.
pub(crate) struct HeaderFacts {
    peripherals: BTreeMap<String, PeripheralHeader>,
}

/// Where a vendor register is in its peripheral's header.
pub(crate) enum Lookup<'a> {
    /// The overlay names no header for the peripheral.
    Unmapped,
    /// The header's register of the same name and offset; `prefix` is what
    /// the header's names put before the SVD's.
    Matched {
        path: &'a str,
        prefix: &'a str,
        register: &'a HeaderRegister,
    },
    /// The header's register of the same name, at another offset.
    Moved {
        path: &'a str,
        register: &'a HeaderRegister,
    },
    /// The header defines no such register; `reserved` cites the layout's
    /// reserved words at its offset.
    Absent {
        path: &'a str,
        reserved: Option<(&'a str, usize)>,
    },
}

/// The vendor registers of `peripheral` as (element name, offset): an
/// array's every element.
fn svd_registers(peripheral: &svd_rs::Peripheral) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for child in peripheral.registers.as_deref().unwrap_or_default() {
        let RegisterCluster::Register(register) = child else {
            continue;
        };
        out.extend(elements(register));
    }
    out
}

/// A register's elements as (name, offset).
pub(crate) fn elements(register: &svd_rs::Register) -> Vec<(String, u64)> {
    let offset = u64::from(register.address_offset);
    match register {
        MaybeArray::Single(info) => vec![(info.name.clone(), offset)],
        MaybeArray::Array(info, dimension) => svd_rs::array::names(info, dimension)
            .enumerate()
            .map(|(index, name)| {
                (
                    name,
                    offset + index as u64 * u64::from(dimension.dim_increment),
                )
            })
            .collect(),
    }
}

impl HeaderFacts {
    /// Load the headers the overlay names for the peripherals of `device`.
    pub(crate) fn load(
        overlay: &StrictnessOverlay,
        tree: &dyn HeaderTree,
        device: &Device,
    ) -> Result<Self> {
        let mut parsed = BTreeMap::<String, Vec<HeaderRegister>>::new();
        let mut layouts = BTreeMap::<String, Vec<ReservedWords>>::new();
        let mut peripherals = BTreeMap::new();
        for rule in &overlay.headers {
            let Some(peripheral) = device
                .peripherals
                .iter()
                .find(|peripheral| peripheral.name == rule.peripheral)
            else {
                return Err(Error::message(format!(
                    "overlay header of {} names no vendor peripheral",
                    rule.peripheral
                )));
            };
            if !parsed.contains_key(&rule.registers) {
                parsed.insert(
                    rule.registers.clone(),
                    parse_reg_header(&tree.read(&rule.registers)?),
                );
            }
            let registers = parsed.get(&rule.registers).cloned().unwrap_or_default();
            let layout = match &rule.layout {
                None => None,
                Some(path) => {
                    if !layouts.contains_key(path) {
                        layouts.insert(path.clone(), parse_layout_reserved(&tree.read(path)?));
                    }
                    Some((path.clone(), layouts.get(path).cloned().unwrap_or_default()))
                }
            };
            let mut prefixes = BTreeMap::<String, usize>::new();
            for (name, offset) in svd_registers(peripheral) {
                for header in &registers {
                    if header.offset == offset
                        && let Some(prefix) = header.name.strip_suffix(name.as_str())
                        && (prefix.is_empty() || prefix.ends_with('_'))
                    {
                        *prefixes.entry(prefix.to_owned()).or_default() += 1;
                    }
                }
            }
            let prefix = prefixes
                .into_iter()
                .max_by_key(|(prefix, count)| (*count, std::cmp::Reverse(prefix.len())))
                .map(|(prefix, _)| prefix)
                .unwrap_or_default();
            peripherals.insert(
                rule.peripheral.clone(),
                PeripheralHeader {
                    path: rule.registers.clone(),
                    registers,
                    prefix,
                    layout,
                },
            );
        }
        Ok(Self { peripherals })
    }

    /// The header register of `peripheral`'s register `name` at `offset`.
    pub(crate) fn lookup(
        &self,
        peripheral: &str,
        name: &str,
        offset: u64,
        array: bool,
    ) -> Lookup<'_> {
        let Some(header) = self.peripherals.get(peripheral) else {
            return Lookup::Unmapped;
        };
        let path = header.path.as_str();
        let full = format!("{}{name}", header.prefix);
        let at = |offset: u64| header.registers.iter().filter(move |r| r.offset == offset);
        if let Some(register) = at(offset).find(|r| r.name == full) {
            return Lookup::Matched {
                path,
                prefix: &header.prefix,
                register,
            };
        }
        // The SVD and the headers spell some registers differently: an array
        // run's element named by its index in the run (`FUNC0_IN_SEL_CFG`
        // for `FUNC10_IN_SEL_CFG`; only for an array), a prefix (`SPI_MEM_CMD`,
        // `SPI_MEM_C_CMD`). At the same offset, the same name without
        // digits or the SVD's name as the header's tail is the same
        // register; the same last word is only when the header has no
        // register of the SVD's name elsewhere.
        let digitless = |text: &str| text.replace(|c: char| c.is_ascii_digit(), "");
        let last = |text: &str| {
            text.rsplit('_')
                .next()
                .unwrap_or("")
                .trim_end_matches(|c: char| c.is_ascii_digit())
                .to_owned()
        };
        if let Some(register) = at(offset).find(|r| {
            (array && digitless(&r.name) == digitless(&full))
                || r.name.ends_with(&format!("_{name}"))
        }) {
            return Lookup::Matched {
                path,
                prefix: &header.prefix,
                register,
            };
        }
        if let Some(register) = header.registers.iter().find(|r| r.name == full) {
            return Lookup::Moved { path, register };
        }
        if let Some(register) = at(offset).find(|r| last(&r.name) == last(name)) {
            return Lookup::Matched {
                path,
                prefix: &header.prefix,
                register,
            };
        }
        let reserved = header.layout.as_ref().and_then(|(layout, words)| {
            words
                .iter()
                .find(|words| offset >= words.offset && offset < words.offset + words.words * 4)
                .map(|words| (layout.as_str(), words.line))
        });
        Lookup::Absent { path, reserved }
    }

    /// The prefix of `peripheral`'s header names (`EFUSE_`) and the header
    /// register named `name` after it, if the peripheral is mapped.
    pub(crate) fn by_name(
        &self,
        peripheral: &str,
        name: &str,
    ) -> Option<(&str, &str, Option<&HeaderRegister>)> {
        let header = self.peripherals.get(peripheral)?;
        let full = format!("{}{name}", header.prefix);
        Some((
            header.path.as_str(),
            header.prefix.as_str(),
            header.registers.iter().find(|r| r.name == full),
        ))
    }
}

/// One vendor field as the derivation sees it.
pub(crate) struct SvdField<'a> {
    pub(crate) name: &'a str,
    pub(crate) low: u32,
    pub(crate) width: u32,
    pub(crate) description: &'a str,
}

/// What the header states about one vendor field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DerivedField {
    pub(crate) class: Class,
    pub(crate) access: String,
    pub(crate) source: String,
}

/// What the header states about one vendor register.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Derived {
    /// The register's definition line.
    pub(crate) source: String,
    /// Why the register has no API at all.
    pub(crate) none: Option<String>,
    /// Why the register is only read.
    pub(crate) narrow: Vec<String>,
    /// Every field is write-one-to-clear.
    pub(crate) one_to_clear: bool,
    /// Bits no header field declares and those of reserved fields.
    pub(crate) reserved: u32,
    /// Vendor fields the header calls reserved.
    pub(crate) reserved_fields: BTreeSet<String>,
    pub(crate) fields: BTreeMap<String, DerivedField>,
    /// The reasons in `narrow` that are only a field the hardware also
    /// changes: a command port's data word lifts them.
    pub(crate) hardware: BTreeSet<String>,
    /// Vendor fields the header names otherwise at the same bits: the
    /// header's name without its peripheral prefix, which the API takes.
    pub(crate) renamed: BTreeMap<String, String>,
}

fn bits(low: u32, width: u32) -> u32 {
    if width == 0 || width > 32 || low >= 32 {
        0
    } else {
        (u32::MAX >> (32 - width)) << low
    }
}

fn cite(path: &str, field: &HeaderField) -> String {
    format!("esp-idf:{path}:{}", field.line)
}

/// Derive a vendor register's semantics from its header register.
pub(crate) fn derive_register(
    path: &str,
    prefix: &str,
    header: &HeaderRegister,
    fields: &[SvdField<'_>],
    array: bool,
) -> Derived {
    let mut derived = Derived {
        source: format!("esp-idf:{path}:{}", header.line),
        ..Derived::default()
    };
    let mut declared = 0_u32;
    for field in &header.fields {
        let mask = bits(field.low, field.width());
        if !reserved_description(&field.description) {
            declared |= mask;
        }
        if derived.none.is_none() {
            if read_clears(&field.description) {
                derived.none = Some(format!(
                    "field {} is cleared by a read ({})",
                    field.name,
                    cite(path, field)
                ));
            } else if sequential_port(&field.description) {
                derived.none = Some(format!(
                    "field {} is a port read and written in sequence ({})",
                    field.name,
                    cite(path, field)
                ));
            }
        }
    }
    derived.reserved = !declared;
    for field in fields {
        if derived.none.is_none() && read_clears(field.description) {
            derived.none = Some(format!(
                "field {} is cleared by a read (the vendor SVD's description)",
                field.name
            ));
        }
        let Some(header_field) = header
            .fields
            .iter()
            .find(|h| h.low == field.low && h.width() == field.width)
        else {
            if bits(field.low, field.width) & declared == 0 {
                // Bits only reserved header lanes cover.
                derived.reserved_fields.insert(field.name.to_owned());
            } else {
                derived.narrow.push(format!(
                    "field {}: {} has no field at bits [{}:{}]",
                    field.name,
                    derived.source,
                    field.low + field.width - 1,
                    field.low
                ));
            }
            continue;
        };
        if reserved_description(&header_field.description) {
            derived.reserved_fields.insert(field.name.to_owned());
            continue;
        }
        // An array's fields name its first element (`FILTER_CH0_EN` in
        // every `GLITCH_FILTER_CH<n>`): their numbers are not compared.
        let digitless = |text: &str| text.replace(|c: char| c.is_ascii_digit(), "");
        let same = if array {
            crate::soc_overlay::same_field(&digitless(&header_field.name), &digitless(field.name))
        } else {
            crate::soc_overlay::same_field(&header_field.name, field.name)
        };
        if !same {
            let own = header_field
                .name
                .strip_prefix(prefix)
                .filter(|rest| !rest.is_empty())
                .unwrap_or(&header_field.name);
            // An array's field names no element: `RX_ARB_WEIGHT_VALUE_CH0`
            // of every channel is `RX_ARB_WEIGHT_VALUE_CH`.
            let own = if array {
                own.split('_')
                    .map(|word| {
                        let stem = word.trim_end_matches(|c: char| c.is_ascii_digit());
                        if stem.is_empty() { word } else { stem }
                    })
                    .collect::<Vec<_>>()
                    .join("_")
            } else {
                own.to_owned()
            };
            derived.renamed.insert(field.name.to_owned(), own);
        }
        derived.fields.insert(
            field.name.to_owned(),
            DerivedField {
                class: class(&header_field.access),
                access: header_field.access.clone(),
                source: cite(path, header_field),
            },
        );
    }
    let classes = derived
        .fields
        .values()
        .map(|field| field.class)
        .collect::<BTreeSet<_>>();
    let one_to_clear = !derived.fields.is_empty() && classes == BTreeSet::from([Class::OneToClear]);
    derived.one_to_clear = one_to_clear && derived.narrow.is_empty();
    for (name, field) in &derived.fields {
        let width = fields
            .iter()
            .find(|svd| svd.name == name)
            .map_or(1, |svd| svd.width);
        let reason = match field.class {
            Class::HardwareWritable => Some("the hardware also changes it"),
            Class::Command if width > 1 => Some("the hardware also changes it"),
            Class::SetOnly => Some("a one written sets it"),
            Class::Unknown => Some("an access class without a rule"),
            Class::OneToClear if !one_to_clear => {
                Some("write-one-to-clear beside fields that are not")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            let text = format!(
                "field {name} is {} ({}): {reason}",
                field.access, field.source
            );
            if matches!(field.class, Class::HardwareWritable | Class::Command) {
                derived.hardware.insert(text.clone());
            }
            derived.narrow.push(text);
        }
    }
    derived
}

#[cfg(test)]
mod tests;
