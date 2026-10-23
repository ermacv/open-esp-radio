//! One corpus producer for the shared result/argument port, in two analyses.
use super::*;
use crate::registers::{CallKey, CallLinks, CallSites, Callees, Calls};
use oer_analysis_blobray_analysis::registers::{ArgumentProgram, ArgumentSource};
use oer_analysis_blobray_artifacts::{MemberIndex, PreparedObject, prepare_captured};
use oer_analysis_elf::classify as elf;
use oer_analysis_riscv_analysis::{AnalysisSummary, PreparedReferences, ProvenTables};
use oer_analysis_riscv_program::{
    facts::ImageFacts,
    tables::{ImageTables, ObjectTables},
};
use std::collections::BTreeMap;

#[derive(Clone)]
struct RawConflict {
    origin: ArgumentSource,
    usage: ReadUse,
}
struct RawSummary<'m> {
    value: CallSummary,
    _memory: MemoryReservation<'m>,
}
struct OwnedSummary<'m> {
    value: CallSummary,
    raw: Option<Box<RawSummary<'m>>>,
    _arguments: MemoryReservation<'m>,
    _results: MemoryReservation<'m>,
}
struct Entry<'m> {
    function: Option<LibraryFunction>,
    object: Option<usize>,
    key: Option<CallKey>,
    summary: SummaryOutcome<OwnedSummary<'m>>,
    edges: AdmittedVec<'m, usize>,
    discovery: usize,
    low: usize,
    active: bool,
}
struct Group {
    members: Vec<usize>,
    remaining: usize,
    representative: Option<usize>,
    conflict: bool,
}
struct Scoped<'s> {
    table: &'s dyn CallSummaries,
    current: Option<CallSummary>,
}
/// Preserve the original physical return wiring while producing a summary.
/// The current function's header projection is added only after its audit.
struct UnshapedCurrent<'s>(&'s dyn CallSummaries);
impl CallSummaries for UnshapedCurrent<'_> {
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary> {
        self.0.summary(callee)
    }
}
impl CallSummaries for Scoped<'_> {
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary> {
        self.table.summary(callee)
    }
    fn current(&self) -> Option<&CallSummary> {
        self.current.as_ref()
    }
}
struct Summaries<'m> {
    contracts: BTreeMap<Vec<u8>, CallSummary>,
    contract_addresses: BTreeMap<u32, Option<Vec<u8>>>,
    entries: AdmittedVec<'m, Entry<'m>>,
    groups: Groups,
    ledger: Ledger<'m>,
}
#[derive(Default)]
struct Groups {
    addresses: BTreeMap<u32, Group>,
    symbols: BTreeMap<Vec<u8>, Group>,
}
impl Groups {
    fn get(&self, key: &CallKey) -> Option<&Group> {
        self.callee(key.borrowed())
    }
    fn get_mut(&mut self, key: &CallKey) -> Option<&mut Group> {
        match key {
            CallKey::Address(a) => self.addresses.get_mut(a),
            CallKey::Symbol(n) => self.symbols.get_mut(n.as_slice()),
        }
    }
    fn callee(&self, key: Callee<'_>) -> Option<&Group> {
        match key {
            Callee::Address(a) => self.addresses.get(&a),
            Callee::Symbol(n) => self.symbols.get(n),
        }
    }
    fn entry<'s>(
        &'s mut self,
        key: &CallKey,
        ledger: &mut Ledger<'_>,
        c: &mut dyn RunControl,
    ) -> Result<&'s mut Group> {
        let create = || Group {
            members: Vec::new(),
            remaining: 0,
            representative: None,
            conflict: false,
        };
        match key {
            CallKey::Address(a) => ledger.entry(&mut self.addresses, *a, create, c),
            CallKey::Symbol(n) => ledger.entry(&mut self.symbols, n.clone(), create, c),
        }
    }
}
impl Summaries<'_> {
    fn computed(&self, callee: Callee<'_>) -> Option<&OwnedSummary<'_>> {
        let group = self.groups.callee(callee)?;
        if group.remaining != 0 || group.conflict {
            return None;
        }
        self.entries.get(group.representative?)?.summary.available()
    }
}
impl CallSummaries for Summaries<'_> {
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary> {
        self.computed(callee)
            .map(|s| &s.value)
            .or_else(|| match callee {
                Callee::Symbol(name) => self.contracts.get(name),
                Callee::Address(address) => self
                    .contract_addresses
                    .get(&address)
                    .and_then(|n| n.as_deref())
                    .and_then(|n| self.contracts.get(n)),
            })
    }
}
/// The existing computed port without any signature fallback or projection.
struct Computed<'s, 'm>(&'s Summaries<'m>);
impl CallSummaries for Computed<'_, '_> {
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary> {
        self.0
            .computed(callee)
            .map(|s| s.raw.as_ref().map_or(&s.value, |r| &r.value))
    }
}

