//! The whole vendor SVD of a chip as typed register blocks: the `soc`
//! module of the PAC API (`generated.rs`).
//!
//! Every vendor peripheral is rendered by the typed renderer
//! ([`crate::pac_typed`]) after one conservative pass. The vendor SVD names
//! registers, fields and their access, but not what a read or a write does
//! beyond that: it declares no read action and almost no modified-write
//! values, and it leaves reserved bits undeclared. So by default
//!
//! - a register whose every bit is a declared plain field or a reserved bit
//!   the overlay cites, and which no name marks as a trigger, a FIFO or a
//!   raw status, keeps its typed field API (reads and read-modify-writes);
//! - a register with undeclared bits, a write-only field, a write-trigger
//!   candidate, overlapping fields, vendor modified-write values or a write
//!   constraint, or a raw interrupt status, is only read;
//! - a register a read may change (a FIFO or data port by its name), one
//!   that is not 32 bits wide, one without fields, and one whose word the
//!   chip's curated model already describes, gets no API at all.
//!
//! The chip's strictness overlay ([`StrictnessOverlay`]) adds what the SVD
//! lacks, each entry with its source: access, read side effects,
//! write-one-to-clear, write triggers, command registers, FIFO and command
//! data ports, enumerated values and reserved bits, and the interrupt
//! sources the SVD does not list. The report
//! ([`SocRender::report`]) lists every register the pass narrowed, with its
//! reasons.
//!
//! One partition, `SocPeripherals`, holds every block: the PAC root claims
//! them at once. The rendered `const` assertion checks that no two blocks of
//! the module, or of it and the chip's curated typed blocks, list the same
//! word.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

use serde::Deserialize;
use svd_rs::{
    Access, Device, EnumeratedValue, EnumeratedValues, MaybeArray, ModifiedWriteValues, Peripheral,
    RegisterCluster, ValidateLevel, WriteConstraint, WriteConstraintRange,
};

use crate::soc_headers::{
    Class, Derived, HeaderFacts, Lookup, SvdField, derive_register, elements, read_clears,
};
use crate::{
    CommandRegister, DataPort, Error, FifoPort, InterruptSource, OwnershipPartition, PacApiPack,
    Result, WriteTriggerField,
};

/// A chip's strictness overlay over its vendor SVD (`model/strictness.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StrictnessOverlay {
    pub schema: u32,
    /// The pinned ESP-IDF revision `esp-idf:` sources cite.
    pub esp_idf: String,
    /// The pinned header of each vendor peripheral.
    #[serde(default)]
    pub headers: Vec<HeaderRule>,
    /// Peripherals whose registers get no API at all.
    #[serde(default)]
    pub no_api: Vec<NoApiRule>,
    #[serde(default)]
    pub registers: Vec<RegisterRule>,
    #[serde(default)]
    pub fields: Vec<FieldRule>,
    #[serde(default)]
    pub interrupts: Vec<InterruptRule>,
    /// Command registers: the only fields the API starts.
    #[serde(default)]
    pub commands: Vec<CommandRule>,
}

/// A command register (`[[commands]]`): its `fields` are commands software
/// starts by writing one and the hardware clears when done. The register is
/// written only by these starts: a start refuses while any self-clearing
/// bit of the register reads one, so the zero it writes to the others
/// never lands on a running command. Every other field is only read. The
/// sources state the start and the completion of each command; a
/// one-bit `R/W/SC` or `R/W/WTC` field not named is held: read, and zero in
/// every start.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CommandRule {
    pub peripheral: String,
    pub register: String,
    /// The vendor names of the command fields.
    pub fields: Vec<String>,
    /// One-bit fields a start writes together with its command, each set
    /// or clear by the start's argument (a program/erase qualifier).
    #[serde(default)]
    pub qualifiers: Vec<String>,
    pub sources: Vec<String>,
}

/// The pinned ESP-IDF headers of one vendor peripheral: the source of its
/// registers' access classes, reserved lanes and reserved words.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HeaderRule {
    pub peripheral: String,
    /// The `*_reg.h` path in the pinned ESP-IDF.
    pub registers: String,
    /// The `*_struct.h` path, whose device layout reserves words.
    #[serde(default)]
    pub layout: Option<String>,
}

/// A vendor peripheral whose semantics the headers describe only in text, so
/// none of its registers has an API (not even a read) until a typed API is
/// written for it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct NoApiRule {
    pub peripheral: String,
    pub reason: String,
    pub sources: Vec<String>,
}

/// What the overlay states about one vendor register.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterRule {
    pub peripheral: String,
    pub register: String,
    /// The register's offset where the SVD places it elsewhere, or of a
    /// register the SVD lacks: its header's.
    #[serde(default)]
    pub offset: Option<u32>,
    /// A register the vendor SVD lacks, built from its header definition.
    #[serde(default)]
    pub add: bool,
    /// The register's access, replacing the SVD's.
    #[serde(default)]
    pub access: Option<OverlayAccess>,
    /// Bits the source marks reserved: never fields, kept by every write.
    #[serde(default)]
    pub reserved: Option<u32>,
    /// Whether a read changes the register: `true` gives it no API unless
    /// it is a FIFO port, `false` clears a name's read candidate.
    #[serde(default)]
    pub read_side_effect: Option<bool>,
    /// A FIFO port: popped and pushed only.
    #[serde(default)]
    pub fifo: bool,
    /// Every field is write-one-to-clear: sampled and acknowledged.
    #[serde(default)]
    pub write_one_to_clear: bool,
    /// A raw interrupt status whose writes the source defines: its fields
    /// keep their access.
    #[serde(default)]
    pub raw_status_writable: bool,
    /// The data words of a command port: one field the hardware also
    /// writes, read and stored whole, never modified.
    #[serde(default)]
    pub data_port: bool,
    /// The register's fields where the vendor SVD and its header disagree
    /// on them, from the cited source: they replace the SVD's fields, and
    /// each takes its class from the header field whose bits hold it.
    #[serde(default)]
    pub define_fields: Vec<DefinedField>,
    /// The fields the vendor's own driver writes, each on the cited line,
    /// where the header keeps the field to internal debugging or calls a
    /// defined field's bits reserved: the field keeps its header's access.
    #[serde(default)]
    pub driver_writes: Vec<DriverWrite>,
    /// The fields the vendor's driver writes through a value it stores in
    /// the whole word, where one line does not name both.
    #[serde(default)]
    pub driver_writes_word: Vec<DriverWordWrite>,
    pub sources: Vec<String>,
}

impl RegisterRule {
    /// The fields `driver-writes` and `driver-writes-word` name.
    pub(crate) fn driver_written(&self) -> BTreeSet<&str> {
        self.driver_writes
            .iter()
            .map(|write| write.field.as_str())
            .chain(
                self.driver_writes_word
                    .iter()
                    .map(|write| write.field.as_str()),
            )
            .collect()
    }
}

/// One field the vendor's driver writes on one line (`driver-writes`):
/// `source` is an `esp-idf:` line outside `components/soc/` that assigns
/// the field of the register's device member or passes the register and
/// the field's macro to a register write macro.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DriverWrite {
    pub field: String,
    pub source: String,
}

/// One field the vendor's driver writes through a stored word
/// (`driver-writes-word`): `value` assigns an identifier, `store` writes
/// the whole register word.
///
/// Without `return`, `value` names the field's macro and `store`, in the
/// same function, stores a value holding the identifier. With `return`, a
/// one-bit field's `value` assigns its bit as a literal, `return` returns
/// the identifier from the same function and `store` is a whole-word write
/// of the register in the same file; the caller's path from that function
/// to the store is not checked but listed, line by line, in
/// `reviewed-chain`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DriverWordWrite {
    pub field: String,
    pub value: String,
    #[serde(default, rename = "return")]
    pub returned: Option<String>,
    pub store: String,
    #[serde(default)]
    pub reviewed_chain: Vec<String>,
}

/// Whether `source` is an `esp-idf:<path>:<line>` citation of a file
/// outside the register headers.
fn driver_citation(source: &str) -> bool {
    source
        .strip_prefix("esp-idf:")
        .and_then(|rest| rest.rsplit_once(':'))
        .is_some_and(|(path, line)| {
            !path.starts_with("components/soc/")
                && !path.is_empty()
                && line.parse::<usize>().is_ok_and(|line| line > 0)
        })
}

/// One field an overlay register entry defines (`define-fields`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DefinedField {
    pub name: String,
    pub offset: u32,
    pub width: u32,
    pub description: String,
}

