//! Fail-closed provenance for inlined task construction in a thread
//! executor's initializer. This is deliberately a small proof: unfamiliar
//! lowering, an escaped poll address, or a spawn on another spawner leaves
//! the task available to every executor.
use crate::{Analysis, FunctionFacts, TypeFacts};
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_analysis::{FunctionInput, PreparedReferences, research};
use oer_analysis_riscv_lift::{RiscvDecoder, is_base_instruction};
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::{ProgramView, facts::ImageFacts};
use std::collections::{BTreeMap, BTreeSet};

/// Software interrupt identities are exclusive in the platform executor:
/// both `run` and `start` claim their line before publishing a spawner.
pub(super) fn executor(name: &str) -> Option<u8> {
    let rest = name.strip_prefix("<oer_espressif_executor::executor::Executor<")?;
    let (line, rest) = rest.split_once(">>::run")?;
    (rest.is_empty() || rest.starts_with("::<")).then_some(line.parse().ok()?)
}

pub(super) fn poll_executor(name: &str) -> Option<u8> {
    executor(name).or_else(|| {
        name.strip_prefix("oer_espressif_executor::executor::interrupt_handler::<")?
            .strip_suffix('>')?
            .parse()
            .ok()
    })
}

/// The structural decoder can identify a non-control instruction even when
/// its integer effects are unsupported. Visit its continuation, with every
/// value killed by `Unsupported`, rather than silently losing an incoming
/// edge (for example the executor loop's `wfi`).
struct ProofDecoder;
impl FunctionDecoder for ProofDecoder {
    fn identity(&self) -> &'static str {
        "rv32-task-spawn-proof/structural-1"
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        RiscvDecoder.decode(bytes).or_else(|| {
            let half = u16::from_le_bytes(bytes.get(..2)?.try_into().ok()?);
            let length = if half & 3 != 3 {
                2
            } else if half & 0x1f != 0x1f {
                4
            } else {
                return None;
            };
            (RiscvDecoder.unsupported_flow(bytes.get(..length)?) == UnsupportedFlow::NonControl)
                .then_some(DecodedOp {
                    length: length as u8,
                    text: String::new(),
                    flow: InstructionFlow::Next,
                })
        })
    }
    fn reference(
        &self,
        relocation: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        control: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        RiscvDecoder.reference(relocation, all, section, control)
    }
}

fn closes_graph(record: &FunctionRecord) -> bool {
    match record {
        FunctionRecord::Gap { reason, .. } => *reason == GapReason::Unvisited,
        FunctionRecord::Edge {
            target, relation, ..
        } => {
            // An unknown call has its ABI continuation. An unknown
            // non-call can enter any local block, invalidating values
            // obtained from the graph's known predecessors alone.
            !matches!(relation, EdgeKind::Conflict | EdgeKind::Stop)
                && (target.is_some() || matches!(relation, EdgeKind::Call | EdgeKind::Return))
        }
        _ => true,
    }
}

struct Closed(bool);
impl FunctionSink for Closed {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0 &= closes_graph(record);
        Ok(())
    }
}
impl FunctionSemantics for ProofDecoder {
    fn semantic_identity(&self) -> &'static str {
        "rv32-task-spawn-proof/conservative-1"
    }
    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        RiscvDecoder.branch(bytes)
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        RiscvDecoder.lift(bytes)
    }
    fn value_relocation(
        &self,
        relocation: &FunctionRelocation,
        operation: SemanticOp,
    ) -> ValueRelocation {
        RiscvDecoder.value_relocation(relocation, operation)
    }
}

fn initializer_of(poll: &str, initializer: &str) -> bool {
    let Some(future) = poll
        .strip_prefix("<embassy_executor::raw::TaskStorage<")
        .and_then(|name| name.strip_suffix(">>::poll"))
    else {
        return false;
    };
    initializer
        .strip_prefix("<embassy_executor::raw::util::UninitCell<")
        .and_then(|name| name.strip_prefix(future))
        .and_then(|name| name.strip_prefix(">>::write_in_place"))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::<"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Exact(u32),
    Receiver(u32),
    Unknown,
}

#[derive(Clone, Copy)]
struct Store {
    address: Value,
    value: Value,
}