fn integrity() -> Error {
    Error::new(ErrorCode::Integrity, "invalid corpus summary graph")
}
impl<'m> Summaries<'m> {
    fn new(
        catalog: &Callees<'m>,
        signatures: Option<&crate::signatures::Signatures>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            contracts: BTreeMap::new(),
            contract_addresses: BTreeMap::new(),
            entries: AdmittedVec::new(memory),
            groups: Groups::default(),
            ledger: Ledger::new(memory, c)?,
        };
        if let Some(signatures) = signatures {
            for signature in signatures.functions() {
                c.checkpoint(1)?;
                let name = signature.name.as_bytes().to_vec();
                out.ledger.admit(name.capacity(), c)?;
                *out.ledger
                    .entry(&mut out.contracts, name, || signature.summary(), c)? =
                    signature.summary();
            }
            for (address, name) in catalog.named_addresses() {
                c.checkpoint(1)?;
                let slot = out.ledger.entry(
                    &mut out.contract_addresses,
                    address,
                    || Some(name.to_vec()),
                    c,
                )?;
                if slot.as_deref().is_some_and(|n| n != name) {
                    *slot = None;
                }
                out.ledger.admit(name.len(), c)?;
            }
        }
        for id in 0..catalog.len() {
            c.checkpoint(1)?;
            let definition = catalog.definition(id);
            let (function, key) = definition.map_or((None, None), |(f, k)| (Some(f), k));
            // Only ET_EXEC definitions have address keys. A captured linked body
            // must never acquire a header fallback, including through a name or alias.
            if let Some(CallKey::Address(address)) = &key {
                if let Some(name) = function.as_ref().and_then(|f| f.name.as_deref()) {
                    out.contracts.remove(name);
                }
                if let Some(name) = out.contract_addresses.get_mut(address) {
                    *name = None;
                }
            }
            if let Some(f) = &function {
                out.ledger.admit(
                    f.symbol.object.artifact.allocated_bytes() as usize
                        + f.name.as_ref().map_or(0, Vec::capacity),
                    c,
                )?;
            }
            if let Some(key) = &key {
                if let CallKey::Symbol(n) = key {
                    out.ledger.admit(n.capacity() * 2, c)?;
                }
                let group = out.groups.entry(key, &mut out.ledger, c)?;
                out.ledger.push(&mut group.members, id, c)?;
                group.remaining += 1;
            }
            out.entries.push(
                Entry {
                    function,
                    object: None,
                    key,
                    summary: SummaryOutcome::NoProof,
                    edges: AdmittedVec::new(memory),
                    discovery: usize::MAX,
                    low: 0,
                    active: false,
                },
                c.position(),
            )?;
        }
        // Absolute aliases at a captured body's address follow the same rule
        // when the value consumer names the symbol rather than its address.
        for (address, name) in catalog.named_addresses() {
            c.checkpoint(1)?;
            if out.groups.addresses.contains_key(&address) {
                out.contracts.remove(name);
            }
        }
        Ok(out)
    }
    fn entry(&self, id: usize) -> Result<&Entry<'m>> {
        self.entries.get(id).ok_or_else(integrity)
    }
    fn entry_mut(&mut self, id: usize) -> Result<&mut Entry<'m>> {
        self.entries.get_mut(id).ok_or_else(integrity)
    }
    fn finish(&mut self, id: usize, summary: SummaryOutcome<OwnedSummary<'m>>) -> Result<()> {
        self.entry_mut(id)?.summary = summary;
        let Some(key) = self.entries.get(id).ok_or_else(integrity)?.key.as_ref() else {
            return Ok(());
        };
        let group = self.groups.get(key).ok_or_else(integrity)?;
        let representative = group.representative;
        let same = match (
            representative
                .and_then(|r| self.entries.get(r))
                .and_then(|r| r.summary.available()),
            self.entry(id)?.summary.available(),
        ) {
            (Some(a), Some(b)) => a.value == b.value,
            _ => false,
        };
        let group = self.groups.get_mut(key).ok_or_else(integrity)?;
        group.remaining = group.remaining.checked_sub(1).ok_or_else(integrity)?;
        if self
            .entries
            .get(id)
            .and_then(|e| e.summary.available())
            .is_none()
            || representative.is_some() && !same
        {
            group.conflict = true;
        }
        group.representative = representative.or(Some(id));
        Ok(())
    }
}

