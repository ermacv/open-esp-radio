//! Explicit preparation of native Blobray requests and captured probe catalogs.
//!
//! This module supplies no hardware model or expected result. Callers select every
//! input, peripheral assumption, reset and observation relation.
use blobray_application::in_process::{Executable, Limits};
use blobray_domain::{
    ArtifactInventory, ComparisonRelation, DataRequest, DataSelector, DeviceDeclaration,
    EventChannels, ExecutionCase, ExecutionGoal, ExecutionRegion, Invocation, MemoryPair,
    MemorySeed, MemorySelection, ObjectInventory, RegionLifetime, ReturnWords, SectionRecord,
    SessionReset, SymbolRecord, TimelineCapture,
};
use oer_probe_codegen::Catalog;
use oer_riscv_model::{CallAbi, WorkingMemory};
use std::{collections::BTreeMap, fs, path::Path};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub fn invalid(message: impl Into<String>) -> Error {
    message.into().into()
}

/// No internal timeline observations unless a scenario selects them.
pub const TIMELINE: TimelineCapture = TimelineCapture {
    reads: false,
    writes: false,
    atomics: false,
    branches: false,
    written: false,
};

/// One captured input and its independently pinned identity, if any.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub role: &'a str,
    pub path: &'a Path,
    /// `None` selects digest identification without a pinned expected value,
    /// as required for freshly compiled production.
    pub sha256: Option<&'a str>,
}

/// Per-operation Blobray budget selected by the scenario operator, enforced
/// cooperatively inside this process.
#[derive(Clone, Copy, Debug, clap::Args)]
pub struct Budget {
    /// Wall-clock deadline of each Blobray operation.
    #[arg(long, default_value_t = 600)]
    pub timeout_secs: u64,
    /// Algorithm working memory of each operation.
    #[arg(long, default_value_t = 256)]
    pub working_memory_mib: u64,
    /// Work-unit budget of each operation.
    #[arg(long, default_value_t = 2_000_000_000)]
    pub max_work_units: u64,
}

impl Budget {
    /// Working memory of one operation.
    pub fn memory(&self) -> oer_riscv_model::Result<WorkingMemory> {
        WorkingMemory::new(self.working_memory_mib << 20)
    }
    /// Work and wall-clock limits of one operation.
    pub fn control(&self) -> Limits {
        Limits::new(
            self.max_work_units,
            std::time::Duration::from_secs(self.timeout_secs),
        )
    }
}

/// Read every input once and check its pinned digest; returns each digest and
/// the authenticated bytes.
pub fn authenticate(inputs: &[Input<'_>]) -> Result<(Vec<String>, Vec<Vec<u8>>)> {
    let mut identities = Vec::with_capacity(inputs.len());
    let mut contents = Vec::with_capacity(inputs.len());
    for input in inputs {
        let bytes = fs::read(input.path).map_err(|error| {
            invalid(format!(
                "{}: cannot read {}: {error}",
                input.role,
                input.path.display()
            ))
        })?;
        let actual = oer_durable::sha256_bytes(&bytes);
        if let Some(expected) = input.sha256
            && actual != expected
        {
            return Err(invalid(format!(
                "{}: authenticated input mismatch: {actual}",
                input.role
            )));
        }
        identities.push(actual);
        contents.push(bytes);
    }
    Ok((identities, contents))
}

pub fn objects(inventory: &[ArtifactInventory], input: usize) -> &[ObjectInventory] {
    inventory.get(input).map_or(&[], |i| &i.objects)
}

/// Resolve exactly one defined symbol in a caller-selected captured input.
pub fn symbol<'a>(
    inventory: &'a [ArtifactInventory],
    input: usize,
    name: &str,
) -> Result<&'a SymbolRecord> {
    let matches: Vec<_> = objects(inventory, input)
        .iter()
        .filter_map(|o| o.elf.as_ref())
        .flat_map(|elf| &elf.symbols)
        .filter(|s| s.name.as_deref() == Some(name.as_bytes()) && s.raw_section != 0)
        .collect();
    match matches[..] {
        [one] => Ok(one),
        _ => Err(invalid(format!(
            "missing or ambiguous symbol {name}: {} matches",
            matches.len()
        ))),
    }
}

