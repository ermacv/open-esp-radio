//! Names and definitions of already resolved call targets. No diagnostic policy.
use super::*;
use crate::captured::visit_members;
use crate::in_process::Executable;

struct Definition {
    input: u64,
    symbol: SymbolId,
    name: Option<Vec<u8>>,
    offset: u64,
    section: Option<u32>,
    address: Option<u32>,
    function: bool,
    binding: u8,
}

/// One operation's symbol index, including absolute ROM symbols. Relocatable
/// section offsets never enter the image-address index.
pub(crate) struct Callees<'m> {
    definitions: AdmittedVec<'m, Definition>,
    addresses: AdmittedVec<'m, usize>,
    names: AdmittedVec<'m, usize>,
    ledger: Ledger<'m>,
}

impl<'m> Callees<'m> {
    pub fn capture(
        inputs: &[Executable],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            definitions: AdmittedVec::new(memory),
            addresses: AdmittedVec::new(memory),
            names: AdmittedVec::new(memory),
            ledger: Ledger::new(memory, c)?,
        };
        for (input, executable) in inputs.iter().enumerate() {
            let mut position = RunPosition {
                input: Some(input as u64),
                ..RunPosition::default()
            };
            position.artifact(executable.id());
            c.set_position(position);
            visit_members(executable, memory, c, &mut |member, c| {
                let Ok(bytes) = member.bytes else {
                    return Ok(());
                };
                let start = out.definitions.len();
                let mut symbols = Symbols {
                    input: input as u64,
                    catalog: &mut out,
                    executable: AdmittedVec::new(memory),
                };
                let (_, elf) = inspect_source(bytes, &member.id, memory, c, &mut symbols)?;
                if !elf.as_ref().is_some_and(|elf| {
                    elf.bits == 32
                        && elf.little_endian
                        && elf.machine == 243
                        && matches!(elf.object_type, 1 | 2)
                }) {
                    out.definitions.truncate(start);
                    return Ok(());
                }
                if elf.is_some_and(|elf| elf.object_type == 2) {
                    for definition in out.definitions.get_mut(start..).ok_or_else(integrity)? {
                        definition.address = u32::try_from(definition.offset).ok();
                    }
                }
                Ok(())
            })?;
        }
        c.checkpoint(sort_work(out.definitions.len()))?;
        out.definitions
            .sort_unstable_by(|a, b| (&a.symbol, a.input).cmp(&(&b.symbol, b.input)));
        for (id, definition) in out.definitions.iter().enumerate() {
            c.checkpoint(1)?;
            if definition.address.is_some() {
                out.addresses.push(id, c.position())?;
            }
            if definition.binding != 0 && definition.name.is_some() {
                out.names.push(id, c.position())?;
            }
        }
        c.checkpoint(sort_work(out.addresses.len()) + sort_work(out.names.len()))?;
        out.addresses.sort_unstable_by_key(|&id| {
            out.definitions
                .get(id)
                .map(|d| (d.address, !d.function, d.binding != 1, d.input, id))
        });
        out.names.sort_unstable_by(|&a, &b| {
            out.definitions
                .get(a)
                .map(|d| &d.name)
                .cmp(&out.definitions.get(b).map(|d| &d.name))
        });
        Ok(out)
    }

    pub(crate) fn named_addresses(&self) -> impl Iterator<Item = (u32, &[u8])> {
        self.definitions
            .iter()
            .filter_map(|d| Some((d.address?, d.name.as_deref()?)))
    }
    pub(crate) fn len(&self) -> usize {
        self.definitions.len()
    }
    pub(crate) fn function_id(
        &self,
        function: &LibraryFunction,
        c: &mut dyn RunControl,
    ) -> Result<Option<usize>> {
        c.checkpoint(lookup_work(self.definitions.len()))?;
        let start = self
            .definitions
            .partition_point(|d| d.symbol < function.symbol);
        for (offset, d) in self
            .definitions
            .get(start..)
            .ok_or_else(integrity)?
            .iter()
            .enumerate()
        {
            c.checkpoint(1)?;
            if d.symbol != function.symbol {
                break;
            }
            if d.input == function.input && d.function {
                return Ok(Some(start + offset));
            }
        }
        Ok(None)
    }
    pub(crate) fn definition(&self, id: usize) -> Option<(LibraryFunction, Option<CallKey>)> {
        let d = self.definitions.get(id).filter(|d| d.function)?;
        let f = LibraryFunction {
            input: d.input,
            symbol: d.symbol.clone(),
            name: d.name.clone(),
        };
        let key = d.address.map(CallKey::Address).or_else(|| {
            d.name
                .clone()
                .filter(|n| !n.is_empty())
                .map(CallKey::Symbol)
        });
        Some((f, key))
    }

    fn at_address(&self, address: u32, input: u64, c: &mut dyn RunControl) -> Result<CallTarget> {
        c.checkpoint(lookup_work(self.addresses.len()))?;
        let start = self.addresses.partition_point(|&id| {
            self.definitions
                .get(id)
                .is_some_and(|d| d.address < Some(address))
        });
        let mut definition = None;
        let mut ambiguous = false;
        let mut same_name = true;
        for &id in self.addresses.get(start..).ok_or_else(integrity)? {
            c.checkpoint(1)?;
            let d = self.definitions.get(id).ok_or_else(integrity)?;
            if d.address != Some(address) {
                break;
            }
            match definition {
                None => definition = Some(d),
                Some(previous) if d.input == input && previous.input != input => {
                    definition = Some(d);
                    ambiguous = false;
                    same_name = true;
                }
                Some(previous)
                    if previous.input != input && previous.symbol.object != d.symbol.object =>
                {
                    ambiguous = true;
                    same_name &= previous.name == d.name;
                }
                _ => {}
            }
        }
        // Equal addresses in different images do not select an input.
        let mut target = definition.map_or(
            CallTarget {
                address: Some(address),
                function: None,
                name: None,
            },
            |d| self.target(d),
        );
        if ambiguous {
            target.function = None;
            if !same_name {
                target.name = None;
            }
        }
        Ok(target)
    }

    fn target(&self, definition: &Definition) -> CallTarget {
        CallTarget {
            address: definition.address,
            function: definition.function.then(|| LibraryFunction {
                input: definition.input,
                symbol: definition.symbol.clone(),
                name: definition.name.clone(),
            }),
            name: definition
                .name
                .as_deref()
                .filter(|name| !name.is_empty())
                .and_then(|name| std::str::from_utf8(name).ok())
                .map(str::to_owned),
        }
    }

    fn at_section(
        &self,
        object: &ObjectId,
        section: u32,
        offset: u64,
        input: u64,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallTarget>> {
        for definition in self.definitions.iter() {
            c.checkpoint(1)?;
            if definition.input == input
                && &definition.symbol.object == object
                && definition.section == Some(section)
                && definition.offset == offset
            {
                return Ok(Some(self.target(definition)));
            }
        }
        Ok(None)
    }

    fn reference(
        &self,
        reference: &ReferenceTarget,
        addend: i64,
        input: u64,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallTarget>> {
        if reference.definition == SymbolDefinition::Absolute {
            return reference
                .offset
                .checked_add_signed(addend)
                .and_then(|a| u32::try_from(a).ok())
                .map(|a| self.at_address(a, input, c))
                .transpose();
        }
        c.checkpoint(lookup_work(self.definitions.len()))?;
        let start = self
            .definitions
            .partition_point(|d| d.symbol < reference.symbol);
        if let Some(d) = self
            .definitions
            .get(start)
            .filter(|d| d.symbol == reference.symbol)
        {
            if addend != 0 {
                if let Some(address) = d.address {
                    return self
                        .at_address(address.wrapping_add(addend as u32) & !1, input, c)
                        .map(Some);
                }
                return match (d.section, d.offset.checked_add_signed(addend)) {
                    (Some(section), Some(offset)) => {
                        self.at_section(&d.symbol.object, section, offset, d.input, c)
                    }
                    _ => Ok(None),
                };
            }
            return Ok(Some(self.target(d)));
        }
        if reference.definition == SymbolDefinition::Section
            && let (Some(section), Some(offset)) = (
                reference.section,
                reference.offset.checked_add_signed(addend),
            )
        {
            if let Some(target) =
                self.at_section(&reference.symbol.object, section, offset, input, c)?
            {
                return Ok(Some(target));
            }
            return Ok(Some(CallTarget {
                address: None,
                function: None,
                name: (addend == 0)
                    .then(|| {
                        std::str::from_utf8(&reference.name)
                            .ok()
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned)
                    })
                    .flatten(),
            }));
        }
        if addend != 0 {
            return Ok(None);
        }
        c.checkpoint(lookup_work(self.names.len()))?;
        let start = self.names.partition_point(|&id| {
            self.definitions
                .get(id)
                .is_some_and(|d| d.name.as_deref() < Some(reference.name.as_slice()))
        });
        let mut unique = None;
        for &id in self.names.get(start..).ok_or_else(integrity)? {
            let d = self.definitions.get(id).ok_or_else(integrity)?;
            if d.name.as_deref() != Some(reference.name.as_slice()) {
                break;
            }
            c.checkpoint(1)?;
            if unique.is_some_and(|previous: &Definition| {
                previous.symbol.object != d.symbol.object || previous.offset != d.offset
            }) {
                unique = None;
                break;
            }
            unique = unique.or(Some(d));
        }
        Ok(Some(unique.map_or_else(
            || {
                CallTarget {
                    address: None,
                    function: None,
                    name: std::str::from_utf8(&reference.name)
                        .ok()
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned),
                }
            },
            |d| self.target(d),
        )))
    }
}

