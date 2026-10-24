//! The strictness overlay against the pinned ESP-IDF register headers:
//! `import` builds overlay entries from them, `check` verifies every entry
//! against the file and line it cites and against the vendor SVD.
//!
//! An `esp-idf:<path>:<line>` source is one of
//!
//! - a register definition line of a `*_reg.h` (`#define <NAME>_REG (... + 0x<offset>)`):
//!   the register's name ends with the SVD register's name and its offset
//!   equals the SVD's;
//! - a field line of a `*_reg.h` (`/** <NAME> : <access>; bitpos: [<hi>:<lo>]; ...`):
//!   the field belongs to the SVD register, ends with the SVD field's name
//!   and has the SVD field's bits;
//! - the closing line of a register union of a `*_struct.h`
//!   (`} <name>_reg_t;`): its `reserved_*` members are the entry's reserved
//!   bits and its other members are the SVD register's fields;
//! - an enumerator line of an `interrupts.h`: its position in the enum is
//!   the source number and its name is the interrupt's.
//!
//! The header access class decides what an entry may state: `R/W` is a
//! plain field, `WT` a write trigger, `HRO` a hardware-modified read-only
//! field, a register with a hardware-modified field (`SC`, `SS`, ...) is
//! read-only, and a register whose fields are all `WTC` is write-one-to-clear.
//! `trm:` and `esp-hal:` sources, `read-side-effect` and enumerated values
//! are not in the headers: `check` counts them as unchecked.

use std::collections::{BTreeMap, BTreeSet};

use svd_rs::{Access, MaybeArray, RegisterCluster};

use crate::soc::{derive, raw_status, trigger_candidate};
use crate::{
    Error, FieldKind, FieldRule, InterruptRule, OverlayAccess, RegisterRule, Result,
    StrictnessOverlay,
};

/// The files of a pinned ESP-IDF checkout.
pub trait HeaderTree {
    /// The text of `path`, relative to the checkout root.
    fn read(&self, path: &str) -> Result<String>;
    /// The file names directly in `directory`, relative to the checkout root.
    fn list(&self, directory: &str) -> Result<Vec<String>>;
}

/// The verdict of [`check_overlay`].
#[derive(Debug, Default)]
pub struct OverlayCheck {
    /// Entries verified against the headers.
    pub checked: usize,
    /// Entries or claims no header states: `trm:`, `esp-hal:` sources,
    /// `read-side-effect` and enumerated values.
    pub unchecked: Vec<String>,
    /// Entries that do not match their source or the SVD.
    pub failures: Vec<String>,
    /// Disagreements that do not make an entry wrong: the SVD and the
    /// headers place the register at different offsets.
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------- headers

#[derive(Clone, Debug)]
struct HeaderField {
    line: usize,
    name: String,
    access: String,
    low: u32,
    high: u32,
}

#[derive(Clone, Debug)]
struct HeaderRegister {
    line: usize,
    /// The macro name without its `_REG` suffix.
    name: String,
    offset: u64,
    fields: Vec<HeaderField>,
}

#[derive(Clone, Debug)]
struct HeaderStruct {
    close_line: usize,
    type_name: String,
    /// More than one view of the register: the members are not one layout.
    ambiguous: bool,
    reserved: u32,
    /// Named members: (name, low bit, width).
    members: Vec<(String, u32, u32)>,
}

fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    text.get(..end).unwrap_or("")
}

fn parse_number(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text.trim_end_matches(['U', 'u', 'L', 'l']);
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

fn parse_reg_header(text: &str) -> Vec<HeaderRegister> {
    let mut registers: Vec<HeaderRegister> = Vec::new();
    let mut open = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if let Some(rest) = line.strip_prefix("#define ") {
            let name = identifier(rest);
            if let Some(base) = name.strip_suffix("_REG") {
                let offset = rest
                    .split_once("+ 0x")
                    .map(|(_, tail)| tail)
                    .map(|tail| format!("0x{}", identifier(tail)))
                    .and_then(|hex| parse_number(&hex));
                if let Some(offset) = offset {
                    registers.push(HeaderRegister {
                        line: number,
                        name: base.to_owned(),
                        offset,
                        fields: Vec::new(),
                    });
                    open = true;
                }
            }
            continue;
        }
        let Some(rest) = line.trim_start().strip_prefix("/** ") else {
            continue;
        };
        let name = identifier(rest);
        let Some(tail) = rest.get(name.len()..).and_then(|t| t.strip_prefix(" : ")) else {
            continue;
        };
        let Some((access, tail)) = tail.split_once(';') else {
            continue;
        };
        let Some(bits) = tail
            .trim_start()
            .strip_prefix("bitpos: [")
            .and_then(|t| t.split_once(']'))
            .map(|(bits, _)| bits)
        else {
            continue;
        };
        let (high, low) = bits.split_once(':').unwrap_or((bits, bits));
        let (Some(high), Some(low)) = (parse_number(high), parse_number(low)) else {
            continue;
        };
        let (Ok(high), Ok(low)) = (u32::try_from(high), u32::try_from(low)) else {
            continue;
        };
        if open && let Some(register) = registers.last_mut() {
            register.fields.push(HeaderField {
                line: number,
                name: name.to_owned(),
                access: access.trim().to_owned(),
                low,
                high,
            });
        }
    }
    registers
}

