//! Finite-height lattice: unreachable -> bounded exact alternatives -> unknown.
//! Queue membership is bounded by the graph; no expression trees. The only
//! memory state is a bounded set of entry-stack slots holding words: a
//! word stored to an exact entry-stack address and loaded back unchanged, as
//! `core::hint::black_box` spills a function pointer.
use super::value_sets::Sets;
use super::*;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Value {
    Set(u32),
    Widened,
    Unknown,
    Expr(u32),
    Constant(u32),
    Image(u32),
    Section(u32, i64),
    Symbol(usize, i64),
    Stack(i64),
    // An incomplete relocation upper is never exported as a resolved address.
    Upper {
        symbol: usize,
        addend: i64,
        site: usize,
        pc: bool,
    },
}
/// Entry-stack slots one state tracks; a store beyond them is forgotten.
const SLOTS: usize = 8;
/// Integer registers, indexed through `Deref`, and the entry-stack words
/// known exactly, sorted by offset with the free slots last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    registers: [Value; 32],
    slots: [Option<(i64, Value)>; SLOTS],
}
impl std::ops::Deref for State {
    type Target = [Value; 32];
    fn deref(&self) -> &[Value; 32] {
        &self.registers
    }
}
impl std::ops::DerefMut for State {
    fn deref_mut(&mut self) -> &mut [Value; 32] {
        &mut self.registers
    }
}
impl State {
    fn slot(&self, offset: i64) -> Option<Value> {
        self.slots
            .iter()
            .flatten()
            .find(|(at, _)| *at == offset)
            .map(|(_, value)| *value)
    }
    /// Forget every slot a `width`-byte store at `offset` overlaps.
    fn clobber(&mut self, offset: i64, width: u8) {
        for slot in &mut self.slots {
            if let Some((at, _)) = *slot
                && at < offset.saturating_add(i64::from(width))
                && offset < at.saturating_add(4)
            {
                *slot = None;
            }
        }
        self.canonical();
    }
    fn remember(&mut self, offset: i64, value: Value) {
        self.clobber(offset, 4);
        if let Some(free) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *free = Some((offset, value));
        }
        self.canonical();
    }
    fn capture(&mut self, offset: i64, value: Value) {
        self.clobber(offset, 4);
        if let Some(last) = self.slots.last_mut()
            && last.is_some_and(|(at, _)| offset < at)
        {
            // Keep a fixed projection onto the lowest stack addresses, so
            // entry register saves cannot occupy every word indefinitely.
            *last = None;
        }
        self.remember(offset, value);
    }
    fn canonical(&mut self) {
        self.slots
            .sort_unstable_by_key(|slot| slot.map_or((1, 0), |(at, _)| (0, at)));
    }
}
fn unknown_state() -> State {
    let mut registers = [Value::Unknown; 32];
    registers[0] = Value::Constant(0);
    State {
        registers,
        slots: [None; SLOTS],
    }
}
/// Whether a slot may hold `v`: an exact leaf or a bounded set of them.
fn exact(v: Value) -> bool {
    matches!(
        v,
        Value::Set(_)
            | Value::Constant(_)
            | Value::Image(_)
            | Value::Section(..)
            | Value::Symbol(..)
            | Value::Stack(_)
    )
}
/// Join `new` into `old`: registers pointwise, slots only where both hold one.
fn join(
    old: &mut State,
    new: &State,
    sets: &mut Sets<'_>,
    control: &mut dyn RunControl,
) -> Result<bool> {
    let mut changed = false;
    for (a, b) in old.registers.iter_mut().zip(new.registers) {
        let joined = sets.join(*a, b, control)?;
        changed |= *a != joined;
        *a = joined;
    }
    for i in 0..SLOTS {
        let Some((at, value)) = old.slots[i] else {
            continue;
        };
        let joined = match new.slot(at) {
            Some(other) => sets.join(value, other, control)?,
            None => Value::Unknown,
        };
        let next = (exact(joined) || matches!(joined, Value::Expr(_))).then_some((at, joined));
        changed |= old.slots[i] != next;
        old.slots[i] = next;
    }
    old.canonical();
    Ok(changed)
}
#[derive(Clone, Copy)]
struct Relocation {
    role: ValueRelocation,
    site: usize,
    symbol: usize,
    addend: i64,
    pair: Option<usize>,
}
#[derive(Clone, Copy)]
struct Operation {
    op: SemanticOp,
    opaque_call: bool,
    /// The function a call reaches, where its instruction or relocation
    /// names it.
    callee: Option<CalleeAt>,
    relocation: Option<Relocation>,
    gap: Option<SemanticGapReason>,
}

impl Operation {
    fn name_callee(&mut self, callee: CalleeAt) {
        if self.callee.replace(callee).is_some() {
            self.gap = Some(SemanticGapReason::UnresolvedRelocation);
        }
    }
}

/// Where a call names its callee: an address in an image, or the
/// relocation (by index) a relocatable object's call carries.
#[derive(Clone, Copy)]
enum CalleeAt {
    Address(u32),
    Relocation(usize),
}