struct Symbols<'c, 'm> {
    input: u64,
    catalog: &'c mut Callees<'m>,
    executable: AdmittedVec<'m, u32>,
}
impl ElfSink for Symbols<'_, '_> {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if r.flags & 4 != 0 && r.size != 0 {
            self.executable.push(r.index, c.position())?;
        }
        Ok(())
    }
    fn symbol(&mut self, r: &SymbolRecord, c: &mut dyn RunControl) -> Result<()> {
        c.checkpoint(1)?;
        let section = match r.raw_section {
            0xffff => r.extended_section,
            1..0xff00 => Some(u32::from(r.raw_section)),
            _ => None,
        };
        let function = r.symbol_type == 2 && section.is_some_and(|s| self.executable.contains(&s));
        let absolute = r.raw_section == 0xfff1;
        if !function && !absolute {
            return Ok(());
        }
        if let Some(name) = &r.name {
            self.catalog.ledger.admit(name.len(), c)?;
        }
        self.catalog.definitions.push(
            Definition {
                input: self.input,
                symbol: r.id.clone(),
                name: r.name.clone(),
                offset: r.value,
                section,
                address: absolute.then(|| u32::try_from(r.value).ok()).flatten(),
                function,
                binding: r.binding,
            },
            c.position(),
        )
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}

