//! Numerical stack facts for an explicit invocation context. No interval
//! constructor is exposed: roots come from canonical machine inputs, and
//! descendants from the same evaluator's all-incoming-path stack coordinates.
use crate::writes::{WriteBase, WriteValue, closes_local_graph};
use oer_analysis_riscv_model::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CONTEXT: AtomicU64 = AtomicU64::new(1);

fn identity() -> Result<u64> {
    NEXT_CONTEXT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "stack context identities exhausted",
            )
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackOriginKind {
    Call,
    Tail,
    SetupReturn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackOrigin {
    pub kind: StackOriginKind,
    pub caller: u64,
    pub site: u64,
    pub callee: u64,
    pub interval: (u32, u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StackBasis {
    Entry,
    // The evaluator still names the pre-installation EntryStack. Only sp,
    // with no subsequent writes, denotes the newly installed pointer. Other
    // registers and spilled pointers may retain the original entry value.
    Installed { previous_coordinate: i64 },
}

/// An invocation, never an unconditional property of a function. Consumers
/// must supply the current context again during evaluation; a prepared call
/// cannot install its old context into a new run.
pub struct StackContext<'m> {
    capture_id: u64,
    owner: u64,
    start: u64,
    basis: StackBasis,
    interval: (u32, u32),
    residues: u8,
    id: u64,
    origins: AdmittedVec<'m, StackOrigin>,
    executor_invariant: bool,
}
impl std::fmt::Debug for StackContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StackContext")
            .field("owner", &self.owner)
            .field("start", &self.start)
            .field("basis", &self.basis)
            .field("interval", &self.interval)
            .field("origins", &&*self.origins)
            .field("assumptions", &self.assumptions())
            .finish()
    }
}

impl StackContext<'_> {
    pub fn owner(&self) -> u64 {
        self.owner
    }
    pub fn interval(&self) -> (u32, u32) {
        self.interval
    }
    pub fn origins(&self) -> &[StackOrigin] {
        &self.origins
    }
    pub fn assumptions(&self) -> &[Assumption] {
        if self.executor_invariant {
            &[Assumption::ExecutorInvariant]
        } else {
            &[]
        }
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn matches_image(&self, image: Option<&dyn ImageMemory>) -> bool {
        image.and_then(ImageMemory::capture_id) == Some(self.capture_id)
    }
    pub(crate) fn valid_for(
        &self,
        caller: &FunctionIndex<'_, '_>,
        entry: u64,
        site: u64,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        if entry != self.owner
            || site < self.start
            || caller.capture_id() != Some(self.capture_id)
            || caller.extent().is_none_or(|extent| extent.start != entry)
        {
            return Ok(false);
        }
        for record in caller.records() {
            c.checkpoint(1)?;
            if let FunctionRecord::Value {
                offset,
                register: 2,
                value,
                ..
            } = record
                && *offset >= self.start
                && resolve(value, Some(self)).is_none()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(crate) fn bounds(&self, entry: u64, site: u64, value: WriteValue) -> Option<(u32, u32)> {
        shift(self.interval, self.displacement(entry, site, value)?)
    }
    pub(crate) fn aligned(&self, entry: u64, site: u64, value: WriteValue) -> bool {
        self.displacement(entry, site, value)
            .is_some_and(|delta| rotate_residues(self.residues, delta) == 1)
    }
    fn displacement(&self, entry: u64, site: u64, value: WriteValue) -> Option<i64> {
        if entry != self.owner || site < self.start || self.basis != StackBasis::Entry {
            return None;
        }
        let WriteValue::Base {
            base: WriteBase::Stack { function, offset },
            offset: delta,
        } = value
        else {
            return None;
        };
        if function != entry {
            return None;
        }
        offset.checked_add(i64::from(delta as i32))
    }
}

fn shift((low, high): (u32, u32), delta: i64) -> Option<(u32, u32)> {
    Some((
        u32::try_from(i64::from(low).checked_add(delta)?).ok()?,
        u32::try_from(i64::from(high).checked_add(delta)?).ok()?,
    ))
}

fn exact(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            Some(*value)
        }
        _ => None,
    }
}