pub fn named_object<'a>(
    inventory: &'a [ArtifactInventory],
    input: usize,
    name: &str,
) -> Result<&'a ObjectInventory> {
    objects(inventory, input)
        .iter()
        .find(|o| o.name.as_deref() == Some(name.as_bytes()))
        .ok_or_else(|| invalid(format!("missing object {name}")))
}

pub fn named_section<'a>(object: &'a ObjectInventory, name: &str) -> Result<&'a SectionRecord> {
    object
        .elf
        .as_ref()
        .and_then(|elf| {
            elf.sections
                .iter()
                .find(|s| s.name.as_deref() == Some(name.as_bytes()))
        })
        .ok_or_else(|| invalid(format!("missing section {name}")))
}

/// Explicit placement and byte seed; extent comes from a fixed-array ABI type.
#[derive(Clone, Debug)]
pub struct Buffer {
    pub address: u32,
    pub data: Vec<u8>,
    pub fill: Option<u8>,
    pub lifetime: RegionLifetime,
}
impl Buffer {
    pub fn new(address: u32, data: impl Into<Vec<u8>>) -> Self {
        Self {
            address,
            data: data.into(),
            fill: None,
            lifetime: RegionLifetime::Phase,
        }
    }
    pub fn filled(address: u32, fill: u8) -> Self {
        Self {
            fill: Some(fill),
            ..Self::new(address, [])
        }
    }
    pub fn session(self) -> Self {
        Self {
            lifetime: RegionLifetime::Session,
            ..self
        }
    }
}

/// One named probe argument: an ABI scalar/pointer (or explicit unknown) or a buffer.
#[derive(Clone, Debug)]
pub enum Arg {
    Word(Option<i64>),
    Buffer(Buffer),
}
impl From<i64> for Arg {
    fn from(value: i64) -> Self {
        Self::Word(Some(value))
    }
}
impl From<Buffer> for Arg {
    fn from(value: Buffer) -> Self {
        Self::Buffer(value)
    }
}

/// Size and alignment of a fixed primitive-array reference; opaque layouts fail.
pub fn array_layout(rust_type: &str) -> Result<(u32, u32)> {
    let spelling: String = rust_type.chars().filter(|c| *c != ' ').collect();
    let inner = spelling
        .strip_prefix('&')
        .map(|s| s.strip_prefix("mut").unwrap_or(s))
        .filter(|s| s.starts_with('['))
        .ok_or_else(|| invalid("automatic buffers require a fixed-array reference"))?;
    fn layout(value: &str, rust_type: &str) -> Result<(u32, u32)> {
        if let Some(body) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            let (element, count) = body.rsplit_once(';').ok_or_else(|| {
                invalid(format!("explicit buffer layout required for {rust_type}"))
            })?;
            let count: u32 = count
                .parse()
                .map_err(|_| invalid(format!("explicit buffer layout required for {rust_type}")))?;
            let (size, alignment) = layout(element, rust_type)?;
            return Ok((size * count, alignment));
        }
        match value {
            "u8" | "i8" => Ok((1, 1)),
            "u16" | "i16" => Ok((2, 2)),
            "u32" | "i32" => Ok((4, 4)),
            _ => Err(invalid(format!(
                "explicit buffer layout required for {rust_type}"
            ))),
        }
    }
    layout(inner, rust_type)
}