/// What the overlay states about one vendor field.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldRule {
    pub peripheral: String,
    pub register: String,
    pub field: String,
    /// The field's name by its source, where the SVD names it otherwise.
    #[serde(default)]
    pub rename: Option<String>,
    /// The field's access, replacing the SVD's.
    #[serde(default)]
    pub access: Option<OverlayAccess>,
    /// `plain` clears a name's trigger candidate; `write-trigger` makes the
    /// field one only its explicit trigger sets.
    #[serde(default)]
    pub kind: Option<FieldKind>,
    /// The field's values.
    #[serde(default)]
    pub values: Vec<ValueRule>,
    pub sources: Vec<String>,
}

/// One enumerated value of a field.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ValueRule {
    pub name: String,
    pub value: u32,
    #[serde(default)]
    pub description: Option<String>,
}

/// An interrupt-matrix source the vendor SVD does not list.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InterruptRule {
    pub name: String,
    pub source: u16,
    #[serde(default)]
    pub description: Option<String>,
    pub sources: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayAccess {
    ReadOnly,
    ReadWrite,
    WriteOnly,
}

impl OverlayAccess {
    fn svd(self) -> Access {
        match self {
            Self::ReadOnly => Access::ReadOnly,
            Self::ReadWrite => Access::ReadWrite,
            Self::WriteOnly => Access::WriteOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum FieldKind {
    Plain,
    WriteTrigger,
}

impl StrictnessOverlay {
    /// Read and validate the overlay at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let overlay: Self = toml_edit::de::from_str(&text)
            .map_err(|error| Error::message(format!("{}: {error}", path.display())))?;
        overlay.validate()?;
        Ok(overlay)
    }

    /// Parse and validate an overlay from text.
    pub fn parse(text: &str) -> Result<Self> {
        let overlay: Self =
            toml_edit::de::from_str(text).map_err(|error| Error::message(error.to_string()))?;
        overlay.validate()?;
        Ok(overlay)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::message("a strictness overlay is schema 1"));
        }
        if !is_revision(&self.esp_idf) {
            return Err(Error::message(
                "the overlay's esp-idf pin is a hexadecimal revision",
            ));
        }
        let mut seen = BTreeSet::new();
        for rule in &self.headers {
            if !seen.insert(rule.peripheral.clone()) {
                return Err(Error::message(format!(
                    "overlay names the header of {} twice",
                    rule.peripheral
                )));
            }
            let header = |path: &str, suffix: &str| {
                path.starts_with("components/soc/") && path.ends_with(suffix)
            };
            if !header(&rule.registers, "_reg.h")
                || rule
                    .layout
                    .as_deref()
                    .is_some_and(|layout| !header(layout, "_struct.h"))
            {
                return Err(Error::message(format!(
                    "overlay header of {}: registers is a components/soc/**/*_reg.h, layout a *_struct.h",
                    rule.peripheral
                )));
            }
        }
        let mut seen = BTreeSet::new();
        for rule in &self.no_api {
            if !seen.insert(rule.peripheral.clone()) {
                return Err(Error::message(format!(
                    "overlay states {} without API twice",
                    rule.peripheral
                )));
            }
            if rule.reason.trim().is_empty() {
                return Err(Error::message(format!(
                    "overlay no-api entry {} states no reason",
                    rule.peripheral
                )));
            }
            self.validate_sources(&rule.peripheral, &rule.sources)?;
        }
        let mut seen = BTreeSet::new();
        for rule in &self.registers {
            let subject = format!("{}.{}", rule.peripheral, rule.register);
            if !seen.insert(subject.clone()) {
                return Err(Error::message(format!("overlay states {subject} twice")));
            }
            self.validate_sources(&subject, &rule.sources)?;
            if (rule.add || rule.offset.is_some())
                && !rule
                    .sources
                    .iter()
                    .any(|source| source.starts_with("esp-idf:"))
            {
                return Err(Error::message(format!(
                    "overlay register {subject} adds or moves a register without its esp-idf definition"
                )));
            }
            if rule.fifo && rule.write_one_to_clear {
                return Err(Error::message(format!(
                    "overlay register {subject} is a FIFO port and write-one-to-clear"
                )));
            }
            if rule.data_port && (rule.fifo || rule.write_one_to_clear) {
                return Err(Error::message(format!(
                    "overlay register {subject} is a data port and a FIFO port or write-one-to-clear"
                )));
            }
            if !rule.define_fields.is_empty() {
                if rule.add {
                    return Err(Error::message(format!(
                        "overlay register {subject} adds a register and defines its fields"
                    )));
                }
                let mut names = BTreeSet::new();
                let mut taken = 0_u32;
                for field in &rule.define_fields {
                    let fits =
                        field.width > 0 && field.offset < 32 && field.width <= 32 - field.offset;
                    let mask = if fits {
                        (u32::MAX >> (32 - field.width)) << field.offset
                    } else {
                        0
                    };
                    if !fits
                        || mask & taken != 0
                        || !names.insert(field.name.as_str())
                        || field.description.trim().is_empty()
                    {
                        return Err(Error::message(format!(
                            "overlay register {subject} defines field {}: each field named once, \
                             described, inside the word and apart from the others",
                            field.name
                        )));
                    }
                    taken |= mask;
                }
            }
            let mut written = BTreeSet::new();
            let fields = rule
                .driver_writes
                .iter()
                .map(|write| (&write.field, vec![&write.source]))
                .chain(rule.driver_writes_word.iter().map(|write| {
                    let mut lines = vec![&write.value, &write.store];
                    lines.extend(&write.returned);
                    lines.extend(&write.reviewed_chain);
                    (&write.field, lines)
                }));
            for (field, lines) in fields {
                if !written.insert(field.as_str()) {
                    return Err(Error::message(format!(
                        "overlay register {subject}: driver writes of {field} stated twice"
                    )));
                }
                if let Some(line) = lines.iter().find(|line| !driver_citation(line)) {
                    return Err(Error::message(format!(
                        "overlay register {subject}: the driver write of {field} cites {line}, \
                         not an esp-idf:<path>:<line> outside components/soc/"
                    )));
                }
            }
            for write in &rule.driver_writes_word {
                if write.returned.is_some() == write.reviewed_chain.is_empty() {
                    return Err(Error::message(format!(
                        "overlay register {subject}: the word write of {} lists a reviewed-chain \
                         exactly when its value is returned",
                        write.field
                    )));
                }
            }
        }
        let mut seen = BTreeSet::new();
        for rule in &self.fields {
            let subject = format!("{}.{}.{}", rule.peripheral, rule.register, rule.field);
            if !seen.insert(subject.clone()) {
                return Err(Error::message(format!("overlay states {subject} twice")));
            }
            self.validate_sources(&subject, &rule.sources)?;
            let mut values = BTreeSet::new();
            for value in &rule.values {
                if !values.insert(value.value) {
                    return Err(Error::message(format!(
                        "overlay field {subject} repeats value {}",
                        value.value
                    )));
                }
            }
        }
        let mut seen = BTreeSet::new();
        for rule in &self.commands {
            let subject = format!("{}.{}", rule.peripheral, rule.register);
            if !seen.insert(subject.clone()) {
                return Err(Error::message(format!(
                    "overlay states the commands of {subject} twice"
                )));
            }
            self.validate_sources(&subject, &rule.sources)?;
            if !rule
                .sources
                .iter()
                .any(|source| source.starts_with("esp-idf:"))
            {
                return Err(Error::message(format!(
                    "overlay commands of {subject} cite no esp-idf register definition"
                )));
            }
            let mut fields = BTreeSet::new();
            if rule.fields.is_empty()
                || !rule
                    .fields
                    .iter()
                    .chain(&rule.qualifiers)
                    .all(|field| fields.insert(field))
            {
                return Err(Error::message(format!(
                    "overlay commands of {subject}: one or more commands, each field named once"
                )));
            }
            if let Some(register) = self.registers.iter().find(|register| {
                register.peripheral == rule.peripheral && register.register == rule.register
            }) && (register.access.is_some()
                || register.fifo
                || register.write_one_to_clear
                || register.data_port)
            {
                return Err(Error::message(format!(
                    "overlay register {subject} is a command register and has an access, a FIFO, write-one-to-clear or data-port rule"
                )));
            }
        }
        let mut names = BTreeSet::new();
        for rule in &self.interrupts {
            if !names.insert(&rule.name) {
                return Err(Error::message(format!(
                    "overlay states interrupt {} twice",
                    rule.name
                )));
            }
            self.validate_sources(&rule.name, &rule.sources)?;
        }
        Ok(())
    }