fn closed(index: &FunctionIndex<'_, '_>, c: &mut dyn RunControl) -> Result<bool> {
    for record in index.records() {
        c.checkpoint(1)?;
        if !closes_local_graph(record) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Close a single unresolved terminal edge only when the existing evaluator
/// proves its target outside the function, and every other edge/gap closes.
/// With no other unmodeled entry, all paths reaching this transfer were
/// evaluated; it cannot introduce a new local incoming path. Re-run the
/// ordinary CFG with the returned jump before making any context proof.
pub fn external_terminal(
    records: &[FunctionRecord],
    extent: CodeRange,
    c: &mut dyn RunControl,
) -> Result<Option<(u64, u64)>> {
    let Some(end) = extent.start.checked_add(extent.length) else {
        return Ok(None);
    };
    let mut site = None;
    for record in records {
        c.checkpoint(1)?;
        if closes_local_graph(record) {
            continue;
        }
        match record {
            FunctionRecord::Edge {
                from,
                target: None,
                relation: EdgeKind::Indirect,
                external: true,
            } if site.is_none() => site = Some(*from),
            _ => return Ok(None),
        }
    }
    let Some(site) = site else {
        return Ok(None);
    };
    for record in records {
        c.checkpoint(1)?;
        if let FunctionRecord::Transfer {
            offset,
            target: AbstractValue::ImageAddress { address },
            call: false,
        } = record
            && *offset == site
            && !(extent.start..end).contains(&u64::from(*address))
        {
            return Ok(Some((site, u64::from(*address))));
        }
    }
    Ok(None)
}

fn find_record<'a, T>(
    records: &'a [FunctionRecord],
    c: &mut dyn RunControl,
    f: impl Fn(&'a FunctionRecord) -> Option<T>,
) -> Result<Option<T>> {
    for record in records {
        c.checkpoint(1)?;
        if let Some(value) = f(record) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn resolve(value: &AbstractValue, parent: Option<&StackContext<'_>>) -> Option<(u32, u32)> {
    if let Some(n) = exact(value) {
        return Some((n, n));
    }
    match value {
        AbstractValue::EntryStack { offset } => {
            let parent = parent?;
            if parent.basis != StackBasis::Entry {
                return None;
            }
            shift(parent.interval, *offset)
        }
        AbstractValue::Alternatives { values } => {
            let mut interval: Option<(u32, u32)> = None;
            for value in values.values() {
                let next = resolve(&value.as_value(), parent)?;
                interval =
                    Some(interval.map_or(next, |old| (old.0.min(next.0), old.1.max(next.1))));
            }
            interval
        }
        _ => None,
    }
}

fn rotate_residues(mask: u8, delta: i64) -> u8 {
    let n = delta.rem_euclid(4) as u32;
    ((mask << n) | (mask >> (4 - n))) & 15
}

fn residues(value: &AbstractValue, parent: Option<&StackContext<'_>>) -> Option<u8> {
    if let Some(n) = exact(value) {
        return Some(1 << (n % 4));
    }
    match value {
        AbstractValue::EntryStack { offset } => {
            let parent = parent?;
            (parent.basis == StackBasis::Entry).then(|| rotate_residues(parent.residues, *offset))
        }
        AbstractValue::Alternatives { values } => {
            values.values().iter().try_fold(0, |mask, value| {
                Some(mask | residues(&value.as_value(), parent)?)
            })
        }
        _ => None,
    }
}

/// Prove one call/tail invocation from its canonical inputs. All local paths
/// to the site participate in CallInputs. Other invocations of the callee
/// require their own contexts and must be merged explicitly when applicable.
pub fn call_stack_context<'m>(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    parent: Option<&StackContext<'_>>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<StackContext<'m>>> {
    call_stack_context_with_targets(caller, entry, site, parent, None, None, memory, c)
}

/// Existing resolution facts may select a target of an indirect call. Their
/// original assumptions remain part of the numerical context and application.
#[allow(clippy::too_many_arguments)]
pub fn call_stack_context_with_targets<'m>(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    parent: Option<&StackContext<'_>>,
    resolutions: Option<&Resolutions>,
    target: Option<u32>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<StackContext<'m>>> {
    let Some(capture_id) = caller.capture_id() else {
        return Ok(None);
    };
    if caller.extent().is_none_or(|extent| extent.start != entry)
        || !closed(caller, c)?
        || parent.is_some_and(|p| p.owner != entry || site < p.start || p.capture_id != capture_id)
    {
        return Ok(None);
    }
    let Some(transfer) = caller.transfer(site, c)? else {
        return Ok(None);
    };
    let mut executor_invariant = parent.is_some_and(|p| p.executor_invariant);
    let target = match (exact(transfer.target), target) {
        (Some(actual), requested) if requested.is_none_or(|target| target == actual) => actual,
        (None, Some(target)) if transfer.call => {
            let Some(resolution) = u32::try_from(site)
                .ok()
                .and_then(|site| resolutions?.get(site))
            else {
                return Ok(None);
            };
            if !resolution.resolves() || !resolution.targets.contains(&target) {
                return Ok(None);
            }
            executor_invariant |= resolution
                .facts
                .iter()
                .any(|f| f.assumption() == Some(Assumption::ExecutorInvariant));
            target
        }
        _ => return Ok(None),
    };
    let Some(inputs) = caller.call_inputs(site, c)? else {
        return Ok(None);
    };
    let Some(stack) = inputs.registers.get(2) else {
        return Ok(None);
    };
    let (interval, residues) = match parent {
        Some(parent)
            if matches!(parent.basis, StackBasis::Installed { previous_coordinate }
                if *stack == AbstractValue::EntryStack { offset: previous_coordinate }) =>
        {
            // This exception is for the live sp input only. The continuation
            // below must contain no sp writes; it cannot rebase any other
            // EntryStack value, even one equal to this old coordinate.
            (parent.interval, parent.residues)
        }
        _ => {
            let (Some(interval), Some(residues)) =
                (resolve(stack, parent), residues(stack, parent))
            else {
                return Ok(None);
            };
            (interval, residues)
        }
    };
    // A wrapping intermediate sp cannot lend a nonwrapping coordinate to
    // the final inputs, even when later arithmetic cancels its displacement.
    for record in caller.records() {
        c.checkpoint(1)?;
        if let FunctionRecord::Value {
            offset,
            register: 2,
            value,
            ..
        } = record
            && parent.is_none_or(|p| *offset >= p.start)
            && (parent.is_some_and(|p| matches!(p.basis, StackBasis::Installed { .. }))
                || resolve(value, parent).is_none())
        {
            return Ok(None);
        }
    }
    let mut origins = AdmittedVec::new(memory);
    if let Some(parent) = parent {
        for origin in parent.origins() {
            c.checkpoint(1)?;
            origins.push(*origin, c.position())?;
        }
    }
    origins.push(
        StackOrigin {
            kind: if transfer.call {
                StackOriginKind::Call
            } else {
                StackOriginKind::Tail
            },
            caller: entry,
            site,
            callee: target.into(),
            interval,
        },
        c.position(),
    )?;
    Ok(Some(StackContext {
        capture_id,
        owner: target.into(),
        start: target.into(),
        basis: StackBasis::Entry,
        interval,
        residues,
        id: identity()?,
        origins,
        executor_invariant,
    }))
}

/// Union every admissible invocation context, including different roots.
/// Missing contexts are contagious; a known path cannot stand in for one.
pub fn merge_stack_contexts<'m>(
    contexts: &[Option<&StackContext<'_>>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<StackContext<'m>>> {
    let Some(Some(first)) = contexts.first() else {
        return Ok(None);
    };
    let mut origins = AdmittedVec::new(memory);
    let mut interval = first.interval;
    let mut residues = 0;
    let mut executor_invariant = false;
    for context in contexts {
        c.checkpoint(1)?;
        let Some(context) = context else {
            return Ok(None);
        };
        if (
            context.capture_id,
            context.owner,
            context.start,
            context.basis,
        ) != (first.capture_id, first.owner, first.start, first.basis)
        {
            return Ok(None);
        }
        interval = (
            interval.0.min(context.interval.0),
            interval.1.max(context.interval.1),
        );
        executor_invariant |= context.executor_invariant;
        residues |= context.residues;
        for origin in context.origins() {
            c.checkpoint(1)?;
            origins.push(*origin, c.position())?;
        }
    }
    Ok(Some(StackContext {
        capture_id: first.capture_id,
        owner: first.owner,
        start: first.start,
        basis: first.basis,
        interval,
        residues,
        id: identity()?,
        origins,
        executor_invariant,
    }))
}

/// Machine-established values at every normal return of a stack installer.
/// Storage names select the routine and the expected values outside this
/// checker; they never supply either pointer's value here.
pub struct StackSetup {
    capture_id: u64,
    entry: u64,
    pointer: u32,
    scratch: u32,
}
impl std::fmt::Debug for StackSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StackSetup")
            .field("entry", &self.entry)
            .field("pointer", &self.pointer)
            .field("scratch", &self.scratch)
            .finish()
    }
}
impl StackSetup {
    pub fn entry(&self) -> u64 {
        self.entry
    }
    pub fn pointer(&self) -> u32 {
        self.pointer
    }
    pub fn scratch(&self) -> u32 {
        self.scratch
    }
}

pub fn stack_setup(
    index: &FunctionIndex<'_, '_>,
    entry: u64,
    isa: &dyn FunctionSemantics,
    c: &mut dyn RunControl,
) -> Result<Option<StackSetup>> {
    let Some(capture_id) = index.capture_id() else {
        return Ok(None);
    };
    if index.extent().is_none_or(|extent| extent.start != entry) || !closed(index, c)? {
        return Ok(None);
    }
    let mut result: Option<StackSetup> = None;
    for record in index.records() {
        c.checkpoint(1)?;
        let FunctionRecord::Instruction {
            offset, decoded, ..
        } = record
        else {
            continue;
        };
        if !matches!(decoded.flow, InstructionFlow::Return { .. }) {
            continue;
        }
        let Some(registers) = find_record(index.records(), c, |r| match r {
            FunctionRecord::ReturnInputs {
                offset: at,
                registers,
            } if at == offset => Some(registers),
            _ => None,
        })?
        else {
            return Ok(None);
        };
        let Some(pointer) = registers.get(2).and_then(exact) else {
            return Ok(None);
        };
        // Require the scratch write immediately before the return, and its
        // unique local predecessor. Its source survives a pure CSR write.
        let Some((previous, bytes)) = find_record(index.records(), c, |r| match r {
            FunctionRecord::Instruction {
                offset: at,
                decoded,
                bytes,
            } if at.checked_add(decoded.length.into()) == Some(*offset) => Some((*at, bytes)),
            _ => None,
        })?
        else {
            return Ok(None);
        };
        let Some(source) = isa.scratch_write(bytes) else {
            return Ok(None);
        };
        for r in index.records() {
            c.checkpoint(1)?;
            if let FunctionRecord::Edge {
                from,
                target: Some(to),
                external: false,
                ..
            } = r
                && to == offset
                && *from != previous
            {
                return Ok(None);
            }
        }
        let scratch = match source {
            Operand::Immediate(n) => Some(n),
            Operand::Register(r) => registers.get(usize::from(r)).and_then(exact),
        };
        let Some(scratch) = scratch else {
            return Ok(None);
        };
        if result
            .as_ref()
            .is_some_and(|old| (old.pointer, old.scratch) != (pointer, scratch))
        {
            return Ok(None);
        }
        result = Some(StackSetup {
            capture_id,
            entry,
            pointer,
            scratch,
        });
    }
    Ok(result)
}

/// Bind a checked installer to its actual call and straight-line startup
/// continuation. Calls in the continuation use the explicitly selected ABI.
/// An entry bypass, another stack adjustment or an unknown jump is refused.
/// This proves the live sp only: without stack epochs in the evaluator,
/// EntryStack-derived pointers in this function cannot use the new interval.
pub fn resume_stack_context<'m>(
    setup: &StackSetup,
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<StackContext<'m>>> {
    if !closed(caller, c)?
        || caller.capture_id() != Some(setup.capture_id)
        || caller.extent().is_none_or(|extent| extent.start != entry)
    {
        return Ok(None);
    }
    let Some(transfer) = caller.transfer(site, c)? else {
        return Ok(None);
    };
    if !transfer.call || exact(transfer.target).map(u64::from) != Some(setup.entry) {
        return Ok(None);
    }
    let Some(start) = find_record(caller.records(), c, |r| match r {
        FunctionRecord::Instruction {
            offset, decoded, ..
        } if *offset == site => offset.checked_add(decoded.length.into()),
        _ => None,
    })?
    else {
        return Ok(None);
    };
    let Some(AbstractValue::EntryStack { offset: coordinate }) = caller
        .call_inputs(site, c)?
        .and_then(|i| i.registers.get(2))
    else {
        return Ok(None);
    };
    for record in caller.records() {
        c.checkpoint(1)?;
        match record {
            FunctionRecord::Edge {
                from,
                target: Some(to),
                external: false,
                ..
            } if *to >= start && (*from < start && *from != site || *from >= *to) => {
                return Ok(None);
            }
            FunctionRecord::Value {
                offset,
                register: 2,
                ..
            } if *offset >= start => return Ok(None),
            FunctionRecord::Transfer {
                offset,
                target,
                call: true,
            } if *offset >= start && exact(target).map(u64::from) == Some(setup.entry) => {
                return Ok(None);
            }
            _ => (),
        }
    }
    let interval = (setup.pointer, setup.pointer);
    let mut origins = AdmittedVec::new(memory);
    origins.push(
        StackOrigin {
            kind: StackOriginKind::SetupReturn,
            caller: entry,
            site,
            callee: setup.entry,
            interval,
        },
        c.position(),
    )?;
    Ok(Some(StackContext {
        capture_id: setup.capture_id,
        owner: entry,
        start,
        basis: StackBasis::Installed {
            previous_coordinate: *coordinate,
        },
        interval,
        residues: 1 << (setup.pointer % 4),
        id: identity()?,
        origins,
        executor_invariant: false,
    }))
}