fn parse_struct_header(text: &str) -> Vec<HeaderStruct> {
    let mut structs = Vec::new();
    let mut current: Option<HeaderStruct> = None;
    let mut offset = 0_u32;
    let mut views = 0_u32;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("typedef union") {
            current = Some(HeaderStruct {
                close_line: 0,
                type_name: String::new(),
                ambiguous: false,
                reserved: 0,
                members: Vec::new(),
            });
            offset = 0;
            views = 0;
            continue;
        }
        let Some(open) = current.as_mut() else {
            continue;
        };
        if trimmed.starts_with("struct") && trimmed.ends_with('{') {
            views += 1;
            offset = 0;
            if views > 1 {
                open.ambiguous = true;
            }
        } else if let Some(rest) = trimmed.strip_prefix("uint32_t ") {
            let name = identifier(rest);
            let width = rest
                .get(name.len()..)
                .and_then(|t| t.trim_start().strip_prefix(':'))
                .map(|t| t.trim_end_matches(';'))
                .and_then(parse_number)
                .and_then(|w| u32::try_from(w).ok());
            if let Some(width) = width
                && views == 1
                && width > 0
                && offset + width <= 32
            {
                if name.starts_with("reserved") {
                    let mask = u32::MAX >> (32 - width);
                    open.reserved |= mask << offset;
                } else {
                    open.members.push((name.to_owned(), offset, width));
                }
                offset += width;
            } else if width.is_some() {
                open.ambiguous = true;
            }
        } else if trimmed.starts_with('}')
            && trimmed.ends_with("_t;")
            && let Some(mut done) = current.take()
        {
            done.close_line = index + 1;
            done.type_name = trimmed
                .trim_start_matches('}')
                .trim()
                .trim_end_matches(';')
                .to_owned();
            structs.push(done);
        }
    }
    structs
}

#[derive(Clone, Debug)]
struct Enumerator {
    line: usize,
    name: String,
    ordinal: u64,
}

fn parse_enums(text: &str) -> Vec<(String, Vec<Enumerator>)> {
    let mut enums = Vec::new();
    let mut open: Option<Vec<Enumerator>> = None;
    let mut next = 0_u64;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("typedef enum") {
            open = Some(Vec::new());
            next = 0;
            continue;
        }
        let Some(items) = open.as_mut() else {
            continue;
        };
        if let Some(rest) = trimmed.strip_prefix('}') {
            let name = rest.trim().trim_end_matches(';').trim().to_owned();
            enums.push((name, std::mem::take(items)));
            open = None;
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with(['/', '#', '*']) {
            continue;
        }
        let name = identifier(trimmed);
        if name.is_empty() {
            continue;
        }
        let tail = trimmed.get(name.len()..).unwrap_or("").trim_start();
        if let Some(value) = tail.strip_prefix('=') {
            let value = value.split([',', '/']).next().unwrap_or("");
            if let Some(value) = parse_number(value) {
                next = value;
            }
        }
        items.push(Enumerator {
            line: index + 1,
            name: name.to_owned(),
            ordinal: next,
        });
        next += 1;
    }
    enums
}

/// The interrupt name an enumerator stands for: `ETS_<NAME>_INTR_SOURCE` or
/// `ETS_<NAME>_SOURCE`; the CPU software interrupts follow the vendor SVD's
/// spelling `FROM_CPU_INTR<n>`.
fn interrupt_name(enumerator: &str) -> Option<String> {
    let name = enumerator.strip_prefix("ETS_")?;
    let name = name
        .strip_suffix("_INTR_SOURCE")
        .or_else(|| name.strip_suffix("_SOURCE"))?;
    if let Some(number) = name.strip_prefix("CPU_INTR_FROM_CPU_") {
        return Some(format!("FROM_CPU_INTR{number}"));
    }
    Some(name.to_owned())
}

/// Parsed headers by path.
struct Headers<'a> {
    tree: &'a dyn HeaderTree,
    registers: BTreeMap<String, Vec<HeaderRegister>>,
    structs: BTreeMap<String, Vec<HeaderStruct>>,
    enums: BTreeMap<String, Vec<(String, Vec<Enumerator>)>>,
}

impl<'a> Headers<'a> {
    fn new(tree: &'a dyn HeaderTree) -> Self {
        Self {
            tree,
            registers: BTreeMap::new(),
            structs: BTreeMap::new(),
            enums: BTreeMap::new(),
        }
    }

    fn registers(&mut self, path: &str) -> Result<&[HeaderRegister]> {
        if !self.registers.contains_key(path) {
            let parsed = parse_reg_header(&self.tree.read(path)?);
            self.registers.insert(path.to_owned(), parsed);
        }
        Ok(self.registers.get(path).map_or(&[], Vec::as_slice))
    }

    fn structs(&mut self, path: &str) -> Result<&[HeaderStruct]> {
        if !self.structs.contains_key(path) {
            let parsed = parse_struct_header(&self.tree.read(path)?);
            self.structs.insert(path.to_owned(), parsed);
        }
        Ok(self.structs.get(path).map_or(&[], Vec::as_slice))
    }

    fn enums(&mut self, path: &str) -> Result<&[(String, Vec<Enumerator>)]> {
        if !self.enums.contains_key(path) {
            let parsed = parse_enums(&self.tree.read(path)?);
            self.enums.insert(path.to_owned(), parsed);
        }
        Ok(self.enums.get(path).map_or(&[], Vec::as_slice))
    }
}