/// Lower one declared scalar/pointer ABI value with explicit range checks.
/// References are guest addresses; unknown words remain `None`.
pub fn abi_word(rust_type: &str, value: Option<i64>) -> Result<Option<u32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let spelling: String = rust_type.chars().filter(|c| *c != ' ').collect();
    let (bits, signed) = if spelling == "bool" {
        if !(0..=1).contains(&value) {
            return Err(invalid("Rust bool requires zero or one"));
        }
        return Ok(Some(value as u32));
    } else if spelling.starts_with('&')
        || spelling.starts_with("*const")
        || spelling.starts_with("*mut")
    {
        (32, false)
    } else if let Some(width) = ["8", "16", "32"]
        .into_iter()
        .find(|w| spelling.len() > 1 && &spelling[1..] == *w && matches!(&spelling[..1], "i" | "u"))
    {
        (width.parse::<u32>()?, spelling.starts_with('i'))
    } else if spelling == "usize" || spelling == "isize" {
        (32, spelling == "isize")
    } else {
        return Err(invalid(format!(
            "explicit ABI adapter required for {rust_type}"
        )));
    };
    let lower = if signed { -(1i64 << (bits - 1)) } else { 0 };
    let upper = (1i64 << (bits - u32::from(signed))) - 1;
    if !(lower..=upper).contains(&value) {
        return Err(invalid(format!("value outside {rust_type}: {value}")));
    }
    Ok(Some(value as u32))
}

/// Little-endian RV32 words.
pub fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Words padded to an explicit count with an explicit fill.
pub fn words_padded(values: &[u32], pad_to: usize, fill: u32) -> Result<Vec<u8>> {
    if values.len() > pad_to {
        return Err(invalid("word padding requires sufficient capacity"));
    }
    let mut padded = values.to_vec();
    padded.resize(pad_to, fill);
    Ok(words(&padded))
}

pub fn seed(address: u32, length: u32, data: &[u8], fill: Option<u8>) -> Result<MemorySeed> {
    if u64::from(address) + u64::from(length) > 1 << 32 || data.len() > length as usize {
        return Err(invalid("invalid RV32 memory extent"));
    }
    Ok(MemorySeed {
        address,
        length,
        fill,
        bytes: data.to_vec(),
    })
}

pub fn region(
    address: u32,
    length: u32,
    data: &[u8],
    fill: Option<u8>,
    lifetime: RegionLifetime,
) -> Result<ExecutionRegion> {
    Ok(ExecutionRegion {
        seed: seed(address, length, data, fill)?,
        lifetime,
    })
}

/// Phase-owned region with known bytes and unknown remainder.
pub fn known(address: u32, length: u32, data: &[u8]) -> Result<ExecutionRegion> {
    region(address, length, data, None, RegionLifetime::Phase)
}

/// Phase-owned region filled with one byte.
pub fn filled(address: u32, length: u32, fill: u8) -> Result<ExecutionRegion> {
    region(address, length, &[], Some(fill), RegionLifetime::Phase)
}

pub fn invocation(
    entry: u32,
    arguments: Vec<Option<u32>>,
    memory: Vec<ExecutionRegion>,
    models: Vec<DeviceDeclaration>,
    observe: Vec<MemorySelection>,
) -> Invocation {
    Invocation {
        entry,
        goal: ExecutionGoal::Return,
        arguments,
        memory,
        preload: vec![],
        models,
        calls: vec![],
        observe_memory: observe,
        observe_calls: None,
        observe_timeline: TIMELINE,
    }
}

pub fn selection(address: u32, length: u32) -> MemorySelection {
    MemorySelection {
        name: "selected-output".into(),
        address,
        length,
    }
}

/// Every MMIO/fence/delay event and optionally the first selected RAM pair.
pub fn relation(memory: bool) -> ComparisonRelation {
    ComparisonRelation {
        effects: None,
        projection: None,
        returns: ReturnWords {
            low: false,
            high: false,
        },
        events: EventChannels {
            timeline: TIMELINE,
            mmio_read: true,
            mmio_write: true,
            fence: true,
            delay: true,
        },
        memory: if memory {
            vec![MemoryPair {
                vendor: 0,
                replacement: 0,
            }]
        } else {
            vec![]
        },
        calls: false,
    }
}

/// A setup case: both sides run to prepare the state later cases compare,
/// and nothing of it is compared, observed or claimed.
pub fn setup(
    name: impl Into<String>,
    vendor: Invocation,
    replacement: Invocation,
    reset: SessionReset,
) -> ExecutionCase {
    ExecutionCase {
        name: name.into(),
        reset,
        stack_fill: None,
        relation: None,
        vendor,
        replacement: Some(replacement),
    }
}

