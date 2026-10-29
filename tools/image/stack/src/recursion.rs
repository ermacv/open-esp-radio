//! A direct recursive call can construct an argument that makes its next
//! invocation terminal. Prove that from instructions, including bytes of a
//! payload passed by pointer on the caller's stack. No source names or ABI
//! layouts identify such a call.
//!
//! This deliberately small specialization follows every feasible path of an
//! acyclic local graph. Unknown conditions fork; missing semantics, unknown
//! local jumps, cycles and the local resource limits leave recursion unknown.
use crate::{Function, Transfer, TransferKind, image};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;

const MAX_INSTRUCTIONS: usize = 1024;
const MAX_PENDING: usize = 128;
const MAX_SEEDS: usize = 128;
const MAX_STEPS: usize = 16_384;
const STACK_BYTES: usize = 64;
const MAX_STACK_OFFSET: i64 = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Unknown,
    Constant(u32),
    Stack(i64),
}

#[derive(Clone)]
struct State {
    registers: [Value; 32],
    bytes: [Option<(i64, u8)>; STACK_BYTES],
}

impl State {
    fn entry() -> Self {
        let mut out = Self {
            registers: [Value::Unknown; 32],
            bytes: [None; STACK_BYTES],
        };
        out.registers[0] = Value::Constant(0);
        out.registers[2] = Value::Stack(0);
        out
    }

    fn operand(&self, operand: Operand) -> Value {
        match operand {
            Operand::Register(register) => self.register(register),
            Operand::Immediate(value) => Value::Constant(value),
        }
    }

    fn register(&self, register: u8) -> Value {
        self.registers
            .get(register as usize)
            .copied()
            .unwrap_or(Value::Unknown)
    }

    fn write(&mut self, register: u8, value: Value) {
        if register != 0
            && let Some(destination) = self.registers.get_mut(register as usize)
        {
            *destination = value;
        }
    }

    fn store(&mut self, at: i64, width: u8, value: Value) {
        for byte in &mut self.bytes {
            if byte.is_some_and(|(offset, _)| offset >= at && offset < at + i64::from(width)) {
                *byte = None;
            }
        }
        if let Value::Constant(value) = value {
            for n in 0..width {
                if let Some(free) = self.bytes.iter_mut().find(|byte| byte.is_none()) {
                    *free = Some((at + i64::from(n), (value >> (8 * u32::from(n))) as u8));
                }
            }
        }
    }

    fn load(&self, at: i64, width: u8, signed: bool) -> Value {
        let mut value = 0;
        for n in 0..width {
            let Some((_, byte)) = self
                .bytes
                .iter()
                .flatten()
                .find(|(offset, _)| *offset == at + i64::from(n))
            else {
                return Value::Unknown;
            };
            value |= u32::from(*byte) << (8 * u32::from(n));
        }
        Value::Constant(extend(value, width, signed))
    }

    /// The callee's entry sp equals the caller's sp at the call. Retain
    /// only integer arguments and stack bytes whose concrete addresses are
    /// known; all other inputs are unknown.
    fn callee(&self) -> Option<Self> {
        let Value::Stack(sp) = self.registers[2] else {
            return None;
        };
        let mut out = Self::entry();
        for (destination, source) in out
            .registers
            .iter_mut()
            .zip(self.registers)
            .skip(10)
            .take(8)
        {
            *destination = match source {
                Value::Stack(at) => stack_value(at.checked_sub(sp)),
                value => value,
            };
        }
        for (new, old) in out.bytes.iter_mut().zip(self.bytes) {
            *new = old.and_then(|(at, byte)| {
                let at = at.checked_sub(sp)?;
                (at.abs() <= MAX_STACK_OFFSET).then_some((at, byte))
            });
        }
        Some(out)
    }

    fn call(&mut self) {
        // Even a direct call has no local effect summary. Forget every
        // non-sp register and all escaped payload bytes.
        let sp = self.registers[2];
        *self = Self::entry();
        self.registers[2] = sp;
    }
}

