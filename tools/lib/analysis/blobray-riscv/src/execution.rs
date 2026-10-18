//! Concrete RV32IMAC execution. Instruction fetch, data, and events use explicit ports.
use super::*;
use oer_riscv_decode::{self as extensions, Extension};

/// Executor of the full decoded ISA: RV32IMAC with Zba, Zbb, Zbs, Zcb and
/// Zcmp, as ESP-IDF builds the `riscv32imafc` chips.
pub struct RiscvExecutor;

/// Executor of the base RV32IMAC ISA, as ESP-IDF builds the `riscv32imac` chips: an
/// extension or floating-point encoding stops the run as an unsupported
/// instruction instead of executing an instruction the chip does not have.
pub struct Rv32imacExecutor;

/// The instruction set an executor admits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Profile {
    Full,
    Rv32imac,
}

impl Profile {
    /// Whether `decoded` belongs to the profile.
    fn admits(self, bytes: &[u8], decoded: &Decoded) -> bool {
        // No profile executes floating point: the FP register file and its
        // rounding and exception state are not modeled.
        match self {
            Self::Full => !matches!(decoded.inst, Instruction::Float(_)),
            Self::Rv32imac => oer_riscv_decode::decode(bytes, Extensions::RV32IMAC).is_some(),
        }
    }
}

/// Slots of the direct-mapped decode cache. Decoding depends only on the
/// instruction bytes and the profile, so a slot is valid for any address, for
/// code that later changes (changed bytes are another key) and for any later
/// execution under the same profile.
const DECODE_CACHE_BITS: u32 = 14;
/// Instructions charged to the run control, and recorded as its position, at
/// once. Work beyond the last full interval of one execution is not charged.
const ACCOUNTING_INTERVAL: u64 = 256;

thread_local! {
    /// The decode cache of this thread's executions. It is several hundred
    /// kilobytes, so allocating and clearing one per execution dominated
    /// requests of many short cases.
    static DECODE_CACHE: std::cell::RefCell<Option<DecodeCache>> =
        const { std::cell::RefCell::new(None) };
}

/// Direct-mapped cache of decoded instruction words.
struct DecodeCache {
    slots: Vec<Option<(u64, Decoded)>>,
    profile: Profile,
}
impl DecodeCache {
    fn new(profile: Profile) -> Self {
        Self {
            slots: vec![None; 1 << DECODE_CACHE_BITS],
            profile,
        }
    }
    fn slot(key: u64) -> usize {
        (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - DECODE_CACHE_BITS)) as usize
    }
    fn get(&mut self, key: u64, bytes: &[u8]) -> Option<Decoded> {
        let slot = &mut self.slots[Self::slot(key)];
        match slot {
            Some((k, decoded)) if *k == key => Some(*decoded),
            _ => {
                let decoded = decode(bytes).filter(|d| self.profile.admits(bytes, d))?;
                *slot = Some((key, decoded));
                Some(decoded)
            }
        }
    }
}

/// One decoded instruction word with its lifted semantics.
#[derive(Clone, Copy)]
struct Decoded {
    inst: Instruction,
    lifted: SemanticOp,
    branch: Option<(BranchTest, Operand, Operand)>,
}

fn decode(bytes: &[u8]) -> Option<Decoded> {
    let (inst, _) = decode_instruction(bytes)?;
    Some(Decoded {
        inst,
        lifted: RiscvDecoder.lift(bytes),
        branch: RiscvDecoder.branch(bytes),
    })
}
impl Executor for RiscvExecutor {
    fn identity(&self) -> &'static str {
        "rv32imac-zba-zbb-zbs-zcb-zcmp/execution-14/rv-asm-0.2.1"
    }
    fn execute(
        &self,
        start: &ExecutionStart,
        memory: &mut dyn ExecutionMemory,
        control: &mut dyn RunControl,
    ) -> Result<(ExecutionStop, u64)> {
        execute(Profile::Full, start, memory, control)
    }
}

impl Executor for Rv32imacExecutor {
    fn identity(&self) -> &'static str {
        "rv32imac/execution-14/rv-asm-0.2.1"
    }
    fn execute(
        &self,
        start: &ExecutionStart,
        memory: &mut dyn ExecutionMemory,
        control: &mut dyn RunControl,
    ) -> Result<(ExecutionStop, u64)> {
        execute(Profile::Rv32imac, start, memory, control)
    }
}