fn prepare(
    input: &FunctionInput<'_>,
    node: &Node,
    isa: &dyn FunctionSemantics,
    control: &mut dyn RunControl,
) -> Result<Operation> {
    let start = (node.offset - input.extent.start) as usize;
    let op = isa.lift(&input.bytes[start..start + node.decoded.length as usize]);
    validate(op)?;
    let mut result = Operation {
        op,
        opaque_call: matches!(
            node.decoded.flow,
            InstructionFlow::Jump { link: true, .. } | InstructionFlow::Indirect { link: true, .. }
        ),
        callee: None,
        relocation: None,
        gap: None,
    };
    if node.conflict {
        result.gap = Some(SemanticGapReason::ConflictingBoundary);
        return Ok(result);
    }
    if op == SemanticOp::Unsupported {
        result.gap = Some(SemanticGapReason::UnsupportedInstruction);
    }
    // ET_EXEC bytes already contain link edits. Retained static relocations
    // remain provenance; applying object-file relocation semantics again is wrong.
    if input.image.is_some() {
        if let InstructionFlow::Jump {
            displacement,
            link: true,
        } = node.decoded.flow
        {
            let target = (node.offset as u32).wrapping_add(displacement as u32);
            result.callee = Some(CalleeAt::Address(target));
        }
        return Ok(result);
    }
    for site in input.relocations.first_at(node.offset.saturating_sub(4))
        ..input.relocations.first_at(node.offset.saturating_add(1))
    {
        let raw = &input.relocations[site];
        control.checkpoint(1)?;
        if raw.offset.checked_add(4) == Some(node.offset)
            && raw.offset >= input.extent.start
            && isa.value_relocation(raw, SemanticOp::Link { dest: 0 }) == ValueRelocation::CallUpper
            && let Some((base, ..)) = node.decoded.flow.through_register()
        {
            let start = (raw.offset - input.extent.start) as usize;
            if matches!(isa.lift(&input.bytes[start..start + 4]), SemanticOp::Upper { dest, pc_relative: true, .. } if dest == base && dest != 0)
            {
                result.opaque_call = true;
                result.name_callee(CalleeAt::Relocation(site));
            }
        }
        if raw.offset != node.offset {
            continue;
        }
        // A `jal` or `c.jal` call names its callee by its own jump
        // relocation (`R_RISCV_JAL`, `R_RISCV_RVC_JUMP`).
        if matches!(node.decoded.flow, InstructionFlow::Jump { link: true, .. })
            && input.relocations.normalized(site).kind == ReferenceKind::Branch
        {
            result.name_callee(CalleeAt::Relocation(site));
        }
        let role = isa.value_relocation(raw, op);
        if role == ValueRelocation::Ignore {
            continue;
        }
        if role == ValueRelocation::Unsupported || result.relocation.is_some() {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        }
        let normalized = input.relocations.normalized(site);
        let compatible = matches!(
            (role, op),
            (
                ValueRelocation::UpperAbsolute,
                SemanticOp::Upper {
                    pc_relative: false,
                    ..
                }
            ) | (
                ValueRelocation::UpperPcRelative | ValueRelocation::CallUpper,
                SemanticOp::Upper {
                    pc_relative: true,
                    ..
                }
            ) | (
                ValueRelocation::LowerAbsolute | ValueRelocation::LowerPcRelative,
                SemanticOp::Integer {
                    op: IntegerOp::Add,
                    right: Operand::Immediate(_),
                    ..
                } | SemanticOp::Memory { .. }
            )
        );
        if !compatible || !normalized.known {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        }
        let symbol = input.relocations.symbol(site);
        let pair = input.relocations.pair(site);
        let (Some(symbol), Some(addend)) = (symbol, normalized.addend) else {
            result.gap = Some(SemanticGapReason::UnresolvedRelocation);
            continue;
        };
        result.relocation = Some(Relocation {
            role,
            site,
            symbol,
            addend,
            pair,
        });
    }
    Ok(result)
}
fn position(control: &mut dyn RunControl, section: u32, offset: u64) {
    let mut p = control.position();
    p.table = Some(u64::from(section));
    p.entry = Some(offset);
    control.set_position(p);
}
fn validate(op: SemanticOp) -> Result<()> {
    let operand = |v| matches!(v, Operand::Immediate(_) | Operand::Register(0..=31));
    let valid = match op {
        SemanticOp::Integer {
            dest, left, right, ..
        } => dest < 32 && operand(left) && operand(right),
        SemanticOp::Upper { dest, .. }
        | SemanticOp::Link { dest }
        | SemanticOp::Opaque { dest } => dest < 32,
        SemanticOp::Memory {
            kind,
            base,
            dest,
            source,
            width,
            ..
        } => {
            base < 32
                && dest.is_none_or(|r| r < 32)
                && source.is_none_or(|r| r < 32)
                && match kind {
                    MemoryKind::Load => {
                        matches!(width, 1 | 2 | 4) && dest.is_some() && source.is_none()
                    }
                    MemoryKind::Store => {
                        matches!(width, 1 | 2 | 4) && dest.is_none() && source.is_some()
                    }
                    MemoryKind::LoadReserved => width == 4 && dest.is_some() && source.is_none(),
                    MemoryKind::StoreConditional | MemoryKind::Atomic => {
                        width == 4 && dest.is_some() && source.is_some()
                    }
                    MemoryKind::FloatLoad | MemoryKind::FloatStore => {
                        width == 4 && dest.is_none() && source.is_none()
                    }
                }
        }
        SemanticOp::Fence {
            fm,
            predecessor,
            successor,
        } => fm < 16 && predecessor < 16 && successor < 16,
        SemanticOp::None | SemanticOp::Unsupported => true,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::Integrity,
            "semantic producer returned invalid operands",
        ))
    }
}
fn operand(s: &State, op: Operand) -> Value {
    match op {
        Operand::Register(r) => s.get(r as usize).copied().unwrap_or(Value::Unknown),
        Operand::Immediate(v) => Value::Constant(v),
    }
}
fn offset(v: Value, delta: i64) -> Value {
    if delta == 0 {
        return v;
    }
    match v {
        Value::Constant(x) => Value::Constant(x.wrapping_add(delta as u32)),
        Value::Image(x) => Value::Image(x.wrapping_add(delta as u32)),
        Value::Section(s, n) => n
            .checked_add(delta)
            .map_or(Value::Unknown, |n| Value::Section(s, n)),
        Value::Symbol(s, n) => n
            .checked_add(delta)
            .map_or(Value::Unknown, |n| Value::Symbol(s, n)),
        Value::Stack(n) => n.checked_add(delta).map_or(Value::Unknown, Value::Stack),
        _ => Value::Unknown,
    }
}
fn integer(op: IntegerOp, a: Value, b: Value) -> Value {
    use IntegerOp::*;
    if let (Value::Constant(a), Value::Constant(b)) = (a, b) {
        return Value::Constant(op.evaluate(a, b));
    }
    match (op, a, b) {
        (Add, v, Value::Constant(n)) | (Add, Value::Constant(n), v) => {
            offset(v, i64::from(n as i32))
        }
        (Sub, v, Value::Constant(n)) => offset(v, -i64::from(n as i32)),
        _ => Value::Unknown,
    }
}
pub fn fold_integer(op: IntegerOp, a: u32, b: u32) -> u32 {
    match integer(op, Value::Constant(a), Value::Constant(b)) {
        Value::Constant(value) => value,
        _ => unreachable!("two RV32 constants always produce a constant"),
    }
}
fn lower(base: Value, r: Relocation) -> Value {
    match base {
        Value::Upper {
            symbol,
            addend,
            site,
            pc,
        } if symbol == r.symbol
            && addend == r.addend
            && ((r.role == ValueRelocation::LowerAbsolute && !pc)
                || (r.role == ValueRelocation::LowerPcRelative && pc && r.pair == Some(site))) =>
        {
            Value::Symbol(symbol, addend)
        }
        _ => Value::Unknown,
    }
}
#[derive(Default)]
struct Effects {
    register: Option<(u8, Value)>,
    memory: Option<(MemoryKind, u8, Value, Option<Value>)>,
    gap: Option<SemanticGapReason>,
}
fn transfer(
    operation: Operation,
    node: &Node,
    input: &FunctionInput<'_>,
    incoming: State,
    control: &mut dyn RunControl,
    sets: &mut Sets<'_>,
) -> Result<(State, Effects)> {
    let mut state = incoming;
    let mut effects = Effects::default();
    if let Some(gap) = operation.gap {
        effects.gap = Some(gap);
        if gap == SemanticGapReason::UnresolvedRelocation {
            match operation.op {
                SemanticOp::Memory {
                    kind,
                    width,
                    dest,
                    source,
                    swap,
                    ..
                } => {
                    effects.memory = Some((
                        kind,
                        width,
                        Value::Unknown,
                        source.map(|r| {
                            if kind == MemoryKind::Atomic && !swap {
                                Value::Unknown
                            } else {
                                match operand(&incoming, Operand::Register(r)) {
                                    Value::Constant(n) => Value::Constant(match width {
                                        1 => n & 255,
                                        2 => n & 65535,
                                        _ => n,
                                    }),
                                    v if width == 4 => v,
                                    _ => Value::Unknown,
                                }
                            }
                        }),
                    ));
                    effects.register = dest.map(|r| {
                        (
                            r,
                            if r == 0 {
                                Value::Constant(0)
                            } else {
                                Value::Unknown
                            },
                        )
                    });
                }
                SemanticOp::Integer { dest, .. }
                | SemanticOp::Upper { dest, .. }
                | SemanticOp::Link { dest }
                | SemanticOp::Opaque { dest } => {
                    effects.register = Some((
                        dest,
                        if dest == 0 {
                            Value::Constant(0)
                        } else {
                            Value::Unknown
                        },
                    ));
                }
                _ => {}
            }
        }
        return Ok((unknown_state(), effects));
    }
    let rel = operation.relocation;
    match operation.op {
        SemanticOp::Integer {
            op,
            dest,
            left,
            right,
        } => {
            let value = if let Some(r) = rel {
                sets.map(operand(&state, left), control, |v, _| Ok(lower(v, r)))?
            } else {
                sets.binary(
                    operand(&state, left),
                    operand(&state, right),
                    control,
                    |a, b| integer(op, a, b),
                )?
            };
            effects.register = Some((dest, value));
            if rel.is_some() && value == Value::Unknown {
                effects.gap = Some(SemanticGapReason::UnresolvedRelocation);
            }
        }
        SemanticOp::Upper {
            dest,
            value,
            pc_relative,
        } => {
            let v = if let Some(r) = rel {
                Value::Upper {
                    symbol: r.symbol,
                    addend: r.addend,
                    site: r.site,
                    pc: pc_relative,
                }
            } else if pc_relative {
                offset(
                    if input.image.is_some() {
                        Value::Image(node.offset as u32)
                    } else {
                        Value::Section(input.section, node.offset as i64)
                    },
                    i64::from(value as i32),
                )
            } else {
                Value::Constant(value)
            };
            effects.register = Some((dest, v));
        }
        SemanticOp::Link { dest } => {
            effects.register = Some((
                dest,
                if input.image.is_some() {
                    Value::Image((node.offset + u64::from(node.decoded.length)) as u32)
                } else {
                    Value::Section(
                        input.section,
                        (node.offset + u64::from(node.decoded.length)) as i64,
                    )
                },
            ))
        }
        SemanticOp::Memory {
            kind,
            base,
            displacement,
            width,
            dest,
            source,
            swap,
            signed,
        } => {
            let address = if let Some(r) = rel {
                sets.map(operand(&state, Operand::Register(base)), control, |v, _| {
                    Ok(lower(v, r))
                })?
            } else {
                sets.map(operand(&state, Operand::Register(base)), control, |v, _| {
                    Ok(offset(v, i64::from(displacement)))
                })?
            };
            if rel.is_some() && address == Value::Unknown {
                effects.gap = Some(SemanticGapReason::UnresolvedRelocation);
            }
            let value = source
                .map(|r| {
                    let v = operand(&state, Operand::Register(r));
                    if kind == MemoryKind::Atomic && !swap {
                        Ok(Value::Unknown)
                    } else {
                        sets.map(v, control, |v, _| {
                            Ok(match v {
                                Value::Constant(n) => Value::Constant(match width {
                                    1 => n & 255,
                                    2 => n & 65535,
                                    _ => n,
                                }),
                                v if width == 4 => v,
                                _ => Value::Unknown,
                            })
                        })
                    }
                })
                .transpose()?
                // An FP register's word is outside the integer model.
                .or((kind == MemoryKind::FloatStore).then_some(Value::Unknown));
            effects.memory = Some((kind, width, address, value));
            match (kind, address) {
                (MemoryKind::Load | MemoryKind::LoadReserved | MemoryKind::FloatLoad, _) => {}
                (MemoryKind::Store, Value::Stack(at)) if width == 4 => match value {
                    Some(v) if exact(v) => state.remember(at, v),
                    _ => state.clobber(at, width),
                },
                (_, Value::Stack(at)) => state.clobber(at, width),
                // Any other address may alias the stack.
                _ => state.slots = [None; SLOTS],
            }
            let mut loaded = Value::Unknown;
            if kind == MemoryKind::Load
                && width == 4
                && let Value::Stack(at) = address
                && let Some(v) = state.slot(at)
            {
                loaded = v;
            } else if kind == MemoryKind::Load
                && let Some(image) = input.image
            {
                loaded = sets.map(address, control, |address, control| {
                    if let Value::Constant(address) | Value::Image(address) = address {
                        let mut bytes = [0; 4];
                        if image.read_constant(address, &mut bytes[..width as usize], control)? {
                            let value = u32::from_le_bytes(bytes);
                            return Ok(Value::Constant(match (width, signed) {
                                (1, true) => value as i8 as i32 as u32,
                                (2, true) => value as i16 as i32 as u32,
                                _ => value,
                            }));
                        }
                    }
                    Ok(Value::Unknown)
                })?;
            }
            effects.register = dest.map(|r| (r, loaded));
        }
        SemanticOp::Opaque { dest } => effects.register = Some((dest, Value::Unknown)),
        SemanticOp::None | SemanticOp::Fence { .. } => {}
        SemanticOp::Unsupported => unreachable!("unsupported operation carries a gap"),
    }
    if let Some((r, v)) = effects.register {
        if r != 0 {
            if let Some(slot) = state.get_mut(r as usize) {
                *slot = v;
            }
        } else {
            effects.register = Some((0, Value::Constant(0)));
        }
    }
    if operation.opaque_call {
        state = unknown_state();
        effects.gap = Some(SemanticGapReason::OpaqueCall);
    }
    Ok((state, effects))
}
fn public(v: Value, input: &FunctionInput<'_>, sets: &Sets<'_>) -> AbstractValue {
    match v {
        Value::Unknown | Value::Widened | Value::Upper { .. } => AbstractValue::Unknown,
        Value::Set(_) => {
            let set = sets.get(v);
            let mut values: Vec<_> = set.values[..set.len]
                .iter()
                .map(|v| {
                    ValueAlternative::from_value(&public(*v, input, sets))
                        .expect("sets contain exact leaves")
                })
                .collect();
            values.sort_unstable();
            values.dedup();
            if values.len() == 1 {
                values[0].as_value()
            } else {
                AbstractValue::Alternatives {
                    values: ValueAlternatives::new(values)
                        .expect("canonical nontrivial internal set"),
                }
            }
        }
        Value::Expr(id) => AbstractValue::Expression { id },
        Value::Constant(value) => AbstractValue::Constant { value },
        Value::Image(address) => AbstractValue::ImageAddress { address },
        Value::Section(section, offset) => AbstractValue::Section { section, offset },
        Value::Symbol(i, addend) => AbstractValue::Symbol {
            symbol: input.relocations[i].target.symbol.clone(),
            addend,
        },
        Value::Stack(offset) => AbstractValue::EntryStack { offset },
    }
}
fn known(v: &AbstractValue) -> bool {
    !matches!(v, AbstractValue::Unknown | AbstractValue::Expression { .. })
}