fn extend(value: u32, width: u8, signed: bool) -> u32 {
    match (width, signed) {
        (1, true) => value as i8 as i32 as u32,
        (2, true) => value as i16 as i32 as u32,
        _ => value,
    }
}

fn integer(op: IntegerOp, left: Value, right: Value) -> Value {
    match (op, left, right) {
        (_, Value::Constant(a), Value::Constant(b)) => Value::Constant(op.evaluate(a, b)),
        (IntegerOp::Add, Value::Stack(at), Value::Constant(offset))
        | (IntegerOp::Add, Value::Constant(offset), Value::Stack(at)) => {
            stack_value(at.checked_add(i64::from(offset as i32)))
        }
        (IntegerOp::Sub, Value::Stack(at), Value::Constant(offset)) => {
            stack_value(at.checked_sub(i64::from(offset as i32)))
        }
        _ => Value::Unknown,
    }
}

// A small relative window also prevents two distinct retained offsets from
// aliasing after RV32 address arithmetic wraps around the address space.
fn stack_value(at: Option<i64>) -> Value {
    at.filter(|at| at.abs() <= MAX_STACK_OFFSET)
        .map_or(Value::Unknown, Value::Stack)
}

fn condition(test: BranchTest, left: Value, right: Value) -> Option<bool> {
    let (Value::Constant(a), Value::Constant(b)) = (left, right) else {
        return None;
    };
    Some(test.evaluate(a, b))
}

struct Operation {
    pc: u32,
    length: u8,
    flow: InstructionFlow,
    op: SemanticOp,
    branch: Option<(BranchTest, Operand, Operand)>,
}

struct Pending {
    at: usize,
    state: State,
    visited: [u64; MAX_INSTRUCTIONS / 64],
}

struct Execution<'a, 'i> {
    image: &'a ImageFacts<'i>,
    operations: &'a [Operation],
    jumps: &'a [(u32, Vec<u32>)],
    function: u32,
    self_sites: &'a [u32],
}

impl Execution<'_, '_> {
    fn index(&self, target: u32) -> Option<usize> {
        self.operations
            .binary_search_by_key(&target, |operation| operation.pc)
            .ok()
    }

    fn malformed_local_target(&self, target: u32) -> bool {
        let Some(last) = self.operations.last() else {
            return true;
        };
        target >= self.function
            && target < last.pc + u32::from(last.length)
            && self.index(target).is_none()
    }