pub fn case(
    name: impl Into<String>,
    vendor: Invocation,
    replacement: Option<Invocation>,
    reset: SessionReset,
    memory: bool,
) -> ExecutionCase {
    ExecutionCase {
        name: name.into(),
        reset,
        stack_fill: None,
        relation: Some(relation(memory)),
        vendor,
        replacement,
    }
}

/// Cases of one profile with its stack fill: one request can hold profiles of
/// several fills.
pub fn with_stack_fill(mut rows: Vec<ExecutionCase>, fill: u8) -> Vec<ExecutionCase> {
    for row in &mut rows {
        row.stack_fill = Some(fill);
    }
    rows
}

pub fn data_request(object: &ObjectInventory, range: DataSelector) -> DataRequest {
    DataRequest {
        object: object.id.clone(),
        symbol: None,
        ranges: vec![range],
    }
}

pub const RISCV_INTEGER: CallAbi = CallAbi::RiscvInteger;

/// Resolve declared entries from the same captured ELF as their metadata.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProbeCatalog {
    entries: BTreeMap<String, (oer_probe_codegen::Entry, u32)>,
}

impl ProbeCatalog {
    /// Validate every declaration, not only entries selected by one scenario.
    pub fn new(catalog: Catalog, resolve: impl Fn(&str) -> Result<u32>) -> Result<Self> {
        if catalog.schema != 1 || catalog.image.is_empty() || catalog.entries.is_empty() {
            return Err(invalid("unsupported or empty probe catalog"));
        }
        let mut entries = BTreeMap::new();
        for entry in catalog.entries {
            let identifier =
                entry.symbol.chars().enumerate().all(|(i, c)| {
                    c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                });
            if entry.symbol.is_empty()
                || !identifier
                || entries.contains_key(&entry.symbol)
                || !matches!(entry.adapter.as_str(), "call" | "body")
            {
                return Err(invalid(format!(
                    "invalid or duplicate probe {}",
                    entry.symbol
                )));
            }
            let address = resolve(&entry.symbol)?;
            entries.insert(entry.symbol.clone(), (entry, address));
        }
        Ok(Self { entries })
    }

    /// Read the catalog section of one captured input and validate its entries.
    pub fn capture(
        inventory: &[ArtifactInventory],
        executables: &[Executable],
        input: usize,
        budget: &Budget,
    ) -> Result<Self> {
        let matches: Vec<_> = objects(inventory, input)
            .iter()
            .filter_map(|o| o.elf.as_ref().map(|elf| (o, elf)))
            .flat_map(|(o, elf)| {
                elf.sections
                    .iter()
                    .filter(|s| s.name.as_deref() == Some(b".blobray.probes"))
                    .map(move |s| (o, elf, s))
            })
            .collect();
        let [(object, elf, section)] = matches[..] else {
            return Err(invalid("expected one captured probe catalog section"));
        };
        let request = data_request(
            object,
            DataSelector::Section {
                section: section.index,
                offset: 0,
                length: section.size,
            },
        );
        let bytes = blobray_application::data::export(
            &request,
            executables,
            &budget.memory()?,
            &mut budget.control(),
        )?
        .bytes;
        let catalog: Catalog = serde_json::from_slice(&bytes)?;
        Self::new(catalog, |name| {
            let selected = symbol(inventory, input, name)?;
            let index = selected
                .extended_section
                .unwrap_or(u32::from(selected.raw_section));
            let sections: Vec<_> = elf.sections.iter().filter(|s| s.index == index).collect();
            if selected.id.object != object.id || selected.symbol_type != 2 || sections.len() != 1 {
                return Err(invalid(format!(
                    "probe is not a function in the catalog image: {name}"
                )));
            }
            let section = sections[0];
            if section.flags & 6 != 6
                || !(section.address <= selected.value
                    && selected.value < section.address + section.size)
            {
                return Err(invalid(format!(
                    "probe is not in allocated executable code: {name}"
                )));
            }
            Ok(u32::try_from(selected.value)?)
        })
    }