    /// Each source is `esp-idf:<path>:<line>` (with the overlay's pin) or
    /// `trm:<document and section>`.
    fn validate_sources(&self, subject: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Err(Error::message(format!(
                "overlay entry {subject} cites no source"
            )));
        }
        for source in sources {
            let cited = if let Some(rest) = source.strip_prefix("esp-idf:") {
                file_line(rest)
            } else if let Some(rest) = source.strip_prefix("trm:") {
                !rest.trim().is_empty()
            } else {
                false
            };
            if !cited {
                return Err(Error::message(format!(
                    "overlay entry {subject} cites {source:?}: a source is esp-idf:<path>:<line> or trm:<section>"
                )));
            }
        }
        Ok(())
    }
}

fn is_revision(value: &str) -> bool {
    value.len() >= 7 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn file_line(rest: &str) -> bool {
    rest.rsplit_once(':').is_some_and(|(path, line)| {
        !path.is_empty() && !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// What the rendered API of one vendor register is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RegisterApi {
    /// Typed field reads and writes (or a FIFO port, or a W1C snapshot).
    Full,
    /// Field reads only.
    Read,
    /// Not rendered.
    None,
}

impl RegisterApi {
    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Read => "read",
            Self::None => "none",
        }
    }
}

/// The verdict on one vendor register.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterVerdict {
    pub peripheral: String,
    pub register: String,
    pub api: RegisterApi,
    pub reasons: Vec<String>,
}

/// The rendered `soc` module and its report.
pub struct SocRender {
    /// The `soc` module, unformatted.
    pub rust: String,
    /// The generated report (TOML).
    pub report: String,
    /// Every vendor register's verdict.
    pub verdicts: Vec<RegisterVerdict>,
    /// The peripherals rendered as typed blocks.
    pub peripherals: Vec<String>,
}

/// The inputs of the `soc` module.
pub struct SocInputs<'a> {
    /// The vendor SVD text.
    pub vendor_svd: &'a str,
    /// The repository path of the vendor SVD, which `SOURCE:` lines name.
    pub vendor_path: &'a str,
    pub overlay: &'a StrictnessOverlay,
    /// The chip's published (curated) SVD: its words are not rendered again.
    pub curated_svd: &'a str,
    /// The curated typed peripherals: the module also asserts that its
    /// blocks list no word of theirs.
    pub curated_typed: &'a BTreeSet<&'a str>,
    /// The pinned ESP-IDF files the overlay names.
    pub headers: &'a dyn crate::HeaderTree,
}

/// Name candidates of a write-trigger field: writing one starts an action.
const TRIGGER_SUFFIXES: &[&str] = &[
    "_CLR", "_CLEAR", "_START", "_STOP", "_UPDATE", "_UPD", "_TRIG", "_TRIGGER", "_LOAD",
];
const TRIGGER_NAMES: &[&str] = &[
    "CLR", "CLEAR", "START", "STOP", "UPDATE", "UPD", "TRIG", "TRIGGER", "LOAD",
];