    fn effect(
        &self,
        state: &mut State,
        operation: &Operation,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        match operation.op {
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                state.write(dest, integer(op, state.operand(left), state.operand(right)));
            }
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                state.write(
                    dest,
                    Value::Constant(if pc_relative {
                        operation.pc.wrapping_add(value)
                    } else {
                        value
                    }),
                );
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
            } => {
                if !matches!(width, 1 | 2 | 4) {
                    return Ok(false);
                }
                let address = integer(
                    IntegerOp::Add,
                    state.register(base),
                    Value::Constant(displacement as u32),
                );
                if kind == MemoryKind::Load {
                    let loaded = match address {
                        Value::Stack(at) => state.load(at, width, signed),
                        Value::Constant(address) => {
                            control.checkpoint(1)?;
                            let aligned = address & !3;
                            match self.image.read_only_word(aligned) {
                                Some(word) if (address & 3) + u32::from(width) <= 4 => {
                                    let mask = u32::MAX >> (8 * (4 - u32::from(width)));
                                    Value::Constant(extend(
                                        (word >> (8 * (address & 3))) & mask,
                                        width,
                                        signed,
                                    ))
                                }
                                _ => Value::Unknown,
                            }
                        }
                        _ => Value::Unknown,
                    };
                    if let Some(dest) = dest {
                        state.write(dest, loaded)
                    }
                } else if matches!(kind, MemoryKind::Store | MemoryKind::FloatStore) {
                    match address {
                        Value::Stack(at) => state.store(
                            at,
                            width,
                            source.map_or(Value::Unknown, |r| state.register(r)),
                        ),
                        _ => state.bytes = [None; STACK_BYTES],
                    }
                } else {
                    // Atomics and FP accesses are outside this proof's
                    // payload model, even when the ordinary analyzer handles them.
                    return Ok(false);
                }
            }
            SemanticOp::Opaque { dest } => state.write(dest, Value::Unknown),
            SemanticOp::None | SemanticOp::Fence { .. } => {}
            SemanticOp::Link { .. } => {}
            SemanticOp::Unsupported => return Ok(false),
        }
        Ok(true)
    }

    /// Collect all direct recursive callees, or prove that none is reached
    /// from `entry` when `terminal` is set.
    fn run(
        &self,
        entry: State,
        terminal: bool,
        ledger: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Option<Vec<State>>> {
        let mut pending = Vec::new();
        ledger.push(
            &mut pending,
            Pending {
                at: 0,
                state: entry,
                visited: [0; MAX_INSTRUCTIONS / 64],
            },
            control,
        )?;
        let mut seeds = Vec::new();
        let mut steps = 0;
        while let Some(mut path) = pending.pop() {
            loop {
                steps += 1;
                control.checkpoint(1)?;
                let Some(visited) = path.visited.get_mut(path.at / 64) else {
                    return Ok(None);
                };
                if steps > MAX_STEPS || *visited & (1 << (path.at % 64)) != 0 {
                    return Ok(None);
                }
                *visited |= 1 << (path.at % 64);
                let Some(operation) = self.operations.get(path.at) else {
                    return Ok(None);
                };
                let next_pc = operation.pc.wrapping_add(u32::from(operation.length));
                let mut next = self.index(next_pc);
                let mut tail_exit = false;
                match operation.flow {
                    InstructionFlow::Trap => return Ok(None),
                    InstructionFlow::Return { .. } | InstructionFlow::Stop => break,
                    InstructionFlow::Branch { displacement } => {
                        let Some((test, left, right)) = operation.branch else {
                            return Ok(None);
                        };
                        let target_pc = operation.pc.wrapping_add(displacement as u32);
                        let target = self.index(target_pc);
                        if target.is_none() {
                            return Ok(None);
                        }
                        match condition(test, path.state.operand(left), path.state.operand(right)) {
                            Some(true) => next = target,
                            Some(false) => {}
                            None => {
                                if let Some(target) = target {
                                    if pending.len() >= MAX_PENDING {
                                        return Ok(None);
                                    }
                                    ledger.push(
                                        &mut pending,
                                        Pending {
                                            at: target,
                                            state: path.state.clone(),
                                            visited: path.visited,
                                        },
                                        control,
                                    )?;
                                }
                            }
                        }
                        // A branch outside the function is another call
                        // graph edge; it cannot lead to local code again.
                    }
                    InstructionFlow::Jump { displacement, link } => {
                        let target = operation.pc.wrapping_add(displacement as u32);
                        if self.malformed_local_target(target) {
                            return Ok(None);
                        }
                        if link {
                            if target == self.function {
                                if terminal || !self.self_sites.contains(&operation.pc) {
                                    return Ok(None);
                                }
                                let Some(seed) = path.state.callee() else {
                                    return Ok(None);
                                };
                                if seeds.len() >= MAX_SEEDS {
                                    return Ok(None);
                                }
                                ledger.push(&mut seeds, seed, control)?;
                            }
                            path.state.call();
                        } else {
                            next = self.index(target);
                            tail_exit = next.is_none();
                        }
                    }
                    InstructionFlow::Indirect { base, offset, link } => {
                        let target = match path.state.register(base) {
                            Value::Constant(value) => Some(value.wrapping_add(offset as u32) & !1),
                            _ => None,
                        };
                        if link {
                            // A direct call pair must agree with the
                            // ordinary sweep before it can be suppressed.
                            if target == Some(self.function) {
                                if terminal || !self.self_sites.contains(&operation.pc) {
                                    return Ok(None);
                                }
                                let Some(seed) = path.state.callee() else {
                                    return Ok(None);
                                };
                                if seeds.len() >= MAX_SEEDS {
                                    return Ok(None);
                                }
                                ledger.push(&mut seeds, seed, control)?;
                            } else if self.self_sites.contains(&operation.pc) {
                                return Ok(None);
                            }
                            path.state.call();
                        } else if let Some(target) = target {
                            if self.malformed_local_target(target) {
                                return Ok(None);
                            }
                            next = self.index(target);
                            tail_exit = next.is_none();
                        } else {
                            let Some((_, targets)) =
                                self.jumps.iter().find(|(site, _)| *site == operation.pc)
                            else {
                                return Ok(None);
                            };
                            // The sweep's proven complete jump table gives
                            // every local successor of an unknown dispatch.
                            for target in targets {
                                if self.malformed_local_target(*target & !1) {
                                    return Ok(None);
                                }
                                if let Some(target) = self.index(*target & !1) {
                                    if pending.len() >= MAX_PENDING {
                                        return Ok(None);
                                    }
                                    ledger.push(
                                        &mut pending,
                                        Pending {
                                            at: target,
                                            state: path.state.clone(),
                                            visited: path.visited,
                                        },
                                        control,
                                    )?;
                                }
                            }
                            break;
                        }
                    }
                    InstructionFlow::Next => {
                        if !self.effect(&mut path.state, operation, control)? {
                            return Ok(None);
                        }
                    }
                }
                let Some(at) = next else {
                    if tail_exit {
                        break;
                    }
                    // Falling off a symbol is not a proved return.
                    return Ok(None);
                };
                path.at = at;
            }
        }
        Ok(Some(seeds))
    }
}