/// Instructions a dispatch's slice walks back through its block.
const MAX_SLICE: usize = 32;

/// The table dispatch of the jump at node `at` through register `target`:
/// `target` is the word loaded from `base + (index << scale)`, the load, the
/// addition and the scaling all in the jump's own block. Returns the base
/// and index values on entry to the instructions that read them; the base
/// must be a constant, an image address or a symbol. A base the block or a
/// loop's preheader forms holds on every pass while the index may merge; one
/// a later pass merges leaves that pass without the record.
fn table_dispatch(
    at: usize,
    target: u8,
    nodes: &[Node],
    operations: &[Operation],
    states: &[Option<State>],
    incoming: &[u8],
    control: &mut dyn RunControl,
) -> Result<Option<(Value, Value, u8)>> {
    // The nearest node before `from` in the same block that writes
    // `register`, within the slice's reach.
    let mut writer = |from: usize, register: u8| -> Result<Option<usize>> {
        let mut j = from;
        for _ in 0..MAX_SLICE {
            control.checkpoint(1)?;
            if j == 0 {
                return Ok(None);
            }
            let previous = &nodes[j - 1];
            let continues = matches!(previous.decoded.flow, InstructionFlow::Next)
                && incoming[j] == 1
                && previous.offset + u64::from(previous.decoded.length) == nodes[j].offset;
            if !continues {
                return Ok(None);
            }
            j -= 1;
            let op = &operations[j];
            let dest = match op.op {
                SemanticOp::Integer { dest, .. }
                | SemanticOp::Upper { dest, .. }
                | SemanticOp::Link { dest }
                | SemanticOp::Opaque { dest } => Some(dest),
                SemanticOp::Memory { dest, .. } => dest,
                _ => None,
            };
            if dest == Some(register) {
                return Ok((op.relocation.is_none() && op.gap.is_none()).then_some(j));
            }
        }
        Ok(None)
    };
    let value = |node: usize, register: u8| states[node].map(|state| state[register as usize]);
    let Some(load) = writer(at, target)? else {
        return Ok(None);
    };
    let SemanticOp::Memory {
        kind: MemoryKind::Load,
        base: address,
        displacement: 0,
        width: 4,
        ..
    } = operations[load].op
    else {
        return Ok(None);
    };
    let Some(add) = writer(load, address)? else {
        return Ok(None);
    };
    let SemanticOp::Integer {
        op,
        left: Operand::Register(left),
        right: Operand::Register(right),
        ..
    } = operations[add].op
    else {
        return Ok(None);
    };
    let (index, base, scale) = match op {
        IntegerOp::ShiftAdd1 | IntegerOp::ShiftAdd2 | IntegerOp::ShiftAdd3 => {
            let scale = match op {
                IntegerOp::ShiftAdd1 => 1,
                IntegerOp::ShiftAdd2 => 2,
                _ => 3,
            };
            (value(add, left), value(add, right), scale)
        }
        IntegerOp::Add => {
            let mut found = None;
            for (scaled, base) in [(left, right), (right, left)] {
                let Some(shift) = writer(add, scaled)? else {
                    continue;
                };
                if let SemanticOp::Integer {
                    op: IntegerOp::Shl,
                    left: Operand::Register(index),
                    right: Operand::Immediate(scale),
                    ..
                } = operations[shift].op
                    && scale < 32
                {
                    found = Some((value(shift, index), value(add, base), scale as u8));
                    break;
                }
            }
            match found {
                Some(found) => found,
                None => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    let (Some(index), Some(base)) = (index, base) else {
        return Ok(None);
    };
    Ok(matches!(
        base,
        Value::Constant(_) | Value::Image(_) | Value::Symbol(..)
    )
    .then_some((base, index, scale)))
}

#[cfg(test)]
pub(super) fn analyze(
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    isa: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
) -> Result<SemanticSummary> {
    analyze_with(
        input,
        nodes,
        edges,
        isa,
        memory,
        control,
        sink,
        false,
        None,
        false,
        None,
        &mut AdmittedVec::new(memory),
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn analyze_with(
    input: &FunctionInput<'_>,
    nodes: &[Node],
    edges: &[Edge],
    isa: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
    symbolic: bool,
    abi: Option<CallAbi>,
    symbolic_spills: bool,
    stack_calls: Option<&writes::StackCalls<'_>>,
    noreturn: &mut AdmittedVec<'_, u64>,
) -> Result<SemanticSummary> {
    control.phase(RunPhase::AnalyzeValues)?;
    let bytes_per_node = std::mem::size_of::<Option<State>>()
        + std::mem::size_of::<Operation>()
        + std::mem::size_of::<usize>()
        + 2;
    let bytes = nodes
        .len()
        .checked_mul(bytes_per_node)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "value state capacity overflow"))?;
    let _reservation = memory.reserve(bytes as u64, control.position())?;
    // All section references use this captured object's fixed-size artifact ID.
    let symbol_bytes = input
        .relocations
        .first()
        .map_or(0, |r| r.target.symbol.object.artifact.allocated_bytes());
    let public_value_bytes = MAX_VALUE_ALTERNATIVES as u64
        * (std::mem::size_of::<ValueAlternative>() as u64 + symbol_bytes);
    let _public_workspace = memory.reserve(
        32 * (std::mem::size_of::<AbstractValue>() as u64 + public_value_bytes),
        control.position(),
    )?;
    let mut symbols = Symbols::new(if symbolic { nodes.len() } else { 0 }, memory, control)?;
    symbols.symbolic_spills = symbolic_spills;
    symbols.stack_calls = stack_calls;
    let mut states = reserve_vec::<Option<State>>(nodes.len())?;
    states.resize(nodes.len(), None);
    let mut queued = reserve_vec::<bool>(nodes.len())?;
    queued.resize(nodes.len(), false);
    let mut operations = reserve_vec::<Operation>(nodes.len())?;
    for node in nodes {
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        operations.push(prepare(input, node, isa, control)?);
    }
    // The edges into each node, saturating: a node another path enters
    // starts a block, where a dispatch's slice stops.
    let mut incoming = reserve_vec::<u8>(nodes.len())?;
    incoming.resize(nodes.len(), 0);
    for edge in edges {
        control.checkpoint(1)?;
        if let Some(target) = edge.target
            && let Ok(at) = nodes.binary_search_by_key(&target, |n| n.offset)
        {
            incoming[at] = incoming[at].saturating_add(1);
        }
    }
    let mut queue = VecDeque::new();
    queue
        .try_reserve_exact(nodes.len())
        .map_err(|_| Error::new(ErrorCode::ResourceLimited, "value queue allocation refused"))?;
    if let Ok(entry) = nodes.binary_search_by_key(&input.extent.start, |n| n.offset) {
        let mut state = unknown_state();
        if symbolic {
            for (r, slot) in state.iter_mut().enumerate().skip(1) {
                *slot = symbols.expression(
                    u64::MAX,
                    Expression::EntryRegister { register: r as u8 },
                    control,
                )?;
            }
        }
        state[2] = Value::Stack(0);
        states[entry] = Some(state);
        queued[entry] = true;
        queue.push_back(entry);
    }
    while let Some(index) = queue.pop_front() {
        queued[index] = false;
        let node = &nodes[index];
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        let (out, _) = evaluate(
            operations[index],
            node,
            input,
            states[index].unwrap(),
            control,
            &mut symbols,
            symbolic,
            abi,
        )?;
        let first = edges.partition_point(|e| e.from < node.offset);
        for edge in edges[first..].iter().take_while(|e| e.from == node.offset) {
            control.checkpoint(1)?;
            if !matches!(
                edge.relation,
                EdgeKind::Fallthrough
                    | EdgeKind::Taken
                    | EdgeKind::Jump
                    | EdgeKind::PossibleContinuation
            ) {
                continue;
            }
            let Some(target) = edge.target else { continue };
            let Ok(to) = nodes.binary_search_by_key(&target, |n| n.offset) else {
                continue;
            };
            let changed = match &mut states[to] {
                None => {
                    states[to] = Some(out);
                    true
                }
                Some(old) => join(old, &out, &mut symbols.sets, control)?,
            };
            if changed && !queued[to] {
                queued[to] = true;
                queue.push_back(to);
            }
        }
    }
    // Emit after convergence. Re-evaluation below only reuses stable DAG nodes.
    let mut emitted = 0;
    let mut summary = SemanticSummary {
        complete: true,
        ..Default::default()
    };
    for (i, node) in nodes.iter().enumerate() {
        position(control, input.section, node.offset);
        control.checkpoint(1)?;
        let start = (node.offset - input.extent.start) as usize;
        sink.record(
            &FunctionRecord::Instruction {
                offset: node.offset,
                bytes: input.bytes[start..start + node.decoded.length as usize].into(),
                decoded: node.decoded.clone(),
            },
            control,
        )?;
        let Some(state) = states[i] else { continue };
        if proven_noreturn(operations[i], node, input, &state, &symbols.sets, control)? {
            // Nodes are sorted by offset, so this is also the sorted set
            // the next CFG pass uses to omit impossible continuations.
            noreturn.push(node.offset, control.position())?;
        }
        if let SemanticOp::Fence {
            fm,
            predecessor,
            successor,
        } = operations[i].op
        {
            sink.record(
                &FunctionRecord::Fence {
                    offset: node.offset,
                    fm,
                    predecessor,
                    successor,
                },
                control,
            )?;
        }
        let (_, mut effects) = evaluate(
            operations[i],
            node,
            input,
            state,
            control,
            &mut symbols,
            symbolic,
            abi,
        )?;
        if symbolic
            && let InstructionFlow::Indirect {
                base,
                offset: 0,
                link: false,
            } = node.decoded.flow
            && let Some((table, index, scale)) =
                table_dispatch(i, base, nodes, &operations, &states, &incoming, control)?
        {
            sink.record(
                &FunctionRecord::Dispatch {
                    offset: node.offset,
                    base: public(table, input, &symbols.sets),
                    index: public(index, input, &symbols.sets),
                    scale,
                },
                control,
            )?;
        }
        // In a relocatable object, a jump the graph left without a target (an
        // `indirect` edge, which already makes its control flow incomplete),
        // such as one through a jump table whose base a register holds, is a
        // gap. Returns, tail calls and relocated jumps have targets and are
        // not. An image finds its unexpanded jumps below, where their
        // targets are known.
        let first = edges.partition_point(|e| e.from < node.offset);
        let unexpanded = input.image.is_none()
            && edges[first..]
                .iter()
                .take_while(|e| e.from == node.offset)
                .any(|e| e.relation == EdgeKind::Indirect);
        if unexpanded {
            effects
                .gap
                .get_or_insert(SemanticGapReason::UnexpandedControlFlow);
        }
        if symbolic {
            if let Some((test, left, right)) =
                isa.branch(&input.bytes[start..start + node.decoded.length as usize])
            {
                sink.record(
                    &FunctionRecord::Condition {
                        offset: node.offset,
                        test,
                        left: public(operand(&state, left), input, &symbols.sets),
                        right: public(operand(&state, right), input, &symbols.sets),
                    },
                    control,
                )?;
            }
            // The registers at a call, and at a relocatable object's jump the
            // graph leaves without a target.
            if operations[i].opaque_call || unexpanded {
                sink.record(
                    &FunctionRecord::CallInputs {
                        offset: node.offset,
                        registers: state
                            .iter()
                            .map(|v| public(*v, input, &symbols.sets))
                            .collect(),
                    },
                    control,
                )?;
            }
            if matches!(node.decoded.flow, InstructionFlow::Return { .. }) {
                sink.record(
                    &FunctionRecord::ReturnValue {
                        offset: node.offset,
                        low: public(state[10], input, &symbols.sets),
                        high: public(state[11], input, &symbols.sets),
                    },
                    control,
                )?;
            }
        }
        while emitted < symbols.records.len() {
            let (offset, expression) = &symbols.records[emitted];
            sink.record(
                &FunctionRecord::Expression {
                    id: emitted as u32,
                    offset: *offset,
                    expression: expression.clone(),
                },
                control,
            )?;
            emitted += 1;
        }
        if input.image.is_some() {
            let transfer = match node.decoded.flow {
                InstructionFlow::Jump { displacement, link } => {
                    let address = (node.offset as u32).wrapping_add(displacement as u32);
                    (link
                        || u64::from(address) < input.extent.start
                        || u64::from(address) >= input.extent.start + input.extent.length)
                        .then_some((Value::Image(address), link))
                }
                flow @ (InstructionFlow::Indirect { .. } | InstructionFlow::Return { .. }) => {
                    let (base, displacement, link) = flow.through_register().unwrap_or_default();
                    let target = indirect_target(
                        state[base as usize],
                        displacement,
                        &mut symbols.sets,
                        control,
                    )?;
                    // A return whose address the values know is a transfer.
                    let return_pattern =
                        matches!(flow, InstructionFlow::Return { .. }) && target == Value::Unknown;
                    let targets = symbols.sets.get(target);
                    let is_local = |v: &Value| matches!(v, Value::Image(a) if u64::from(*a) >= input.extent.start && u64::from(*a) < input.extent.start + input.extent.length);
                    let local = targets.values[..targets.len].iter().all(is_local);
                    if !link && targets.values[..targets.len].iter().any(is_local) {
                        effects
                            .gap
                            .get_or_insert(SemanticGapReason::UnexpandedControlFlow);
                        // The registers at a jump the graph does not expand
                        // yet, such as one through a jump table whose base a
                        // register holds: a caller that reads the table from
                        // them can supply its targets.
                        if !return_pattern && !operations[i].opaque_call {
                            sink.record(
                                &FunctionRecord::CallInputs {
                                    offset: node.offset,
                                    registers: state
                                        .iter()
                                        .map(|v| public(*v, input, &symbols.sets))
                                        .collect(),
                                },
                                control,
                            )?;
                        }
                    }
                    if !link && (return_pattern || local) {
                        None
                    } else {
                        Some((target, link))
                    }
                }
                _ => None,
            };
            if let Some((target, call)) = transfer {
                if !call && !operations[i].opaque_call {
                    sink.record(
                        &FunctionRecord::CallInputs {
                            offset: node.offset,
                            registers: state
                                .iter()
                                .map(|v| public(*v, input, &symbols.sets))
                                .collect(),
                        },
                        control,
                    )?;
                }
                // A jump the graph follows through a known table, whose
                // target the values leave unknown, goes where the table's
                // edges go: no opaque transfer and no gap. Targets the values
                // resolve stay transfers. Its registers stay for a caller
                // that proves the table again.
                let followed = !call
                    && target == Value::Unknown
                    && matches!(node.decoded.flow, InstructionFlow::Indirect { .. })
                    && super::known_jump(input.jumps, node.offset, control)?.is_some();
                if !followed {
                    effects.gap.get_or_insert(SemanticGapReason::OpaqueCall);
                    sink.record(
                        &FunctionRecord::Transfer {
                            offset: node.offset,
                            target: if node.conflict {
                                AbstractValue::Unknown
                            } else {
                                public(target, input, &symbols.sets)
                            },
                            call,
                        },
                        control,
                    )?;
                }
            }
        }
        let relocation = operations[i].relocation.map(|r| {
            let raw = &input.relocations[r.site];
            RelocationSite {
                section: raw.section,
                index: raw.index,
            }
        });
        if let Some((register, v)) = effects.register {
            let value = public(v, input, &symbols.sets);
            summary.values += 1;
            summary.known_values += u64::from(known(&value));
            sink.record(
                &FunctionRecord::Value {
                    offset: node.offset,
                    register,
                    value,
                    relocation,
                },
                control,
            )?;
        }
        if let Some((access, width, addr, v)) = effects.memory {
            let address = public(addr, input, &symbols.sets);
            summary.accesses += 1;
            summary.known_addresses += u64::from(known(&address));
            sink.record(
                &FunctionRecord::MemoryAccess {
                    offset: node.offset,
                    access,
                    width,
                    address,
                    value: v.map(|v| public(v, input, &symbols.sets)),
                    relocation,
                },
                control,
            )?;
        }
        if state.contains(&Value::Widened)
            || effects.register.is_some_and(|(_, v)| v == Value::Widened)
            || effects
                .memory
                .is_some_and(|(_, _, a, v)| a == Value::Widened || v == Some(Value::Widened))
        {
            summary.complete = false;
            summary.gaps += 1;
            sink.record(
                &FunctionRecord::SemanticGap {
                    offset: node.offset,
                    reason: SemanticGapReason::AlternativeLimit,
                },
                control,
            )?;
        }
        if let Some(reason) = effects.gap {
            summary.complete = false;
            summary.gaps += 1;
            sink.record(
                &FunctionRecord::SemanticGap {
                    offset: node.offset,
                    reason,
                },
                control,
            )?;
        }
    }
    Ok(summary)
}

struct Symbols<'a> {
    stack_calls: Option<&'a writes::StackCalls<'a>>,
    symbolic_spills: bool,
    sets: Sets<'a>,
    records: AdmittedVec<'a, (u64, Expression)>,
    slots: AdmittedVec<'a, Option<u32>>,
    payloads: AdmittedVec<'a, MemoryReservation<'a>>,
    memory: &'a WorkingMemory,
    limit: usize,
}
impl<'a> Symbols<'a> {
    fn new(nodes: usize, memory: &'a WorkingMemory, _c: &mut dyn RunControl) -> Result<Self> {
        let limit = if nodes == 0 {
            0
        } else {
            nodes
                .checked_mul(8)
                .and_then(|n| n.checked_add(32))
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimited, "expression capacity overflow")
                })?
        };
        Ok(Self {
            symbolic_spills: false,
            stack_calls: None,
            sets: Sets::new(memory),
            records: AdmittedVec::new(memory),
            slots: AdmittedVec::new(memory),
            payloads: AdmittedVec::new(memory),
            memory,
            limit,
        })
    }
    fn expression(
        &mut self,
        offset: u64,
        expr: Expression,
        c: &mut dyn RunControl,
    ) -> Result<Value> {
        let hash = Self::hash(offset, &expr, c)?;
        let mut at = self.find(hash, offset, &expr, c)?;
        if let Some(id) = at.and_then(|at| self.slots[at]) {
            return Ok(Value::Expr(id));
        }
        if self.records.len() == self.limit {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "expression capacity exhausted",
            ));
        }
        let id = u32::try_from(self.records.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "expression ID overflow"))?;
        if self.slots.is_empty() || self.records.len() >= self.slots.len() / 2 {
            self.grow(c)?;
            at = self.find(hash, offset, &expr, c)?;
        }
        let payload = self.memory.reserve(expr.allocated_bytes(), c.position())?;
        self.payloads.push(payload, c.position())?;
        if let Err(error) = self.records.push((offset, expr), c.position()) {
            self.payloads.pop();
            return Err(error);
        }
        // Publish the slot only after both admitted owners accepted the record.
        self.slots[at.expect("nonempty expression index")] = Some(id);
        Ok(Value::Expr(id))
    }
    fn hash(offset: u64, expr: &Expression, c: &mut dyn RunControl) -> Result<usize> {
        // Leaves are bounded/nonrecursive; charge their dynamic storage too.
        c.checkpoint(1 + expr.allocated_bytes())?;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (offset, expr).hash(&mut hash);
        Ok(hash.finish() as usize)
    }
    fn find(
        &self,
        hash: usize,
        offset: u64,
        expr: &Expression,
        c: &mut dyn RunControl,
    ) -> Result<Option<usize>> {
        if self.slots.is_empty() {
            return Ok(None);
        }
        let mut at = hash & (self.slots.len() - 1);
        loop {
            c.checkpoint(1)?;
            match self.slots[at] {
                Some(id)
                    if self.records[id as usize].0 != offset
                        || &self.records[id as usize].1 != expr =>
                {
                    at = (at + 1) & (self.slots.len() - 1);
                }
                _ => return Ok(Some(at)),
            }
        }
    }
    fn grow(&mut self, c: &mut dyn RunControl) -> Result<()> {
        let size = self
            .slots
            .len()
            .checked_mul(2)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "expression index overflow"))?
            .max(64);
        // Build beside the old index, so cancellation/admission failure leaves it usable.
        let mut slots = AdmittedVec::new(self.memory);
        for _ in 0..size {
            c.checkpoint(1)?;
            slots.push(None, c.position())?;
        }
        for (id, (offset, expr)) in self.records.iter().enumerate() {
            let mut at = Self::hash(*offset, expr, c)? & (size - 1);
            loop {
                c.checkpoint(1)?;
                if slots[at].is_none() {
                    break;
                }
                at = (at + 1) & (size - 1);
            }
            slots[at] = Some(id as u32);
        }
        self.slots = slots;
        Ok(())
    }
    fn binary(
        &mut self,
        site: u64,
        op: IntegerOp,
        a: Value,
        b: Value,
        input: &FunctionInput<'_>,
        c: &mut dyn RunControl,
    ) -> Result<Value> {
        let simple = self.sets.binary(a, b, c, |a, b| integer(op, a, b))?;
        if simple != Value::Unknown {
            return Ok(simple);
        }
        if a == Value::Unknown || b == Value::Unknown {
            return Ok(Value::Unknown);
        }
        self.expression(
            site,
            Expression::Integer {
                op,
                left: public(a, input, &self.sets),
                right: public(b, input, &self.sets),
            },
            c,
        )
    }
}
#[allow(clippy::too_many_arguments)]
fn evaluate(
    op: Operation,
    node: &Node,
    input: &FunctionInput<'_>,
    incoming: State,
    c: &mut dyn RunControl,
    symbols: &mut Symbols<'_>,
    enabled: bool,
    abi: Option<CallAbi>,
) -> Result<(State, Effects)> {
    let (mut state, mut effects) = transfer(op, node, input, incoming, c, &mut symbols.sets)?;
    if !enabled || op.gap.is_some() {
        return Ok((state, effects));
    }
    match op.op {
        SemanticOp::Integer {
            op: int,
            dest,
            left,
            right,
        } if op.relocation.is_none() => {
            let v = symbols.binary(
                node.offset,
                int,
                operand(&incoming, left),
                operand(&incoming, right),
                input,
                c,
            )?;
            effects.register = Some((dest, v));
            if dest != 0 {
                state[dest as usize] = v;
            }
        }
        SemanticOp::Memory {
            kind,
            base,
            displacement,
            width,
            dest,
            source,
            signed,
            ..
        } if op.relocation.is_none() => {
            let address = symbols.binary(
                node.offset,
                IntegerOp::Add,
                incoming[base as usize],
                Value::Constant(displacement as u32),
                input,
                c,
            )?;
            let value = match source {
                Some(r) if kind == MemoryKind::Store => Some(symbols.binary(
                    node.offset,
                    IntegerOp::And,
                    incoming[r as usize],
                    Value::Constant(match width {
                        1 => 255,
                        2 => 65535,
                        _ => u32::MAX,
                    }),
                    input,
                    c,
                )?),
                _ => effects.memory.and_then(|m| m.3),
            };
            effects.memory = Some((kind, width, address, value));
            if symbols.symbolic_spills
                && kind == MemoryKind::Store
                && width == 4
                && let Value::Stack(at) = address
                && let Some(v @ Value::Expr(_)) = value
            {
                // A captured value, not a reread of its source field. Reuse
                // the stack lattice's overlap, alias, call and join rules.
                state.capture(at, v);
            }
            if kind == MemoryKind::Load
                && let Some(dest) = dest
                && dest != 0
                && state[dest as usize] == Value::Unknown
            {
                let v = symbols.expression(
                    node.offset,
                    Expression::Load {
                        address: public(address, input, &symbols.sets),
                        width,
                        signed,
                    },
                    c,
                )?;
                state[dest as usize] = v;
                effects.register = Some((dest, v));
            }
        }
        _ => (),
    }
    if op.opaque_call {
        state = unknown_state();
        if let Some(calls) = symbols.stack_calls
            && abi == Some(CallAbi::RiscvInteger)
            && input.image.is_some()
            && op.relocation.is_none()
            && !node.conflict
        {
            let target = match op.callee {
                Some(CalleeAt::Address(address)) => Value::Image(address),
                None => match node.decoded.flow.through_register() {
                    Some((base, displacement, true)) => indirect_target(
                        incoming[base as usize],
                        displacement,
                        &mut symbols.sets,
                        c,
                    )?,
                    _ => Value::Unknown,
                },
                _ => Value::Unknown,
            };
            let target = public(target, input, &symbols.sets);
            for (at, value) in incoming.slots.iter().flatten() {
                if calls.preserves(
                    input.extent.start,
                    node.offset,
                    &target,
                    |r| public(incoming[r as usize], input, &symbols.sets),
                    *at,
                    c,
                )? {
                    state.remember(*at, *value);
                }
            }
        }
        if let Some(abi) = abi {
            for r in abi.preserved() {
                state[usize::from(r)] = incoming[usize::from(r)];
            }
        }
        // A call's result registers name its results whatever is assumed
        // about the registers it preserves, unless the callee's own analysis
        // proved every value they hold when it returns.
        let callee = callee(op, node, input, &incoming);
        for r in CallAbi::RiscvInteger.results() {
            let returned = match callee {
                Some(callee) => returned(input.returns.returns(callee, r), &mut symbols.sets, c)?,
                None => None,
            };
            state[usize::from(r)] = match returned {
                Some(value) => value,
                None => symbols.expression(
                    node.offset,
                    Expression::CallResult {
                        callsite: node.offset,
                        register: r,
                    },
                    c,
                )?,
            };
        }
    }
    state[0] = Value::Constant(0);
    if matches!(effects.register, Some((0, _))) {
        effects.register = Some((0, Value::Constant(0)));
    }
    Ok((state, effects))
}