// -------------------------------------------------------------------- SVD

#[derive(Clone, Debug)]
struct SvdField {
    name: String,
    low: u32,
    width: u32,
    access: Access,
}

#[derive(Clone, Debug)]
struct SvdRegister {
    name: String,
    offset: u64,
    access: Access,
    fields: Vec<SvdField>,
}

#[derive(Debug, Default)]
struct Svd {
    /// Peripheral names in the SVD's order.
    order: Vec<String>,
    peripherals: BTreeMap<String, Vec<SvdRegister>>,
    /// Every interrupt source number the SVD lists.
    interrupts: BTreeSet<u64>,
}

fn load_svd(text: &str) -> Result<Svd> {
    let mut device =
        svd_parser::parse(text).map_err(|error| Error::message(format!("vendor SVD: {error}")))?;
    derive(&mut device)?;
    let mut svd = Svd::default();
    for peripheral in &device.peripherals {
        let default = peripheral
            .default_register_properties
            .access
            .or(device.default_register_properties.access)
            .unwrap_or(Access::ReadWrite);
        let mut registers = Vec::new();
        for child in peripheral.registers.as_deref().unwrap_or_default() {
            let RegisterCluster::Register(register) = child else {
                continue;
            };
            let access = register.properties.access.unwrap_or(default);
            let fields = register
                .fields
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|field| match field {
                    MaybeArray::Single(info) => Some(SvdField {
                        name: info.name.clone(),
                        low: info.bit_range.offset,
                        width: info.bit_range.width,
                        access: info.access.unwrap_or(access),
                    }),
                    MaybeArray::Array(..) => None,
                })
                .collect();
            registers.push(SvdRegister {
                name: register.name.clone(),
                offset: u64::from(register.address_offset),
                access,
                fields,
            });
        }
        for interrupt in &peripheral.interrupt {
            svd.interrupts.insert(u64::from(interrupt.value));
        }
        svd.order.push(peripheral.name.clone());
        svd.peripherals.insert(peripheral.name.clone(), registers);
    }
    Ok(svd)
}

/// Whether the header register is the SVD register: the vendor SVD and the
/// headers spell some names differently (`SPI_MEM_CMD` and `SPI_MEM_C_CMD`),
/// so a register also agrees by its offset and last word.
fn same_register(header: &HeaderRegister, register: &SvdRegister) -> bool {
    ends_with_name(&header.name, &register.name)
        || (header.offset == register.offset && {
            let word = |name: &str| {
                name.rsplit('_')
                    .next()
                    .unwrap_or("")
                    .replace("%s", "")
                    .trim_end_matches(|c: char| c.is_ascii_digit())
                    .to_owned()
            };
            word(&header.name) == word(&register.name)
        })
}

/// Whether the header field name is the SVD field name: the same name, the
/// SVD's name inside the header's (`SPI1_MEM_S_PER_END_INT_CLR` holds
/// `PER_END`) or the same last word (`SPI_FMEM_C_OUTMINBYTELEN` and
/// `SPI_FMEM_OUTMINBYTELEN`); the register and the bits identify the field,
/// the name only guards against a wrong line.
fn same_field(header: &str, name: &str) -> bool {
    ends_with_name(header, name)
        || format!("_{header}_").contains(&format!("_{name}_"))
        || header.rsplit('_').next() == name.rsplit('_').next()
}

fn ends_with_name(header: &str, name: &str) -> bool {
    header == name
        || header
            .strip_suffix(name)
            .is_some_and(|rest| rest.ends_with('_'))
}

// ---------------------------------------------------------- access classes

fn tokens(access: &str) -> Vec<&str> {
    access.split('/').collect()
}

fn hardware_modified(access: &str) -> bool {
    tokens(access)
        .iter()
        .any(|token| !matches!(*token, "R" | "W" | "RO" | "WO" | "WT" | "HRO"))
}

fn write_one_to_clear(fields: &[HeaderField]) -> bool {
    !fields.is_empty()
        && fields.iter().all(|field| {
            let tokens = tokens(&field.access);
            tokens.contains(&"WTC") && !tokens.contains(&"W")
        })
}

/// Whether the register's header fields justify `access`.
fn register_access_holds(access: OverlayAccess, fields: &[HeaderField]) -> bool {
    match access {
        OverlayAccess::ReadOnly => {
            fields.iter().any(|field| hardware_modified(&field.access))
                || fields
                    .iter()
                    .all(|field| matches!(field.access.as_str(), "RO" | "HRO"))
        }
        OverlayAccess::ReadWrite => fields
            .iter()
            .all(|field| matches!(field.access.as_str(), "R/W" | "WT")),
        OverlayAccess::WriteOnly => fields
            .iter()
            .all(|field| matches!(field.access.as_str(), "WO" | "WT")),
    }
}

/// Whether the header access of a field justifies what the rule states.
fn field_access_holds(rule: &FieldRule, header: &str) -> bool {
    let access_fits = |access: OverlayAccess| match access {
        OverlayAccess::ReadOnly => matches!(header, "RO" | "HRO"),
        OverlayAccess::ReadWrite => matches!(header, "R/W" | "WT"),
        OverlayAccess::WriteOnly => matches!(header, "WO" | "WT"),
    };
    match (rule.kind, rule.access) {
        (Some(FieldKind::Plain), access) => {
            header == "R/W" && access.is_none_or(|a| a == OverlayAccess::ReadWrite)
        }
        (Some(FieldKind::WriteTrigger), access) => header == "WT" && access.is_none_or(access_fits),
        (None, Some(access)) => access_fits(access),
        (None, None) => false,
    }
}

