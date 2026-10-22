//! References selected from captured symbols and already proven function facts.
//! No instruction decoding, inferred callback edges or interprocedural expansion.
use crate::{captured, data, in_process::Executable};
use oer_analysis_blobray_domain::{DataRequest, DataSelector, LibraryFunction, SymbolRecord};
use oer_analysis_elf::{SymbolKind, classify as elf};
use oer_analysis_riscv_model::{
    AbstractValue, FunctionRecord, MemoryKind, MemoryReservation, ReferenceKind, Result,
    RunControl, ValueAlternative, WorkingMemory,
};
use serde::{Deserialize, Serialize};

/// One exact named target in one executable input. Equal addresses in different
/// inputs remain distinct; symbol-table aliases of the same target do not.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub address: u64,
    pub input: u64,
    #[serde(with = "oer_analysis_riscv_model::symbol_name")]
    pub name: Vec<u8>,
}

/// Sorted, cross-input address lookup, with its owned names admitted.
pub struct Targets<'m> {
    entries: Vec<Target>,
    _memory: MemoryReservation<'m>,
}
impl<'m> Targets<'m> {
    pub fn capture(
        inputs: &[Executable],
        names: &[String],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        let mut admitted = memory.reserve(0, c.position())?;
        for (input, executable) in inputs.iter().enumerate() {
            let mut position = c.position();
            position.input = Some(input as u64);
            c.set_position(position);
            let inventory = captured::inventory(executable, memory, c)?;
            for object in &inventory.objects {
                let Some(elf) = &object.elf else { continue };
                if elf::object_kind(elf.object_type) != elf::ObjectKind::Executable {
                    continue;
                }
                for symbol in &elf.symbols {
                    c.checkpoint(1)?;
                    if !elf::symbol_section(symbol.raw_section, symbol.extended_section)
                        .is_defined()
                        || (SymbolKind::of_type(symbol.symbol_type) != SymbolKind::Text
                            && elf::symbol_section(symbol.raw_section, symbol.extended_section)
                                != elf::SymbolSection::Absolute)
                    {
                        continue;
                    }
                    let Some(name) = symbol.name.as_ref().filter(|name| {
                        names
                            .iter()
                            .any(|wanted| wanted.as_bytes() == name.as_slice())
                    }) else {
                        continue;
                    };
                    admitted.grow(
                        (2 * std::mem::size_of::<Target>() + name.len()) as u64,
                        c.position(),
                    )?;
                    entries
                        .try_reserve_exact(1)
                        .map_err(|_| allocation_error())?;
                    entries.push(Target {
                        address: symbol.value,
                        input: input as u64,
                        name: name.clone(),
                    });
                }
            }
        }
        c.checkpoint(sort_work(entries.len()))?;
        entries.sort_unstable();
        entries.dedup();
        Ok(Self {
            entries,
            _memory: admitted,
        })
    }

    /// Build a target index from already captured named addresses.
    pub fn new(
        entries: impl IntoIterator<Item = Target>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut admitted = memory.reserve(0, c.position())?;
        let mut targets = Vec::new();
        for target in entries {
            c.checkpoint(1)?;
            admitted.grow(
                (2 * std::mem::size_of::<Target>() + target.name.len()) as u64,
                c.position(),
            )?;
            targets
                .try_reserve_exact(1)
                .map_err(|_| allocation_error())?;
            targets.push(target);
        }
        c.checkpoint(sort_work(targets.len()))?;
        targets.sort_unstable();
        targets.dedup();
        Ok(Self {
            entries: targets,
            _memory: admitted,
        })
    }

    pub fn entries(&self) -> &[Target] {
        &self.entries
    }

    fn at(&self, address: u64, c: &mut dyn RunControl) -> Result<&[Target]> {
        c.checkpoint(2 * (self.entries.len().max(1).ilog2() as u64 + 1))?;
        let start = self
            .entries
            .partition_point(|target| target.address < address);
        let end = self
            .entries
            .partition_point(|target| target.address <= address);
        self.entries.get(start..end).ok_or_else(|| {
            oer_analysis_riscv_model::Error::new(
                oer_analysis_riscv_model::ErrorCode::Integrity,
                "invalid target index range",
            )
        })
    }
}

