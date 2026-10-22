//! One corpus producer for the shared result/argument port, in two analyses.
use super::*;
use crate::registers::{CallKey, CallLinks, CallSites, Callees, Calls};
use oer_analysis_blobray_analysis::registers::{ArgumentProgram, ArgumentSource};
use oer_analysis_blobray_artifacts::{MemberIndex, PreparedObject, prepare_captured};
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
struct OwnedSummary<'m> {
    value: CallSummary,
    _arguments: MemoryReservation<'m>,
    _results: MemoryReservation<'m>,
}
struct Entry<'m> {
    function: Option<LibraryFunction>,
    object: Option<usize>,
    key: Option<CallKey>,
    summary: Option<OwnedSummary<'m>>,
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
impl CallSummaries for Summaries<'_> {
    fn summary(&self, callee: Callee<'_>) -> Option<&CallSummary> {
        let computed = self.groups.callee(callee).and_then(|group| {
            if group.remaining != 0 || group.conflict {
                return None;
            }
            self.entries
                .get(group.representative?)?
                .summary
                .as_ref()
                .map(|s| &s.value)
        });
        computed.or_else(|| match callee {
            Callee::Symbol(name) => self.contracts.get(name),
            Callee::Address(address) => self
                .contract_addresses
                .get(&address)
                .and_then(|n| n.as_deref())
                .and_then(|n| self.contracts.get(n)),
        })
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
                    summary: None,
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
    fn finish(&mut self, id: usize, summary: Option<OwnedSummary<'m>>) -> Result<()> {
        self.entry_mut(id)?.summary = summary;
        let Some(key) = self.entries.get(id).ok_or_else(integrity)?.key.as_ref() else {
            return Ok(());
        };
        let group = self.groups.get(key).ok_or_else(integrity)?;
        let representative = group.representative;
        let same = match (
            representative
                .and_then(|r| self.entries.get(r))
                .and_then(|r| r.summary.as_ref()),
            self.entry(id)?.summary.as_ref(),
        ) {
            (Some(a), Some(b)) => a.value == b.value,
            _ => false,
        };
        let group = self.groups.get_mut(key).ok_or_else(integrity)?;
        group.remaining = group.remaining.checked_sub(1).ok_or_else(integrity)?;
        if self
            .entries
            .get(id)
            .and_then(|e| e.summary.as_ref())
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
                e.bits == 32
                    && e.little_endian
                    && e.machine == 243
                    && matches!(e.object_type, 1 | 2)
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
                    || elf.as_ref().is_some_and(|e| e.object_type == 2))
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
            let facts_result = match elf.as_ref().map(|e| e.object_type) {
                Some(1) => ObjectTables::new(bytes, memory, c).map(|t| object_tables = Some(t)),
                Some(2) => ImageFacts::new(bytes, assumptions.read_only, memory, c)
                    .map(|t| image_facts = Some(t)),
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
                    linked: elf.as_ref().is_some_and(|e| e.object_type == 2),
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
                    Ok((records, _, _)) => records,
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
                let facts = Facts::new(&records, memory, c)?;
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
                let mut contract = None;
                if let (Some(object), Some(function)) =
                    (object, table.entry(member)?.function.as_ref())
                {
                    let signature = assumptions
                        .signatures
                        .and_then(|s| function.name.as_deref().and_then(|n| s.get(n)));
                    let linked = objects.get(object).ok_or_else(integrity)?.linked;
                    let applied_signature = signature.filter(|_| !linked);
                    let rom_signature = signature.filter(|_| linked);
                    let scoped = Scoped {
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
                        Ok((records, summary, found)) => {
                            let mut conflicts = AdmittedVec::new(memory);
                            let mut rom_diagnostics = AdmittedVec::new(memory);
                            let mut conflict_names = Ledger::new(memory, c)?;
                            let mut raw_conflicts = AdmittedVec::new(memory);
                            let mut audit_complete = None;
                            let mut applied_audit = false;
                            let mut audit_failure = None;
                            let mut analyzed = AnalyzedFunction {
                                function,
                                records: &records,
                                coverage: summary.coverage,
                                semantics: summary.semantics,
                                jump_tables: &found.tables,
                                summaries: &scoped,
                                conflicts: &[],
                                rom_signature_diagnostics: &[],
                                rom_signature_audit_complete: None,
                                signature_audit_complete: None,
                                signature_audit_failure: None,
                            };
                            let publish = !recursive
                                && objects.get(object).ok_or_else(integrity)?.safe
                                && eligible(
                                    analyzed.coverage,
                                    analyzed.semantics,
                                    analyzed.records,
                                    c,
                                )?;
                            if publish || assumptions.signatures.is_some() {
                                let facts = Facts::new(&records, memory, c)?;
                                let links = CallLinks::new(&facts, function, catalog, memory, c)?;
                                applied_audit = needs_audit(&scoped, &links, None);
                                let produced = if publish {
                                    produce(
                                        &facts,
                                        analyzed.records,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut audit_complete,
                                        c,
                                    )
                                } else if needs_audit(&scoped, &links, rom_signature) {
                                    argument_summary(
                                        &facts,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut audit_complete,
                                        None,
                                        c,
                                    )
                                    .map(|_| None)
                                } else {
                                    Ok(None)
                                };
                                contract = produced?;
                                if audit_complete == Some(false) {
                                    raw_conflicts.truncate(0);
                                    match argument_summary(
                                        &facts,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut audit_complete,
                                        Some(16),
                                        c,
                                    ) {
                                        Ok(_) => {}
                                        Err(e)
                                            if matches!(
                                                e.code,
                                                ErrorCode::ResourceLimited | ErrorCode::TimedOut
                                            ) =>
                                        {
                                            audit_failure = Some(e);
                                        }
                                        Err(e) => return Err(e),
                                    }
                                }
                                // A few large recipes need a second diagnostic-only work allowance.
                                // Neither retry is eligible to replace the originally produced summary.
                                if audit_complete == Some(false)
                                    && audit_failure.as_ref().is_some_and(|e| {
                                        e.code == ErrorCode::ResourceLimited && e.memory.is_none()
                                    })
                                {
                                    raw_conflicts.truncate(0);
                                    match argument_summary(
                                        &facts,
                                        &scoped,
                                        rom_signature,
                                        &links,
                                        decoder,
                                        memory,
                                        &mut raw_conflicts,
                                        &mut audit_complete,
                                        Some(256),
                                        c,
                                    ) {
                                        Ok(_) => {
                                            audit_failure = None;
                                        }
                                        Err(e)
                                            if matches!(
                                                e.code,
                                                ErrorCode::ResourceLimited | ErrorCode::TimedOut
                                            ) =>
                                        {
                                            audit_failure = Some(e);
                                        }
                                        Err(e) => return Err(e),
                                    }
                                }
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
                            if let (Some(signature), Some(summary)) =
                                (applied_signature, contract.as_mut())
                            {
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
                            analyzed.conflicts = &conflicts;
                            analyzed.rom_signature_diagnostics = &rom_diagnostics;
                            analyzed.signature_audit_complete =
                                audit_complete.filter(|_| applied_audit);
                            analyzed.rom_signature_audit_complete =
                                rom_signature.and(audit_complete);
                            analyzed.signature_audit_failure = audit_failure.as_ref();
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
        &mut Records(&mut records),
    )?;
    Ok((records, summary, found))
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
    audit_complete: &mut Option<bool>,
    c: &mut dyn RunControl,
) -> Result<Option<OwnedSummary<'a>>> {
    match try_produce(
        facts,
        records,
        summaries,
        rom_signature,
        links,
        decoder,
        memory,
        conflicts,
        audit_complete,
        c,
    ) {
        Err(error) if matches!(error.code, ErrorCode::ResourceLimited | ErrorCode::TimedOut) => {
            Ok(None)
        }
        result => result,
    }
}

#[allow(clippy::too_many_arguments)]
fn try_produce<'a>(
    facts: &Facts<'_, 'a>,
    records: &[FunctionRecord],
    summaries: &dyn CallSummaries,
    rom_signature: Option<&crate::signatures::FunctionSignature>,
    links: &CallLinks<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &'a WorkingMemory,
    conflicts: &mut AdmittedVec<'a, RawConflict>,
    audit_complete: &mut Option<bool>,
    c: &mut dyn RunControl,
) -> Result<Option<OwnedSummary<'a>>> {
    let Some((arguments, argument_memory)) = argument_summary(
        facts,
        summaries,
        rom_signature,
        links,
        decoder,
        memory,
        conflicts,
        audit_complete,
        None,
        c,
    )?
    else {
        return Ok(None);
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
    Ok(Some(OwnedSummary {
        value: CallSummary {
            results,
            arguments,
            ..CallSummary::default()
        },
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
    audit_complete: &mut Option<bool>,
    diagnostic_multiplier: Option<u64>,
    c: &mut dyn RunControl,
) -> Result<Option<(ArgumentSummary, MemoryReservation<'a>)>> {
    let audit = needs_audit(summaries, links, rom_signature);
    if audit {
        *audit_complete = Some(false);
    }
    let program = ArgumentProgram::capture(facts, decoder, memory, c)?;
    let calls = Calls { links, summaries };
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
            conflicts.push(
                RawConflict {
                    origin,
                    usage: usage.clone(),
                },
                c.position(),
            )?;
        }
        Ok(())
    };
    let result = if let Some(multiplier) = diagnostic_multiplier {
        program.audit(&calls, memory, c, &mut observe, multiplier)?
    } else {
        program.summarize_observed(&calls, memory, c, &mut observe)?
    };
    *audit_complete = Some(result.is_some());
    Ok(result)
}
fn needs_audit(
    summaries: &dyn CallSummaries,
    links: &CallLinks<'_>,
    rom_signature: Option<&crate::signatures::FunctionSignature>,
) -> bool {
    rom_signature.is_some()
        || summaries.current().is_some()
        || links.keys().any(|key| {
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
}