/// Shared with emitted Transfer facts and stack-call proof revalidation.
fn indirect_target(
    value: Value,
    displacement: i32,
    sets: &mut Sets<'_>,
    c: &mut dyn RunControl,
) -> Result<Value> {
    sets.map(value, c, |v, _| {
        Ok(match offset(v, i64::from(displacement)) {
            Value::Constant(n) | Value::Image(n) => Value::Image(n & !1),
            _ => Value::Unknown,
        })
    })
}

/// The callee of the call `op` at `node` makes: the address or relocated
/// global symbol its instruction names, or, in an image, the address the
/// register it jumps through holds before the call. A paired object call
/// also needs its exact relocated upper in that register on every path.
fn callee<'a>(
    op: Operation,
    node: &Node,
    input: &'a FunctionInput<'_>,
    incoming: &State,
) -> Option<Callee<'a>> {
    match op.callee {
        Some(CalleeAt::Address(address)) => Some(Callee::Address(address)),
        Some(CalleeAt::Relocation(site)) => {
            let reference = input.relocations.normalized(site);
            if input.relocations.get(site)?.offset != node.offset {
                let (base, ..) = node.decoded.flow.through_register()?;
                let upper = Value::Upper {
                    symbol: input.relocations.symbol(site)?,
                    addend: reference.addend?,
                    site,
                    pc: true,
                };
                // Adjacency identifies a candidate pair, not a dataflow
                // proof. A bypass, overwrite or different upper at a join
                // must not lend this relocation's callee to either port.
                if incoming.get(usize::from(base)) != Some(&upper) {
                    return None;
                }
            }
            let target = &reference.target;
            // Only a global symbol names one function across objects.
            (target.binding == 1 && !target.name.is_empty()).then_some(Callee::Symbol(&target.name))
        }
        None if input.image.is_some() => {
            let (base, offset, true) = node.decoded.flow.through_register()? else {
                return None;
            };
            match incoming[usize::from(base)] {
                Value::Constant(at) | Value::Image(at) => {
                    Some(Callee::Address(at.wrapping_add(offset as u32) & !1))
                }
                _ => None,
            }
        }
        None => None,
    }
}