fn execute(
    profile: Profile,
    start: &ExecutionStart,
    memory: &mut dyn ExecutionMemory,
    control: &mut dyn RunControl,
) -> Result<(ExecutionStop, u64)> {
    // A cache of another profile holds words that profile admits; start over.
    let mut cache = DECODE_CACHE
        .with_borrow_mut(Option::take)
        .filter(|cache| cache.profile == profile)
        .unwrap_or_else(|| DecodeCache::new(profile));
    let result = run(start, memory, control, &mut cache);
    DECODE_CACHE.with_borrow_mut(|slot| *slot = Some(cache));
    result
}

fn run(
    start: &ExecutionStart,
    memory: &mut dyn ExecutionMemory,
    control: &mut dyn RunControl,
    cache: &mut DecodeCache,
) -> Result<(ExecutionStop, u64)> {
    let ExecutionStart {
        entry,
        stack,
        arguments,
        goal,
    } = *start;
    // An isolated root starts with known temporaries and callee-saved
    // registers (zero), so a prologue can save them; gp and tp stay unknown
    // and argument registers carry exactly the explicit ABI words.
    let mut regs = [Some(0); 32];
    regs[1] = Some(u32::MAX - 1);
    regs[2] = Some(stack);
    regs[3] = None;
    regs[4] = None;
    for (i, value) in arguments.iter().enumerate() {
        regs[i + 10] = *value;
    }
    let mut pc = entry;
    let mut steps = 0;
    let mut pending = 0u64;
    macro_rules! stop {
        ($reason:expr) => {
            return Ok((
                ExecutionStop::Incomplete {
                    pc,
                    reason: $reason,
                },
                steps,
            ))
        };
    }
    macro_rules! reg {
        ($r:expr) => {
            match regs[$r as usize] {
                Some(v) => v,
                None => stop!(ExecutionGap::UnknownRegister { register: $r }),
            }
        };
    }
    loop {
        pending += 1;
        if pending == ACCOUNTING_INTERVAL {
            let mut position = control.position();
            position.entry = Some(u64::from(pc));
            control.set_position(position);
            control.checkpoint(pending)?;
            pending = 0;
        }
        if pc == u32::MAX - 1 {
            memory.instruction(pc);
            return Ok((
                if goal == ResolvedExecutionGoal::Return {
                    ExecutionStop::Returned {
                        low: regs[10],
                        high: regs[11],
                    }
                } else {
                    ExecutionStop::GoalNotReached {
                        low: regs[10],
                        high: regs[11],
                    }
                },
                steps,
            ));
        }
        if pc & 1 != 0 {
            memory.instruction(pc);
            stop!(ExecutionGap::Memory {
                address: pc,
                access: MemoryAccess::Fetch
            });
        }
        let (lo, high) = memory.fetch(pc, control)?;
        let Some(lo) = lo else {
            stop!(ExecutionGap::Memory {
                address: pc,
                access: MemoryAccess::Fetch
            });
        };
        if goal == (ResolvedExecutionGoal::ReachSymbol { address: pc }) {
            return Ok((ExecutionStop::ReachedSymbol { pc }, steps));
        }
        let mut bytes = [0; 4];
        bytes[..2].copy_from_slice(&(lo as u16).to_le_bytes());
        let width = if lo & 3 == 3 {
            let Some(hi) = high else {
                stop!(ExecutionGap::Memory {
                    address: pc,
                    access: MemoryAccess::Fetch
                });
            };
            bytes[2..].copy_from_slice(&(hi as u16).to_le_bytes());
            4
        } else {
            2
        };
        let key = u64::from(u32::from_le_bytes(bytes)) | ((width as u64) << 32);
        let Some(decoded) = cache.get(key, &bytes[..width]) else {
            stop!(ExecutionGap::UnsupportedInstruction);
        };
        let inst = decoded.inst;
        steps += 1;
        let mut next = pc.wrapping_add(width as u32);
        let mut transfer = None;
        // The lifted integer and memory semantics, shared by base and
        // extension instructions.
        macro_rules! lifted {
            () => {
                match decoded.lifted {
                    SemanticOp::Integer {
                        op,
                        dest,
                        left,
                        right,
                    } => {
                        let operand = |o: Operand| -> Option<u32> {
                            match o {
                                Operand::Immediate(v) => Some(v),
                                Operand::Register(r) => regs[r as usize],
                            }
                        };
                        // An unknown operand yields an unknown result; only a
                        // decision, an address or an observed value stops.
                        regs[dest as usize] = match (operand(left), operand(right)) {
                            (Some(a), Some(b)) => Some(op.evaluate(a, b)),
                            _ => None,
                        };
                    }
                    SemanticOp::Upper {
                        dest,
                        value,
                        pc_relative,
                    } => {
                        regs[dest as usize] = Some(if pc_relative {
                            pc.wrapping_add(value)
                        } else {
                            value
                        })
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
                        if !matches!(kind, MemoryKind::Load | MemoryKind::Store) {
                            stop!(ExecutionGap::UnsupportedInstruction);
                        }
                        let address = reg!(base).wrapping_add_signed(displacement);
                        if kind == MemoryKind::Load {
                            let loaded = match memory.load(address, width, control)? {
                                MemoryReadValue::Known { value } => Some(match (signed, width) {
                                    (true, 1) => value as i8 as i32 as u32,
                                    (true, 2) => value as i16 as i32 as u32,
                                    _ => value,
                                }),
                                // Unknown bytes load an unknown value.
                                MemoryReadValue::Unknown => None,
                                MemoryReadValue::Unavailable => stop!(ExecutionGap::Memory {
                                    address,
                                    access: MemoryAccess::Read
                                }),
                            };
                            regs[dest.unwrap() as usize] = loaded;
                        } else {
                            let source = source.unwrap();
                            let written = match regs[source as usize] {
                                Some(value) => memory.write(address, width, value, control)?,
                                // An unknown value stored to memory stays
                                // unknown there; only a device refuses it.
                                None => {
                                    if !memory.write_unknown(address, width, control)? {
                                        stop!(ExecutionGap::UnknownRegister { register: source });
                                    }
                                    true
                                }
                            };
                            if !written {
                                stop!(ExecutionGap::Memory {
                                    address,
                                    access: MemoryAccess::Write
                                });
                            }
                        }
                    }
                    _ => stop!(ExecutionGap::UnsupportedInstruction),
                }
            };
        }
        match inst {
            Instruction::Base(inst) => match inst {
                Inst::LrW { order, dest, addr } => {
                    let address = reg!(addr.0);
                    let Some(value) = memory.load_reserved(address, ordering(order), control)?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(value);
                }
                Inst::ScW {
                    order,
                    dest,
                    addr,
                    src,
                } => {
                    let (address, value) = (reg!(addr.0), reg!(src.0));
                    let Some(stored) =
                        memory.store_conditional(address, value, ordering(order), control)?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(u32::from(!stored));
                }
                Inst::AmoW {
                    order,
                    op,
                    dest,
                    addr,
                    src,
                } => {
                    let (address, value) = (reg!(addr.0), reg!(src.0));
                    let Some(old) = memory.modify_word(
                        address,
                        ordering(order),
                        &mut |old| atomic(op, old, value),
                        control,
                    )?
                    else {
                        stop!(ExecutionGap::Memory {
                            address,
                            access: MemoryAccess::Atomic
                        });
                    };
                    regs[dest.0 as usize] = Some(old);
                }
                Inst::Jal { dest, offset } => {
                    regs[dest.0 as usize] = Some(next);
                    next = pc.wrapping_add_signed(offset.as_i32());
                    if matches!(dest.0, 0 | 1 | 5) {
                        transfer = Some((dest.0 == 0, pc.wrapping_add(width as u32), false));
                    }
                }
                Inst::Jalr { dest, base, offset } => {
                    let target = reg!(base.0).wrapping_add_signed(offset.as_i32()) & !1;
                    regs[dest.0 as usize] = Some(next);
                    next = target;
                    let is_return = dest.0 == 0 && matches!(base.0, 1 | 5) && offset.as_i32() == 0;
                    if !is_return && matches!(dest.0, 0 | 1 | 5) {
                        transfer = Some((dest.0 == 0, pc.wrapping_add(width as u32), true));
                    }
                }
                Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. } => {
                    let Some((test, Operand::Register(a), Operand::Register(b))) = decoded.branch
                    else {
                        stop!(ExecutionGap::UnsupportedInstruction);
                    };
                    let (a, b) = (reg!(a), reg!(b));
                    let taken = match test {
                        BranchTest::Eq => a == b,
                        BranchTest::Ne => a != b,
                        BranchTest::Lt => (a as i32) < b as i32,
                        BranchTest::Ge => (a as i32) >= b as i32,
                        BranchTest::Ltu => a < b,
                        BranchTest::Geu => a >= b,
                    };
                    let target = pc.wrapping_add_signed(offset.as_i32());
                    memory.event(
                        ExecutionEvent::Branch {
                            site: pc,
                            target,
                            fallthrough: next,
                            taken,
                        },
                        control,
                    )?;
                    if taken {
                        next = target;
                    }
                }
                Inst::Fence { fence } => {
                    if fence.fm != 0 {
                        stop!(ExecutionGap::UnsupportedInstruction);
                    }
                    let bits = |s: oer_riscv_decode::FenceSet| {
                        u8::from(s.device_input) * 8
                            + u8::from(s.device_output) * 4
                            + u8::from(s.memory_read) * 2
                            + u8::from(s.memory_write)
                    };
                    memory.event(
                        ExecutionEvent::Fence {
                            predecessor: bits(fence.pred),
                            successor: bits(fence.succ),
                        },
                        control,
                    )?;
                }
                _ => lifted!(),
            },
            Instruction::Extension(extension) => match extension {
                Extension::Push { list, adjustment } => {
                    let sp = reg!(2);
                    let mut address = sp;
                    for register in extensions::list_registers(list) {
                        address = address.wrapping_sub(4);
                        let written = match regs[register as usize] {
                            Some(value) => memory.write(address, 4, value, control)?,
                            None => memory.write_unknown(address, 4, control)?,
                        };
                        if !written {
                            stop!(ExecutionGap::Memory {
                                address,
                                access: MemoryAccess::Write
                            });
                        }
                    }
                    regs[2] = Some(sp.wrapping_sub(adjustment));
                }
                Extension::Pop {
                    list,
                    adjustment,
                    ret,
                } => {
                    let top = reg!(2).wrapping_add(adjustment);
                    let mut address = top;
                    for register in extensions::list_registers(list) {
                        address = address.wrapping_sub(4);
                        regs[register as usize] = match memory.load(address, 4, control)? {
                            MemoryReadValue::Known { value } => Some(value),
                            MemoryReadValue::Unknown => None,
                            MemoryReadValue::Unavailable => stop!(ExecutionGap::Memory {
                                address,
                                access: MemoryAccess::Read
                            }),
                        };
                    }
                    if ret == Some(true) {
                        regs[extensions::A0 as usize] = Some(0);
                    }
                    regs[2] = Some(top);
                    if ret.is_some() {
                        next = reg!(extensions::RA);
                    }
                }
                Extension::MoveToSaved { first, second } => {
                    let (a0, a1) = (regs[extensions::A0 as usize], regs[extensions::A1 as usize]);
                    regs[first as usize] = a0;
                    regs[second as usize] = a1;
                }
                Extension::MoveFromSaved { first, second } => {
                    let (s0, s1) = (regs[first as usize], regs[second as usize]);
                    regs[extensions::A0 as usize] = s0;
                    regs[extensions::A1 as usize] = s1;
                }
                Extension::Integer { .. } | Extension::Memory { .. } => lifted!(),
                // Concrete execution has no CSR state.
                Extension::Csr { .. } => stop!(ExecutionGap::UnsupportedInstruction),
            },
            // Excluded by every profile's admission.
            Instruction::Float(_) => stop!(ExecutionGap::UnsupportedInstruction),
        }
        if let Some((tail, return_pc, indirect)) = transfer
            && next != u32::MAX - 1
        {
            let mut arguments = [None; 8];
            arguments.copy_from_slice(&regs[10..18]);
            let input = CallInput {
                site: pc,
                target: next,
                tail,
                indirect,
                stack: regs[2],
                arguments,
            };
            memory.observe_call(&input, control)?;
            if observed_call(goal, if tail { 0 } else { 1 }, next, false).is_some() {
                return Ok((
                    ExecutionStop::ObservedCall {
                        pc,
                        target: next,
                        tail,
                    },
                    steps,
                ));
            }
            match memory.call(&input, control)? {
                CallDispatch::Code => {}
                CallDispatch::Incomplete { issue } => stop!(ExecutionGap::CallModel {
                    target: next,
                    issue
                }),
                CallDispatch::Returned { words } => {
                    next = if tail { reg!(1) } else { return_pc };
                    // psABI caller-saved registers become unknown, then explicit return words apply.
                    for r in [1, 5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17, 28, 29, 30, 31] {
                        regs[r] = None;
                    }
                    regs[10] = words[0];
                    regs[11] = words[1];
                }
            }
        }
        regs[0] = Some(0);
        pc = next;
    }
}
fn ordering(order: oer_riscv_decode::AmoOrdering) -> ExecutionOrdering {
    let (acquire, release) = order.aq_rl();
    ExecutionOrdering { acquire, release }
}
fn atomic(op: oer_riscv_decode::AmoOp, old: u32, value: u32) -> u32 {
    use oer_riscv_decode::AmoOp;
    match op {
        AmoOp::Swap => value,
        AmoOp::Add => old.wrapping_add(value),
        AmoOp::Xor => old ^ value,
        AmoOp::And => old & value,
        AmoOp::Or => old | value,
        AmoOp::Min => (old as i32).min(value as i32) as u32,
        AmoOp::Max => (old as i32).max(value as i32) as u32,
        AmoOp::Minu => old.min(value),
        AmoOp::Maxu => old.max(value),
    }
}