    /// Every declared entry with its address.
    pub fn entries(&self) -> impl Iterator<Item = (&str, u32)> {
        self.entries
            .iter()
            .map(|(name, (_, address))| (name.as_str(), *address))
    }

    pub fn entry(&self, name: &str) -> Result<u32> {
        self.entries
            .get(name)
            .map(|(_, address)| *address)
            .ok_or_else(|| invalid(format!("undeclared probe {name}")))
    }

    /// Lower named ABI scalars/pointers; names must match the declaration exactly.
    pub fn arguments(
        &self,
        name: &str,
        values: &[(&str, Option<i64>)],
    ) -> Result<Vec<Option<u32>>> {
        let (entry, _) = self
            .entries
            .get(name)
            .ok_or_else(|| invalid(format!("undeclared probe {name}")))?;
        let mut given: Vec<&str> = values.iter().map(|(n, _)| *n).collect();
        let mut declared: Vec<&str> = entry.arguments.iter().map(|a| a.name.as_str()).collect();
        given.sort_unstable();
        declared.sort_unstable();
        if given != declared || given.windows(2).any(|w| w[0] == w[1]) {
            return Err(invalid(format!("argument names do not match {name}")));
        }
        entry
            .arguments
            .iter()
            .map(|p| {
                let value = values
                    .iter()
                    .find(|(n, _)| *n == p.name)
                    .and_then(|(_, v)| *v);
                abi_word(&p.rust_type, value)
            })
            .collect()
    }

    /// Prepare named arguments and fixed-array buffers from the declaration.
    ///
    /// Buffer addresses, byte contents and lifetime are explicit; only size and
    /// alignment follow the declared primitive array. Opaque layouts are never
    /// guessed. Use a raw guest address to refer to already-owned memory.
    pub fn invoke(
        &self,
        name: &str,
        values: Vec<(&str, Arg)>,
        models: Vec<DeviceDeclaration>,
        observe: Vec<MemorySelection>,
    ) -> Result<Invocation> {
        let (entry, address) = self
            .entries
            .get(name)
            .ok_or_else(|| invalid(format!("undeclared probe {name}")))?;
        let mut memory: Vec<ExecutionRegion> = vec![];
        let mut scalars = vec![];
        for (argument, value) in values {
            match value {
                Arg::Word(word) => scalars.push((argument, word)),
                Arg::Buffer(buffer) => {
                    let parameter = entry
                        .arguments
                        .iter()
                        .find(|p| p.name == argument)
                        .ok_or_else(|| invalid(format!("argument names do not match {name}")))?;
                    let (length, alignment) = array_layout(&parameter.rust_type)?;
                    if buffer.address % alignment != 0 {
                        return Err(invalid("misaligned probe buffer"));
                    }
                    let new = region(
                        buffer.address,
                        length,
                        &buffer.data,
                        buffer.fill,
                        buffer.lifetime,
                    )?;
                    let end = u64::from(buffer.address) + u64::from(length);
                    if memory.iter().any(|old| {
                        u64::from(buffer.address)
                            < u64::from(old.seed.address) + u64::from(old.seed.length)
                            && u64::from(old.seed.address) < end
                    }) {
                        return Err(invalid("overlapping probe buffer owners"));
                    }
                    memory.push(new);
                    scalars.push((argument, Some(i64::from(buffer.address))));
                }
            }
        }
        let arguments = self.arguments(name, &scalars)?;
        Ok(invocation(*address, arguments, memory, models, observe))
    }
}

/// Enter `target` directly. `arguments` fill `a0`.. and unused argument
/// registers start at zero; words beyond eight are ascending stack words.
pub fn direct(
    target: u32,
    arguments: &[u32],
    memory: Vec<ExecutionRegion>,
    models: Vec<DeviceDeclaration>,
    observe: Vec<MemorySelection>,
) -> Invocation {
    let mut words: Vec<Option<u32>> = arguments.iter().map(|w| Some(*w)).collect();
    if words.len() < 8 {
        words.resize(8, Some(0));
    }
    invocation(target, words, memory, models, observe)
}

#[cfg(test)]
mod tests;