/// Every possible callee must have an explicit behavior proof. Reuse the
/// result analysis's callee identity for direct and relocated calls; a
/// register merge additionally needs every exact alternative, with no
/// unknown leaf. No proof depends on pruning a continuation first.
fn proven_noreturn(
    op: Operation,
    node: &Node,
    input: &FunctionInput<'_>,
    incoming: &State,
    sets: &Sets<'_>,
    control: &mut dyn RunControl,
) -> Result<bool> {
    if !op.opaque_call || op.gap.is_some() || node.conflict {
        return Ok(false);
    }
    if let Some(CalleeAt::Relocation(site)) = op.callee
        && input.relocations.normalized(site).addend != Some(0)
    {
        return Ok(false);
    }
    if let Some(callee) = callee(op, node, input, incoming) {
        return Ok(input.call_behavior.noreturn(callee));
    }
    if op.callee.is_some() || input.image.is_none() {
        return Ok(false);
    }
    let Some((base, displacement, true)) = node.decoded.flow.through_register() else {
        return Ok(false);
    };
    let targets = sets.get(
        incoming
            .get(usize::from(base))
            .copied()
            .unwrap_or(Value::Unknown),
    );
    for value in targets.values.iter().take(targets.len) {
        control.checkpoint(1)?;
        let (Value::Constant(address) | Value::Image(address)) = value else {
            return Ok(false);
        };
        let address = address.wrapping_add(displacement as u32) & !1;
        if !input.call_behavior.noreturn(Callee::Address(address)) {
            return Ok(false);
        }
    }
    Ok(targets.len != 0)
}

