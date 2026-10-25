//! The strictness overlay against the pinned ESP-IDF register headers:
//! `import` pins the headers of a peripheral and proposes the offset
//! corrections they imply, `check` verifies every entry against the file and
//! line it cites and against the vendor SVD.
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

use svd_rs::{MaybeArray, RegisterCluster};

use crate::soc_headers::Lookup;
use crate::{
    CommandRule, Error, FieldKind, FieldRule, InterruptRule, OverlayAccess, RegisterRule, Result,
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
}

// ---------------------------------------------------------------- headers

#[derive(Clone, Debug)]
pub(crate) struct HeaderField {
    pub(crate) line: usize,
    pub(crate) name: String,
    pub(crate) access: String,
    pub(crate) low: u32,
    pub(crate) high: u32,
    /// The comment lines after the field line, joined by single spaces.
    pub(crate) description: String,
}

impl HeaderField {
    pub(crate) fn width(&self) -> u32 {
        self.high.saturating_sub(self.low) + 1
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HeaderRegister {
    pub(crate) line: usize,
    /// The macro name without its `_REG` suffix.
    pub(crate) name: String,
    pub(crate) offset: u64,
    pub(crate) fields: Vec<HeaderField>,
}

#[derive(Clone, Debug)]
pub(crate) struct HeaderStruct {
    pub(crate) close_line: usize,
    pub(crate) type_name: String,
    /// More than one view of the register: the members are not one layout.
    pub(crate) ambiguous: bool,
    pub(crate) reserved: u32,
    /// Named members: (name, low bit, width).
    pub(crate) members: Vec<(String, u32, u32)>,
}

/// Reserved words of a device layout (`} <name>_dev_t;`): the member
/// `uint32_t reserved_<offset>[<words>];` at `line`.
#[derive(Clone, Debug)]
pub(crate) struct ReservedWords {
    pub(crate) line: usize,
    pub(crate) offset: u64,
    pub(crate) words: u64,
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

pub(crate) fn parse_reg_header(text: &str) -> Vec<HeaderRegister> {
    let mut registers: Vec<HeaderRegister> = Vec::new();
    let mut open = false;
    // Whether the comment of the last field is still open.
    let mut describing = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if describing {
            let text = line
                .trim()
                .trim_start_matches('*')
                .trim_end_matches("*/")
                .trim();
            if let Some(field) = registers.last_mut().and_then(|r| r.fields.last_mut())
                && !text.is_empty()
                && !line.trim_start().starts_with("*/")
            {
                if !field.description.is_empty() {
                    field.description.push(' ');
                }
                field.description.push_str(text);
            }
            if line.contains("*/") {
                describing = false;
            }
            continue;
        }
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
                description: String::new(),
            });
            describing = !line.contains("*/");
        }
    }
    registers
}

/// The reserved words of the one device layout of a `*_struct.h`; none
/// when it has no device layout or more than one.
pub(crate) fn parse_layout_reserved(text: &str) -> Vec<ReservedWords> {
    let mut layouts: Vec<Vec<ReservedWords>> = Vec::new();
    let mut open: Option<Vec<ReservedWords>> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("typedef struct") && trimmed.ends_with('{') {
            open = Some(Vec::new());
            continue;
        }
        let Some(words) = open.as_mut() else {
            continue;
        };
        if let Some(rest) = trimmed.strip_prefix("uint32_t reserved_")
            && !rest.contains(':')
        {
            let hex = identifier(rest);
            let count = rest
                .get(hex.len()..)
                .and_then(|t| t.strip_prefix('['))
                .and_then(|t| t.split_once(']'))
                .map_or(Some(1), |(n, _)| parse_number(n));
            if let (Ok(offset), Some(count)) = (u64::from_str_radix(hex, 16), count) {
                words.push(ReservedWords {
                    line: index + 1,
                    offset,
                    words: count,
                });
            }
        } else if trimmed.starts_with('}')
            && let Some(done) = open.take()
            && trimmed.trim_end_matches(';').ends_with("_dev_t")
        {
            layouts.push(done);
        }
    }
    if layouts.len() == 1 {
        layouts.pop().unwrap_or_default()
    } else {
        Vec::new()
    }
}

pub(crate) fn parse_struct_header(text: &str) -> Vec<HeaderStruct> {
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
}