fn sort_work(len: usize) -> u64 {
    (len as u64).saturating_mul(lookup_work(len))
}
fn lookup_work(len: usize) -> u64 {
    u64::from(len.max(1).ilog2() + 1)
}
fn integrity() -> Error {
    Error::new(ErrorCode::Integrity, "invalid read callee index")
}

/// Per-function relocation sites point at the transfer (the second instruction
/// of an AUIPC/JALR call pair), including tail-call pairs.
pub(crate) struct CallSites<'f, 'a, 'm> {
    facts: &'f Facts<'a, 'm>,
    relocations: AdmittedVec<'m, (u64, &'a ReferenceTarget, i64)>,
    indirect: AdmittedVec<'m, (u64, u8, i32)>,
    direct: AdmittedVec<'m, (u64, u64)>,
    ready: bool,
}
impl<'f, 'a, 'm> CallSites<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            facts,
            relocations: AdmittedVec::new(memory),
            indirect: AdmittedVec::new(memory),
            direct: AdmittedVec::new(memory),
            ready: false,
        }
    }
    pub fn at(
        &mut self,
        site: u64,
        caller: &LibraryFunction,
        callees: &Callees<'_>,
        c: &mut dyn RunControl,
    ) -> Result<Option<CallTarget>> {
        let input = caller.input;
        if let Some(transfer) = self.facts.transfer(site, c)? {
            return match transfer.target {
                AbstractValue::ImageAddress { address }
                | AbstractValue::Constant { value: address } => {
                    callees.at_address(*address, input, c).map(Some)
                }
                AbstractValue::Symbol { symbol, addend } => {
                    match self.facts.reference(symbol, c)? {
                        Some(reference) => callees.reference(reference, *addend, input, c),
                        None => Ok(None),
                    }
                }
                _ => Ok(None),
            };
        }
        if !self.ready {
            for record in self.facts.records() {
                c.checkpoint(1)?;
                if let FunctionRecord::Edge {
                    from,
                    target: Some(target),
                    relation: EdgeKind::Call | EdgeKind::Jump,
                    ..
                } = record
                {
                    self.direct.push((*from, *target), c.position())?;
                }
                if let FunctionRecord::Instruction {
                    offset,
                    decoded:
                        DecodedOp {
                            flow:
                                InstructionFlow::Indirect {
                                    base,
                                    offset: displacement,
                                    ..
                                },
                            ..
                        },
                    ..
                } = record
                {
                    self.indirect
                        .push((*offset, *base, *displacement), c.position())?;
                }
                if let FunctionRecord::Reference {
                    raw,
                    reference_kind,
                    target,
                    addend: Some(addend),
                    known: true,
                    ..
                } = record
                {
                    let site = match reference_kind {
                        ReferenceKind::Call => raw.offset.checked_add(4),
                        ReferenceKind::Branch => Some(raw.offset),
                        _ => None,
                    };
                    if let Some(site) = site {
                        self.relocations
                            .push((site, target, *addend), c.position())?;
                    }
                }
            }
            c.checkpoint(
                sort_work(self.relocations.len())
                    + sort_work(self.indirect.len())
                    + sort_work(self.direct.len()),
            )?;
            self.relocations.sort_unstable_by_key(|r| r.0);
            self.indirect.sort_unstable_by_key(|r| r.0);
            self.direct.sort_unstable_by_key(|r| r.0);
            self.ready = true;
        }
        c.checkpoint(lookup_work(self.relocations.len()))?;
        let start = self.relocations.partition_point(|r| r.0 < site);
        if let Some((_, reference, addend)) = self.relocations.get(start).filter(|r| r.0 == site) {
            return callees.reference(reference, *addend, input, c);
        }
        c.checkpoint(lookup_work(self.direct.len()) + lookup_work(callees.definitions.len()))?;
        let start = self.direct.partition_point(|r| r.0 < site);
        if let Some((_, target)) = self.direct.get(start).filter(|r| r.0 == site) {
            let current = callees
                .definitions
                .partition_point(|d| d.symbol < caller.symbol);
            if let Some(definition) = callees
                .definitions
                .get(current)
                .filter(|d| d.symbol == caller.symbol)
                && let Some(section) = definition.section
            {
                return callees.at_section(&caller.symbol.object, section, *target, input, c);
            }
        }
        c.checkpoint(lookup_work(self.indirect.len()))?;
        let start = self.indirect.partition_point(|r| r.0 < site);
        let Some((_, base, displacement)) = self.indirect.get(start).filter(|r| r.0 == site) else {
            return Ok(None);
        };
        let Some(state) = self.facts.call_inputs(site, c)? else {
            return Ok(None);
        };
        match state.registers.get(usize::from(*base)) {
            Some(
                AbstractValue::Constant { value: address }
                | AbstractValue::ImageAddress { address },
            ) => callees
                .at_address(address.wrapping_add(*displacement as u32) & !1, input, c)
                .map(Some),
            Some(AbstractValue::Symbol { symbol, addend }) => {
                let Some(addend) = addend.checked_add(i64::from(*displacement)) else {
                    return Ok(None);
                };
                match self.facts.reference(symbol, c)? {
                    Some(reference) => callees.reference(reference, addend, input, c),
                    None => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rom_addresses_keep_a_common_name_without_selecting_an_image() {
        let memory = WorkingMemory::new(1 << 20).expect("working memory");
        {
            let mut callees =
                Callees::capture(&[], &memory, &mut || Ok(())).expect("empty catalog");
            for input in 0..2 {
                callees
                    .ledger
                    .admit(6, &mut || Ok(()))
                    .expect("name admission");
                callees
                    .definitions
                    .push(
                        Definition {
                            input,
                            symbol: SymbolId {
                                object: ObjectId {
                                    artifact: ArtifactId::of_bytes(&[input as u8]),
                                    location: ObjectLocation::Standalone,
                                },
                                table: SymbolTableKind::Static,
                                table_section: 1,
                                index: 1,
                            },
                            name: Some(b"printf".to_vec()),
                            offset: 0x4000,
                            section: Some(1),
                            address: Some(0x4000),
                            function: true,
                            binding: 1,
                        },
                        RunPosition::default(),
                    )
                    .expect("ROM definition");
                callees
                    .addresses
                    .push(input as usize, RunPosition::default())
                    .expect("address index");
            }
            let shared = callees
                .at_address(0x4000, 2, &mut || Ok(()))
                .expect("cross-image target");
            assert_eq!(shared.address, Some(0x4000));
            assert_eq!(shared.name.as_deref(), Some("printf"));
            assert_eq!(shared.function, None);
            let local = callees
                .at_address(0x4000, 1, &mut || Ok(()))
                .expect("same-image target");
            assert_eq!(local.function.expect("defining function").input, 1);
            callees.definitions.get_mut(1).expect("second ROM").name = Some(b"memcpy".to_vec());
            assert_eq!(
                callees
                    .at_address(0x4000, 2, &mut || Ok(()))
                    .expect("ambiguous target")
                    .name,
                None
            );
            assert_eq!(
                callees
                    .at_address(0x5000, 2, &mut || Ok(()))
                    .expect("unnamed target"),
                CallTarget {
                    address: Some(0x5000),
                    function: None,
                    name: None
                }
            );
        }
        assert_eq!(memory.used(), 0);
    }
}