struct Object<'a> {
    prepared: PreparedObject<'a, 'a>,
    references: AdmittedVec<'a, (u32, PreparedReferences<'a>)>,
    object_tables: Option<ObjectTables<'a>>,
    image_facts: Option<ImageFacts<'a>>,
    payload: ArtifactId,
    input: u64,
    safe: bool,
    linked: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze<'a>(
    inputs: &'a [Executable],
    assumptions: Assumptions<'_>,
    catalog: &Callees<'a>,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    let mut table = Summaries::new(catalog, assumptions.signatures, memory, c)?;
    let mut objects = AdmittedVec::new(memory);
    // Preparation is structural; each function is analyzed exactly once here
    // without summaries, and once below with its completed callee contracts.
    for (input, executable) in inputs.iter().enumerate() {
        let input = input as u64;
        let source = executable.bytes();
        let index = match MemberIndex::new(&source, memory, c) {
            Ok(index) => Some(index),
            Err(e) if e.code == ErrorCode::Integrity => None,
            Err(e) => return Err(e),
        };
        let container = visit_members(executable, memory, c, &mut |member, c| {
            let object = member.id;
            let mut selection = Selection::default();
            let (payload, elf) = match member.bytes {
                Ok(bytes) => {
                    let (payload, elf) = inspect_source(bytes, &object, memory, c, &mut selection)?;
                    (Some(payload), elf)
                }
                Err(d) => {
                    selection.diagnostics.push(d);
                    (None, None)
                }
            };
            let mut safe = true;
            let mut gap = |reason: &str, c: &mut dyn RunControl| {
                safe = false;
                visit(
                    LibraryOutcome::Gap {
                        input,
                        object: Some(&object),
                        reason,
                    },
                    c,
                )
            };
            if payload.is_none() {
                gap("object payload unavailable", c)?;
            }
            if !elf.as_ref().is_some_and(|e| {
                elf::is_riscv32_little_endian(e.bits, e.little_endian, e.machine)
                    && matches!(
                        elf::object_kind(e.object_type),
                        elf::ObjectKind::Relocatable | elf::ObjectKind::Executable
                    )
            }) {
                gap(
                    "object is not a supported ELF32 little-endian RISC-V relocatable or executable",
                    c,
                )?;
            }
            for d in &selection.diagnostics {
                gap(&format!("{:?}: {}: {}", d.code, d.context, d.message), c)?;
            }
            if selection.functions.is_empty()
                && (!selection.executable.is_empty()
                    || elf.as_ref().is_some_and(|e| {
                        elf::object_kind(e.object_type) == elf::ObjectKind::Executable
                    }))
            {
                gap("executable sections have no selected function symbols", c)?;
            }
            let Some(payload) = payload else {
                return Ok(());
            };
            if selection.functions.is_empty() {
                return Ok(());
            }
            let ordinal = match object.location {
                ObjectLocation::Standalone => 0,
                ObjectLocation::ArchiveMember { ordinal } => ordinal,
            };
            let Some((offset, length)) = index.as_ref().ok_or_else(integrity)?.get(ordinal)? else {
                return Ok(());
            };
            let end = offset.checked_add(length).ok_or_else(integrity)?;
            let bytes = source
                .get(
                    usize::try_from(offset).map_err(|_| integrity())?
                        ..usize::try_from(end).map_err(|_| integrity())?,
                )
                .ok_or_else(integrity)?;
            let prepared = match prepare_captured(bytes, &payload, assumptions.read_only, memory, c)
            {
                Ok(p) => p,
                Err(e) if blocks_function(&e) => {
                    for f in &selection.functions {
                        let function = LibraryFunction {
                            input,
                            symbol: f.id.clone(),
                            name: f.name.clone(),
                        };
                        visit(
                            LibraryOutcome::Blocked {
                                function: &function,
                                error: &e,
                            },
                            c,
                        )?;
                    }
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            let mut object_tables = None;
            let mut image_facts = None;
            let facts_result = match elf.as_ref().map(|e| elf::object_kind(e.object_type)) {
                Some(elf::ObjectKind::Relocatable) => {
                    ObjectTables::new(bytes, memory, c).map(|t| object_tables = Some(t))
                }
                Some(elf::ObjectKind::Executable) => {
                    ImageFacts::new(bytes, assumptions.read_only, memory, c)
                        .map(|t| image_facts = Some(t))
                }
                _ => Ok(()),
            };
            if let Err(e) = facts_result {
                if e.code != ErrorCode::Integrity {
                    return Err(e);
                }
                gap(&format!("jump-table facts unreadable: {}", e.message), c)?;
            }
            let object_id = objects.len();
            objects.push(
                Object {
                    prepared,
                    references: AdmittedVec::new(memory),
                    object_tables,
                    image_facts,
                    payload,
                    input,
                    safe,
                    linked: elf.as_ref().is_some_and(|e| {
                        elf::object_kind(e.object_type) == elf::ObjectKind::Executable
                    }),
                },
                c.position(),
            )?;
            for selected in &selection.functions {
                let function = LibraryFunction {
                    input,
                    symbol: selected.id.clone(),
                    name: selected.name.clone(),
                };
                let Some(id) = catalog.function_id(&function, c)? else {
                    return Err(integrity());
                };
                table.entry_mut(id)?.object = Some(object_id);
                let records = match research(
                    &mut objects,
                    object_id,
                    &function,
                    assumptions,
                    &NoSummaries,
                    decoder,
                    memory,
                    c,
                ) {
                    Ok((records, _, _, _)) => records,
                    Err(e) if blocks_function(&e) => {
                        table.entry_mut(id)?.object = None;
                        visit(
                            LibraryOutcome::Blocked {
                                function: &function,
                                error: &e,
                            },
                            c,
                        )?;
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let facts = function_facts(&function, &records, memory, c)?;
                let links = CallLinks::new(&facts, &function, catalog, memory, c)?;
                let mut edges = AdmittedVec::new(memory);
                for key in links.keys() {
                    c.checkpoint(1)?;
                    if let Some(group) = table.groups.get(key) {
                        for target in group.members.iter().copied() {
                            edges.push(target, c.position())?;
                        }
                    }
                }
                c.checkpoint(edges.len() as u64 * u64::from(edges.len().max(1).ilog2() + 1))?;
                edges.sort_unstable(); // repeated edges are harmless and admitted
                table.entry_mut(id)?.edges = edges;
            }
            Ok(())
        })?;
        for reason in [
            container
                .framing
                .is_some()
                .then_some("archive member enumeration incomplete"),
            (container.kind == ContainerKind::Unsupported).then_some("unsupported input format"),
        ]
        .into_iter()
        .flatten()
        {
            visit(
                LibraryOutcome::Gap {
                    input,
                    object: None,
                    reason,
                },
                c,
            )?;
        }
        if container.framing.is_some() || container.kind == ContainerKind::Unsupported {
            for object in objects.iter_mut() {
                if object.input == input {
                    object.safe = false;
                }
            }
        }
    }
    // Tarjan emits components in callee-first order. Recursive components are
    // analyzed for final records, but no member publishes a narrowing summary.
    let mut stack = AdmittedVec::new(memory);
    let mut frames = AdmittedVec::new(memory);
    let mut component = AdmittedVec::new(memory);
    let mut sequence = 0;
    for root in 0..table.entries.len() {
        if table.entry(root)?.discovery != usize::MAX {
            continue;
        }
        frames.push((root, 0usize), c.position())?;
        while let Some((id, cursor)) = frames.last().copied() {
            c.checkpoint(1)?;
            if table.entry(id)?.discovery == usize::MAX {
                let entry = table.entry_mut(id)?;
                entry.discovery = sequence;
                entry.low = sequence;
                entry.active = true;
                sequence += 1;
                stack.push(id, c.position())?;
            }
            if let Some(target) = table.entry(id)?.edges.get(cursor).copied() {
                frames.last_mut().ok_or_else(integrity)?.1 += 1;
                if table.entry(target)?.discovery == usize::MAX {
                    frames.push((target, 0), c.position())?;
                } else if table.entry(target)?.active {
                    table.entry_mut(id)?.low =
                        table.entry(id)?.low.min(table.entry(target)?.discovery);
                }
                continue;
            }
            frames.pop();
            if let Some((parent, _)) = frames.last().copied() {
                table.entry_mut(parent)?.low = table.entry(parent)?.low.min(table.entry(id)?.low);
            }
            if table.entry(id)?.low != table.entry(id)?.discovery {
                continue;
            }
            component.truncate(0);
            loop {
                let member = stack.pop().ok_or_else(integrity)?;
                table.entry_mut(member)?.active = false;
                component.push(member, c.position())?;
                if member == id {
                    break;
                }
            }
            let recursive = component.len() > 1 || table.entry(id)?.edges.contains(&id);
            for member in component.iter().copied() {
                let object = table.entry(member)?.object;
                let mut contract = SummaryOutcome::NoProof;
                let mut disable_contract = false;
                if let (Some(object), Some(function)) =
                    (object, table.entry(member)?.function.as_ref())
                {
                    let signature = assumptions
                        .signatures
                        .and_then(|s| function.name.as_deref().and_then(|n| s.get(n)));
                    let linked = objects.get(object).ok_or_else(integrity)?.linked;
                    let applied_signature = signature.filter(|_| !linked);
                    let rom_signature = signature.filter(|_| linked);
                    let computed = Computed(&table);
                    let mut scoped = Scoped {
                        table: &table,
                        current: applied_signature.map(|s| s.summary()),
                    };
                    match research(
                        &mut objects,
                        object,
                        function,
                        assumptions,
                        &scoped,
                        decoder,
                        memory,
                        c,
                    ) {
                        Ok((mut records, mut summary, mut found, mut expression_budget)) => {
                            let mut conflicts = AdmittedVec::new(memory);
                            let mut rom_diagnostics = AdmittedVec::new(memory);
                            let mut conflict_names = Ledger::new(memory, c)?;
                            let mut raw_conflicts = AdmittedVec::new(memory);
                            let mut applied_audit = false;
                            let mut entry_complete = None;
                            let mut audit_outcome = None;
                            let publish = expression_budget.is_none()
                                && !recursive
                                && objects.get(object).ok_or_else(integrity)?.safe
                                && eligible(summary.coverage, summary.semantics, &records, c)?;
                            if publish || assumptions.signatures.is_some() {
                                let facts = function_facts(function, &records, memory, c)?;
                                let links = CallLinks::new(&facts, function, catalog, memory, c)?;
                                let entry_required =
                                    applied_signature.is_some_and(|s| s.arguments < 8);
                                let results_required = needs_result_audit(&scoped, &links);
                                applied_audit = entry_required || results_required;
                                contract = if publish {
                                    produce(
                                        &facts,
                                        &records,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut entry_complete,
                                        c,
                                    )?
                                } else if entry_required || rom_signature.is_some() {
                                    let _ = argument_summary(
                                        &facts,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut entry_complete,
                                        c,
                                    )?;
                                    SummaryOutcome::NoProof
                                } else {
                                    SummaryOutcome::NoProof
                                };
                                let audit_entries = entry_required
                                    && entry_complete != Some(SummaryOutcome::Available(()));
                                if results_required || audit_entries {
                                    let entries = if audit_entries {
                                        (0x3fc00u32)
                                            & (u32::MAX
                                                << (10
                                                    + applied_signature.map_or(8, |s| s.arguments)))
                                    } else {
                                        0
                                    };
                                    match audit(
                                        &facts,
                                        &scoped,
                                        &links,
                                        entries,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        c,
                                    ) {
                                        Ok(SummaryOutcome::Available(())) => {
                                            audit_outcome = Some(SummaryOutcome::Available(()));
                                        }
                                        Ok(SummaryOutcome::LocalBudgetExceeded(reason)) => {
                                            audit_outcome =
                                                Some(SummaryOutcome::LocalBudgetExceeded(reason));
                                        }
                                        Ok(SummaryOutcome::NoProof) => {
                                            return Err(Error::new(
                                                ErrorCode::Unavailable,
                                                "audit has no fixed point",
                                            ));
                                        }
                                        Err(e) => return Err(e),
                                    }
                                } else if entry_required {
                                    audit_outcome = entry_complete;
                                }
                            }
                            // A failed function cannot publish or observe a signature projection.
                            // All audit recipes and states above have been dropped before this retry.
                            if audit_outcome.is_some_and(|s| s != SummaryOutcome::Available(())) {
                                disable_contract = true;
                                contract = SummaryOutcome::NoProof;
                                scoped.current = None;
                                scoped.table = &computed;
                                drop(records);
                                (records, summary, found, expression_budget) = research(
                                    &mut objects,
                                    object,
                                    function,
                                    assumptions,
                                    &scoped,
                                    decoder,
                                    memory,
                                    c,
                                )?;
                                if publish && expression_budget.is_none() {
                                    let facts = function_facts(function, &records, memory, c)?;
                                    let links =
                                        CallLinks::new(&facts, function, catalog, memory, c)?;
                                    contract = produce(
                                        &facts,
                                        &records,
                                        &scoped,
                                        None,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut None,
                                        c,
                                    )?;
                                }
                            }
                            {
                                let facts = function_facts(function, &records, memory, c)?;
                                let mut sites = CallSites::new(&facts, memory);
                                for raw in raw_conflicts.iter() {
                                    c.checkpoint(1)?;
                                    let (origin, target) = match raw.origin {
                                        ArgumentSource::Entry(register) => {
                                            (SignatureOrigin::Argument { register }, None)
                                        }
                                        ArgumentSource::MissingResult { site, register } => {
                                            let target = sites.at(site, function, catalog, c)?;
                                            (
                                                SignatureOrigin::Result {
                                                    site,
                                                    register,
                                                    callee: target.clone(),
                                                },
                                                target,
                                            )
                                        }
                                    };
                                    let selected = match &origin {
                                        SignatureOrigin::Argument { .. } => signature,
                                        SignatureOrigin::Result { .. } => target
                                            .as_ref()
                                            .and_then(|t| t.name.as_deref())
                                            .and_then(|name| {
                                                assumptions
                                                    .signatures
                                                    .and_then(|s| s.get(name.as_bytes()))
                                            }),
                                    };
                                    let mut usage = raw.usage.clone();
                                    if let ReadSink::CallArgument { site, callee, .. } =
                                        &mut usage.sink
                                    {
                                        *callee = sites.at(*site, function, catalog, c)?;
                                    }
                                    if let (
                                        Some(signature),
                                        SignatureOrigin::Argument { register },
                                    ) = (rom_signature, &origin)
                                    {
                                        let diagnostic = RomSignatureDiagnostic {
                                            function: function.clone(),
                                            signature: signature.name.clone(),
                                            declaration: signature.source.clone(),
                                            register: *register,
                                            usage,
                                        };
                                        conflict_names.admit(
                                            serde_json::to_vec(&diagnostic)
                                                .map_err(|e| {
                                                    Error::new(ErrorCode::Integrity, e.to_string())
                                                })?
                                                .len(),
                                            c,
                                        )?;
                                        rom_diagnostics.push(diagnostic, c.position())?;
                                        continue;
                                    }
                                    let conflict = SignatureConflict {
                                        function: function.clone(),
                                        signature: selected
                                            .map(|s| s.name.clone())
                                            .or_else(|| {
                                                target.as_ref().and_then(|t| t.name.clone())
                                            })
                                            .unwrap_or_else(|| "unresolved signature".into()),
                                        declaration: selected.map(|s| s.source.clone()),
                                        origin,
                                        usage,
                                    };
                                    // Output ownership is separate; retain admitted copies through this visit.
                                    conflict_names.admit(
                                        serde_json::to_vec(&conflict)
                                            .map_err(|e| {
                                                Error::new(ErrorCode::Integrity, e.to_string())
                                            })?
                                            .len(),
                                        c,
                                    )?;
                                    conflicts.push(conflict, c.position())?;
                                }
                            }
                            if let (Some(signature), Some(summary)) = (
                                applied_signature.filter(|_| {
                                    audit_outcome.is_none_or(|s| s == SummaryOutcome::Available(()))
                                }),
                                contract.available_mut(),
                            ) {
                                let bytes = summary.value.arguments.returned.capacity()
                                    * std::mem::size_of::<ReturnedArgument>()
                                    + summary
                                        .value
                                        .results
                                        .iter()
                                        .flatten()
                                        .map(|v| v.capacity() * std::mem::size_of::<u32>())
                                        .sum::<usize>();
                                summary.raw = Some(Box::new(RawSummary {
                                    _memory: memory.reserve(
                                        (bytes + std::mem::size_of::<RawSummary<'_>>()) as u64,
                                        c.position(),
                                    )?,
                                    value: summary.value.clone(),
                                }));
                                for (word, consumed) in summary
                                    .value
                                    .arguments
                                    .consumed
                                    .iter_mut()
                                    .enumerate()
                                    .skip(usize::from(signature.arguments))
                                {
                                    if let Some(bits) = summary.value.narrowed_may.get_mut(word) {
                                        *bits = consumed.may;
                                    }
                                    consumed.may = 0;
                                }
                                let shape = signature.summary().contract;
                                summary.value.contract = shape;
                                for register in [10, 11] {
                                    if !summary.value.returns(register)
                                        && let Some(result) = summary
                                            .value
                                            .results
                                            .get_mut(usize::from(register - 10))
                                    {
                                        *result = None;
                                    }
                                }
                                summary.value.arguments.returned.retain(|r| {
                                    shape.is_none_or(|s| {
                                        r.register < 10 + s.results && r.argument < 10 + s.arguments
                                    })
                                });
                            }
                            if let Some(reason) = expression_budget {
                                contract = SummaryOutcome::LocalBudgetExceeded(reason);
                            }
                            let analyzed = AnalyzedFunction {
                                function,
                                records: &records,
                                coverage: summary.coverage,
                                semantics: summary.semantics,
                                jump_tables: &found.tables,
                                summaries: &scoped,
                                conflicts: &conflicts,
                                rom_signature_diagnostics: &rom_diagnostics,
                                summary_outcome: Some(contract.status()),
                                rom_signature_summary_outcome: rom_signature.and(entry_complete),
                                signature_audit_outcome: audit_outcome.filter(|_| applied_audit),
                            };
                            visit(LibraryOutcome::Analyzed(analyzed), c)?;
                        }
                        Err(e) if blocks_function(&e) => visit(
                            LibraryOutcome::Blocked {
                                function,
                                error: &e,
                            },
                            c,
                        )?,
                        Err(e) => return Err(e),
                    }
                }
                if disable_contract
                    && let Some(name) = table
                        .entry(member)?
                        .function
                        .as_ref()
                        .and_then(|f| f.name.as_deref())
                {
                    let name = name.to_vec();
                    table.contracts.remove(name.as_slice());
                }
                table.finish(member, contract)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn research<'a>(
    objects: &mut AdmittedVec<'a, Object<'a>>,
    object: usize,
    function: &LibraryFunction,
    assumptions: Assumptions<'_>,
    summaries: &dyn CallSummaries,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<(
    RecordBuffer<'a>,
    AnalysisSummary,
    ProvenTables<'a, JumpDispatch>,
    Option<LocalBudgetExceeded>,
)> {
    let object = objects.get_mut(object).ok_or_else(integrity)?;
    let image_tables = object
        .image_facts
        .as_ref()
        .map(|image| ImageTables { image });
    let tables: &dyn TableEntries = match (&object.object_tables, &image_tables) {
        (Some(t), _) => t,
        (None, Some(t)) => t,
        _ => &NoTables,
    };
    let mut records = RecordBuffer::new(memory);
    let mut sink = SummaryRecords {
        records: Records(&mut records),
        expressions: 0,
        exhausted: false,
    };
    let (summary, found) = research_function(
        &mut object.prepared,
        &mut object.references,
        &function.symbol,
        function.input,
        &object.payload,
        assumptions.abi,
        summaries,
        tables,
        decoder,
        memory,
        c,
        &mut sink,
    )?;
    let budget = sink.expression_budget();
    Ok((records, summary, found, budget))
}
/// Capture expression-limit provenance in the existing charged record visit.
/// Counting interned records gives the exhausted capacity without duplicating
/// the pool's policy formula or scanning an ineligible function again.
struct SummaryRecords<'a, 'm> {
    records: Records<'a, 'm>,
    expressions: u64,
    exhausted: bool,
}
impl SummaryRecords<'_, '_> {
    fn expression_budget(&self) -> Option<LocalBudgetExceeded> {
        self.exhausted.then_some(LocalBudgetExceeded {
            limit: self.expressions,
            scope: LocalBudgetScope::FunctionExpressions,
        })
    }
}
impl FunctionSink for SummaryRecords<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.records.record(record, c)?;
        match record {
            FunctionRecord::Expression { .. } => self.expressions += 1,
            FunctionRecord::SemanticGap {
                reason: SemanticGapReason::ExpressionLimit,
                ..
            } => {
                self.exhausted = true;
            }
            _ => {}
        }
        Ok(())
    }
}

fn eligible(
    coverage: FunctionCoverage,
    semantics: SemanticSummary,
    records: &[FunctionRecord],
    c: &mut dyn RunControl,
) -> Result<bool> {
    if !coverage.complete() {
        return Ok(false);
    }
    let mut explained = semantics.complete;
    for record in records {
        c.checkpoint(1)?;
        match record {
            FunctionRecord::Gap { .. } => return Ok(false),
            FunctionRecord::SemanticGap { reason, .. } => match reason {
                SemanticGapReason::OpaqueCall | SemanticGapReason::AlternativeLimit => {
                    explained = true
                }
                _ => return Ok(false),
            },
            _ => {}
        }
    }
    Ok(explained)
}
#[allow(clippy::too_many_arguments)]
fn produce<'a>(
    facts: &Facts<'_, 'a>,
    records: &[FunctionRecord],
    summaries: &dyn CallSummaries,
    rom_signature: Option<&crate::signatures::FunctionSignature>,
    links: &CallLinks<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    conflicts: &mut AdmittedVec<'a, RawConflict>,
    audit_complete: &mut Option<SummaryOutcome<()>>,
    c: &mut dyn RunControl,
) -> Result<SummaryOutcome<OwnedSummary<'a>>> {
    let (arguments, argument_memory) = match argument_summary(
        facts,
        summaries,
        rom_signature,
        links,
        decoder,
        memory,
        conflicts,
        audit_complete,
        c,
    )? {
        SummaryOutcome::Available(value) => value,
        SummaryOutcome::NoProof => return Ok(SummaryOutcome::NoProof),
        SummaryOutcome::LocalBudgetExceeded(reason) => {
            return Ok(SummaryOutcome::LocalBudgetExceeded(reason));
        }
    };
    let constants = returned_constants(records, summaries, links, c)?;
    let mut ledger = Ledger::new(memory, c)?;
    let mut results = [None, None];
    for (slot, values) in results.iter_mut().zip(constants) {
        if let Some(values) = values.filter(|v| !v.is_empty() && v.len() <= MAX_VALUE_ALTERNATIVES)
        {
            let mut output = Vec::new();
            for value in values {
                ledger.push(&mut output, value, c)?;
            }
            *slot = Some(output);
        }
    }
    Ok(SummaryOutcome::Available(OwnedSummary {
        value: CallSummary {
            results,
            arguments,
            ..CallSummary::default()
        },
        raw: None,
        _arguments: argument_memory,
        _results: ledger.into_reservation(),
    }))
}

/// Audit uses the summary interpreter's fixed-point events. It never promotes
/// May to Exact, and stack spill/reload keeps its normal non-sink semantics.
#[allow(clippy::too_many_arguments)] // One audit of the shared summary interpreter.
fn argument_summary<'a>(
    facts: &Facts<'_, 'a>,
    summaries: &dyn CallSummaries,
    rom_signature: Option<&crate::signatures::FunctionSignature>,
    links: &CallLinks<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    conflicts: &mut AdmittedVec<'a, RawConflict>,
    audit_complete: &mut Option<SummaryOutcome<()>>,
    c: &mut dyn RunControl,
) -> Result<SummaryOutcome<(ArgumentSummary, MemoryReservation<'a>)>> {
    let audit = rom_signature.is_some()
        || summaries
            .current()
            .and_then(|s| s.contract)
            .is_some_and(|s| s.arguments < 8);
    if audit {
        *audit_complete = Some(SummaryOutcome::NoProof);
    }
    let program = ArgumentProgram::capture(facts, decoder, memory, c)?;
    let unshaped = UnshapedCurrent(summaries);
    let calls = Calls {
        links,
        summaries: &unshaped,
    };
    if !audit {
        return program.summarize(&calls, memory, c);
    }
    let mut observe = |origin, usage: &ReadUse, c: &mut dyn RunControl| {
        let conflict = match origin {
            ArgumentSource::Entry(register) => {
                rom_signature.is_some_and(|s| register >= 10 + s.arguments)
                    || usage.precision == ReadPrecision::Exact
                        && summaries
                            .current()
                            .and_then(|s| s.contract)
                            .is_some_and(|s| register >= 10 + s.arguments)
            }
            ArgumentSource::MissingResult { .. } => usage.precision == ReadPrecision::Exact,
        };
        if conflict {
            record_conflict(conflicts, origin, usage, c)?;
        }
        Ok(())
    };
    let result = program.summarize_observed(&calls, memory, c, &mut observe)?;
    *audit_complete = Some(result.status());
    Ok(result)
}
#[allow(clippy::too_many_arguments)]
fn audit<'a>(
    facts: &Facts<'_, 'a>,
    summaries: &dyn CallSummaries,
    links: &CallLinks<'_>,
    entries: u32,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    conflicts: &mut AdmittedVec<'a, RawConflict>,
    c: &mut dyn RunControl,
) -> Result<SummaryOutcome<()>> {
    let program = ArgumentProgram::capture(facts, decoder, memory, c)?;
    let calls = Calls { links, summaries };
    let mut observe = |origin, usage: &ReadUse, c: &mut dyn RunControl| {
        if usage.precision == ReadPrecision::Exact {
            record_conflict(conflicts, origin, usage, c)?;
        }
        Ok(())
    };
    Ok(program
        .audit(&calls, memory, c, &mut observe, entries)?
        .status())
}
fn record_conflict(
    conflicts: &mut AdmittedVec<'_, RawConflict>,
    origin: ArgumentSource,
    usage: &ReadUse,
    c: &mut dyn RunControl,
) -> Result<()> {
    if !conflicts
        .iter()
        .any(|r| r.origin == origin && r.usage == *usage)
    {
        conflicts.push(
            RawConflict {
                origin,
                usage: usage.clone(),
            },
            c.position(),
        )?;
    }
    Ok(())
}

fn needs_result_audit(summaries: &dyn CallSummaries, links: &CallLinks<'_>) -> bool {
    links.keys().any(|key| {
        summaries
            .summary(key.borrowed())
            .and_then(|s| s.contract)
            .is_some_and(|s| s.results < 2)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_opaque_calls_and_alternative_limits_allow_partial_summaries() {
        let coverage = FunctionCoverage::default();
        let semantics = SemanticSummary {
            complete: false,
            ..Default::default()
        };
        for reason in [
            SemanticGapReason::OpaqueCall,
            SemanticGapReason::AlternativeLimit,
        ] {
            assert!(
                eligible(
                    coverage,
                    semantics,
                    &[FunctionRecord::SemanticGap { offset: 0, reason }],
                    &mut || Ok(())
                )
                .expect("allowed partial")
            );
        }
        for reason in [
            SemanticGapReason::UnsupportedInstruction,
            SemanticGapReason::WaitMemory,
            SemanticGapReason::UnexpandedControlFlow,
            SemanticGapReason::ConflictingBoundary,
            SemanticGapReason::UnresolvedRelocation,
            SemanticGapReason::ExpressionLimit,
        ] {
            let records = [
                FunctionRecord::SemanticGap {
                    offset: 0,
                    reason: SemanticGapReason::OpaqueCall,
                },
                FunctionRecord::SemanticGap { offset: 4, reason },
            ];
            assert!(
                !eligible(coverage, semantics, &records, &mut || Ok(())).expect("rejected partial")
            );
        }
        assert!(!eligible(coverage, semantics, &[], &mut || Ok(())).expect("unexplained partial"));
        assert!(
            !eligible(
                FunctionCoverage {
                    control_flow: false,
                    ..coverage
                },
                SemanticSummary {
                    complete: true,
                    ..semantics
                },
                &[],
                &mut || Ok(())
            )
            .expect("incomplete CFG")
        );
    }
    #[test]
    fn expression_limit_keeps_its_budget_even_with_other_gaps() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut records = RecordBuffer::new(&memory);
        let mut sink = SummaryRecords {
            records: Records(&mut records),
            expressions: 0,
            exhausted: false,
        };
        sink.record(
            &FunctionRecord::SemanticGap {
                offset: 0,
                reason: SemanticGapReason::UnsupportedInstruction,
            },
            &mut || Ok(()),
        )
        .unwrap();
        for id in 0..3 {
            sink.record(
                &FunctionRecord::Expression {
                    id,
                    offset: u64::MAX,
                    expression: Expression::EntryRegister {
                        register: id as u8 + 1,
                    },
                },
                &mut || Ok(()),
            )
            .unwrap();
        }
        assert_eq!(sink.expression_budget(), None);
        sink.record(
            &FunctionRecord::SemanticGap {
                offset: 0,
                reason: SemanticGapReason::ExpressionLimit,
            },
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(
            sink.expression_budget(),
            Some(LocalBudgetExceeded {
                limit: 3,
                scope: LocalBudgetScope::FunctionExpressions
            })
        );
        assert_eq!(
            sink.record(
                &FunctionRecord::SemanticGap {
                    offset: 0,
                    reason: SemanticGapReason::ExpressionLimit
                },
                &mut || Err(Error::new(ErrorCode::Cancelled, "caller cancellation"))
            )
            .unwrap_err()
            .code,
            ErrorCode::Cancelled
        );
        assert!(
            !eligible(
                FunctionCoverage::default(),
                SemanticSummary::default(),
                &records,
                &mut || Ok(())
            )
            .unwrap()
        );
        let mut other = RecordBuffer::new(&memory);
        let sink = SummaryRecords {
            records: Records(&mut other),
            expressions: 0,
            exhausted: false,
        };
        assert_eq!(
            sink.expression_budget(),
            None,
            "the other function has its own pool"
        );
    }

    fn returning_records() -> Vec<FunctionRecord> {
        vec![FunctionRecord::Instruction {
            offset: 0,
            bytes: 0x00008067u32.to_le_bytes().to_vec(),
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Return { base: 1 },
            },
        }]
    }

    fn run_producer(
        records: &[FunctionRecord],
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
        auditing: bool,
    ) -> Result<SummaryOutcome<()>> {
        let mut setup = || Ok(());
        let facts = Facts::new(records, memory, &mut setup)?;
        let catalog = Callees::capture(&[], memory, &mut setup)?;
        let function = LibraryFunction {
            input: 0,
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"summary fixture"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 1,
                index: 1,
            },
            name: None,
        };
        let links = CallLinks::new(&facts, &function, &catalog, memory, &mut setup)?;
        let mut conflicts = AdmittedVec::new(memory);
        let decoder = oer_analysis_riscv_lift::RiscvDecoder;
        if auditing {
            audit(
                &facts,
                &NoSummaries,
                &links,
                u32::MAX,
                &decoder,
                memory,
                &mut conflicts,
                c,
            )
        } else {
            Ok(produce(
                &facts,
                records,
                &NoSummaries,
                None,
                &links,
                &decoder,
                memory,
                &mut conflicts,
                &mut None,
                c,
            )?
            .status())
        }
    }

    #[test]
    fn producer_and_audit_preserve_every_parent_checkpoint_failure() {
        let records = returning_records();
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        for auditing in [false, true] {
            let mut checkpoints = 0;
            assert_eq!(
                run_producer(
                    &records,
                    &memory,
                    &mut || {
                        checkpoints += 1;
                        Ok(())
                    },
                    auditing
                )
                .expect("complete"),
                SummaryOutcome::Available(())
            );
            for expected in [
                Error::new(ErrorCode::ResourceLimited, "work units exhausted"),
                Error::new(ErrorCode::ResourceLimited, "deadline exceeded"),
                Error::new(ErrorCode::TimedOut, "deadline fixture"),
                Error::new(ErrorCode::Cancelled, "cancel fixture"),
            ] {
                for fail_at in 1..=checkpoints {
                    let mut count = 0;
                    let result = run_producer(
                        &records,
                        &memory,
                        &mut || {
                            count += 1;
                            if count == fail_at {
                                Err(expected.clone())
                            } else {
                                Ok(())
                            }
                        },
                        auditing,
                    );
                    assert_eq!(
                        result,
                        Err(expected.clone()),
                        "audit={auditing}, checkpoint={fail_at}"
                    );
                    assert_eq!(memory.used(), 0);
                }
            }
        }
    }

    #[test]
    fn producer_and_audit_refuse_memory_instead_of_degrading() {
        let records = returning_records();
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        for auditing in [false, true] {
            let mut blocker = None;
            let error = run_producer(
                &records,
                &memory,
                &mut || {
                    if blocker.is_none() {
                        blocker = Some(memory.reserve(
                            memory.observation().limit_bytes - memory.used(),
                            RunPosition::default(),
                        )?);
                    }
                    Ok(())
                },
                auditing,
            )
            .expect_err("admission failure");
            assert_eq!(error.code, ErrorCode::ResourceLimited);
            assert!(error.memory.is_some());
            drop(blocker);
            assert_eq!(memory.used(), 0);
        }
    }

    #[test]
    fn catalog_and_counters_keep_all_three_summary_outcomes() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let catalog = Callees::capture(&[], &memory, &mut || Ok(())).expect("catalog");
        let mut table = Summaries::new(&catalog, None, &memory, &mut || Ok(())).expect("table");
        let mut counts = SummaryOutcomeCounts::default();
        let reason = LocalBudgetExceeded {
            limit: 100_000,
            scope: LocalBudgetScope::ArgumentSummary,
        };
        for outcome in [
            SummaryOutcome::Available(OwnedSummary {
                value: CallSummary::default(),
                raw: None,
                _arguments: memory
                    .reserve(0, RunPosition::default())
                    .expect("arguments"),
                _results: memory.reserve(0, RunPosition::default()).expect("results"),
            }),
            SummaryOutcome::NoProof,
            SummaryOutcome::LocalBudgetExceeded(reason),
        ] {
            let id = table.entries.len();
            table
                .entries
                .push(
                    Entry {
                        function: None,
                        object: None,
                        key: None,
                        summary: SummaryOutcome::NoProof,
                        edges: AdmittedVec::new(&memory),
                        discovery: 0,
                        low: 0,
                        active: false,
                    },
                    RunPosition::default(),
                )
                .expect("entry");
            let expected = outcome.status();
            table.finish(id, outcome).expect("finish");
            let actual = table.entry(id).expect("stored entry").summary.status();
            assert_eq!(actual, expected);
            counts.record(actual);
        }
        assert_eq!(
            counts,
            SummaryOutcomeCounts {
                available: 1,
                no_proof: 1,
                local_budget: 1
            }
        );
    }
}