struct Values<'a, 'm> {
    ledger: &'a mut Ledger<'m>,
    arguments: [Value; 32],
    expressions: BTreeMap<u32, Value>,
    stores: BTreeMap<u32, Store>,
    edges: BTreeMap<u32, Vec<u32>>,
    closed: bool,
}
impl Values<'_, '_> {
    fn value(&self, value: &AbstractValue) -> Value {
        match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                Value::Exact(*value)
            }
            AbstractValue::Expression { id } => {
                self.expressions.get(id).copied().unwrap_or(Value::Unknown)
            }
            _ => Value::Unknown,
        }
    }

    /// Every path to the materialization passes a matching publication.
    /// Address ordering alone does not establish that a store dominates it.
    fn published_before(
        &mut self,
        entry: u32,
        site: u32,
        slot: Value,
        receiver: Value,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        let mut visited = BTreeSet::new();
        let mut pending = Vec::new();
        self.ledger.push(&mut pending, entry, control)?;
        while let Some(at) = pending.pop() {
            control.checkpoint(1)?;
            if at == site {
                return Ok(false);
            }
            if visited.contains(&at) {
                continue;
            }
            self.ledger.add(&mut visited, at, control)?;
            if self
                .stores
                .get(&at)
                .is_some_and(|store| store.address == slot && store.value == receiver)
            {
                continue;
            }
            if let Some(targets) = self.edges.get(&at) {
                for &target in targets {
                    self.ledger.push(&mut pending, target, control)?;
                }
            }
        }
        Ok(true)
    }
}
impl FunctionSink for Values<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.closed &= closes_graph(record);
        match record {
            FunctionRecord::Expression { id, expression, .. } => {
                let value = match expression {
                    Expression::EntryRegister { register } => self
                        .arguments
                        .get(*register as usize)
                        .copied()
                        .unwrap_or(Value::Unknown),
                    Expression::Integer { op, left, right } => {
                        match (op, self.value(left), self.value(right)) {
                            (_, Value::Exact(a), Value::Exact(b)) => {
                                Value::Exact(op.evaluate(a, b))
                            }
                            (IntegerOp::Add, Value::Receiver(a), Value::Exact(b))
                            | (IntegerOp::Add, Value::Exact(b), Value::Receiver(a)) => {
                                Value::Receiver(a.wrapping_add(b))
                            }
                            (IntegerOp::Sub, Value::Receiver(a), Value::Exact(b)) => {
                                Value::Receiver(a.wrapping_sub(b))
                            }
                            _ => Value::Unknown,
                        }
                    }
                    _ => Value::Unknown,
                };
                self.ledger
                    .insert(&mut self.expressions, *id, value, control)?;
            }
            FunctionRecord::MemoryAccess {
                offset,
                access: MemoryKind::Store,
                width: 4,
                address,
                value: Some(value),
                ..
            } => {
                let store = Store {
                    address: self.value(address),
                    value: self.value(value),
                };
                self.ledger
                    .insert(&mut self.stores, *offset as u32, store, control)?;
            }
            FunctionRecord::Edge {
                from,
                target: Some(target),
                relation,
                external: false,
            } if !matches!(relation, EdgeKind::Call | EdgeKind::Return) => {
                let targets =
                    self.ledger
                        .entry(&mut self.edges, *from as u32, Vec::new, control)?;
                self.ledger.push(targets, *target as u32, control)?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// A straight-line callee either kills or preserves the register without
/// reading it. The ABI permits clobbering; it does not prove a clobber.
fn unused(
    image: &ImageFacts<'_>,
    callee: &FunctionFacts,
    register: u8,
    control: &mut dyn RunControl,
) -> Result<Option<bool>> {
    let code = crate::image::function_bytes(image.file(), &callee.function)?;
    let mut at = 0;
    while let Some(bytes) = code.get(at..) {
        control.checkpoint(1)?;
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            return Ok(None);
        };
        if !is_base_instruction(bytes)
            || RiscvDecoder.lift(bytes).reads_register(register)
            || decoded
                .flow
                .through_register()
                .is_some_and(|(base, _, _)| base == register)
        {
            return Ok(None);
        }
        if RiscvDecoder.lift(bytes).destination_register() == Some(register) {
            return Ok(Some(true));
        }
        match decoded.flow {
            InstructionFlow::Next => at += usize::from(decoded.length),
            InstructionFlow::Return { .. } => return Ok(Some(false)),
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Follow the materialized poll address until it is killed. Every use must
/// be the one typed initialization store; a branch, copy, second store,
/// return or call that can consume the address makes provenance unknown.
fn consumed(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    owner: &FunctionFacts,
    mut site: u32,
    register: u8,
    control: &mut dyn RunControl,
) -> Result<bool> {
    let code = crate::image::function_bytes(image.file(), &owner.function)?;
    while let Some(bytes) = code.get(site.wrapping_sub(owner.function.address) as usize..) {
        control.checkpoint(1)?;
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            return Ok(false);
        };
        let op = RiscvDecoder.lift(bytes);
        if !is_base_instruction(bytes)
            || op.reads_register(register)
            || decoded
                .flow
                .through_register()
                .is_some_and(|(base, _, _)| base == register)
        {
            return Ok(false);
        }
        match decoded.flow {
            InstructionFlow::Next => {
                if op.destination_register() == Some(register) {
                    return Ok(true);
                }
            }
            InstructionFlow::Jump { link: true, .. }
            | InstructionFlow::Indirect { link: true, .. } => {
                let Some(target) = owner
                    .transfers
                    .iter()
                    .find(|t| t.site == site)
                    .and_then(|t| t.target)
                else {
                    return Ok(false);
                };
                let Some(callee) = analysis.functions.get(&target) else {
                    return Ok(false);
                };
                match unused(image, callee, register, control)? {
                    Some(true) => return Ok(true),
                    Some(false) => {}
                    None => return Ok(false),
                }
            }
            _ => return Ok(false),
        }
        site += u32::from(decoded.length);
    }
    Ok(false)
}

/// Every relocation taking `poll` must construct a header and immediately
/// spawn it on the initializer's own executor. The registry publication
/// identifies that executor; DWARF supplies the header layout. No addresses or byte offsets are
/// supplied by the firmware or accepted as a baseline.
pub(super) fn owners(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    polls: &BTreeSet<u32>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeMap<u32, Vec<u8>>> {
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let mut taken = BTreeSet::new();
    let mut uses: BTreeMap<u32, BTreeMap<u32, Role>> = BTreeMap::new();
    for address_use in image.address_uses() {
        control.checkpoint(1)?;
        let Some(projected) = crate::relocations::function_use(
            address_use,
            analysis,
            crate::relocations::FunctionUsePolicy::TaskPoll,
        )?
        else {
            continue;
        };
        if let Some(symbol) = projected.symbol {
            scratch.add(&mut taken, symbol, control)?;
        }
        scratch.add(&mut taken, projected.canonical, control)?;
        if let Some(containing) = projected.containing {
            scratch.add(&mut taken, containing, control)?;
        }
        let Some(target) = projected.function else {
            continue;
        };
        if !polls.contains(&target) {
            continue;
        }
        // Arithmetic, odd and interior targets remain in the all-uses audit,
        // but cannot prove an exact poll-address materialization.
        let mut role = if projected.exact {
            address_use.role
        } else {
            Role::Other
        };
        let entries = scratch.entry(&mut uses, target, BTreeMap::new, control)?;
        let site = u32::try_from(address_use.relocation.at)
            .map_err(|_| Error::new(ErrorCode::Integrity, "task relocation exceeds RV32"))?;
        if entries.get(&site).is_some_and(|previous| *previous != role) {
            role = Role::Other;
        }
        scratch.insert(entries, site, role, control)?;
    }
    let (Some(poll_offset), Some(executor_offset)) = (
        types.member_offset("TaskHeader", "poll_fn", control)?,
        types.member_offset("TaskHeader", "executor", control)?,
    ) else {
        return Ok(BTreeMap::new());
    };
    let mut registry = None;
    for symbol in image.file().symbols() {
        control.checkpoint(1)?;
        if oer_analysis_elf::demangle(symbol.name)
            == "oer_espressif_executor::executor::EMBASSY_EXECUTORS"
        {
            registry = u32::try_from(symbol.address).ok();
            break;
        }
    }
    let Some(registry) = registry else {
        return Ok(BTreeMap::new());
    };
    let view = ProgramView::new(image.bytes(), image.file(), &[], keep.memory(), control)?;
    let mut result = BTreeMap::new();
    let mut closed_callers = BTreeMap::new();
    for (poll, uses) in uses {
        let Some(poll_facts) = analysis.functions.get(&poll) else {
            continue;
        };
        let mut lines = Vec::new();
        let mut proven = !uses.is_empty();
        for (&site, &role) in &uses {
            control.checkpoint(1)?;
            if role == Role::AbsoluteLow
                && uses.get(&site.wrapping_sub(4)) == Some(&Role::AbsoluteHigh)
            {
                continue;
            }
            if role != Role::AbsoluteHigh || uses.get(&(site + 4)) != Some(&Role::AbsoluteLow) {
                proven = false;
                break;
            }
            let Some(owner) = analysis
                .functions
                .range(..=site)
                .next_back()
                .map(|(_, f)| f)
                .filter(|f| site < f.function.address + f.function.size)
            else {
                proven = false;
                break;
            };
            let line = owner
                .function
                .names
                .iter()
                .find_map(|name| executor(&oer_analysis_elf::demangle(name)));
            let Some(line) = line else {
                proven = false;
                break;
            };
            let mut local = Ledger::new(keep.memory(), control)?;
            let mut arguments = [Value::Unknown; 32];
            arguments[10] = Value::Receiver(0);
            let mut direct = !taken.contains(&owner.function.address);
            for caller in analysis.functions.values() {
                for transfer in &caller.transfers {
                    control.checkpoint(1)?;
                    if transfer.target != Some(owner.function.address) {
                        continue;
                    }
                    if transfer.kind != crate::TransferKind::Call {
                        direct = false;
                        break;
                    }
                    let closed = if let Some(&closed) = closed_callers.get(&caller.function.address)
                    {
                        closed
                    } else {
                        let mut closed = Closed(true);
                        let references = PreparedReferences::empty();
                        oer_analysis_riscv_analysis::analyze(
                            FunctionInput {
                                image: Some(&view),
                                section: caller.function.section as u32,
                                extent: CodeRange {
                                    start: u64::from(caller.function.address),
                                    length: u64::from(caller.function.size),
                                },
                                bytes: crate::image::function_bytes(
                                    image.file(),
                                    &caller.function,
                                )?,
                                relocations: &references,
                                data_ranges: &[],
                                jumps: &[],
                                returns: &NoReturns,
                                call_behavior: &UnknownCallBehavior,
                            },
                            &RiscvDecoder,
                            keep.memory(),
                            control,
                            &mut closed,
                        )?;
                        scratch.insert(
                            &mut closed_callers,
                            caller.function.address,
                            closed.0,
                            control,
                        )?;
                        closed.0
                    };
                    direct &= closed;
                }
                if !direct {
                    break;
                }
            }
            if direct {
                for (index, argument) in arguments.iter_mut().enumerate().take(18).skip(10) {
                    if let Ok(constants) =
                        analysis.constant_arguments(owner.function.address, index - 10)
                        && constants.len() == 1
                        && let Some(&constant) = constants.first()
                    {
                        *argument = Value::Exact(constant);
                    }
                }
            }
            let mut values = Values {
                ledger: &mut local,
                arguments,
                expressions: BTreeMap::new(),
                stores: BTreeMap::new(),
                edges: BTreeMap::new(),
                closed: true,
            };
            let code = crate::image::function_bytes(image.file(), &owner.function)?;
            let references = PreparedReferences::new(
                &[],
                owner.function.section as u32,
                &RiscvDecoder,
                keep.memory(),
                control,
            )?;
            research(
                FunctionInput {
                    image: Some(&view),
                    section: owner.function.section as u32,
                    extent: CodeRange {
                        start: u64::from(owner.function.address),
                        length: u64::from(owner.function.size),
                    },
                    bytes: code,
                    relocations: &references,
                    data_ranges: &[],
                    jumps: &[],
                    returns: &NoReturns,
                    call_behavior: &UnknownCallBehavior,
                },
                &ProofDecoder,
                keep.memory(),
                control,
                &mut values,
                Some(CallAbi::RiscvInteger),
            )?;
            if !values.closed {
                proven = false;
                break;
            }
            let mut published = values
                .stores
                .iter()
                .filter(|(at, store)| {
                    **at < site && store.address == Value::Exact(registry + 4 * u32::from(line))
                })
                .map(|(_, store)| store.value);
            let Some(receiver) = published.next().filter(|value| *value != Value::Unknown) else {
                proven = false;
                break;
            };
            if published.any(|value| value != receiver) {
                proven = false;
                break;
            }
            if !values.published_before(
                owner.function.address,
                site,
                Value::Exact(registry + 4 * u32::from(line)),
                receiver,
                control,
            )? {
                proven = false;
                break;
            }
            let Some(store) = values.stores.get(&(site + 8)) else {
                proven = false;
                break;
            };
            let Value::Exact(address) = store.address else {
                proven = false;
                break;
            };
            let mut typed = false;
            for name in &poll_facts.function.names {
                control.checkpoint(1)?;
                typed |=
                    types.task_poll_field(address, &oer_analysis_elf::demangle(name), control)?;
            }
            if !typed || store.value != Value::Exact(poll) {
                proven = false;
                break;
            }
            let Some(bytes) = code.get((site + 8 - owner.function.address) as usize..) else {
                proven = false;
                break;
            };
            let SemanticOp::Memory {
                kind: MemoryKind::Store,
                source: Some(register),
                width: 4,
                ..
            } = RiscvDecoder.lift(bytes)
            else {
                proven = false;
                break;
            };
            let Some(decoded) = RiscvDecoder.decode(bytes) else {
                proven = false;
                break;
            };
            let Some(prefix) = code.get((site - owner.function.address) as usize..) else {
                proven = false;
                break;
            };
            let Some(low) = prefix.get(4..) else {
                proven = false;
                break;
            };
            if !matches!(RiscvDecoder.lift(prefix), SemanticOp::Upper { dest, pc_relative: false, .. } if dest == register)
                || !matches!(RiscvDecoder.lift(low), SemanticOp::Integer { op: IntegerOp::Add, dest, left: Operand::Register(base), .. } if dest == register && base == register)
            {
                proven = false;
                break;
            }
            let mut next = site + 8 + u32::from(decoded.length);
            if !consumed(image, analysis, owner, next, register, control)? {
                proven = false;
                break;
            }
            let executor_slot = address
                .wrapping_sub(poll_offset)
                .wrapping_add(executor_offset);
            let mut spawned = false;
            while let Some(bytes) = code.get((next - owner.function.address) as usize..) {
                control.checkpoint(1)?;
                let Some(decoded) = RiscvDecoder.decode(bytes) else {
                    break;
                };
                if let Some(store) = values.stores.get(&next)
                    && store.address == Value::Exact(executor_slot)
                {
                    spawned = store.value == receiver;
                    break;
                }
                match decoded.flow {
                    InstructionFlow::Next => {}
                    InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. } => {
                        let callee = owner
                            .transfers
                            .iter()
                            .find(|t| t.site == next)
                            .and_then(|t| t.target)
                            .and_then(|target| analysis.functions.get(&target));
                        let mut initializer = false;
                        if let Some(callee) = callee {
                            for name in &callee.function.names {
                                control.checkpoint(1)?;
                                let name = oer_analysis_elf::demangle(name);
                                for poll in &poll_facts.function.names {
                                    control.checkpoint(1)?;
                                    initializer |=
                                        initializer_of(&oer_analysis_elf::demangle(poll), &name);
                                }
                            }
                        }
                        if !initializer {
                            break;
                        }
                    }
                    _ => break,
                }
                next += u32::from(decoded.length);
            }
            if !spawned {
                proven = false;
                break;
            }
            scratch.push(&mut lines, line, control)?;
        }
        if proven {
            control.checkpoint(
                (lines.len() as u64)
                    .saturating_mul(u64::from(usize::BITS - lines.len().leading_zeros())),
            )?;
            lines.sort_unstable();
            lines.dedup();
            scratch.insert(&mut result, poll, lines, control)?;
        }
    }
    // The result lives in the caller's ledger, not the temporary proof ledger.
    keep.admit(
        result.len() * size_of::<(u32, Vec<u8>)>()
            + result.values().map(Vec::capacity).sum::<usize>(),
        control,
    )?;
    Ok(result)
}