// ------------------------------------------------------------------ check

struct Source<'a> {
    path: &'a str,
    line: usize,
}

fn split_source(source: &str) -> Option<Source<'_>> {
    let rest = source.strip_prefix("esp-idf:")?;
    let (path, line) = rest.rsplit_once(':')?;
    Some(Source {
        path,
        line: line.parse().ok()?,
    })
}

fn mask(low: u32, width: u32) -> u32 {
    if width == 0 || width > 32 || low >= 32 {
        0
    } else {
        (u32::MAX >> (32 - width)) << low
    }
}

/// Verify every entry of `overlay` against `tree` and `vendor_svd`.
pub fn check_overlay(
    overlay: &StrictnessOverlay,
    vendor_svd: &str,
    tree: &dyn HeaderTree,
) -> Result<OverlayCheck> {
    let svd = load_svd(vendor_svd)?;
    let mut headers = Headers::new(tree);
    let mut result = OverlayCheck::default();
    for rule in &overlay.registers {
        let subject = format!("register {}.{}", rule.peripheral, rule.register);
        match check_register(rule, &svd, &mut headers, &subject, &mut result.notes)? {
            Verdict::Checked => result.checked += 1,
            Verdict::Unchecked(why) => result.unchecked.push(format!("{subject}: {why}")),
            Verdict::Failed(why) => result.failures.extend(why),
        }
        if rule.read_side_effect.is_some() {
            result
                .unchecked
                .push(format!("{subject}: read-side-effect is not in the headers"));
        }
    }
    for rule in &overlay.fields {
        let subject = format!("field {}.{}.{}", rule.peripheral, rule.register, rule.field);
        match check_field(rule, &svd, &mut headers, &subject)? {
            Verdict::Checked => result.checked += 1,
            Verdict::Unchecked(why) => result.unchecked.push(format!("{subject}: {why}")),
            Verdict::Failed(why) => result.failures.extend(why),
        }
        if !rule.values.is_empty() {
            result
                .unchecked
                .push(format!("{subject}: enumerated values are not parsed"));
        }
    }
    for rule in &overlay.interrupts {
        let subject = format!("interrupt {}", rule.name);
        match check_interrupt(rule, &mut headers, &subject)? {
            Verdict::Checked => result.checked += 1,
            Verdict::Unchecked(why) => result.unchecked.push(format!("{subject}: {why}")),
            Verdict::Failed(why) => result.failures.extend(why),
        }
    }
    Ok(result)
}

enum Verdict {
    Checked,
    Unchecked(String),
    Failed(Vec<String>),
}

fn idf_sources<'a>(sources: &'a [String]) -> (Vec<Source<'a>>, usize) {
    let mut cited = Vec::new();
    let mut other = 0;
    for source in sources {
        match split_source(source) {
            Some(source) => cited.push(source),
            None => other += 1,
        }
    }
    (cited, other)
}