/// Whether every direct self-call passes inputs that cannot execute another
/// direct self-call. The bound walker may then unfold those edges once.
pub(crate) fn terminal_callees(
    image: &ImageFacts<'_>,
    function: &Function,
    transfers: &[Transfer],
    jumps: &[(u32, Vec<u32>)],
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<bool> {
    let mut self_sites = Vec::new();
    for transfer in transfers {
        if transfer.target == Some(function.address) && transfer.kind == TransferKind::Call {
            ledger.push(&mut self_sites, transfer.site, control)?;
        }
    }
    if self_sites.is_empty() {
        return Ok(false);
    }
    let bytes = image::function_bytes(image.file(), function)?;
    let isa = RiscvDecoder;
    let mut operations = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        control.checkpoint(1)?;
        let Some(remaining) = bytes.get(offset..) else {
            return Ok(false);
        };
        let Some(decoded) = isa.decode(remaining) else {
            return Ok(false);
        };
        if operations.len() >= MAX_INSTRUCTIONS {
            return Ok(false);
        }
        let length = usize::from(decoded.length);
        let Some(raw) = remaining.get(..length) else {
            return Ok(false);
        };
        ledger.push(
            &mut operations,
            Operation {
                pc: function.address + offset as u32,
                length: decoded.length,
                flow: decoded.flow,
                op: isa.lift(raw),
                branch: isa.branch(raw),
            },
            control,
        )?;
        offset += length;
    }
    if operations.is_empty() {
        return Ok(false);
    }
    let execution = Execution {
        image,
        operations: &operations,
        jumps,
        function: function.address,
        self_sites: &self_sites,
    };
    let Some(seeds) = execution.run(State::entry(), false, ledger, control)? else {
        return Ok(false);
    };
    for seed in seeds {
        if execution.run(seed, true, ledger, control)?.is_none() {
            return Ok(false);
        }
    }
    Ok(true)
}