fn allocation_error() -> oer_analysis_riscv_model::Error {
    oer_analysis_riscv_model::Error::new(
        oer_analysis_riscv_model::ErrorCode::ResourceLimited,
        "reference allocation refused",
    )
}

fn sort_work(len: usize) -> u64 {
    len as u64 * (len.max(1).ilog2() as u64 + 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Relocation,
    Transfer,
    TransferMay,
    StoredWord,
    Materialized,
}

/// A relocation or an exact/may transfer. Relocations retain their
/// original offset and classification; `site` identifies the transfer's inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub offset: u64,
    pub site: u64,
    pub kind: Kind,
    #[serde(with = "oer_analysis_riscv_model::symbol_name")]
    pub target: Vec<u8>,
    pub destination: Option<Target>,
    pub relocation_kind: Option<ReferenceKind>,
    pub alternatives: Option<usize>,
}

/// The shared reference search used by both CLI commands. Address and other
/// relocations retain their classification; the arguments view selects only
/// Call/Branch relocations. A transfer record
/// already means a call or an out-of-function jump (including a tail call).
pub fn call_sites(
    records: &[FunctionRecord],
    names: &[String],
    targets: &Targets<'_>,
    c: &mut dyn RunControl,
) -> Result<Vec<Reference>> {
    let mut found = Vec::new();
    for record in records {
        c.checkpoint(1)?;
        match record {
            FunctionRecord::Reference {
                raw,
                reference_kind: kind,
                target,
                ..
            } if names.iter().any(|name| name.as_bytes() == target.name) => {
                found.push(Reference {
                    offset: raw.offset,
                    site: raw.offset + if *kind == ReferenceKind::Call { 4 } else { 0 },
                    kind: Kind::Relocation,
                    target: target.name.clone(),
                    destination: None,
                    relocation_kind: Some(*kind),
                    alternatives: None,
                });
            }
            FunctionRecord::Transfer { offset, target, .. } => match target {
                AbstractValue::ImageAddress { address } => {
                    transfer(&mut found, *offset, *address, None, targets, c)?;
                }
                AbstractValue::Alternatives { values } => {
                    for alternative in values.values() {
                        c.checkpoint(1)?;
                        if let ValueAlternative::ImageAddress { address } = alternative {
                            transfer(
                                &mut found,
                                *offset,
                                *address,
                                Some(values.values().len()),
                                targets,
                                c,
                            )?;
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    c.checkpoint(sort_work(found.len()))?;
    found.sort_unstable_by(|a, b| {
        (
            a.offset,
            a.kind as u8,
            &a.target,
            &a.destination,
            a.relocation_kind.map(|kind| kind as u8),
            a.alternatives,
        )
            .cmp(&(
                b.offset,
                b.kind as u8,
                &b.target,
                &b.destination,
                b.relocation_kind.map(|kind| kind as u8),
                b.alternatives,
            ))
    });
    found.dedup();
    Ok(found)
}

fn transfer(
    found: &mut Vec<Reference>,
    offset: u64,
    address: u32,
    alternatives: Option<usize>,
    targets: &Targets<'_>,
    c: &mut dyn RunControl,
) -> Result<()> {
    for target in targets.at(u64::from(address), c)? {
        c.checkpoint(1)?;
        found.push(Reference {
            offset,
            site: offset,
            kind: if alternatives.is_some() {
                Kind::TransferMay
            } else {
                Kind::Transfer
            },
            target: target.name.clone(),
            destination: Some(target.clone()),
            relocation_kind: None,
            alternatives,
        });
    }
    Ok(())
}

/// Candidate pointer storage; neither variant proves an indirect call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Stored {
    StoredWord {
        input: u64,
        section: u32,
        #[serde(with = "oer_analysis_riscv_model::symbol_name::option")]
        section_name: Option<Vec<u8>>,
        address: u64,
        owners: Vec<SymbolRecord>,
        destination: Target,
    },
    Materialized {
        function: LibraryFunction,
        offset: u64,
        register: u8,
        destination: Target,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanSummary {
    pub sections: u64,
    pub words: u64,
    /// NOBITS has no captured words. Runtime-populated data is unknown.
    pub nobits_sections: u64,
    pub incomplete_inventories: u64,
}

/// Keeps candidate-output allocations admitted until the document is emitted.
pub struct StoredScan<'m> {
    pub references: Vec<Stored>,
    pub summary: ScanSummary,
    admitted: MemoryReservation<'m>,
}
impl<'m> StoredScan<'m> {
    pub fn new(memory: &'m WorkingMemory, c: &mut dyn RunControl) -> Result<Self> {
        Ok(Self {
            references: Vec::new(),
            summary: ScanSummary::default(),
            admitted: memory.reserve(0, c.position())?,
        })
    }

    fn admit(&mut self, owned_bytes: usize, c: &mut dyn RunControl) -> Result<()> {
        self.admitted.grow(
            (2 * std::mem::size_of::<Stored>() + owned_bytes) as u64,
            c.position(),
        )?;
        self.references
            .try_reserve_exact(1)
            .map_err(|_| allocation_error())
    }

    pub fn scan_data(
        &mut self,
        inputs: &[Executable],
        targets: &Targets<'_>,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for (input, executable) in inputs.iter().enumerate() {
            let mut position = c.position();
            position.input = Some(input as u64);
            c.set_position(position);
            let inventory = captured::inventory(executable, memory, c)?;
            // Classification, symbols and section metadata all come from the
            // artifact parser's captured inventory, including for archive inputs.
            if !inventory.objects.iter().any(|object| {
                object.elf.as_ref().is_some_and(|elf| {
                    elf::object_kind(elf.object_type) == elf::ObjectKind::Executable
                })
            }) {
                continue;
            }
            self.summary.incomplete_inventories += u64::from(!inventory.complete());
            for object in &inventory.objects {
                let Some(elf) = &object.elf else { continue };
                if elf::object_kind(elf.object_type) != elf::ObjectKind::Executable {
                    continue;
                }
                for section in &elf.sections {
                    c.checkpoint(1)?;
                    if !elf::SectionFlags(section.flags).allocated()
                        || elf::SectionFlags(section.flags).executable()
                        || section.size == 0
                    {
                        continue;
                    }
                    if oer_analysis_elf::SectionKind::of(section.section_type)
                        == oer_analysis_elf::SectionKind::NoBits
                    {
                        self.summary.nobits_sections += 1;
                        continue;
                    }
                    let mut position = c.position();
                    position.table = Some(u64::from(section.index));
                    position.entry = None;
                    c.set_position(position);
                    // The exported copy remains admitted for its entire scan.
                    let _bytes = memory.reserve(section.size.max(8), c.position())?;
                    let export = data::export(
                        &DataRequest {
                            object: object.id.clone(),
                            symbol: None,
                            ranges: vec![DataSelector::Section {
                                section: section.index,
                                offset: 0,
                                length: section.size,
                            }],
                        },
                        inputs,
                        memory,
                        c,
                    )?;
                    self.summary.sections += 1;
                    let first = ((4 - section.address % 4) % 4) as usize;
                    for (word, bytes) in export
                        .bytes
                        .get(first..)
                        .unwrap_or_default()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .enumerate()
                    {
                        let mut position = c.position();
                        position.phase = oer_analysis_riscv_model::RunPhase::Materialize;
                        position.table = Some(u64::from(section.index));
                        position.entry = Some(word as u64);
                        c.set_position(position);
                        c.checkpoint(1)?;
                        self.summary.words += 1;
                        let value = if elf.little_endian {
                            u32::from_le_bytes(*bytes)
                        } else {
                            u32::from_be_bytes(*bytes)
                        };
                        let address = section.address + first as u64 + word as u64 * 4;
                        for target in targets.at(u64::from(value), c)? {
                            self.admit(
                                target.name.len() + section.name.as_ref().map_or(0, Vec::len),
                                c,
                            )?;
                            let mut owners = Vec::new();
                            for symbol in &elf.symbols {
                                c.checkpoint(1)?;
                                let index = elf::symbol_section(
                                    symbol.raw_section,
                                    symbol.extended_section,
                                )
                                .index();
                                if SymbolKind::of_type(symbol.symbol_type) == SymbolKind::Data
                                    && index == Some(section.index)
                                    && symbol.value <= address
                                    && address - symbol.value < symbol.size
                                    && symbol.size - (address - symbol.value) >= 4
                                {
                                    self.admitted.grow(
                                        (2 * std::mem::size_of::<SymbolRecord>()
                                            + symbol.name.as_ref().map_or(0, Vec::len)
                                            + symbol.id.object.artifact.as_str().len())
                                            as u64,
                                        c.position(),
                                    )?;
                                    owners
                                        .try_reserve_exact(1)
                                        .map_err(|_| allocation_error())?;
                                    owners.push(symbol.clone());
                                }
                            }
                            self.references.push(Stored::StoredWord {
                                input: input as u64,
                                section: section.index,
                                section_name: section.name.clone(),
                                address,
                                owners,
                                destination: target.clone(),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn materialized(
        &mut self,
        function: &LibraryFunction,
        records: &[FunctionRecord],
        targets: &Targets<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for record in records {
            c.checkpoint(1)?;
            let FunctionRecord::Value {
                offset,
                register,
                value,
                ..
            } = record
            else {
                continue;
            };
            if *register == 0 {
                continue;
            }
            let address = match value {
                AbstractValue::Constant { value } => *value,
                AbstractValue::ImageAddress { address } => *address,
                _ => continue,
            };
            let selected = targets.at(u64::from(address), c)?;
            if selected.is_empty() || feeds_transfer(records, *offset, *register, address, c)? {
                continue;
            }
            for target in selected {
                self.admit(
                    target.name.len()
                        + function.name.as_ref().map_or(0, Vec::len)
                        + function.symbol.object.artifact.as_str().len(),
                    c,
                )?;
                self.references.push(Stored::Materialized {
                    function: function.clone(),
                    offset: *offset,
                    register: *register,
                    destination: target.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Exclude values used as a following indirect transfer's base before that
/// register is overwritten. A stored value remains a candidate even if called
/// later. All evidence comes from emitted records and decoded flow metadata.
fn feeds_transfer(
    records: &[FunctionRecord],
    offset: u64,
    register: u8,
    address: u32,
    c: &mut dyn RunControl,
) -> Result<bool> {
    let mut end = u64::MAX;
    let mut transfer = None;
    let mut store = None;
    for record in records {
        c.checkpoint(1)?;
        match record {
            FunctionRecord::Block {
                start,
                end: block_end,
                ..
            } if *start <= offset && offset < *block_end => end = end.min(*block_end),
            FunctionRecord::Value {
                offset: next,
                register: written,
                ..
            } if *next > offset && *written == register => end = end.min(*next),
            FunctionRecord::Instruction {
                offset: next,
                decoded,
                ..
            } if *next > offset => {
                if matches!(decoded.flow, oer_analysis_riscv_model::InstructionFlow::Indirect { base, offset: 0, .. } if base == register)
                {
                    transfer = Some(transfer.unwrap_or(u64::MAX).min(*next));
                } else if decoded.flow != oer_analysis_riscv_model::InstructionFlow::Next {
                    // Do not infer register use across a different control transfer.
                    end = end.min(*next);
                }
            }
            FunctionRecord::MemoryAccess {
                offset: next,
                access: MemoryKind::Store | MemoryKind::StoreConditional,
                value: Some(value),
                ..
            } if *next >= offset
                && (matches!(value, AbstractValue::Constant { value } if *value == address)
                    || matches!(value, AbstractValue::ImageAddress { address: value } if *value == address)) =>
            {
                store = Some(store.unwrap_or(u64::MAX).min(*next));
            }
            _ => {}
        }
    }
    let Some(site) =
        transfer.filter(|site| *site <= end && store.is_none_or(|store| store > *site))
    else {
        return Ok(false);
    };
    for record in records {
        c.checkpoint(1)?;
        if matches!(record, FunctionRecord::Transfer { offset, target: AbstractValue::ImageAddress { address: target }, .. } if *offset == site && *target == address)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One function's references to the requested symbols.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub function: LibraryFunction,
    pub references: Vec<crate::references::Reference>,
}

#[cfg(test)]
mod tests;