/// The value a call's result register holds: the constants its callee
/// returns, while they fit one set of alternatives.
fn returned(
    values: Option<&[u32]>,
    sets: &mut Sets<'_>,
    c: &mut dyn RunControl,
) -> Result<Option<Value>> {
    let Some((&first, rest)) = values.and_then(<[u32]>::split_first) else {
        return Ok(None);
    };
    let mut value = Value::Constant(first);
    for &next in rest {
        value = sets.join(value, Value::Constant(next), c)?;
    }
    Ok((value != Value::Widened && value != Value::Unknown).then_some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rv32_arithmetic_handles_wrap_shifts_and_division_boundaries() {
        use IntegerOp::*;
        for (op, a, b, expected) in [
            (Add, u32::MAX, 1, 0),
            (Sub, 0, 1, u32::MAX),
            (Shl, 1, 33, 2),
            (Shr, 0x80000000, 32, 0x80000000),
            (Sar, 0x80000000, 31, u32::MAX),
            (Lt, u32::MAX, 0, 1),
            (Ltu, u32::MAX, 0, 0),
            (Mul, u32::MAX, 2, 0xfffffffe),
            (Mulh, u32::MAX, 2, u32::MAX),
            (Mulhsu, 0x80000000, u32::MAX, 0x80000000),
            (Mulhu, u32::MAX, u32::MAX, 0xfffffffe),
            (Div, 7, 0, u32::MAX),
            (Div, 0x80000000, u32::MAX, 0x80000000),
            (Rem, 0x80000000, u32::MAX, 0),
            (Rem, 0xfffffff9, 3, u32::MAX),
            (Divu, u32::MAX, 2, 0x7fffffff),
            (Remu, 8, 0, 8),
        ] {
            assert_eq!(
                integer(op, Value::Constant(a), Value::Constant(b)),
                Value::Constant(expected),
                "{op:?}"
            );
        }
        assert_eq!(offset(Value::Stack(i64::MAX), 1), Value::Unknown);
    }
    struct Isa(Vec<SemanticOp>);
    impl FunctionDecoder for Isa {
        fn identity(&self) -> &'static str {
            "test"
        }
        fn decode(&self, _: &[u8]) -> Option<DecodedOp> {
            unreachable!()
        }
        fn reference(
            &self,
            _: &FunctionRelocation,
            _: &[FunctionRelocation],
            _: u32,
            _: &mut dyn RunControl,
        ) -> Result<NormalizedReference> {
            unreachable!()
        }
    }
    impl FunctionSemantics for Isa {
        fn semantic_identity(&self) -> &'static str {
            "test-values"
        }
        fn lift(&self, b: &[u8]) -> SemanticOp {
            self.0[b[0] as usize]
        }
        fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
            unreachable!()
        }
    }
    #[derive(Default)]
    struct Records(Vec<FunctionRecord>);
    impl FunctionSink for Records {
        fn record(&mut self, r: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
            self.0.push(r.clone());
            Ok(())
        }
    }
    fn calculate(
        ops: Vec<SemanticOp>,
        links: &[(usize, usize)],
        reverse: bool,
    ) -> Vec<FunctionRecord> {
        let bytes: Vec<u8> = (0..ops.len()).flat_map(|i| [i as u8, 0]).collect();
        let nodes: Vec<_> = (0..ops.len())
            .map(|i| Node {
                offset: i as u64 * 2,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: InstructionFlow::Next,
                },
                conflict: false,
            })
            .collect();
        let mut edges: Vec<_> = links
            .iter()
            .map(|&(from, to)| Edge {
                from: from as u64 * 2,
                target: Some(to as u64 * 2),
                relation: EdgeKind::Jump,
                external: false,
            })
            .collect();
        edges.sort_by_key(|e| {
            (
                e.from,
                if reverse {
                    u64::MAX - e.target.unwrap()
                } else {
                    e.target.unwrap()
                },
            )
        });
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: bytes.len() as u64,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut sink = Records::default();
        analyze(
            &input,
            &nodes,
            &edges,
            &Isa(ops),
            &memory,
            &mut || Ok(()),
            &mut sink,
        )
        .unwrap();
        sink.0
    }
    fn set(dest: u8, n: u32) -> SemanticOp {
        SemanticOp::Integer {
            op: IntegerOp::Add,
            dest,
            left: Operand::Register(0),
            right: Operand::Immediate(n),
        }
    }
    fn read(base: u8) -> SemanticOp {
        SemanticOp::Memory {
            kind: MemoryKind::Load,
            base,
            displacement: 0,
            width: 4,
            dest: Some(10),
            source: None,
            swap: false,
            signed: false,
        }
    }
    #[test]
    fn joins_and_loops_converge_independently_of_visit_order() {
        for (right, expected) in [
            (7, AbstractValue::Constant { value: 7 }),
            (
                8,
                AbstractValue::Alternatives {
                    values: ValueAlternatives::new(vec![
                        ValueAlternative::Constant { value: 7 },
                        ValueAlternative::Constant { value: 8 },
                    ])
                    .unwrap(),
                },
            ),
        ] {
            let ops = vec![SemanticOp::None, set(5, 7), set(5, right), read(5)];
            let links = [(0, 1), (0, 2), (1, 3), (2, 3)];
            let records = calculate(ops.clone(), &links, false);
            assert_eq!(records, calculate(ops, &links, true));
            assert!(records.iter().any(
                |r| matches!(r,FunctionRecord::MemoryAccess{address,..} if address==&expected)
            ));
        }
        let ops = vec![
            set(5, 1),
            SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 5,
                left: Operand::Register(5),
                right: Operand::Immediate(1),
            },
            read(5),
        ];
        let records = calculate(ops, &[(0, 1), (1, 1), (1, 2)], false);
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::SemanticGap {
                reason: SemanticGapReason::AlternativeLimit,
                ..
            }
        )));
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::MemoryAccess {
                address: AbstractValue::Unknown,
                ..
            }
        )));
    }
    #[test]
    fn finite_local_jump_targets_do_not_silently_expand_control_flow() {
        struct Image;
        impl ImageMemory for Image {
            fn read_constant(&self, _: u32, _: &mut [u8], _: &mut dyn RunControl) -> Result<bool> {
                Ok(false)
            }
        }
        let ops = vec![SemanticOp::None, set(6, 2), set(6, 4), SemanticOp::None];
        let bytes = [0, 0, 1, 0, 2, 0, 3, 0];
        let nodes: Vec<_> = (0..4)
            .map(|i| Node {
                offset: i * 2,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: if i == 3 {
                        InstructionFlow::Indirect {
                            base: 6,
                            offset: 0,
                            link: false,
                        }
                    } else {
                        InstructionFlow::Next
                    },
                },
                conflict: false,
            })
            .collect();
        let edges: Vec<_> = [(0, 1), (0, 2), (1, 3), (2, 3)]
            .into_iter()
            .map(|(a, b)| Edge {
                from: a * 2,
                target: Some(b * 2),
                relation: EdgeKind::Jump,
                external: false,
            })
            .collect();
        let input = FunctionInput {
            image: Some(&Image),
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 8,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut sink = Records::default();
        let summary = analyze(
            &input,
            &nodes,
            &edges,
            &Isa(ops),
            &memory,
            &mut || Ok(()),
            &mut sink,
        )
        .unwrap();
        assert!(!summary.complete);
        assert!(sink.0.iter().any(|r| matches!(
            r,
            FunctionRecord::SemanticGap {
                offset: 6,
                reason: SemanticGapReason::UnexpandedControlFlow
            }
        )));
        assert!(
            !sink
                .0
                .iter()
                .any(|r| matches!(r, FunctionRecord::Transfer { .. }))
        );
        // Its registers, from which a caller reads a jump table the graph
        // does not expand yet.
        assert!(
            sink.0
                .iter()
                .any(|r| matches!(r, FunctionRecord::CallInputs { offset: 6, .. }))
        );
    }
    #[test]
    fn an_object_dispatch_through_a_loaded_register_records_its_registers() {
        // `lw a5, 0(a1); jalr zero, 0(a5)` in a relocatable object, and a
        // `jalr zero, 0(ra)` return.
        let ops = vec![
            SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 11,
                displacement: 0,
                width: 4,
                dest: Some(15),
                source: None,
                swap: false,
                signed: false,
            },
            SemanticOp::None,
        ];
        let bytes = [0, 0, 1, 0];
        let through = |base| InstructionFlow::Indirect {
            base,
            offset: 0,
            link: false,
        };
        let run = |jump: InstructionFlow, jumps: &[KnownJump], relocated: bool| {
            let nodes: Vec<_> = (0..2)
                .map(|i| Node {
                    offset: i * 2,
                    decoded: DecodedOp {
                        length: 2,
                        text: String::new(),
                        flow: if i == 1 { jump } else { InstructionFlow::Next },
                    },
                    conflict: false,
                })
                .collect();
            // The graph's edges: a fall-through, then the jump's as the graph
            // builds them (`indirect` when unexpanded, `jump` to each known
            // target, none for a return).
            let mut edges = vec![Edge {
                from: 0,
                target: Some(2),
                relation: EdgeKind::Fallthrough,
                external: false,
            }];
            if relocated {
                // A relocated `jalr zero, %lo(sym)(a5)`: the graph's target.
                edges.push(Edge {
                    from: 2,
                    target: Some(0x100),
                    relation: EdgeKind::Jump,
                    external: true,
                });
            } else if matches!(jump, InstructionFlow::Return { .. }) {
                edges.push(Edge {
                    from: 2,
                    target: None,
                    relation: EdgeKind::Return,
                    external: true,
                });
            } else {
                match jumps.first() {
                    Some(jump) => edges.extend(jump.targets.iter().map(|&target| Edge {
                        from: 2,
                        target: Some(target),
                        relation: EdgeKind::Jump,
                        external: false,
                    })),
                    None => edges.push(Edge {
                        from: 2,
                        target: None,
                        relation: EdgeKind::Indirect,
                        external: false,
                    }),
                }
            }
            let input = FunctionInput {
                image: None,
                section: 1,
                extent: CodeRange {
                    start: 0,
                    length: 4,
                },
                bytes: &bytes,
                relocations: &PreparedReferences::empty(),
                data_ranges: &[],
                jumps,
                returns: &NoReturns,
                call_behavior: &UnknownCallBehavior,
            };
            let memory = WorkingMemory::new(1024 * 1024).unwrap();
            let mut sink = Records::default();
            let summary = analyze_with(
                &input,
                &nodes,
                &edges,
                &Isa(ops.clone()),
                &memory,
                &mut || Ok(()),
                &mut sink,
                true,
                None,
                false,
                None,
                &mut AdmittedVec::new(&memory),
            )
            .unwrap();
            (summary.complete, sink.0)
        };
        let (complete, records) = run(through(15), &[], false);
        assert!(!complete, "the dispatch is not expanded");
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::SemanticGap {
                offset: 2,
                reason: SemanticGapReason::UnexpandedControlFlow
            }
        )));
        let loaded = records.iter().find_map(|r| match r {
            FunctionRecord::CallInputs {
                offset: 2,
                registers,
            } => Some(registers[15].clone()),
            _ => None,
        });
        let Some(AbstractValue::Expression { id }) = loaded else {
            panic!("a5 at the dispatch: {loaded:?} in {records:?}")
        };
        assert!(records.iter().any(|r| matches!(
            r,
            FunctionRecord::Expression {
                id: e,
                expression: Expression::Load { width: 4, .. },
                ..
            } if *e == id
        )));
        // A return records nothing; a dispatch its caller already expanded is
        // no gap and records no registers: its table is a `Dispatch` record.
        let (complete, returned) = run(InstructionFlow::Return { base: 1 }, &[], false);
        assert!(complete);
        assert!(
            returned
                .iter()
                .any(|r| matches!(r, FunctionRecord::ReturnValue { offset: 2, .. })),
            "{returned:?}"
        );
        assert!(
            !returned
                .iter()
                .any(|r| matches!(r, FunctionRecord::CallInputs { .. })),
            "{returned:?}"
        );
        let (complete, expanded) = run(
            through(15),
            &[KnownJump {
                site: 2,
                targets: vec![0],
            }],
            false,
        );
        assert!(complete, "{expanded:?}");
        assert!(
            !expanded
                .iter()
                .any(|r| matches!(r, FunctionRecord::CallInputs { offset: 2, .. }))
        );
        // A jump the graph resolved through its relocation stays complete:
        // only a jump the graph left without a target is a gap.
        let (complete, relocated) = run(through(15), &[], true);
        assert!(complete, "{relocated:?}");
        assert!(!relocated.iter().any(|r| matches!(
            r,
            FunctionRecord::CallInputs { .. } | FunctionRecord::SemanticGap { .. }
        )));
    }
    #[test]
    fn exact_words_round_trip_through_entry_stack_slots_until_clobbered() {
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &[0; 4],
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let node = Node {
            offset: 0,
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Next,
            },
            conflict: false,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut sets = Sets::new(&memory);
        let mut step = |state: State, kind, base, displacement, width, register: u8| {
            let (dest, source) = if kind == MemoryKind::Load {
                (Some(register), None)
            } else {
                (None, Some(register))
            };
            transfer(
                Operation {
                    op: SemanticOp::Memory {
                        kind,
                        base,
                        displacement,
                        width,
                        dest,
                        source,
                        swap: false,
                        signed: false,
                    },
                    opaque_call: false,
                    callee: None,
                    relocation: None,
                    gap: None,
                },
                &node,
                &input,
                state,
                &mut || Ok(()),
                &mut sets,
            )
            .unwrap()
            .0
        };
        let mut entry = unknown_state();
        entry[2] = Value::Stack(-16);
        entry[13] = Value::Constant(0x5006_0dc6);
        // sw a3, 8(sp); lw a5, 8(sp): the spilled pointer comes back.
        let stored = step(entry, MemoryKind::Store, 2, 8, 4, 13);
        assert_eq!(stored.slot(-8), Some(Value::Constant(0x5006_0dc6)));
        assert_eq!(
            step(stored, MemoryKind::Load, 2, 8, 4, 15)[15],
            Value::Constant(0x5006_0dc6)
        );
        // A narrower load, an overlapping byte store and a store through an
        // unknown pointer each lose it.
        assert_eq!(
            step(stored, MemoryKind::Load, 2, 8, 2, 15)[15],
            Value::Unknown
        );
        let byte = step(stored, MemoryKind::Store, 2, 11, 1, 13);
        assert_eq!(
            step(byte, MemoryKind::Load, 2, 8, 4, 15)[15],
            Value::Unknown
        );
        let aliased = step(stored, MemoryKind::Store, 14, 0, 4, 13);
        assert_eq!(
            step(aliased, MemoryKind::Load, 2, 8, 4, 15)[15],
            Value::Unknown
        );
        // A disjoint stack store keeps it; an unknown word replaces it.
        let disjoint = step(stored, MemoryKind::Store, 2, 4, 4, 13);
        assert_eq!(disjoint.slot(-8), Some(Value::Constant(0x5006_0dc6)));
        let replaced = step(stored, MemoryKind::Store, 2, 8, 4, 14);
        assert_eq!(replaced.slot(-8), None);
        // A join keeps a slot only where both paths hold it.
        let mut joined = stored;
        assert!(!join(&mut joined, &disjoint, &mut sets, &mut || Ok(())).unwrap());
        assert_eq!(joined.slot(-8), Some(Value::Constant(0x5006_0dc6)));
        assert_eq!(joined.slot(-12), None);
        assert!(join(&mut joined, &entry, &mut sets, &mut || Ok(())).unwrap());
        assert_eq!(joined.slot(-8), None);
    }
    #[test]
    fn conflicts_and_calls_cannot_propagate_stale_values() {
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &[0; 4],
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let mut node = Node {
            offset: 0,
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Jump {
                    displacement: 128,
                    link: true,
                },
            },
            conflict: false,
        };
        let mut state = unknown_state();
        state[2] = Value::Stack(-16);
        state[10] = Value::Constant(42);
        let (out, effects) = transfer(
            Operation {
                op: SemanticOp::Link { dest: 1 },
                opaque_call: true,
                callee: None,
                relocation: None,
                gap: None,
            },
            &node,
            &input,
            state,
            &mut || Ok(()),
            &mut Sets::new(&WorkingMemory::new(1024 * 1024).unwrap()),
        )
        .unwrap();
        assert_eq!(out, unknown_state());
        assert_eq!(effects.gap, Some(SemanticGapReason::OpaqueCall));
        node.decoded.flow = InstructionFlow::Next;
        let (out, effects) = transfer(
            Operation {
                op: read(10),
                opaque_call: false,
                callee: None,
                relocation: None,
                gap: Some(SemanticGapReason::ConflictingBoundary),
            },
            &node,
            &input,
            state,
            &mut || Ok(()),
            &mut Sets::new(&WorkingMemory::new(1024 * 1024).unwrap()),
        )
        .unwrap();
        assert_eq!(out, unknown_state());
        assert!(effects.memory.is_none());
    }
    #[test]
    fn interrupted_solver_releases_admitted_state_before_emitting_records() {
        let bytes = [0, 0, 1, 0];
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let nodes: Vec<_> = [0, 2]
            .into_iter()
            .map(|offset| Node {
                offset,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: InstructionFlow::Next,
                },
                conflict: false,
            })
            .collect();
        let edges = [Edge {
            from: 0,
            target: Some(2),
            relation: EdgeKind::Fallthrough,
            external: false,
        }];
        for code in [
            ErrorCode::Cancelled,
            ErrorCode::ResourceLimited,
            ErrorCode::TimedOut,
        ] {
            let memory = WorkingMemory::new(65536).unwrap();
            let mut records = Records::default();
            // Admission + two lifts + first transfer, then interruption on its edge.
            let mut calls = 0;
            let result = analyze(
                &input,
                &nodes,
                &edges,
                &Isa(vec![set(5, 1), read(5)]),
                &memory,
                &mut || {
                    calls += 1;
                    if calls == 5 {
                        Err(Error::new(code, "injected control stop"))
                    } else {
                        Ok(())
                    }
                },
                &mut records,
            );
            assert_eq!(result.unwrap_err().code, code);
            assert!(records.0.is_empty());
            assert_eq!(memory.used(), 0);
        }
        assert_eq!(
            validate(SemanticOp::Link { dest: 32 }).unwrap_err().code,
            ErrorCode::Integrity
        );
    }

    #[test]
    fn explicit_integer_abi_preserves_only_declared_registers() {
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: 4,
            },
            bytes: &[0; 4],
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let node = Node {
            offset: 0,
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: InstructionFlow::Jump {
                    displacement: 128,
                    link: true,
                },
            },
            conflict: false,
        };
        let mut incoming = unknown_state();
        incoming[2] = Value::Stack(-16);
        incoming[8] = Value::Constant(0x60000000);
        incoming[5] = Value::Constant(42);
        let operation = Operation {
            op: SemanticOp::Link { dest: 1 },
            opaque_call: true,
            callee: None,
            relocation: None,
            gap: None,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut symbols = Symbols::new(1, &memory, &mut || Ok(())).unwrap();
        let (plain, _) = evaluate(
            operation,
            &node,
            &input,
            incoming,
            &mut || Ok(()),
            &mut symbols,
            true,
            None,
        )
        .unwrap();
        assert_eq!(plain[8], Value::Unknown);
        assert_eq!(plain[2], Value::Unknown);
        let (declared, effects) = evaluate(
            operation,
            &node,
            &input,
            incoming,
            &mut || Ok(()),
            &mut symbols,
            true,
            Some(CallAbi::RiscvInteger),
        )
        .unwrap();
        assert_eq!(declared[8], incoming[8]);
        assert_eq!(declared[2], incoming[2]);
        assert_eq!(declared[5], Value::Unknown);
        assert_eq!(effects.gap, Some(SemanticGapReason::OpaqueCall));
        assert!(matches!(declared[10], Value::Expr(_)));
        drop(symbols);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn upper_address_requires_flowing_identity_and_exact_pcrel_pair() {
        let upper = Value::Upper {
            symbol: 3,
            addend: 8,
            site: 7,
            pc: true,
        };
        let mut r = Relocation {
            role: ValueRelocation::LowerPcRelative,
            site: 9,
            symbol: 3,
            addend: 8,
            pair: Some(7),
        };
        assert_eq!(lower(offset(upper, 0), r), Value::Symbol(3, 8));
        assert_eq!(offset(upper, 1), Value::Unknown);
        r.pair = Some(6);
        assert_eq!(lower(upper, r), Value::Unknown);
        r.pair = Some(7);
        r.symbol = 4;
        assert_eq!(lower(upper, r), Value::Unknown);
        assert_eq!(lower(Value::Constant(0), r), Value::Unknown);
    }

    /// The records of `[a1 = opaque; a2 = 0x5000; a3 = a1 << 2;
    /// a3 = a3 + a2; a5 = load4(a3); jalr zero, 0(a5)]`, with `entered`
    /// edges from the first instruction into later ones.
    fn dispatch_records(entered: &[usize]) -> Vec<FunctionRecord> {
        dispatch_records_through(
            entered,
            InstructionFlow::Indirect {
                base: 15,
                offset: 0,
                link: false,
            },
        )
    }

    /// [`dispatch_records`] with the target loaded into the register `jump`
    /// goes through, and the jump made as `jump`.
    fn dispatch_records_through(entered: &[usize], jump: InstructionFlow) -> Vec<FunctionRecord> {
        let (target, ..) = jump.through_register().unwrap();
        let ops = vec![
            SemanticOp::Opaque { dest: 11 },
            set(12, 0x5000),
            SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: 13,
                left: Operand::Register(11),
                right: Operand::Immediate(2),
            },
            SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 13,
                left: Operand::Register(13),
                right: Operand::Register(12),
            },
            SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 13,
                displacement: 0,
                width: 4,
                dest: Some(target),
                source: None,
                swap: false,
                signed: false,
            },
            SemanticOp::None,
        ];
        let bytes: Vec<u8> = (0..ops.len()).flat_map(|i| [i as u8, 0]).collect();
        let nodes: Vec<_> = (0..ops.len())
            .map(|i| Node {
                offset: i as u64 * 2,
                decoded: DecodedOp {
                    length: 2,
                    text: String::new(),
                    flow: if i + 1 == ops.len() {
                        jump
                    } else if entered.is_empty() || i != 0 {
                        InstructionFlow::Next
                    } else {
                        InstructionFlow::Branch { displacement: 0 }
                    },
                },
                conflict: false,
            })
            .collect();
        let edge = |from: usize, to: usize, relation| Edge {
            from: from as u64 * 2,
            target: Some(to as u64 * 2),
            relation,
            external: false,
        };
        let mut edges: Vec<_> = (0..ops.len() - 1)
            .map(|i| edge(i, i + 1, EdgeKind::Fallthrough))
            .chain(entered.iter().map(|&to| edge(0, to, EdgeKind::Taken)))
            .collect();
        edges.sort_by_key(|e| (e.from, e.target));
        let input = FunctionInput {
            image: None,
            section: 1,
            extent: CodeRange {
                start: 0,
                length: bytes.len() as u64,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        };
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let mut sink = Records::default();
        analyze_with(
            &input,
            &nodes,
            &edges,
            &Isa(ops),
            &memory,
            &mut || Ok(()),
            &mut sink,
            true,
            None,
            false,
            None,
            &mut AdmittedVec::new(&memory),
        )
        .unwrap();
        sink.0
            .into_iter()
            .filter(|record| matches!(record, FunctionRecord::Dispatch { .. }))
            .collect()
    }

    #[test]
    fn a_dispatch_keeps_its_block_formed_base_whatever_its_index() {
        // The index is unknown; the base the block forms still names the table.
        assert_eq!(
            dispatch_records(&[]),
            [FunctionRecord::Dispatch {
                offset: 10,
                base: AbstractValue::Constant { value: 0x5000 },
                index: AbstractValue::Unknown,
                scale: 2,
            }]
        );
        // A path entering between the base and the jump breaks the block.
        assert_eq!(dispatch_records(&[3]), []);
        // A return is never a dispatch.
        assert_eq!(
            dispatch_records_through(&[], InstructionFlow::Return { base: 1 }),
            []
        );
    }
}

#[cfg(test)]
#[path = "expression_tests.rs"]
mod expression_tests;