pub(crate) fn trigger_candidate(name: &str) -> bool {
    TRIGGER_NAMES.contains(&name) || TRIGGER_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Name candidates of a register a read changes: FIFO and data ports.
fn read_candidate(register: &str, fields: &[&str]) -> bool {
    let register = register.replace("%s", "");
    let port = |name: &str| {
        ["RDATA", "RD_BYTE", "RD_DATA", "POP", "PUSH", "FIFO_DATA"]
            .iter()
            .any(|part| name.contains(part))
    };
    matches!(register.as_str(), "FIFO" | "DATA" | "RDATA" | "WDATA")
        || register.ends_with("_FIFO")
        || port(&register)
        || fields.iter().any(|field| port(field))
}

pub(crate) fn raw_status(register: &str) -> bool {
    let register = register.replace("%s", "");
    register.ends_with("_RAW") || register.contains("INT_RAW")
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "gen", "box", "try", "yield", "macro", "abstract", "become", "do", "final",
    "override", "priv", "typeof", "unsized", "virtual",
];

fn module_name(name: &str) -> String {
    crate::pac_api_render::member_binding_name(name)
}

/// A snake-case identifier: lower-case words joined by single underscores.
fn snake_identifier(name: &str) -> bool {
    name.starts_with(|character: char| character.is_ascii_lowercase())
        && !name.ends_with('_')
        && !name.contains("__")
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn valid_module(module: &str) -> bool {
    snake_identifier(module) && !KEYWORDS.contains(&module)
}

/// Resolve `derivedFrom` peripherals: each takes its base's registers.
pub(crate) fn derive(device: &mut Device) -> Result<()> {
    let bases = device
        .peripherals
        .iter()
        .map(|peripheral| (peripheral.name.clone(), peripheral.registers.clone()))
        .collect::<BTreeMap<_, _>>();
    for peripheral in &mut device.peripherals {
        if let Some(base) = peripheral.derived_from.take() {
            if peripheral.registers.is_none() {
                peripheral.registers = bases
                    .get(&base)
                    .ok_or_else(|| {
                        Error::message(format!(
                            "{} derives from {base}, which the SVD lacks",
                            peripheral.name
                        ))
                    })?
                    .clone();
            }
        }
    }
    Ok(())
}

/// The absolute words each register of `device` covers, by peripheral.
fn words(device: &Device) -> BTreeMap<u64, String> {
    let mut words = BTreeMap::new();
    for peripheral in &device.peripherals {
        for child in peripheral.registers.as_deref().unwrap_or_default() {
            let RegisterCluster::Register(register) = child else {
                continue;
            };
            let size = register
                .properties
                .size
                .or(peripheral.default_register_properties.size)
                .or(device.default_register_properties.size)
                .unwrap_or(32);
            let start = peripheral.base_address + u64::from(register.address_offset);
            let span = u64::from(size.div_ceil(32).max(1));
            let elements = match register {
                MaybeArray::Single(_) => vec![start],
                MaybeArray::Array(_, dimension) => (0..u64::from(dimension.dim))
                    .map(|index| start + index * u64::from(dimension.dim_increment))
                    .collect(),
            };
            for element in elements {
                for word in 0..span {
                    words.insert(element / 4 * 4 + word * 4, peripheral.name.clone());
                }
            }
        }
    }
    words
}

fn register_words(base: u64, register: &svd_rs::Register) -> Vec<u64> {
    let start = base + u64::from(register.address_offset);
    match register {
        MaybeArray::Single(_) => vec![start],
        MaybeArray::Array(_, dimension) => (0..u64::from(dimension.dim))
            .map(|index| start + index * u64::from(dimension.dim_increment))
            .collect(),
    }
}

/// Render the `soc` module of a chip.
pub fn render_soc(inputs: &SocInputs<'_>) -> Result<SocRender> {
    let (vendor, facts) = corrected_vendor(inputs.vendor_svd, inputs.overlay, inputs.headers)?;
    let moved = moved_registers(&vendor, &facts);
    if !moved.is_empty() {
        return Err(Error::message(format!(
            "{} vendor registers are elsewhere in their pinned header; the overlay corrects their offsets:\n{}",
            moved.len(),
            moved.join("\n")
        )));
    }
    let check = crate::soc_overlay::check_device(inputs.overlay, &vendor, inputs.headers)?;
    if !check.failures.is_empty() {
        return Err(Error::message(format!(
            "{} overlay entries do not match the pinned headers:\n{}",
            check.failures.len(),
            check.failures.join("\n")
        )));
    }
    let mut curated = svd_parser::parse(inputs.curated_svd)
        .map_err(|error| Error::message(format!("curated SVD: {error}")))?;
    derive(&mut curated)?;
    let curated_words = words(&curated);
    let overlay = inputs.overlay;
    let register_rules = overlay
        .registers
        .iter()
        .map(|rule| ((rule.peripheral.as_str(), rule.register.as_str()), rule))
        .collect::<BTreeMap<_, _>>();
    let field_rules = overlay
        .fields
        .iter()
        .map(|rule| {
            (
                (
                    rule.peripheral.as_str(),
                    rule.register.as_str(),
                    rule.field.as_str(),
                ),
                rule,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let command_rules = overlay
        .commands
        .iter()
        .map(|rule| ((rule.peripheral.as_str(), rule.register.as_str()), rule))
        .collect::<BTreeMap<_, _>>();
    let mut matched_commands = BTreeSet::new();
    let mut matched_registers = BTreeSet::new();
    let mut matched_fields = BTreeSet::new();
    let mut verdicts = Vec::new();
    let mut collected = Collected::default();
    let mut rendered = Vec::new();
    let mut skipped = Vec::new();
    let mut device = vendor.clone();
    device.peripherals.clear();
    let mut modules = BTreeSet::new();
    for peripheral in &vendor.peripherals {
        let name = peripheral.name.clone();
        if matches!(peripheral, MaybeArray::Array(..)) {
            skipped.push((name, "a peripheral array".to_owned()));
            continue;
        }
        let module = module_name(&name);
        if !valid_module(&module) || !modules.insert(module) {
            skipped.push((
                name,
                "its module name is not a unique identifier".to_owned(),
            ));
            continue;
        }
        let mut kept = peripheral.clone();
        kept.description = kept.description.as_deref().map(doc_line);
        let registers = peripheral.registers.as_deref().unwrap_or_default();
        // Registers that share a word, and register modules that collide.
        let mut owners = BTreeMap::<u64, Vec<String>>::new();
        let mut register_modules = BTreeMap::<String, Vec<String>>::new();
        for child in registers {
            if let RegisterCluster::Register(register) = child {
                for word in register_words(peripheral.base_address, register) {
                    owners
                        .entry(word / 4 * 4)
                        .or_default()
                        .push(register.name.clone());
                }
                register_modules
                    .entry(module_name(&register.name))
                    .or_default()
                    .push(register.name.clone());
            }
        }
        let mut output = Vec::new();
        for child in registers {
            let RegisterCluster::Register(register) = child else {
                verdicts.push(RegisterVerdict {
                    peripheral: name.clone(),
                    register: "(cluster)".to_owned(),
                    api: RegisterApi::None,
                    reasons: vec!["a register cluster".to_owned()],
                });
                continue;
            };
            let key = (peripheral.name.as_str(), register.name.as_str());
            let rule = register_rules.get(&key).copied();
            if rule.is_some() {
                matched_registers.insert(key);
            }
            let fields = register
                .fields
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>();
            for field in &fields {
                let field_key = (peripheral.name.as_str(), register.name.as_str(), *field);
                if field_rules.contains_key(&field_key) {
                    matched_fields.insert(field_key);
                }
            }
            let shared = register_words(peripheral.base_address, register)
                .into_iter()
                .filter_map(|word| owners.get(&(word / 4 * 4)))
                .flatten()
                .filter(|other| **other != register.name)
                .cloned()
                .collect::<BTreeSet<_>>();
            let curated_owner = register_words(peripheral.base_address, register)
                .into_iter()
                .find_map(|word| curated_words.get(&(word / 4 * 4)));
            let collides = register_modules
                .get(&module_name(&register.name))
                .is_some_and(|names| names.len() > 1);
            let (api, reasons, typed) = if let Some(rule) = inputs
                .overlay
                .no_api
                .iter()
                .find(|rule| rule.peripheral == name)
            {
                (
                    RegisterApi::None,
                    vec![format!("no API for the peripheral: {}", rule.reason)],
                    None,
                )
            } else if let Some(owner) = curated_owner {
                (
                    RegisterApi::None,
                    vec![format!("the curated model's {owner} describes its word")],
                    None,
                )
            } else if !shared.is_empty() {
                (
                    RegisterApi::None,
                    vec![format!(
                        "shares a word with {}",
                        shared.into_iter().collect::<Vec<_>>().join(", ")
                    )],
                    None,
                )
            } else if collides || !valid_module(&module_name(&register.name)) {
                (
                    RegisterApi::None,
                    vec!["its module name is not a unique identifier".to_owned()],
                    None,
                )
            } else {
                let header = header_view(
                    &facts,
                    &name,
                    register,
                    rule.is_some_and(|rule| !rule.define_fields.is_empty()),
                    &rule.map(RegisterRule::driver_written).unwrap_or_default(),
                )?;
                let commands = command_rules.get(&key).copied();
                if commands.is_some() {
                    matched_commands.insert(key);
                }
                classify(
                    &vendor,
                    peripheral,
                    register,
                    Rules {
                        register: rule,
                        fields: &field_rules,
                        commands,
                    },
                    &header,
                    &mut collected,
                )?
            };
            verdicts.push(RegisterVerdict {
                peripheral: name.clone(),
                register: register.name.clone(),
                api,
                reasons,
            });
            if let Some(typed) = typed {
                output.push(RegisterCluster::Register(typed));
            }
        }
        if output.is_empty() {
            skipped.push((name, "no register keeps an API".to_owned()));
            continue;
        }
        kept.registers = Some(output);
        kept.derived_from = None;
        rendered.push(name);
        device.peripherals.push(kept);
    }
    if let Some(rule) = inputs.overlay.no_api.iter().find(|rule| {
        !vendor
            .peripherals
            .iter()
            .any(|peripheral| peripheral.name == rule.peripheral)
    }) {
        return Err(Error::message(format!(
            "overlay no-api peripheral {} is not in the vendor SVD",
            rule.peripheral
        )));
    }
    if let Some((peripheral, register)) = register_rules
        .keys()
        .find(|key| !matched_registers.contains(*key))
    {
        return Err(Error::message(format!(
            "overlay register {peripheral}.{register} is not in the vendor SVD"
        )));
    }
    if let Some((peripheral, register)) = command_rules
        .keys()
        .find(|key| !matched_commands.contains(*key))
    {
        return Err(Error::message(format!(
            "overlay commands of {peripheral}.{register} name no register the pass classifies"
        )));
    }
    if let Some((peripheral, register, field)) = field_rules
        .keys()
        .find(|key| !matched_fields.contains(*key))
    {
        return Err(Error::message(format!(
            "overlay field {peripheral}.{register}.{field} is not in the vendor SVD"
        )));
    }
    // Rules on registers the pass did not render give nothing; a trigger or
    // FIFO declaration must name a rendered register.
    let rendered_registers = device
        .peripherals
        .iter()
        .flat_map(|peripheral| {
            peripheral
                .registers
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|child| (peripheral.name.clone(), child.name().clone()))
        })
        .collect::<BTreeSet<_>>();
    let Collected {
        mut triggers,
        mut fifos,
        mut commands,
        mut ports,
        renames,
    } = collected;
    triggers.retain(|trigger: &WriteTriggerField| {
        rendered_registers.contains(&(trigger.peripheral.clone(), trigger.register.clone()))
    });
    fifos.retain(|port: &FifoPort| {
        rendered_registers.contains(&(port.peripheral.clone(), port.register.clone()))
    });
    commands.retain(|command: &CommandRegister| {
        rendered_registers.contains(&(command.peripheral.clone(), command.register.clone()))
    });
    ports.retain(|port: &DataPort| {
        rendered_registers.contains(&(port.peripheral.clone(), port.register.clone()))
    });

    let source = format!("the vendor SVD ({})", inputs.vendor_path);
    let mut pack = PacApiPack::empty();
    pack.options.typed_peripherals = rendered.clone();
    pack.write_trigger_fields = triggers;
    pack.fifo_ports = fifos;
    pack.command_registers = commands;
    pack.data_ports = ports;
    for peripheral in &device.peripherals {
        if let [interrupt] = peripheral.interrupt.as_slice() {
            pack.interrupt_sources.push(InterruptSource {
                peripheral: peripheral.name.clone(),
                source: u16::try_from(interrupt.value).map_err(|_| {
                    Error::message(format!("interrupt {} is above u16", interrupt.name))
                })?,
                sources: vec![source.clone()],
            });
        }
    }
    pack.ownership_partitions.push(OwnershipPartition {
        name: "SocPeripherals".to_owned(),
        member: "soc".to_owned(),
        description: "Every block of the vendor SVD the curated model does not describe."
            .to_owned(),
        peripherals: rendered.clone(),
        handoff: false,
    });
    let typed = pack.typed_peripherals();
    pack.validate_typed_operations(&typed)?;
    let mut rust = String::new();
    let _ = writeln!(
        rust,
        "/// Every peripheral of the vendor SVD the curated model does not describe,\n\
         /// as typed register blocks; see the chip's `registers/published/soc.report.toml`\n\
         /// for what each register's API is and why.\n\
         ///\n\
         /// SOURCE: {source}.\n\
         pub mod soc {{\n"
    );
    rust.push_str(&render_interrupts(&vendor, overlay, &source)?);
    rust.push_str(&pack.render_typed_device(&device, &typed, &BTreeMap::new())?);
    if !inputs.curated_typed.is_empty() && !typed.is_empty() {
        let list = typed
            .iter()
            .map(|name| module_name(name))
            .chain(
                inputs
                    .curated_typed
                    .iter()
                    .map(|name| format!("super::{}", module_name(name))),
            )
            .map(|module| {
                format!("({module}::BASE, <{module}::BlockWords as oer_mmio::Registers>::WORDS)")
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let _ = writeln!(
            rust,
            "const _: () = assert!(\n\
             oer_mmio::check::disjoint_blocks(&[\n{list}\n]),\n\
             \"no vendor block lists a word of another typed block\"\n\
             );\n"
        );
    }
    rust.push_str("}\n");
    let report = render_report(inputs.vendor_path, &verdicts, &skipped, &rendered, &renames);
    Ok(SocRender {
        rust,
        report,
        verdicts,
        peripherals: rendered,
    })
}

/// What the pass collects over every register.
#[derive(Default)]
struct Collected {
    triggers: Vec<WriteTriggerField>,
    fifos: Vec<FifoPort>,
    commands: Vec<CommandRegister>,
    ports: Vec<DataPort>,
    renames: Vec<Rename>,
}

/// The overlay's rules on one vendor register.
#[derive(Clone, Copy)]
struct Rules<'r> {
    register: Option<&'r RegisterRule>,
    fields: &'r BTreeMap<(&'r str, &'r str, &'r str), &'r FieldRule>,
    commands: Option<&'r CommandRule>,
}

/// What a field of a command register is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandRole {
    /// Started by writing one, cleared by the hardware.
    Command,
    /// Written with a start, by the start's argument.
    Qualifier,
    /// A self-clearing bit no start sets: read, zero in every start.
    Held,
}

/// A vendor field the API names after its source.
struct Rename {
    peripheral: String,
    register: String,
    svd: String,
    source: String,
}

/// What the pinned header says about one vendor register.
enum HeaderView {
    /// The overlay names no header for its peripheral.
    Unmapped,
    /// The header does not define it; `none` when the layout reserves its
    /// word.
    Absent {
        reason: String,
        none: bool,
    },
    Derived(Derived),
}

fn svd_fields(register: &svd_rs::Register) -> Vec<SvdField<'_>> {
    register
        .fields
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|field| match field {
            MaybeArray::Single(info) => Some(SvdField {
                name: info.name.as_str(),
                low: info.bit_range.offset,
                width: info.bit_range.width,
                description: info.description.as_deref().unwrap_or(""),
            }),
            MaybeArray::Array(..) => None,
        })
        .collect()
}

/// Whether two derivations give the same API.
fn same_semantics(first: &Derived, second: &Derived) -> bool {
    first.none.is_some() == second.none.is_some()
        && first.narrow.is_empty() == second.narrow.is_empty()
        && first.one_to_clear == second.one_to_clear
        && first.reserved == second.reserved
        && first.reserved_fields == second.reserved_fields
        && first
            .fields
            .iter()
            .map(|(name, field)| (name, field.class))
            .eq(second
                .fields
                .iter()
                .map(|(name, field)| (name, field.class)))
}

/// The header's view of `register` of `peripheral`: every element of an
/// array must be in the header with the same semantics. A register the
/// header places at another offset is an error until the overlay corrects
/// its offset.
fn header_view(
    facts: &HeaderFacts,
    peripheral: &str,
    register: &svd_rs::Register,
    defined: bool,
    written: &BTreeSet<&str>,
) -> Result<HeaderView> {
    let fields = svd_fields(register);
    let mut derived: Option<Derived> = None;
    for (name, offset) in elements(register) {
        match facts.lookup(
            peripheral,
            &name,
            offset,
            matches!(register, MaybeArray::Array(..)),
        ) {
            Lookup::Unmapped => return Ok(HeaderView::Unmapped),
            Lookup::Moved {
                path,
                register: header,
            } => {
                return Err(Error::message(format!(
                    "{peripheral}.{name}: the vendor SVD places it at {offset:#x}, \
                     esp-idf:{path}:{} at {:#x}; the overlay corrects its offset",
                    header.line, header.offset
                )));
            }
            Lookup::Absent { path, reserved } => {
                return Ok(HeaderView::Absent {
                    reason: match reserved {
                        Some((layout, line)) => {
                            format!("the pinned layout reserves its word (esp-idf:{layout}:{line})")
                        }
                        None => format!("esp-idf:{path} defines no register {name}"),
                    },
                    none: reserved.is_some(),
                });
            }
            Lookup::Matched {
                path,
                prefix,
                register: header,
            } => {
                let next = derive_register(
                    path,
                    prefix,
                    header,
                    &fields,
                    matches!(register, MaybeArray::Array(..)),
                    defined,
                    written,
                );
                match &derived {
                    None => derived = Some(next),
                    Some(first) if same_semantics(first, &next) => {}
                    Some(_) => {
                        return Ok(HeaderView::Absent {
                            reason: format!(
                                "the header's elements of {} differ (esp-idf:{path}:{})",
                                register.name, header.line
                            ),
                            none: false,
                        });
                    }
                }
            }
        }
    }
    Ok(derived.map_or(HeaderView::Unmapped, HeaderView::Derived))
}

/// The vendor SVD of `text` with the overlay's corrections, `derivedFrom`
/// resolved after them so a derived block takes its base's, and the
/// headers of its peripherals.
pub(crate) fn corrected_vendor(
    text: &str,
    overlay: &StrictnessOverlay,
    tree: &dyn crate::HeaderTree,
) -> Result<(Device, HeaderFacts)> {
    let mut vendor =
        svd_parser::parse(text).map_err(|error| Error::message(format!("vendor SVD: {error}")))?;
    let mut derived = vendor.clone();
    derive(&mut derived)?;
    let facts = HeaderFacts::load(overlay, tree, &derived)?;
    correct(&mut vendor, overlay, &facts)?;
    derive(&mut vendor)?;
    Ok((vendor, facts))
}

/// Every register of `device` its peripheral's header names at another
/// offset: a wrong address is wrong whether or not the module renders it.
fn moved_registers(device: &Device, facts: &HeaderFacts) -> Vec<String> {
    let mut moved = Vec::new();
    for peripheral in &device.peripherals {
        for child in peripheral.registers.as_deref().unwrap_or_default() {
            let RegisterCluster::Register(register) = child else {
                continue;
            };
            for (name, offset) in elements(register) {
                if let Lookup::Moved {
                    path,
                    register: header,
                } = facts.lookup(
                    &peripheral.name,
                    &name,
                    offset,
                    matches!(register, MaybeArray::Array(..)),
                ) {
                    moved.push(format!(
                        "{}.{name}: the vendor SVD places it at {offset:#x}, esp-idf:{path}:{} at {:#x}",
                        peripheral.name, header.line, header.offset
                    ));
                }
            }
        }
    }
    moved
}

/// Apply the overlay's register corrections to the vendor SVD: offsets the
/// header places elsewhere, and registers the SVD lacks, built from their
/// header definition.
fn correct(device: &mut Device, overlay: &StrictnessOverlay, facts: &HeaderFacts) -> Result<()> {
    for rule in &overlay.registers {
        if !rule.add && rule.offset.is_none() && rule.define_fields.is_empty() {
            continue;
        }
        let subject = format!("{}.{}", rule.peripheral, rule.register);
        let Some(peripheral) = device
            .peripherals
            .iter_mut()
            .find(|peripheral| peripheral.name == rule.peripheral)
        else {
            return Err(Error::message(format!(
                "overlay register {subject}: no such vendor peripheral"
            )));
        };
        let registers = peripheral.registers.get_or_insert_with(Vec::new);
        let existing = registers.iter_mut().find_map(|child| match child {
            RegisterCluster::Register(register) if register.name == rule.register => Some(register),
            _ => None,
        });
        match (rule.add, existing) {
            (false, Some(register)) => {
                if let Some(offset) = rule.offset {
                    register.address_offset = offset;
                }
                if !rule.define_fields.is_empty() {
                    register.fields = Some(defined_fields(&subject, &rule.define_fields)?);
                }
            }
            (false, None) => {
                return Err(Error::message(format!(
                    "overlay register {subject} is not in the vendor SVD"
                )));
            }
            (true, Some(_)) => {
                return Err(Error::message(format!(
                    "overlay adds {subject}, which the vendor SVD has"
                )));
            }
            (true, None) => {
                let Some((path, prefix, Some(header))) =
                    facts.by_name(&rule.peripheral, &rule.register)
                else {
                    return Err(Error::message(format!(
                        "overlay adds {subject}, which its peripheral's header does not define"
                    )));
                };
                let offset = u32::try_from(header.offset).map_err(|_| {
                    Error::message(format!("{subject}: header offset above 32 bits"))
                })?;
                if rule.offset.is_some_and(|stated| stated != offset) {
                    return Err(Error::message(format!(
                        "overlay adds {subject} at {:#x}, esp-idf:{path}:{} at {offset:#x}",
                        rule.offset.unwrap_or_default(),
                        header.line
                    )));
                }
                let fields = header
                    .fields
                    .iter()
                    .filter(|field| !crate::soc_headers::reserved_description(&field.description))
                    .map(|field| {
                        let access = match crate::soc_headers::class(&field.access) {
                            Class::ReadOnly => Access::ReadOnly,
                            Class::WriteOnly => Access::WriteOnly,
                            _ => Access::ReadWrite,
                        };
                        svd_rs::FieldInfo::builder()
                            .name(
                                field
                                    .name
                                    .strip_prefix(prefix)
                                    .unwrap_or(&field.name)
                                    .to_owned(),
                            )
                            .description(Some(field.description.clone()))
                            .bit_range(svd_rs::BitRange::from_offset_width(
                                field.low,
                                field.width(),
                            ))
                            .access(Some(access))
                            .build(ValidateLevel::Disabled)
                            .map(MaybeArray::Single)
                            .map_err(|error| Error::message(error.to_string()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let register = svd_rs::RegisterInfo::builder()
                    .name(rule.register.clone())
                    .description(Some(format!(
                        "The register the vendor SVD lacks, from esp-idf:{path}:{}.",
                        header.line
                    )))
                    .address_offset(offset)
                    .size(Some(32))
                    .fields(Some(fields))
                    .build(ValidateLevel::Disabled)
                    .map_err(|error| Error::message(error.to_string()))?;
                let at = registers
                    .iter()
                    .position(|child| match child {
                        RegisterCluster::Register(register) => register.address_offset > offset,
                        RegisterCluster::Cluster(cluster) => cluster.address_offset > offset,
                    })
                    .unwrap_or(registers.len());
                registers.insert(at, RegisterCluster::Register(MaybeArray::Single(register)));
            }
        }
    }
    Ok(())
}

/// The SVD fields of an overlay's `define-fields`; each field's access is
/// its header class's, given when the register is classified.
fn defined_fields(subject: &str, fields: &[DefinedField]) -> Result<Vec<svd_rs::Field>> {
    fields
        .iter()
        .map(|field| {
            svd_rs::FieldInfo::builder()
                .name(field.name.clone())
                .description(Some(field.description.clone()))
                .bit_range(svd_rs::BitRange::from_offset_width(
                    field.offset,
                    field.width,
                ))
                .build(ValidateLevel::Disabled)
                .map(MaybeArray::Single)
                .map_err(|error| Error::message(format!("{subject}: {error}")))
        })
        .collect()
}

/// The conservative verdict on one register, with the register the typed
/// renderer takes when it keeps any API.
#[allow(
    clippy::too_many_lines,
    reason = "one ordered list of the default rules"
)]
fn classify(
    device: &Device,
    peripheral: &Peripheral,
    register: &svd_rs::Register,
    rules: Rules<'_>,
    header: &HeaderView,
    collected: &mut Collected,
) -> Result<(RegisterApi, Vec<String>, Option<svd_rs::Register>)> {
    let Rules {
        register: rule,
        fields: field_rules,
        commands: command_rule,
    } = rules;
    let Collected {
        triggers,
        fifos,
        commands,
        ports,
        renames,
    } = collected;
    let name = peripheral.name.as_str();
    let data_port = rule.is_some_and(|rule| rule.data_port);
    let mut typed = register.clone();
    typed.description = typed.description.as_deref().map(doc_line);
    let properties = crate::pac_api_svd::merged_properties(device, peripheral, register);
    let size = properties.size.unwrap_or(32);
    if size != 32 {
        return Ok((RegisterApi::None, vec![format!("{size} bits wide")], None));
    }
    let mut access = rule.and_then(|rule| rule.access).map_or(
        properties.access.unwrap_or(Access::ReadWrite),
        OverlayAccess::svd,
    );
    let fields = typed.fields.take().unwrap_or_default();
    if fields.is_empty() {
        return Ok((RegisterApi::None, vec!["no fields".to_owned()], None));
    }
    if fields
        .iter()
        .any(|field| matches!(field, MaybeArray::Array(..)))
    {
        return Ok((RegisterApi::None, vec!["a field array".to_owned()], None));
    }
    let field_names = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    let unique = field_names.iter().collect::<BTreeSet<_>>();
    if unique.len() != field_names.len() {
        return Ok((
            RegisterApi::None,
            vec!["two fields share a name".to_owned()],
            None,
        ));
    }
    if typed.read_action.is_some() || fields.iter().any(|field| field.read_action.is_some()) {
        return Ok((
            RegisterApi::None,
            vec!["the vendor SVD declares a read action".to_owned()],
            None,
        ));
    }
    let fifo = rule.is_some_and(|rule| rule.fifo);
    let read_effect = match rule.and_then(|rule| rule.read_side_effect) {
        Some(effect) => effect,
        None => read_candidate(&register.name, &field_names),
    };
    if read_effect && !fifo {
        return Ok((
            RegisterApi::None,
            vec![
                if rule.and_then(|rule| rule.read_side_effect) == Some(true) {
                    "a read changes it (overlay)".to_owned()
                } else {
                    "FIFO or data port candidate: a read may change it".to_owned()
                },
            ],
            None,
        ));
    }
    let mut reasons = Vec::new();
    let mut narrow = false;
    let derived = match header {
        HeaderView::Derived(derived) => {
            if let Some(reason) = &derived.none {
                return Ok((RegisterApi::None, vec![reason.clone()], None));
            }
            if let Some(commands) = command_rule
                && let Some(field) = commands
                    .fields
                    .iter()
                    .chain(&commands.qualifiers)
                    .find(|field| !derived.self_clearing.contains_key(*field))
            {
                return Err(Error::message(format!(
                    "overlay names {name}.{}.{field} a command or qualifier, but its header does not make it one self-clearing bit (R/W/SC, R/W/WTC)",
                    register.name
                )));
            }
            let kept = derived
                .narrow
                .iter()
                .filter(|reason| {
                    !(data_port && derived.hardware.contains(*reason))
                        && !(command_rule.is_some()
                            && derived
                                .self_clearing
                                .values()
                                .any(|lifted| lifted == *reason))
                })
                .cloned()
                .collect::<Vec<_>>();
            if !kept.is_empty() {
                narrow = true;
                reasons.extend(kept);
            }
            Some(derived)
        }
        HeaderView::Absent { reason, none: true } => {
            return Ok((RegisterApi::None, vec![reason.clone()], None));
        }
        HeaderView::Absent {
            reason,
            none: false,
        } => {
            narrow = true;
            reasons.push(reason.clone());
            None
        }
        HeaderView::Unmapped => {
            narrow = true;
            reasons.push("the overlay names no pinned header of its peripheral".to_owned());
            None
        }
    };
    if derived.is_none()
        && let Some(field) = svd_fields(register)
            .into_iter()
            .find(|field| read_clears(field.description))
    {
        return Ok((
            RegisterApi::None,
            vec![format!(
                "field {} is cleared by a read (the vendor SVD's description)",
                field.name
            )],
            None,
        ));
    }
    if command_rule.is_some() && derived.is_none() {
        return Err(Error::message(format!(
            "overlay names commands of {name}.{}, which no pinned header defines",
            register.name
        )));
    }
    let explicit_w1c = rule.is_some_and(|rule| rule.write_one_to_clear);
    let w1c = explicit_w1c || derived.is_some_and(|derived| derived.one_to_clear);
    let mut header_reserved = 0_u32;
    let mut kept = Vec::new();
    // Overlapping fields: neither keeps an accessor.
    let mut overlapping = BTreeSet::new();
    for (index, first) in fields.iter().enumerate() {
        for second in &fields[index + 1..] {
            let mask = |field: &svd_rs::Field| {
                let width = field.bit_range.width;
                if width == 0 || width > 32 {
                    0
                } else {
                    (u32::MAX >> (32 - width)) << field.bit_range.offset
                }
            };
            if mask(first) & mask(second) != 0 {
                overlapping.insert(first.name.clone());
                overlapping.insert(second.name.clone());
            }
        }
    }
    if !overlapping.is_empty() {
        narrow = true;
        reasons.push(format!(
            "overlapping fields {}",
            overlapping.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    let mut declared = 0_u32;
    let mut roles = Vec::new();
    let mut register_triggers = 0_usize;
    let mut zero_lanes = 0_usize;
    for field in fields {
        let MaybeArray::Single(mut info) = field else {
            continue;
        };
        info.description = info.description.as_deref().map(doc_line);
        let width = info.bit_range.width;
        let offset = info.bit_range.offset;
        if width == 0 || width > 32 || offset + width > 32 {
            narrow = true;
            reasons.push(format!("field {} does not fit the word", info.name));
            continue;
        }
        if overlapping.contains(&info.name) {
            continue;
        }
        if derived.is_some_and(|derived| derived.reserved_fields.contains(&info.name)) {
            // The header calls the lane reserved: no field, kept by writes.
            header_reserved |= (u32::MAX >> (32 - width)) << offset;
            continue;
        }
        let svd_name = info.name.clone();
        let field_rule = field_rules
            .get(&(name, register.name.as_str(), svd_name.as_str()))
            .copied();
        let derived_field = derived.and_then(|derived| derived.fields.get(&svd_name));
        // The field's name is its source's: the overlay's, else the header's.
        if let Some(rename) = field_rule
            .and_then(|rule| rule.rename.as_ref())
            .or_else(|| derived.and_then(|derived| derived.renamed.get(&svd_name)))
        {
            renames.push(Rename {
                peripheral: name.to_owned(),
                register: register.name.clone(),
                svd: svd_name.clone(),
                source: rename.clone(),
            });
            info.name.clone_from(rename);
        }
        if !valid_module(&module_name(&info.name)) {
            narrow = true;
            reasons.push(format!(
                "field {} has no snake-case method name that is not a keyword",
                info.name
            ));
            continue;
        }
        // The overlay's field access, else its register access, else the
        // header's class, else the SVD's field access, else the register's.
        let role = command_rule.and_then(|commands| {
            if commands.fields.contains(&svd_name) {
                Some(CommandRole::Command)
            } else if commands.qualifiers.contains(&svd_name) {
                Some(CommandRole::Qualifier)
            } else {
                derived
                    .is_some_and(|derived| derived.self_clearing.contains_key(&svd_name))
                    .then_some(CommandRole::Held)
            }
        });
        // The vendor's driver writes it, as the overlay check verified: its
        // header access stands.
        let debug = derived_field.is_some_and(|field| field.debug)
            && !rule.is_some_and(|rule| rule.driver_written().contains(svd_name.as_str()));
        if debug && matches!(role, Some(CommandRole::Command | CommandRole::Qualifier)) {
            return Err(Error::message(format!(
                "overlay names {name}.{}.{svd_name} a command or qualifier, but its header keeps it to internal debugging",
                register.name
            )));
        }
        let field_access = match (
            field_rule.and_then(|rule| rule.access),
            rule.and_then(|rule| rule.access),
        ) {
            (Some(field), _) => field.svd(),
            (None, Some(_)) => access,
            (None, None) => match derived_field.map(|field| field.class) {
                Some(Class::Plain | Class::Trigger | Class::OneToClear | Class::SelfClearing) => {
                    Access::ReadWrite
                }
                Some(Class::ReadOnly) => Access::ReadOnly,
                Some(Class::WriteOnly) => Access::WriteOnly,
                _ => info.access.unwrap_or(access),
            },
        };
        // A debugging field is never written: read only, or a lane every
        // write leaves zero when a read returns nothing it holds.
        let class = derived_field.map(|field| field.class);
        let zero_lane = debug
            && field_access.can_write()
            && (matches!(class, Some(Class::Trigger)) || !field_access.can_read());
        // A command register is written only by its starts: every other
        // field is only read there.
        let field_access = if role.is_some() {
            Access::ReadWrite
        } else if zero_lane {
            field_access
        } else if field_access.can_write() && (debug || command_rule.is_some()) {
            if !field_access.can_read() {
                narrow = true;
                reasons.push(format!(
                    "write-only field {} of a command register",
                    info.name
                ));
                continue;
            }
            Access::ReadOnly
        } else {
            field_access
        };
        info.access = Some(field_access);
        info.enumerated_values.clear();
        if let Some(field_rule) = field_rule
            && !field_rule.values.is_empty()
        {
            let values = field_rule
                .values
                .iter()
                .map(|value| {
                    EnumeratedValue::builder()
                        .name(value.name.clone())
                        .description(value.description.clone())
                        .value(Some(u64::from(value.value)))
                        .build(ValidateLevel::Disabled)
                        .map_err(|error| Error::message(error.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            info.enumerated_values = vec![
                EnumeratedValues::builder()
                    .values(values)
                    .build(ValidateLevel::Disabled)
                    .map_err(|error| Error::message(error.to_string()))?,
            ];
        }
        if info.write_constraint.take().is_some() {
            narrow = true;
            reasons.push(format!("field {} has a write constraint", info.name));
        }
        if info.modified_write_values.take().is_some() && !w1c {
            narrow = true;
            reasons.push(format!(
                "field {} has vendor modified-write values the overlay does not confirm",
                info.name
            ));
        }
        if w1c {
            info.modified_write_values = Some(ModifiedWriteValues::OneToClear);
        }
        if zero_lane {
            info.write_constraint = Some(WriteConstraint::Range(WriteConstraintRange {
                min: 0,
                max: 0,
            }));
            declared |= (u32::MAX >> (32 - width)) << offset;
            zero_lanes += 1;
            kept.push(MaybeArray::Single(info));
            continue;
        }
        if let Some(role) = role {
            roles.push((role, info.name.clone()));
            declared |= (u32::MAX >> (32 - width)) << offset;
            kept.push(MaybeArray::Single(info));
            continue;
        }
        let kind = field_rule.and_then(|rule| rule.kind).or_else(|| {
            derived_field.and_then(|field| match field.class {
                Class::Trigger if field_access.can_write() => Some(FieldKind::WriteTrigger),
                Class::Plain => Some(FieldKind::Plain),
                _ => None,
            })
        });
        let cited = || {
            field_rule.map_or_else(
                || {
                    derived_field
                        .map(|field| vec![field.source.clone()])
                        .unwrap_or_default()
                },
                |rule| rule.sources.clone(),
            )
        };
        if kind == Some(FieldKind::WriteTrigger) {
            if command_rule.is_some() {
                return Err(Error::message(format!(
                    "overlay makes {name}.{} a command register, but its field {} is a write trigger",
                    register.name, info.name
                )));
            }
            register_triggers += 1;
            triggers.push(WriteTriggerField {
                peripheral: name.to_owned(),
                register: register.name.clone(),
                field: info.name.clone(),
                reads_pending: false,
                sources: cited(),
            });
        } else if field_access == Access::WriteOnly && kind != Some(FieldKind::Plain) {
            narrow = true;
            reasons.push(format!(
                "write-only field {}: a write-trigger candidate",
                info.name
            ));
            continue;
        } else if field_access.can_write()
            && kind != Some(FieldKind::Plain)
            && trigger_candidate(&info.name)
        {
            narrow = true;
            reasons.push(format!("field {}: a write-trigger candidate", info.name));
        }
        declared |= (u32::MAX >> (32 - width)) << offset;
        kept.push(MaybeArray::Single(info));
    }
    let explicit = rule.and_then(|rule| rule.reserved);
    if explicit.is_some_and(|reserved| reserved & declared != 0) {
        return Err(Error::message(format!(
            "overlay reserves bits of declared fields of {name}.{}",
            register.name
        )));
    }
    // The fields an overlay defines are the source's whole word: its other
    // bits are reserved.
    let defined = rule.is_some_and(|rule| !rule.define_fields.is_empty());
    let reserved = explicit.unwrap_or_else(|| {
        if defined {
            !declared
        } else {
            derived.map_or(0, |derived| derived.reserved & !declared)
        }
    }) | header_reserved;
    let undeclared = !(declared | reserved);
    if undeclared != 0 && access.can_write() {
        narrow = true;
        reasons.push(format!("undeclared bits 0x{undeclared:08x}"));
    }
    if raw_status(&register.name)
        && !rule.is_some_and(|rule| rule.raw_status_writable)
        && !derived.is_some_and(|derived| derived.one_to_clear)
    {
        narrow = true;
        reasons.push("a raw interrupt status: what a write does is not stated".to_owned());
    }
    if explicit_w1c && narrow {
        return Err(Error::message(format!(
            "overlay makes {name}.{} write-one-to-clear, but {}",
            register.name,
            reasons.join("; ")
        )));
    }
    if command_rule.is_some() && narrow {
        return Err(Error::message(format!(
            "overlay makes {name}.{} a command register, but it stays read-only: {}",
            register.name,
            reasons.join("; ")
        )));
    }
    // A word of write triggers alone is written from zero, never read.
    if !narrow
        && register_triggers > 0
        && register_triggers + zero_lanes == kept.len()
        && rule.is_none_or(|rule| rule.access.is_none() && !rule.fifo && !rule.data_port)
    {
        access = Access::WriteOnly;
        for field in &mut kept {
            if let MaybeArray::Single(info) = field {
                info.access = Some(Access::WriteOnly);
            }
        }
    }
    if narrow {
        access = Access::ReadOnly;
        triggers
            .retain(|trigger| !(trigger.peripheral == name && trigger.register == register.name));
        kept.retain(|field| field.access.is_none_or(|access| access.can_read()));
        for field in &mut kept {
            field.access = Some(Access::ReadOnly);
            field.modified_write_values = None;
        }
    }
    if kept.is_empty() {
        reasons.push("no readable field remains".to_owned());
        return Ok((RegisterApi::None, reasons, None));
    }
    let names = kept
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    if names.len() != kept.len() {
        reasons.push("two fields share a name after their header's".to_owned());
        return Ok((RegisterApi::None, reasons, None));
    }
    if fifo {
        fifos.push(FifoPort {
            peripheral: name.to_owned(),
            register: register.name.clone(),
            sources: rule.map(|rule| rule.sources.clone()).unwrap_or_default(),
        });
    }
    if let Some(rule) = command_rule {
        let named = |wanted: CommandRole| {
            roles
                .iter()
                .filter(|(role, _)| *role == wanted)
                .map(|(_, field)| field.clone())
                .collect::<Vec<_>>()
        };
        commands.push(CommandRegister {
            peripheral: name.to_owned(),
            register: register.name.clone(),
            commands: named(CommandRole::Command),
            qualifiers: named(CommandRole::Qualifier),
            held: named(CommandRole::Held),
            sources: rule.sources.clone(),
        });
    }
    if data_port {
        if narrow || kept.len() != 1 {
            return Err(Error::message(format!(
                "overlay makes {name}.{} a data port, but it is not one writable field{}",
                register.name,
                if reasons.is_empty() {
                    String::new()
                } else {
                    format!(": {}", reasons.join("; "))
                }
            )));
        }
        ports.push(DataPort {
            peripheral: name.to_owned(),
            register: register.name.clone(),
            sources: rule.map(|rule| rule.sources.clone()).unwrap_or_default(),
        });
    }
    let writes = access.can_write()
        && kept
            .iter()
            .any(|field| field.access.is_none_or(|access| access.can_write()));
    typed.properties.access = Some(access);
    typed.properties.size = Some(32);
    typed.modified_write_values = None;
    typed.write_constraint = None;
    typed.fields = Some(kept);
    let api = if writes {
        RegisterApi::Full
    } else {
        RegisterApi::Read
    };
    Ok((api, reasons, Some(typed)))
}

/// The chip's interrupt sources: the vendor SVD's and the overlay's.
fn render_interrupts(device: &Device, overlay: &StrictnessOverlay, source: &str) -> Result<String> {
    let mut by_name = BTreeMap::<String, (u16, Option<String>, String)>::new();
    for peripheral in &device.peripherals {
        for interrupt in &peripheral.interrupt {
            let value = u16::try_from(interrupt.value).map_err(|_| {
                Error::message(format!("interrupt {} is above u16", interrupt.name))
            })?;
            match by_name.get(&interrupt.name) {
                Some((known, ..)) if *known != value => {
                    return Err(Error::message(format!(
                        "the vendor SVD gives interrupt {} two sources",
                        interrupt.name
                    )));
                }
                Some(_) => {}
                None => {
                    by_name.insert(
                        interrupt.name.clone(),
                        (value, interrupt.description.clone(), source.to_owned()),
                    );
                }
            }
        }
    }
    for rule in &overlay.interrupts {
        if by_name.contains_key(&rule.name) {
            return Err(Error::message(format!(
                "overlay interrupt {} is in the vendor SVD",
                rule.name
            )));
        }
        by_name.insert(
            rule.name.clone(),
            (
                rule.source,
                rule.description.clone(),
                rule.sources.join(", "),
            ),
        );
    }
    let mut by_source = BTreeMap::new();
    for (name, (value, description, cited)) in &by_name {
        if let Some(other) = by_source.insert(*value, (name, description, cited)) {
            return Err(Error::message(format!(
                "interrupts {} and {name} name the same source {value}",
                other.0
            )));
        }
        if !valid_variant(name) {
            return Err(Error::message(format!(
                "interrupt {name} is not an upper-case identifier"
            )));
        }
    }
    if by_source.is_empty() {
        return Ok(String::new());
    }
    let mut variants = String::new();
    let mut arms = String::new();
    for (value, (name, description, cited)) in &by_source {
        if let Some(description) = description {
            let _ = writeln!(variants, "/// {}\n///", doc_line(description));
        }
        let _ = writeln!(variants, "/// SOURCE: {cited}.\n{name} = {value},");
        let _ = writeln!(arms, "{value} => Some(Self::{name}),");
    }
    Ok(format!(
        "/// The chip's interrupt-matrix sources.\n\
         #[allow(non_camel_case_types, clippy::upper_case_acronyms, reason = \"the vendor's source names\")]\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]\n\
         #[repr(u16)]\n\
         pub enum Interrupt {{\n{variants}}}\n\n\
         impl Interrupt {{\n\
         /// The source's number in the interrupt matrix.\n\
         #[inline]\n\
         pub const fn source(self) -> u16 {{\n\
         self as u16\n\
         }}\n\n\
         /// The source numbered `source`, if the chip has one.\n\
         pub const fn from_source(source: u16) -> Option<Self> {{\n\
         match source {{\n{arms}_ => None,\n}}\n\
         }}\n\
         }}\n\n"
    ))
}

/// A vendor description as one documentation line: whitespace collapsed,
/// so no line of it reads as a Markdown list item, and brackets escaped, so
/// none reads as a link.
fn doc_line(description: &str) -> String {
    description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn valid_variant(name: &str) -> bool {
    name.starts_with(|character: char| character.is_ascii_uppercase())
        && name.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

fn render_report(
    vendor_path: &str,
    verdicts: &[RegisterVerdict],
    skipped: &[(String, String)],
    rendered: &[String],
    renames: &[Rename],
) -> String {
    let count = |api: RegisterApi| verdicts.iter().filter(|verdict| verdict.api == api).count();
    let mut report = String::new();
    let _ = writeln!(
        report,
        "# Generated by `cargo registers generate` from {vendor_path}, the\n\
         # strictness overlay and the pinned ESP-IDF headers; do not edit. Every\n\
         # vendor field the API names after its header (`renamed`), and every\n\
         # register the conservative pass narrowed, with its reasons: `read` has\n\
         # field reads only, `none` is not rendered.\n\
         schema = 1\n\n\
         [summary]\n\
         peripherals = {}\n\
         skipped-peripherals = {}\n\
         registers = {}\n\
         full = {}\n\
         read = {}\n\
         none = {}\n\
         renamed-fields = {}\n",
        rendered.len(),
        skipped.len(),
        verdicts.len(),
        count(RegisterApi::Full),
        count(RegisterApi::Read),
        count(RegisterApi::None),
        renames.len(),
    );
    for (peripheral, reason) in skipped {
        let _ = writeln!(
            report,
            "[[skipped]]\nperipheral = {}\nreason = {}\n",
            quote(peripheral),
            quote(reason)
        );
    }
    for rename in renames {
        let _ = writeln!(
            report,
            "[[renamed]]\nperipheral = {}\nregister = {}\nsvd = {}\nsource = {}\n",
            quote(&rename.peripheral),
            quote(&rename.register),
            quote(&rename.svd),
            quote(&rename.source)
        );
    }
    for verdict in verdicts
        .iter()
        .filter(|verdict| verdict.api != RegisterApi::Full)
    {
        let reasons = verdict
            .reasons
            .iter()
            .map(|reason| quote(reason))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            report,
            "[[register]]\nperipheral = {}\nregister = {}\napi = \"{}\"\nreasons = [{reasons}]\n",
            quote(&verdict.peripheral),
            quote(&verdict.register),
            verdict.api.name(),
        );
    }
    report
}

fn quote(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests;