#[derive(Clone, Debug)]
struct SvdRegister {
    name: String,
    offset: u64,
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

fn corrected_device(
    text: &str,
    overlay: &StrictnessOverlay,
    tree: &dyn HeaderTree,
) -> Result<svd_rs::Device> {
    Ok(crate::soc::corrected_vendor(text, overlay, tree)?.0)
}

fn svd_of(device: &svd_rs::Device) -> Svd {
    let mut svd = Svd::default();
    for peripheral in &device.peripherals {
        let mut registers = Vec::new();
        for child in peripheral.registers.as_deref().unwrap_or_default() {
            let RegisterCluster::Register(register) = child else {
                continue;
            };
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
                    }),
                    MaybeArray::Array(..) => None,
                })
                .collect();
            registers.push(SvdRegister {
                name: register.name.clone(),
                offset: u64::from(register.address_offset),
                fields,
            });
        }
        for interrupt in &peripheral.interrupt {
            svd.interrupts.insert(u64::from(interrupt.value));
        }
        svd.order.push(peripheral.name.clone());
        svd.peripherals.insert(peripheral.name.clone(), registers);
    }
    svd
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

/// Whether the header field name is the SVD field name: the SVD's words
/// are the header's in order, the header adding only words such as its
/// prefix (`PCR_CLK_480M_OEN` is `CLK_480M_OEN`, `SPI1_MEM_S_PER_END_INT_CLR`
/// holds `PER_END`, `SPI_FMEM_C_OUTMINBYTELEN` holds `FMEM_OUTMINBYTELEN`),
/// a word possibly without the instance's number the header appends.
/// A different word is a different field: `CLK_320M_OEN` is not
/// `PCR_CLK_480M_OEN`.
pub(crate) fn same_field(header: &str, name: &str) -> bool {
    let mut words = header.split('_');
    name.split('_').filter(|word| !word.is_empty()).all(|word| {
        words.any(|candidate| {
            candidate == word
                // An instance's number the SVD leaves out: `SPI_MEM_SYNC_RESET`
                // is SPI1's `SPI1_MEM_C_SYNC_RESET`, `IN_DONE_CH_INT_RAW`
                // channel 0's `AHB_DMA_IN_DONE_CH0_INT_RAW`.
                || candidate.trim_end_matches(|c: char| c.is_ascii_digit()) == word
        })
    })
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

/// Verify every entry of `overlay` against `tree` and `vendor_svd`.
pub fn check_overlay(
    overlay: &StrictnessOverlay,
    vendor_svd: &str,
    tree: &dyn HeaderTree,
) -> Result<OverlayCheck> {
    let device = corrected_device(vendor_svd, overlay, tree)?;
    check_device(overlay, &device, tree)
}

/// Verify every entry of `overlay` against `tree` and the corrected vendor
/// SVD `device`. A register the header places at another offset than the
/// SVD is a failure.
pub(crate) fn check_device(
    overlay: &StrictnessOverlay,
    device: &svd_rs::Device,
    tree: &dyn HeaderTree,
) -> Result<OverlayCheck> {
    let svd = svd_of(device);
    let mut headers = Headers::new(tree);
    let mut result = OverlayCheck::default();
    for rule in &overlay.registers {
        let subject = format!("register {}.{}", rule.peripheral, rule.register);
        match check_register(rule, &svd, &mut headers, &subject)? {
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
    for rule in &overlay.commands {
        let subject = format!("commands {}.{}", rule.peripheral, rule.register);
        match check_commands(rule, &svd, &mut headers, &subject)? {
            Verdict::Checked => result.checked += 1,
            Verdict::Unchecked(why) => result.unchecked.push(format!("{subject}: {why}")),
            Verdict::Failed(why) => result.failures.extend(why),
        }
        let (cited, other) = idf_sources(&rule.sources);
        let code = cited
            .iter()
            .filter(|source| !source.path.ends_with("_reg.h"))
            .count();
        if code + other > 0 {
            result.unchecked.push(format!(
                "{subject}: {} sources beside the register header are not parsed",
                code + other
            ));
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
                            failures.push(format!(
                                "{cite} has offset {:#x}, the SVD {:#x}: the overlay corrects its offset",
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
        } else if !(rule.driver_writes || !rule.define_fields.is_empty()) {
            // Only a driver's write or a field definition cites a file
            // beside the register headers.
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
    let wants_header = rule.access.is_some()
        || rule.write_one_to_clear
        || rule.raw_status_writable
        || rule.fifo
        || rule.data_port;
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

/// A command register's entry: its `*_reg.h` source defines the register,
/// and the header makes each command and qualifier one self-clearing bit
/// (`R/W/SC`, `R/W/WTC`) an application may use.
fn check_commands(
    rule: &CommandRule,
    svd: &Svd,
    headers: &mut Headers<'_>,
    subject: &str,
) -> Result<Verdict> {
    let Some(register) = svd
        .peripherals
        .get(&rule.peripheral)
        .and_then(|registers| registers.iter().find(|r| r.name == rule.register))
    else {
        return Ok(Verdict::Failed(vec![format!(
            "{subject}: not in the vendor SVD"
        )]));
    };
    let (cited, _) = idf_sources(&rule.sources);
    let mut failures = Vec::new();
    let mut header = None;
    for source in cited
        .iter()
        .filter(|source| source.path.ends_with("_reg.h"))
    {
        let cite = format!("{subject}: {}:{}", source.path, source.line);
        match headers
            .registers(source.path)?
            .iter()
            .find(|r| r.line == source.line)
        {
            None => failures.push(format!("{cite} is not a register definition")),
            Some(found) if !same_register(found, register) => failures.push(format!(
                "{cite} defines {}, not a register named {}",
                found.name, register.name
            )),
            Some(found) => header = Some(found.clone()),
        }
    }
    let Some(header) = header else {
        failures.push(format!("{subject}: cites no register definition"));
        return Ok(Verdict::Failed(failures));
    };
    for name in rule.fields.iter().chain(&rule.qualifiers) {
        let Some(field) = register.fields.iter().find(|f| &f.name == name) else {
            failures.push(format!("{subject}: field {name} is not in the vendor SVD"));
            continue;
        };
        let Some(found) = header
            .fields
            .iter()
            .find(|h| h.low == field.low && h.width() == field.width)
        else {
            failures.push(format!(
                "{subject}: the header has no field at {name}'s bits"
            ));
            continue;
        };
        if found.width() != 1 || !matches!(found.access.as_str(), "R/W/SC" | "R/W/WTC") {
            failures.push(format!(
                "{subject}: {} is {} [{}:{}], not one self-clearing bit (R/W/SC, R/W/WTC)",
                found.name, found.access, found.high, found.low
            ));
        }
        if crate::soc_headers::debug_only(&found.description) {
            failures.push(format!(
                "{subject}: the header keeps {} to internal debugging",
                found.name
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
        let named = rule.rename.as_deref().unwrap_or(&rule.field);
        if !same_field(&header_field.name, named) {
            failures.push(format!(
                "{cite} is field {}, not {named}{}",
                header_field.name,
                if rule.rename.is_none() {
                    ": the overlay renames it after its source"
                } else {
                    ""
                }
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
        if found.name == "ETS_MAX_INTR_SOURCE" {
            failures.push(format!(
                "{cite} is ETS_MAX_INTR_SOURCE, the number of sources, not a source"
            ));
            continue;
        }
        let name = interrupt_name(&found.name);
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

/// What [`import_overlay`] proposes for an overlay.
#[derive(Debug)]
pub struct OverlayImport {
    /// The overlay text with the `[[headers]]` pins of the peripherals that
    /// had none; the text itself when every peripheral is pinned.
    pub text: String,
    /// The pins added to `text`.
    pub pins: Vec<String>,
    /// Correction entries for the registers the headers place at another
    /// offset than the vendor SVD, as overlay text.
    pub corrections: Vec<String>,
    /// Header fields the vendor SVD lacks, one line each.
    pub mismatches: Vec<String>,
    /// `text` with the `corrections` appended.
    pub text_with_corrections: String,
}

/// Pin the headers of `peripherals` (all those the overlay already pins,
/// when empty) and compare them with the vendor SVD.
///
/// A peripheral without a `[[headers]]` entry gets the `*_reg.h` of
/// `components/soc/<chip>/register/soc` that holds most of its registers,
/// and the `*_struct.h` beside it. The registers the headers place at another
/// offset than the SVD come back as correction entries. Everything else the
/// overlay needs (access classes, reserved bits, reserved words) is derived
/// from the pinned headers when the model is generated, so no per-register
/// entry is written.
pub fn import_overlay(
    existing_text: &str,
    existing: &StrictnessOverlay,
    chip: &str,
    vendor_svd: &str,
    peripherals: &[String],
    tree: &dyn HeaderTree,
) -> Result<OverlayImport> {
    let svd = svd_of(&corrected_device(vendor_svd, existing, tree)?);
    let selected: Vec<String> = if peripherals.is_empty() {
        existing
            .headers
            .iter()
            .map(|h| h.peripheral.clone())
            .collect()
    } else {
        peripherals.to_vec()
    };
    for name in &selected {
        if !svd.peripherals.contains_key(name) {
            return Err(Error::message(format!(
                "peripheral {name} is not in the vendor SVD"
            )));
        }
    }
    let directory = format!("components/soc/{chip}/register/soc");
    let files = tree.list(&directory)?;
    let mut headers = Headers::new(tree);
    let mut text = existing_text.to_owned();
    let mut pins = Vec::new();
    for name in &selected {
        if existing.headers.iter().any(|h| &h.peripheral == name) {
            continue;
        }
        let registers = svd.peripherals.get(name).map_or(&[][..], Vec::as_slice);
        let candidates: Vec<String> = files
            .iter()
            .filter(|file| file.ends_with("_reg.h"))
            .cloned()
            .collect();
        let best = best_files(registers, &candidates, &directory, &mut headers)?;
        let Some(registers_path) = best.into_iter().next() else {
            return Err(Error::message(format!(
                "no register header of {directory} holds the registers of {name}"
            )));
        };
        let layout = registers_path
            .strip_suffix("_reg.h")
            .map(|stem| format!("{stem}_struct.h"))
            .filter(|path| {
                path.rsplit('/')
                    .next()
                    .is_some_and(|file| files.iter().any(|f| f == file))
            });
        let mut block = format!(
            "[[headers]]\nperipheral = {}\nregisters = {}\n",
            quote(name),
            quote(&registers_path)
        );
        if let Some(layout) = &layout {
            block.push_str(&format!("layout = {}\n", quote(layout)));
        }
        block.push('\n');
        insert_pin(&mut text, name, &block);
        pins.push(block);
    }
    let pinned = StrictnessOverlay::parse(&text)?;
    let (device, facts) = crate::soc::corrected_vendor(vendor_svd, &pinned, tree)?;
    let svd = svd_of(&device);
    let mut corrections = Vec::new();
    let mut mismatches = Vec::new();
    for name in &selected {
        let Some(registers) = svd.peripherals.get(name) else {
            continue;
        };
        for register in registers {
            match facts.lookup(name, &register.name, register.offset, false) {
                Lookup::Moved {
                    path,
                    register: header,
                } => corrections.push(format!(
                    "[[registers]]\nperipheral = {}\nregister = {}\noffset = {:#x}\n\
                     sources = [{}]\n\n",
                    quote(name),
                    quote(&register.name),
                    header.offset,
                    quote(&format!("esp-idf:{path}:{}", header.line)),
                )),
                Lookup::Matched {
                    path,
                    register: header,
                    ..
                } => {
                    mismatches.extend(field_mismatches(name, register, header, path));
                }
                Lookup::Unmapped | Lookup::Absent { .. } => {}
            }
        }
    }
    let mut text_with_corrections = text.clone();
    for correction in &corrections {
        if !text_with_corrections.ends_with("\n\n") {
            text_with_corrections.push('\n');
        }
        text_with_corrections.push_str(correction);
    }
    StrictnessOverlay::parse(&text_with_corrections)?;
    Ok(OverlayImport {
        text,
        pins,
        corrections,
        mismatches,
        text_with_corrections,
    })
}

/// Insert the `[[headers]]` block of `peripheral` among the existing ones,
/// which are in the order of their peripherals.
fn insert_pin(text: &mut String, peripheral: &str, block: &str) {
    // (start, end, peripheral) of each block, the end after its blank line.
    let mut blocks: Vec<(usize, usize, String)> = Vec::new();
    let mut offset = 0;
    let mut open: Option<(usize, String)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "[[headers]]" {
            open = Some((offset, String::new()));
        } else if let Some((_, name)) = open.as_mut()
            && let Some(value) = trimmed
                .strip_prefix("peripheral = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        {
            *name = value.to_owned();
        }
        offset += line.len();
        if trimmed.is_empty()
            && let Some((start, name)) = open.take()
        {
            blocks.push((start, offset, name));
        }
    }
    let position = blocks
        .iter()
        .find(|(_, _, name)| name.as_str() > peripheral)
        .map(|(start, _, _)| *start)
        .or_else(|| blocks.last().map(|(_, end, _)| *end))
        .unwrap_or_else(|| text.find("\n[[").map_or(text.len(), |i| i + 1));
    text.insert_str(position, block);
}

/// The header's fields the SVD register lacks.
fn field_mismatches(
    peripheral: &str,
    register: &SvdRegister,
    header: &HeaderRegister,
    path: &str,
) -> Vec<String> {
    let from_svd: BTreeSet<(u32, u32)> = register.fields.iter().map(|f| (f.low, f.width)).collect();
    let from_header: BTreeSet<(u32, u32)> = header
        .fields
        .iter()
        .filter(|f| !crate::soc_headers::reserved_description(&f.description))
        .map(|f| (f.low, f.width()))
        .collect();
    let mut out = Vec::new();
    for field in &header.fields {
        let bits = (field.low, field.width());
        if from_header.contains(&bits) && !from_svd.contains(&bits) {
            out.push(format!(
                "{peripheral}.{}: esp-idf:{path}:{} states {} [{}:{}], the vendor SVD has no field there",
                register.name, field.line, field.name, field.high, field.low
            ));
        }
    }
    out
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
/// registers with the SVD's fields, then by name.
fn best_files(
    registers: &[SvdRegister],
    files: &[String],
    directory: &str,
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
            scored.push((hits, path));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(scored.into_iter().map(|(_, path)| path).collect())
}

fn quote(value: &str) -> String {
    format!("\"{value}\"")
}

#[cfg(test)]
mod tests;