fn observed_call(
    goal: ResolvedExecutionGoal,
    dest: u8,
    target: u32,
    is_return: bool,
) -> Option<bool> {
    let ResolvedExecutionGoal::ObserveCall {
        address,
        include_tail,
    } = goal
    else {
        return None;
    };
    if address != target || is_return {
        return None;
    }
    if matches!(dest, 1 | 5) {
        Some(false)
    } else if dest == 0 && include_tail {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admitted(profile: Profile, bytes: &[u8]) -> bool {
        decode(bytes).is_some_and(|decoded| profile.admits(bytes, &decoded))
    }

    #[test]
    fn no_profile_executes_floating_point_and_rv32imac_refuses_extensions() {
        // add a0, a0, a1 and c.addi a0, 1.
        let base: [&[u8]; 2] = [&0x00b5_0533u32.to_le_bytes(), &0x0505u16.to_le_bytes()];
        // sh1add a0, a0, a1 (Zba), cm.push {ra}, -16 (Zcmp), flw fa0, 0(a0).
        let outside: [&[u8]; 3] = [
            &0x20b5_2533u32.to_le_bytes(),
            &0xb842u16.to_le_bytes(),
            &0x0005_2507u32.to_le_bytes(),
        ];
        for bytes in base {
            assert!(admitted(Profile::Rv32imac, bytes));
            assert!(admitted(Profile::Full, bytes));
        }
        for bytes in outside {
            assert!(!admitted(Profile::Rv32imac, bytes), "{bytes:02x?}");
        }
        // The full profile executes the extension forms it decodes, but no
        // profile executes the floating point the decoder now also decodes.
        assert!(admitted(Profile::Full, outside[0]));
        assert!(admitted(Profile::Full, outside[1]));
        assert!(RiscvDecoder.decode(outside[2]).is_some());
        assert!(!admitted(Profile::Full, outside[2]));
        // c.flwsp fa0, 0(sp) and c.fsw fa0, 0(a0) are floating point.
        for bytes in [0x6502u16.to_le_bytes(), 0xe108u16.to_le_bytes()] {
            assert!(!admitted(Profile::Rv32imac, &bytes));
            assert!(!admitted(Profile::Full, &bytes));
        }
    }
}