fn check_register(
    rule: &RegisterRule,
    svd: &Svd,
    headers: &mut Headers<'_>,
    subject: &str,
    notes: &mut Vec<String>,
) -> Result<Verdict> {
    let (cited, other) = idf_sources(&rule.sources);
    if cited.is_empty() {
        return Ok(Verdict::Unchecked(format!(
            "no esp-idf source ({other} other)"
        )));
    }
    let Some(register) = svd
        .peripherals
        .get(&rule.peripheral)
        .and_then(|registers| registers.iter().find(|r| r.name == rule.register))
    else {
        return Ok(Verdict::Failed(vec![format!(
            "{subject}: not in the vendor SVD"
        )]));
    };
    let mut failures = Vec::new();
    let mut header: Option<HeaderRegister> = None;
    let mut layouts: Vec<(String, HeaderStruct)> = Vec::new();
    for source in &cited {
        let cite = format!("{subject}: {}:{}", source.path, source.line);
        if source.path.ends_with("_reg.h") {
            let found = headers
                .registers(source.path)?
                .iter()
                .find(|r| r.line == source.line)
                .cloned();
            match found {
                None => failures.push(format!("{cite} is not a register definition")),
                Some(found) => {
                    if !same_register(&found, register) {
                        failures.push(format!(
                            "{cite} defines {}, not a register named {}",
                            found.name, register.name
                        ));
                    } else {
                        if found.offset != register.offset {
                            notes.push(format!(
                                "{cite} has offset {:#x}, the SVD {:#x}",
                                found.offset, register.offset
                            ));
                        }
                        header = Some(found);
                    }
                }
            }
        } else if source.path.ends_with("_struct.h") {
            let found = headers
                .structs(source.path)?
                .iter()
                .find(|s| s.close_line == source.line)
                .cloned();
            match found {
                None => failures.push(format!("{cite} is not the end of a register union")),
                Some(found) => layouts.push((cite, found)),
            }
        } else {
            failures.push(format!("{cite} is neither a *_reg.h nor a *_struct.h"));
        }
    }
    let mut layout: Option<HeaderStruct> = None;
    for (cite, found) in layouts {
        let own = |name: &str| {
            let expected = format!("{}_reg_t", name.to_lowercase());
            found.type_name == expected || found.type_name.ends_with(&format!("_{expected}"))
        };
        if header
            .as_ref()
            .map_or_else(|| own(&register.name), |h| own(&h.name))
        {
            layout = Some(found);
        } else {
            failures.push(format!(
                "{cite} ends {}, not the union of register {}",
                found.type_name, register.name
            ));
        }
    }
    if let Some(reserved) = rule.reserved {
        match &layout {
            None => failures.push(format!("{subject}: reserved cites no register union")),
            Some(layout) if layout.ambiguous => {
                failures.push(format!("{subject}: its union has more than one view"));
            }
            Some(layout) => {
                if layout.reserved != reserved {
                    failures.push(format!(
                        "{subject}: reserved {reserved:#010x}, the header {:#010x}",
                        layout.reserved
                    ));
                }
                let header_fields: BTreeSet<(u32, u32)> = layout
                    .members
                    .iter()
                    .map(|(_, low, width)| (*low, *width))
                    .collect();
                let svd_fields: BTreeSet<(u32, u32)> = register
                    .fields
                    .iter()
                    .map(|field| (field.low, field.width))
                    .collect();
                if header_fields != svd_fields {
                    failures.push(format!(
                        "{subject}: the header's fields differ from the SVD's"
                    ));
                }
            }
        }
    }
    let wants_header =
        rule.access.is_some() || rule.write_one_to_clear || rule.raw_status_writable || rule.fifo;
    if wants_header && header.is_none() && failures.is_empty() {
        failures.push(format!("{subject}: cites no register definition"));
    }
    if let Some(header) = &header {
        if let Some(access) = rule.access
            && !register_access_holds(access, &header.fields)
        {
            failures.push(format!(
                "{subject}: access {access:?} does not follow from the header's field access ({})",
                header
                    .fields
                    .iter()
                    .map(|f| f.access.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if rule.write_one_to_clear && !write_one_to_clear(&header.fields) {
            failures.push(format!(
                "{subject}: write-one-to-clear, but the header's fields are not all WTC"
            ));
        }
        if rule.raw_status_writable && !rule.write_one_to_clear {
            failures.push(format!(
                "{subject}: raw-status-writable without write-one-to-clear"
            ));
        }
    }
    Ok(if failures.is_empty() {
        Verdict::Checked
    } else {
        Verdict::Failed(failures)
    })
}

fn check_field(
    rule: &FieldRule,
    svd: &Svd,
    headers: &mut Headers<'_>,
    subject: &str,
) -> Result<Verdict> {
    let (cited, other) = idf_sources(&rule.sources);
    if cited.is_empty() {
        return Ok(Verdict::Unchecked(format!(
            "no esp-idf source ({other} other)"
        )));
    }
    let Some(register) = svd
        .peripherals
        .get(&rule.peripheral)
        .and_then(|registers| registers.iter().find(|r| r.name == rule.register))
    else {
        return Ok(Verdict::Failed(vec![format!(
            "{subject}: register not in the vendor SVD"
        )]));
    };
    let Some(field) = register.fields.iter().find(|f| f.name == rule.field) else {
        return Ok(Verdict::Failed(vec![format!(
            "{subject}: not in the vendor SVD"
        )]));
    };
    let mut failures = Vec::new();
    let mut matched = false;
    for source in &cited {
        let cite = format!("{subject}: {}:{}", source.path, source.line);
        if !source.path.ends_with("_reg.h") {
            failures.push(format!("{cite} is not a *_reg.h"));
            continue;
        }
        let found = headers.registers(source.path)?.iter().find_map(|r| {
            r.fields
                .iter()
                .find(|f| f.line == source.line)
                .map(|f| (r.clone(), f.clone()))
        });
        let Some((header_register, header_field)) = found else {
            failures.push(format!("{cite} is not a field line"));
            continue;
        };
        if !same_register(&header_register, register) {
            failures.push(format!(
                "{cite} is a field of {}, not of register {}",
                header_register.name, rule.register
            ));
            continue;
        }
        if !same_field(&header_field.name, &rule.field) {
            failures.push(format!(
                "{cite} is field {}, not {}",
                header_field.name, rule.field
            ));
            continue;
        }
        let width = header_field.high.saturating_sub(header_field.low) + 1;
        if header_field.low != field.low || width != field.width {
            failures.push(format!(
                "{cite} has bits [{}:{}], the SVD [{}:{}]",
                header_field.high,
                header_field.low,
                field.low + field.width - 1,
                field.low
            ));
            continue;
        }
        if !field_access_holds(rule, &header_field.access) {
            failures.push(format!(
                "{cite} has access {}, which does not justify kind {:?} access {:?}",
                header_field.access, rule.kind, rule.access
            ));
            continue;
        }
        matched = true;
    }
    if !matched && failures.is_empty() {
        failures.push(format!("{subject}: no source"));
    }
    Ok(if failures.is_empty() {
        Verdict::Checked
    } else {
        Verdict::Failed(failures)
    })
}

fn check_interrupt(
    rule: &InterruptRule,
    headers: &mut Headers<'_>,
    subject: &str,
) -> Result<Verdict> {
    let (cited, other) = idf_sources(&rule.sources);
    if cited.is_empty() {
        return Ok(Verdict::Unchecked(format!(
            "no esp-idf source ({other} other)"
        )));
    }
    let mut failures = Vec::new();
    for source in &cited {
        let cite = format!("{subject}: {}:{}", source.path, source.line);
        let found = headers
            .enums(source.path)?
            .iter()
            .find_map(|(_, items)| items.iter().find(|e| e.line == source.line).cloned());
        let Some(found) = found else {
            failures.push(format!("{cite} is not an enumerator"));
            continue;
        };
        if found.ordinal != u64::from(rule.source) {
            failures.push(format!(
                "{cite} is number {}, the entry says {}",
                found.ordinal, rule.source
            ));
        }
        let name = if found.name == "ETS_MAX_INTR_SOURCE" {
            Some("MAX".to_owned())
        } else {
            interrupt_name(&found.name)
        };
        if name.as_deref() != Some(rule.name.as_str()) {
            failures.push(format!(
                "{cite} is {}, not interrupt {}",
                found.name, rule.name
            ));
        }
    }
    Ok(if failures.is_empty() {
        Verdict::Checked
    } else {
        Verdict::Failed(failures)
    })
}

// ----------------------------------------------------------------- import

/// Build the overlay's entries for `peripherals` (all those the existing
/// overlay states, when empty) from the headers of `chip`, and render the
/// overlay file: `existing` text up to its first entry, the interrupt
/// sources the SVD lacks, then the entries per peripheral in the SVD's order.
///
/// What the headers cannot state is kept from `existing`: `fifo` and
/// `read-side-effect` flags, enumerated values and the entries that cite
/// `trm:` or `esp-hal:` only.
pub fn import_overlay(
    existing_text: &str,
    existing: &StrictnessOverlay,
    chip: &str,
    vendor_svd: &str,
    peripherals: &[String],
    tree: &dyn HeaderTree,
) -> Result<String> {
    let svd = load_svd(vendor_svd)?;
    let mut headers = Headers::new(tree);
    let directory = format!("components/soc/{chip}/register/soc");
    let files: Vec<String> = tree
        .list(&directory)?
        .into_iter()
        .filter(|name| name.ends_with("_reg.h"))
        .collect();
    let selected: BTreeSet<String> = if peripherals.is_empty() {
        existing
            .registers
            .iter()
            .map(|r| r.peripheral.clone())
            .chain(existing.fields.iter().map(|f| f.peripheral.clone()))
            .collect()
    } else {
        peripherals.iter().cloned().collect()
    };
    for name in &selected {
        if !svd.peripherals.contains_key(name) {
            return Err(Error::message(format!(
                "peripheral {name} is not in the vendor SVD"
            )));
        }
    }
    let mut out = String::new();
    let prefix_end = existing_text
        .find("\n[[")
        .map_or(existing_text.len(), |i| i + 1);
    out.push_str(existing_text.get(..prefix_end).unwrap_or(""));
    // Interrupt sources the SVD lacks.
    let interrupts_path = format!("components/soc/{chip}/include/soc/interrupts.h");
    for rule in derive_interrupts(&svd, &mut headers, &interrupts_path, existing)? {
        render_interrupt(&mut out, &rule);
    }
    // The peripherals in the order the overlay already states them, then
    // the others in the SVD's.
    let mut order: Vec<&String> = Vec::new();
    for name in existing
        .registers
        .iter()
        .map(|r| &r.peripheral)
        .chain(existing.fields.iter().map(|f| &f.peripheral))
    {
        if !order.contains(&name) && svd.peripherals.contains_key(name) {
            order.push(name);
        }
    }
    order.extend(
        svd.order
            .iter()
            .filter(|name| !order.contains(name))
            .collect::<Vec<_>>(),
    );
    for name in order {
        if selected.contains(name) {
            let all = svd.peripherals.get(name).map_or(&[][..], Vec::as_slice);
            // The registers the overlay states, or all of an explicitly
            // named peripheral.
            let stated: Vec<SvdRegister> = all
                .iter()
                .filter(|register| {
                    !peripherals.is_empty()
                        || existing
                            .registers
                            .iter()
                            .any(|r| r.peripheral == *name && r.register == register.name)
                        || existing
                            .fields
                            .iter()
                            .any(|f| f.peripheral == *name && f.register == register.name)
                })
                .cloned()
                .collect();
            let registers = stated.as_slice();
            let cited = cited_files(existing, name);
            let file_paths = best_files(registers, &files, &directory, &cited, &mut headers)?;
            let (reg_rules, field_rules) =
                derive_peripheral(name, registers, &file_paths, &mut headers)?;
            render_peripheral(&mut out, name, registers, existing, reg_rules, field_rules);
        } else {
            for rule in existing.registers.iter().filter(|r| &r.peripheral == name) {
                render_register(&mut out, rule);
                for field in existing
                    .fields
                    .iter()
                    .filter(|f| f.peripheral == rule.peripheral && f.register == rule.register)
                {
                    render_field(&mut out, field);
                }
            }
        }
    }
    Ok(out)
}

fn derive_interrupts(
    svd: &Svd,
    headers: &mut Headers<'_>,
    path: &str,
    existing: &StrictnessOverlay,
) -> Result<Vec<InterruptRule>> {
    let blocks = headers.enums(path)?;
    let Some((_, items)) = blocks.iter().find(|(name, _)| name == "periph_interrupt_t") else {
        return Ok(existing.interrupts.clone());
    };
    let mut rules = Vec::new();
    for item in items {
        if svd.interrupts.contains(&item.ordinal) {
            continue;
        }
        let name = if item.name == "ETS_MAX_INTR_SOURCE" {
            "MAX".to_owned()
        } else if let Some(name) = interrupt_name(&item.name) {
            name
        } else {
            continue;
        };
        let Ok(source) = u16::try_from(item.ordinal) else {
            continue;
        };
        let description = existing
            .interrupts
            .iter()
            .find(|r| r.name == name)
            .and_then(|r| r.description.clone());
        rules.push(InterruptRule {
            name,
            source,
            description,
            sources: vec![format!("esp-idf:{path}:{}", item.line)],
        });
    }
    Ok(rules)
}

/// The `*_reg.h` files the overlay already cites for `peripheral`.
fn cited_files(existing: &StrictnessOverlay, peripheral: &str) -> BTreeSet<String> {
    existing
        .registers
        .iter()
        .filter(|r| r.peripheral == peripheral)
        .flat_map(|r| &r.sources)
        .chain(
            existing
                .fields
                .iter()
                .filter(|f| f.peripheral == peripheral)
                .flat_map(|f| &f.sources),
        )
        .filter_map(|source| split_source(source))
        .filter(|source| source.path.ends_with("_reg.h"))
        .map(|source| source.path.to_owned())
        .collect()
}

/// Whether the header register has exactly the SVD register's fields.
fn same_fields(header: &HeaderRegister, register: &SvdRegister) -> bool {
    let from_header: BTreeSet<(u32, u32)> = header
        .fields
        .iter()
        .map(|f| (f.low, f.high + 1 - f.low))
        .collect();
    let from_svd: BTreeSet<(u32, u32)> = register.fields.iter().map(|f| (f.low, f.width)).collect();
    from_header == from_svd
}

/// The header files that hold the `registers`, the best match first: most
/// registers with the SVD's fields, then the files the overlay cites, then
/// by name.
fn best_files(
    registers: &[SvdRegister],
    files: &[String],
    directory: &str,
    cited: &BTreeSet<String>,
    headers: &mut Headers<'_>,
) -> Result<Vec<String>> {
    let mut scored = Vec::new();
    for file in files {
        let path = format!("{directory}/{file}");
        let parsed = headers.registers(&path)?;
        let hits = registers
            .iter()
            .filter(|register| {
                parsed
                    .iter()
                    .any(|h| same_register(h, register) && same_fields(h, register))
            })
            .count();
        if hits > 0 {
            scored.push((hits, !cited.contains(&path), path));
        }
    }
    scored.sort();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    Ok(scored.into_iter().map(|(_, _, path)| path).collect())
}

type Derived = (BTreeMap<String, RegisterRule>, Vec<FieldRule>);

fn derive_peripheral(
    peripheral: &str,
    registers: &[SvdRegister],
    files: &[String],
    headers: &mut Headers<'_>,
) -> Result<Derived> {
    let mut register_rules = BTreeMap::new();
    let mut field_rules = Vec::new();
    for register in registers {
        let mut found: Option<(String, HeaderRegister)> = None;
        for exact in [true, false] {
            for path in files {
                if let Some(h) = headers
                    .registers(path)?
                    .iter()
                    .find(|h| same_register(h, register) && (!exact || same_fields(h, register)))
                {
                    found = Some((path.clone(), h.clone()));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let Some((path, header)) = found else {
            continue;
        };
        let mut rule = RegisterRule {
            peripheral: peripheral.to_owned(),
            register: register.name.clone(),
            access: None,
            reserved: None,
            read_side_effect: None,
            fifo: false,
            write_one_to_clear: false,
            raw_status_writable: false,
            sources: vec![format!("esp-idf:{path}:{}", header.line)],
        };
        if write_one_to_clear(&header.fields) {
            rule.write_one_to_clear = true;
            rule.raw_status_writable = raw_status(&register.name);
        } else if register.access != Access::ReadOnly
            && header.fields.iter().any(|f| hardware_modified(&f.access))
        {
            rule.access = Some(OverlayAccess::ReadOnly);
        }
        // A read-only register is never written: no reserved bits to keep.
        if rule.access.is_none() {
            // The union of the register: reserved bits, when its members are
            // the SVD's fields.
            let struct_path = path.replace("_reg.h", "_struct.h");
            if struct_path != path
                && let Ok(structs) = headers.structs(&struct_path)
            {
                let expected = format!("{}_reg_t", header.name.to_lowercase());
                if let Some(layout) = structs.iter().find(|s| s.type_name == expected)
                    && !layout.ambiguous
                    && layout.reserved != 0
                {
                    let header_fields: BTreeSet<(u32, u32)> =
                        layout.members.iter().map(|(_, l, w)| (*l, *w)).collect();
                    let svd_fields: BTreeSet<(u32, u32)> =
                        register.fields.iter().map(|f| (f.low, f.width)).collect();
                    let declared = register
                        .fields
                        .iter()
                        .fold(0, |bits, f| bits | mask(f.low, f.width));
                    if header_fields == svd_fields && layout.reserved & declared == 0 {
                        rule.reserved = Some(layout.reserved);
                        rule.sources
                            .push(format!("esp-idf:{struct_path}:{}", layout.close_line));
                    }
                }
            }
        }
        if rule.access.is_some() || rule.reserved.is_some() || rule.write_one_to_clear {
            register_rules.insert(register.name.clone(), rule);
        }
        for field in &register.fields {
            let Some(h) = header.fields.iter().find(|h| {
                same_field(&h.name, &field.name)
                    && h.low == field.low
                    && h.high + 1 - h.low == field.width
            }) else {
                continue;
            };
            let mut rule = FieldRule {
                peripheral: peripheral.to_owned(),
                register: register.name.clone(),
                field: field.name.clone(),
                access: None,
                kind: None,
                values: Vec::new(),
                sources: vec![format!("esp-idf:{path}:{}", h.line)],
            };
            match h.access.as_str() {
                "WT" => {
                    rule.kind = Some(FieldKind::WriteTrigger);
                    rule.access = Some(OverlayAccess::ReadWrite);
                }
                "R/W" if trigger_candidate(&field.name) || field.access != Access::ReadWrite => {
                    rule.kind = Some(FieldKind::Plain);
                    if field.access != Access::ReadWrite {
                        rule.access = Some(OverlayAccess::ReadWrite);
                    }
                }
                "HRO" if field.access != Access::ReadOnly => {
                    rule.access = Some(OverlayAccess::ReadOnly);
                }
                _ => continue,
            }
            field_rules.push(rule);
        }
    }
    Ok((register_rules, field_rules))
}

fn render_peripheral(
    out: &mut String,
    peripheral: &str,
    registers: &[SvdRegister],
    existing: &StrictnessOverlay,
    mut derived: BTreeMap<String, RegisterRule>,
    fields: Vec<FieldRule>,
) {
    for register in registers {
        let kept = existing
            .registers
            .iter()
            .find(|r| r.peripheral == peripheral && r.register == register.name);
        let rule = match (derived.remove(&register.name), kept) {
            (Some(mut rule), Some(kept)) => {
                rule.fifo = kept.fifo;
                rule.read_side_effect = kept.read_side_effect;
                Some(rule)
            }
            (Some(rule), None) => Some(rule),
            (None, Some(kept)) if kept.fifo || kept.read_side_effect.is_some() => {
                Some(kept.clone())
            }
            (None, _) => None,
        };
        if let Some(rule) = rule {
            render_register(out, &rule);
        }
        for field in fields.iter().filter(|f| f.register == register.name) {
            let mut field = field.clone();
            if let Some(kept) = existing.fields.iter().find(|f| {
                f.peripheral == peripheral && f.register == register.name && f.field == field.field
            }) {
                field.values = kept.values.clone();
            }
            render_field(out, &field);
        }
        for kept in existing.fields.iter().filter(|f| {
            f.peripheral == peripheral
                && f.register == register.name
                && !fields.iter().any(|d| d.field == f.field)
                && (!f.values.is_empty() || f.sources.iter().all(|s| !s.starts_with("esp-idf:")))
        }) {
            render_field(out, kept);
        }
    }
}

fn quote(value: &str) -> String {
    format!("{value:?}")
}

fn access_name(access: OverlayAccess) -> &'static str {
    match access {
        OverlayAccess::ReadOnly => "read-only",
        OverlayAccess::ReadWrite => "read-write",
        OverlayAccess::WriteOnly => "write-only",
    }
}

fn sources_line(sources: &[String]) -> String {
    format!(
        "sources = [{}]\n\n",
        sources
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_interrupt(out: &mut String, rule: &InterruptRule) {
    out.push_str("[[interrupts]]\n");
    out.push_str(&format!(
        "name = {}\nsource = {}\n",
        quote(&rule.name),
        rule.source
    ));
    if let Some(description) = &rule.description {
        out.push_str(&format!("description = {}\n", quote(description)));
    }
    out.push_str(&sources_line(&rule.sources));
}

fn render_register(out: &mut String, rule: &RegisterRule) {
    out.push_str("[[registers]]\n");
    out.push_str(&format!(
        "peripheral = {}\nregister = {}\n",
        quote(&rule.peripheral),
        quote(&rule.register)
    ));
    if let Some(access) = rule.access {
        out.push_str(&format!("access = {}\n", quote(access_name(access))));
    }
    if let Some(effect) = rule.read_side_effect {
        out.push_str(&format!("read-side-effect = {effect}\n"));
    }
    if rule.fifo {
        out.push_str("fifo = true\n");
    }
    if rule.write_one_to_clear {
        out.push_str("write-one-to-clear = true\n");
    }
    if rule.raw_status_writable {
        out.push_str("raw-status-writable = true\n");
    }
    if let Some(reserved) = rule.reserved {
        out.push_str(&format!("reserved = {reserved:#010x}\n"));
    }
    out.push_str(&sources_line(&rule.sources));
}

fn render_field(out: &mut String, rule: &FieldRule) {
    out.push_str("[[fields]]\n");
    out.push_str(&format!(
        "peripheral = {}\nregister = {}\nfield = {}\n",
        quote(&rule.peripheral),
        quote(&rule.register),
        quote(&rule.field)
    ));
    if let Some(kind) = rule.kind {
        let kind = match kind {
            FieldKind::Plain => "plain",
            FieldKind::WriteTrigger => "write-trigger",
        };
        out.push_str(&format!("kind = {}\n", quote(kind)));
    }
    if let Some(access) = rule.access {
        out.push_str(&format!("access = {}\n", quote(access_name(access))));
    }
    out.push_str(&sources_line(&rule.sources));
    for value in &rule.values {
        out.push_str(&format!(
            "[[fields.values]]\nname = {}\nvalue = {}\n",
            quote(&value.name),
            value.value
        ));
    }
}

#[cfg(test)]
mod tests;
