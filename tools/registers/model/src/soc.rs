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
//! write-one-to-clear, write triggers, FIFO ports, enumerated values and
//! reserved bits, and the interrupt sources the SVD does not list. The
//! report ([`SocRender::report`]) lists every register the pass narrowed,
//! with its reasons.
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
    RegisterCluster, ValidateLevel,
};

use crate::{
    Error, FifoPort, InterruptSource, OwnershipPartition, PacApiPack, Result, WriteTriggerField,
};

/// A chip's strictness overlay over its vendor SVD (`model/strictness.toml`).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct StrictnessOverlay {
    pub schema: u32,
    /// The pinned ESP-IDF revision `esp-idf:` sources cite.
    pub esp_idf: String,
    /// The pinned esp-hal fork revision `esp-hal:` sources cite.
    #[serde(default)]
    pub esp_hal: Option<String>,
    #[serde(default)]
    pub registers: Vec<RegisterRule>,
    #[serde(default)]
    pub fields: Vec<FieldRule>,
    #[serde(default)]
    pub interrupts: Vec<InterruptRule>,
}

/// What the overlay states about one vendor register.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterRule {
    pub peripheral: String,
    pub register: String,
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
    pub sources: Vec<String>,
}

/// What the overlay states about one vendor field.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldRule {
    pub peripheral: String,
    pub register: String,
    pub field: String,
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
        if let Some(esp_hal) = &self.esp_hal
            && !is_revision(esp_hal)
        {
            return Err(Error::message(
                "the overlay's esp-hal pin is a hexadecimal revision",
            ));
        }
        let mut seen = BTreeSet::new();
        for rule in &self.registers {
            let subject = format!("{}.{}", rule.peripheral, rule.register);
            if !seen.insert(subject.clone()) {
                return Err(Error::message(format!("overlay states {subject} twice")));
            }
            self.validate_sources(&subject, &rule.sources)?;
            if rule.fifo && rule.write_one_to_clear {
                return Err(Error::message(format!(
                    "overlay register {subject} is a FIFO port and write-one-to-clear"
                )));
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

    /// Each source is `esp-idf:<path>:<line>`, `esp-hal:<path>:<line>` (with
    /// the overlay's pin) or `trm:<document and section>`.
    fn validate_sources(&self, subject: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Err(Error::message(format!(
                "overlay entry {subject} cites no source"
            )));
        }
        for source in sources {
            let cited = if let Some(rest) = source.strip_prefix("esp-idf:") {
                file_line(rest)
            } else if let Some(rest) = source.strip_prefix("esp-hal:") {
                self.esp_hal.is_some() && file_line(rest)
            } else if let Some(rest) = source.strip_prefix("trm:") {
                !rest.trim().is_empty()
            } else {
                false
            };
            if !cited {
                return Err(Error::message(format!(
                    "overlay entry {subject} cites {source:?}: a source is esp-idf:<path>:<line>, esp-hal:<path>:<line> with the esp-hal pin, or trm:<section>"
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
    let mut vendor = svd_parser::parse(inputs.vendor_svd)
        .map_err(|error| Error::message(format!("vendor SVD: {error}")))?;
    derive(&mut vendor)?;
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
    let mut matched_registers = BTreeSet::new();
    let mut matched_fields = BTreeSet::new();
    let mut verdicts = Vec::new();
    let mut triggers = Vec::new();
    let mut fifos = Vec::new();
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
            let (api, reasons, typed) = if let Some(owner) = curated_owner {
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
                classify(
                    &vendor,
                    peripheral,
                    register,
                    rule,
                    &field_rules,
                    &mut triggers,
                    &mut fifos,
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
    if let Some((peripheral, register)) = register_rules
        .keys()
        .find(|key| !matched_registers.contains(*key))
    {
        return Err(Error::message(format!(
            "overlay register {peripheral}.{register} is not in the vendor SVD"
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
    triggers.retain(|trigger: &WriteTriggerField| {
        rendered_registers.contains(&(trigger.peripheral.clone(), trigger.register.clone()))
    });
    fifos.retain(|port: &FifoPort| {
        rendered_registers.contains(&(port.peripheral.clone(), port.register.clone()))
    });

    let source = format!("the vendor SVD ({})", inputs.vendor_path);
    let mut pack = PacApiPack::empty();
    pack.options.typed_peripherals = rendered.clone();
    pack.write_trigger_fields = triggers;
    pack.fifo_ports = fifos;
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
    let report = render_report(inputs.vendor_path, &verdicts, &skipped, &rendered);
    Ok(SocRender {
        rust,
        report,
        verdicts,
        peripherals: rendered,
    })
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
    rule: Option<&RegisterRule>,
    field_rules: &BTreeMap<(&str, &str, &str), &FieldRule>,
    triggers: &mut Vec<WriteTriggerField>,
    fifos: &mut Vec<FifoPort>,
) -> Result<(RegisterApi, Vec<String>, Option<svd_rs::Register>)> {
    let name = peripheral.name.as_str();
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
        if !valid_module(&module_name(&info.name)) {
            narrow = true;
            reasons.push(format!(
                "field {} has no snake-case method name that is not a keyword",
                info.name
            ));
            continue;
        }
        let field_rule = field_rules
            .get(&(name, register.name.as_str(), info.name.as_str()))
            .copied();
        // The overlay's field access, else its register access, else the
        // SVD's field access, else the register's.
        let field_access = match (
            field_rule.and_then(|rule| rule.access),
            rule.and_then(|rule| rule.access),
        ) {
            (Some(field), _) => field.svd(),
            (None, Some(_)) => access,
            (None, None) => info.access.unwrap_or(access),
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
        let w1c = rule.is_some_and(|rule| rule.write_one_to_clear);
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
        let kind = field_rule.and_then(|rule| rule.kind);
        if kind == Some(FieldKind::WriteTrigger) {
            triggers.push(WriteTriggerField {
                peripheral: name.to_owned(),
                register: register.name.clone(),
                field: info.name.clone(),
                sources: field_rule
                    .map(|rule| rule.sources.clone())
                    .unwrap_or_default(),
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
    let reserved = rule.and_then(|rule| rule.reserved).unwrap_or(0);
    if reserved & declared != 0 {
        return Err(Error::message(format!(
            "overlay reserves bits of declared fields of {name}.{}",
            register.name
        )));
    }
    let undeclared = !(declared | reserved);
    if undeclared != 0 && access.can_write() {
        narrow = true;
        reasons.push(format!("undeclared bits 0x{undeclared:08x}"));
    }
    if raw_status(&register.name) && !rule.is_some_and(|rule| rule.raw_status_writable) {
        narrow = true;
        reasons.push("a raw interrupt status: what a write does is not stated".to_owned());
    }
    if rule.is_some_and(|rule| rule.write_one_to_clear) && narrow {
        return Err(Error::message(format!(
            "overlay makes {name}.{} write-one-to-clear, but {}",
            register.name,
            reasons.join("; ")
        )));
    }
    if narrow {
        access = Access::ReadOnly;
        triggers
            .retain(|trigger| !(trigger.peripheral == name && trigger.register == register.name));
        kept.retain(|field| field.access.is_none_or(|access| access.can_read()));
        for field in &mut kept {
            field.access = Some(Access::ReadOnly);
        }
    }
    if kept.is_empty() {
        reasons.push("no readable field remains".to_owned());
        return Ok((RegisterApi::None, reasons, None));
    }
    if fifo {
        fifos.push(FifoPort {
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
) -> String {
    let count = |api: RegisterApi| verdicts.iter().filter(|verdict| verdict.api == api).count();
    let mut report = String::new();
    let _ = writeln!(
        report,
        "# Generated by `cargo registers generate` from {vendor_path} and the\n\
         # strictness overlay; do not edit. Every vendor register the conservative\n\
         # pass narrowed, with its reasons: `read` has field reads only, `none` is\n\
         # not rendered.\n\
         schema = 1\n\n\
         [summary]\n\
         peripherals = {}\n\
         skipped-peripherals = {}\n\
         registers = {}\n\
         full = {}\n\
         read = {}\n\
         none = {}\n",
        rendered.len(),
        skipped.len(),
        verdicts.len(),
        count(RegisterApi::Full),
        count(RegisterApi::Read),
        count(RegisterApi::None),
    );
    for (peripheral, reason) in skipped {
        let _ = writeln!(
            report,
            "[[skipped]]\nperipheral = {}\nreason = {}\n",
            quote(peripheral),
            quote(reason)
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
