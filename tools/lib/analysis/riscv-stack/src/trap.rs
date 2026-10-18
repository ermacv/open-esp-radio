//! The entries of the traps: each must move `sp` to the hart's interrupt
//! stack before its first memory access, and its frame below that stack's
//! position counts toward every bound of the trap.
//!
//! A trap arrives with the interrupted context's `sp` and with `mscratch`
//! holding the other stack: the interrupt stack's top in thread mode, the
//! interrupted task's `sp` inside a trap. The interrupt stack (internal SRAM)
//! lies below every task stack (PSRAM), so the interrupt stack's position is
//! the lower of the two. The check executes the entry symbolically from its
//! first instruction to the call of its handler over every path: each memory
//! access must address the frame below that position, which the entry may
//! learn only from an unsigned comparison of the two values.
use crate::image::Function;
use crate::sweep::destination;
use oer_elf::rv32::{self, Role};
use oer_riscv_decode::{Extension, Extensions, Float, Inst, Instruction, decode};
use oer_riscv_model::{Error, ErrorCode, Result, RunControl};
use oer_riscv_program::facts::ImageFacts;

/// `mscratch`.
const MSCRATCH: u16 = 0x340;
/// Instructions one path of an entry may execute before its handler.
const MAX_STEPS: usize = 512;
/// Paths one entry may take before its handler.
const MAX_PATHS: usize = 64;

/// What a trap entry does before its handler runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrapEntry {
    pub entry: u32,
    /// Bytes the entry keeps below the interrupt stack's position while its
    /// handler runs.
    pub frame: u64,
    /// The function the entry calls.
    pub handler: u32,
}

/// The entries of the vector table `symbol`: each word a relocation names,
/// by slot; an empty slot holds zero and no relocation.
pub fn vector_table(
    image: &ImageFacts<'_>,
    symbol: &str,
    control: &mut dyn RunControl,
) -> Result<Vec<Option<u32>>> {
    let file = image.file();
    let table = file
        .symbols()
        .find(|s| s.name == symbol)
        .ok_or_else(|| invalid(format!("no vector table `{symbol}`")))?;
    let (start, size) = (table.address, table.size);
    if size == 0 || size % 4 != 0 {
        return Err(invalid(format!("vector table `{symbol}` has no word size")));
    }
    let section = table
        .section
        .and_then(|index| file.section(index).ok())
        .ok_or_else(|| invalid(format!("vector table `{symbol}` in no section")))?;
    let word = |address: u64| {
        section
            .word(address)
            .ok_or_else(|| invalid(format!("vector table `{symbol}` exceeds its section")))
    };
    control.checkpoint(size / 4)?;
    let mut slots = vec![None; (size / 4) as usize];
    let relocations = file
        .relocations(section.index)
        .map_err(|error| invalid(error.to_string()))?;
    for relocation in relocations {
        control.checkpoint(1)?;
        let offset = relocation.at;
        if offset < start || offset >= start + size {
            continue;
        }
        if rv32::kind(relocation.r_type).role != Role::Word {
            return Err(invalid(format!(
                "vector table `{symbol}` slot at {offset:#010x} is no address word"
            )));
        }
        let target = file
            .target_address(&relocation)
            .map_err(|error| invalid(error.to_string()))? as u32;
        if word(offset)? != target || offset % 4 != 0 {
            return Err(invalid(format!(
                "vector table `{symbol}` slot at {offset:#010x} differs from its relocation"
            )));
        }
        slots[((offset - start) / 4) as usize] = Some(target);
    }
    for (slot, entry) in slots.iter().enumerate() {
        let address = start + 4 * slot as u64;
        if entry.is_none() && word(address)? != 0 {
            return Err(invalid(format!(
                "vector table `{symbol}` slot {slot} holds an address without a relocation"
            )));
        }
    }
    Ok(slots)
}

/// Check the trap entry at `entry` of `image` and measure its frame.
pub fn trap_entry(
    image: &ImageFacts<'_>,
    entry: u32,
    control: &mut dyn RunControl,
) -> Result<TrapEntry> {
    let (file, functions) = (image.file(), image.functions());
    let name = |address: u32| {
        functions
            .iter()
            .find(|f| f.address == address)
            .map(Function::label)
            .unwrap_or_else(|| format!("{address:#010x}"))
    };
    let fail = |pc: u32, message: &str| {
        invalid(format!(
            "trap entry {} at {pc:#010x}: {message}",
            name(entry)
        ))
    };
    let mut pending = vec![State::entry(entry)];
    let mut paths = 0;
    let mut found: Option<TrapEntry> = None;
    while let Some(mut state) = pending.pop() {
        paths += 1;
        if paths > MAX_PATHS {
            return Err(fail(entry, "takes more paths than the check follows"));
        }
        let mut steps = 0;
        let (frame, handler) = loop {
            steps += 1;
            control.checkpoint(1)?;
            if steps > MAX_STEPS {
                return Err(fail(state.pc, "runs longer than the check follows"));
            }
            let pc = state.pc;
            let bytes = code(file, pc).ok_or_else(|| fail(pc, "leaves the code"))?;
            let (instruction, length) =
                decode(bytes, Extensions::ALL).ok_or_else(|| fail(pc, "undecodable"))?;
            let next = pc.wrapping_add(length as u32);
            match state.step(&instruction, pc) {
                Step::Next => state.pc = next,
                Step::Jump(to) => state.pc = to,
                Step::Branch(taken) => {
                    pending.push(*taken);
                    state.pc = next;
                }
                Step::Call(handler) => {
                    let frame = state.frame().ok_or_else(|| {
                        fail(
                            pc,
                            "calls its handler before it moves sp to the interrupt stack",
                        )
                    })?;
                    break (frame, handler);
                }
                Step::Fail(message) => return Err(fail(pc, message)),
            }
        };
        let this = TrapEntry {
            entry,
            frame,
            handler,
        };
        found = match found {
            None => Some(this),
            Some(other) if other.handler != handler => {
                return Err(fail(entry, "calls different handlers on different paths"));
            }
            Some(other) => Some(TrapEntry {
                frame: other.frame.max(frame),
                ..other
            }),
        };
    }
    found.ok_or_else(|| fail(entry, "never calls a handler"))
}

/// The code bytes at `address`, to the end of their section.
fn code<'a>(file: &oer_elf::Elf<'a>, address: u32) -> Option<&'a [u8]> {
    let address = u64::from(address);
    file.sections().find_map(|section| {
        if !section.executable || !section.contains(address) {
            return None;
        }
        section.data.get((address - section.address) as usize..)
    })
}

/// A value at the entry, as the check knows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Unknown,
    Constant(u32),
    /// An entry value plus an offset.
    At(Base, i64),
    /// The exclusive or of two entry values: half of a register swap.
    Xor(Base, Base),
}

/// The values the entry starts from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Base {
    /// The interrupted `sp`.
    Sp,
    /// `mscratch` at the trap.
    Scratch,
    /// The lower of the two: the interrupt stack's position.
    Stack,
}

impl Value {
    fn add(self, offset: i64) -> Value {
        match self {
            Value::Constant(value) => Value::Constant(value.wrapping_add(offset as u32)),
            Value::At(base, at) => Value::At(base, at + offset),
            _ => Value::Unknown,
        }
    }

    fn xor(self, other: Value) -> Value {
        match (self, other) {
            (Value::Constant(a), Value::Constant(b)) => Value::Constant(a ^ b),
            (Value::At(a, 0), Value::At(b, 0)) if a != b => Value::Xor(a, b),
            (Value::Xor(a, b), Value::At(c, 0)) | (Value::At(c, 0), Value::Xor(a, b)) => {
                if c == a {
                    Value::At(b, 0)
                } else if c == b {
                    Value::At(a, 0)
                } else {
                    Value::Unknown
                }
            }
            _ => Value::Unknown,
        }
    }
}

#[derive(Clone)]
struct State {
    pc: u32,
    registers: [Value; 32],
    scratch: Value,
}

enum Step {
    Next,
    Jump(u32),
    /// A branch: the taken path's state; the step leaves the fall-through
    /// path's state in place.
    Branch(Box<State>),
    Call(u32),
    Fail(&'static str),
}

impl State {
    fn entry(pc: u32) -> Self {
        let mut registers = [Value::Unknown; 32];
        registers[0] = Value::Constant(0);
        registers[2] = Value::At(Base::Sp, 0);
        Self {
            pc,
            registers,
            scratch: Value::At(Base::Scratch, 0),
        }
    }

    fn get(&self, register: u8) -> Value {
        self.registers[register as usize]
    }

    fn set(&mut self, register: u8, value: Value) {
        if register != 0 {
            self.registers[register as usize] = value;
        }
    }

    /// Bytes `sp` lies below the interrupt stack's position, once `sp` is
    /// there.
    fn frame(&self) -> Option<u64> {
        match self.get(2) {
            Value::At(Base::Stack, offset) if offset <= 0 => Some(offset.unsigned_abs()),
            _ => None,
        }
    }

    /// Whether `base + offset` addresses `width` bytes of the frame: at or
    /// above `sp`, below the interrupt stack's position.
    fn in_frame(&self, base: u8, offset: i64, width: i64) -> bool {
        let (Value::At(Base::Stack, address), Some(frame)) =
            (self.get(base).add(offset), self.frame())
        else {
            return false;
        };
        address >= -(frame as i64) && address + width <= 0
    }

    /// The paths an unsigned comparison of the interrupted `sp` and
    /// `mscratch` splits: on each, the lower one is the interrupt stack.
    fn compare(&self, lower_if_taken: Value, higher_if_taken: Value) -> Option<(State, State)> {
        let (Value::At(a, 0), Value::At(b, 0)) = (lower_if_taken, higher_if_taken) else {
            return None;
        };
        if !matches!(
            (a, b),
            (Base::Sp, Base::Scratch) | (Base::Scratch, Base::Sp)
        ) {
            return None;
        }
        let rename = |state: &State, lower: Base| {
            let mut state = state.clone();
            let swap = |value: Value| match value {
                Value::At(base, offset) if base == lower => Value::At(Base::Stack, offset),
                Value::Xor(x, y) if x == lower => Value::Xor(Base::Stack, y),
                Value::Xor(x, y) if y == lower => Value::Xor(x, Base::Stack),
                other => other,
            };
            for register in state.registers.iter_mut() {
                *register = swap(*register);
            }
            state.scratch = swap(state.scratch);
            state
        };
        // Taken: `a < b`, so `a` is the interrupt stack; otherwise `b <= a`.
        Some((rename(self, a), rename(self, b)))
    }

    fn step(&mut self, instruction: &Instruction, pc: u32) -> Step {
        let access = |state: &State, base: u8, offset: i64, width: i64| {
            if state.in_frame(base, offset, width) {
                Step::Next
            } else {
                Step::Fail("accesses memory outside its frame on the interrupt stack")
            }
        };
        match *instruction {
            Instruction::Base(inst) => match inst {
                Inst::Lb { offset, base, dest } | Inst::Lbu { offset, base, dest } => {
                    let step = access(self, base.0, offset.as_i32().into(), 1);
                    self.set(dest.0, Value::Unknown);
                    step
                }
                Inst::Lh { offset, base, dest } | Inst::Lhu { offset, base, dest } => {
                    let step = access(self, base.0, offset.as_i32().into(), 2);
                    self.set(dest.0, Value::Unknown);
                    step
                }
                Inst::Lw { offset, base, dest } => {
                    let step = access(self, base.0, offset.as_i32().into(), 4);
                    self.set(dest.0, Value::Unknown);
                    step
                }
                Inst::Sb { offset, base, .. } => access(self, base.0, offset.as_i32().into(), 1),
                Inst::Sh { offset, base, .. } => access(self, base.0, offset.as_i32().into(), 2),
                Inst::Sw { offset, base, .. } => access(self, base.0, offset.as_i32().into(), 4),
                Inst::LrW { .. } | Inst::ScW { .. } | Inst::AmoW { .. } => {
                    Step::Fail("an atomic access in a trap entry")
                }
                Inst::Lui { uimm, dest } => {
                    self.set(dest.0, Value::Constant(uimm.as_u32()));
                    Step::Next
                }
                Inst::Auipc { uimm, dest } => {
                    self.set(dest.0, Value::Constant(pc.wrapping_add(uimm.as_u32())));
                    Step::Next
                }
                Inst::Addi { imm, dest, src1 } => {
                    let value = self.get(src1.0).add(imm.as_i32().into());
                    self.set(dest.0, value);
                    Step::Next
                }
                Inst::Add { dest, src1, src2 } => {
                    let value = match (self.get(src1.0), self.get(src2.0)) {
                        (Value::Constant(c), other) | (other, Value::Constant(c)) => {
                            other.add(i64::from(c as i32))
                        }
                        _ => Value::Unknown,
                    };
                    self.set(dest.0, value);
                    Step::Next
                }
                Inst::Xor { dest, src1, src2 } => {
                    let value = self.get(src1.0).xor(self.get(src2.0));
                    self.set(dest.0, value);
                    Step::Next
                }
                Inst::Jal { offset, dest } => {
                    let target = pc.wrapping_add(offset.as_u32());
                    match dest.0 {
                        0 => Step::Jump(target),
                        1 => Step::Call(target),
                        _ => Step::Fail("a jump that links a register other than ra"),
                    }
                }
                Inst::Jalr { offset, base, dest } => {
                    let Value::Constant(target) = self.get(base.0).add(offset.as_i32().into())
                    else {
                        return Step::Fail("an indirect transfer to an unknown target");
                    };
                    match dest.0 {
                        0 => Step::Jump(target & !1),
                        1 => Step::Call(target & !1),
                        _ => Step::Fail("a jump that links a register other than ra"),
                    }
                }
                Inst::Bltu { offset, src1, src2 } | Inst::Bgeu { offset, src1, src2 } => {
                    let target = pc.wrapping_add(offset.as_u32());
                    let below = matches!(inst, Inst::Bltu { .. });
                    let Some((lower_first, lower_second)) =
                        self.compare(self.get(src1.0), self.get(src2.0))
                    else {
                        return self.fork(target);
                    };
                    // `bltu a, b` is taken when `a` is lower, `bgeu a, b`
                    // when `b` is.
                    let (mut taken, fallthrough) = if below {
                        (lower_first, lower_second)
                    } else {
                        (lower_second, lower_first)
                    };
                    taken.pc = target;
                    *self = fallthrough;
                    Step::Branch(Box::new(taken))
                }
                Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. } => self.fork(pc.wrapping_add(offset.as_u32())),
                Inst::Fence { .. } => Step::Next,
                Inst::Ecall | Inst::Ebreak => Step::Fail("a nested trap in a trap entry"),
                other => match destination(&Instruction::Base(other)) {
                    Some(dest) => {
                        self.set(dest, Value::Unknown);
                        Step::Next
                    }
                    None => Step::Fail("an instruction the trap-entry check does not model"),
                },
            },
            Instruction::Extension(Extension::Csr {
                op,
                dest,
                source,
                csr,
            }) => {
                use oer_riscv_decode::{CsrOp, Operand};
                let operand = match source {
                    Operand::Register(register) => self.get(register),
                    Operand::Immediate(value) => Value::Constant(value),
                };
                let writes = !matches!(
                    (op, source),
                    (
                        CsrOp::Set | CsrOp::Clear,
                        Operand::Register(0) | Operand::Immediate(0)
                    )
                );
                // The entry runs at the interrupt's level until its handler:
                // nothing on the way may lower it.
                if crate::LevelDrop::of_csr(csr, op, source).is_some() {
                    return Step::Fail("an entry that lowers the interrupt level");
                }
                let old = if csr == MSCRATCH {
                    self.scratch
                } else {
                    Value::Unknown
                };
                if csr == MSCRATCH && writes {
                    self.scratch = match op {
                        CsrOp::Write => operand,
                        CsrOp::Set | CsrOp::Clear => Value::Unknown,
                    };
                }
                self.set(dest, old);
                Step::Next
            }
            Instruction::Extension(Extension::Memory {
                load,
                register,
                base,
                offset,
                width,
                ..
            }) => {
                let step = access(self, base, offset.into(), width.into());
                if load {
                    self.set(register, Value::Unknown);
                }
                step
            }
            Instruction::Extension(
                Extension::Push { .. }
                | Extension::Pop { .. }
                | Extension::MoveToSaved { .. }
                | Extension::MoveFromSaved { .. },
            ) => Step::Fail("a Zcmp instruction in a trap entry"),
            Instruction::Extension(Extension::Integer { dest, .. }) => {
                self.set(dest, Value::Unknown);
                Step::Next
            }
            Instruction::Float(Float::Load { base, offset, .. })
            | Instruction::Float(Float::Store { base, offset, .. }) => {
                access(self, base, offset.into(), 4)
            }
            Instruction::Float(_) => match destination(instruction) {
                Some(dest) => {
                    self.set(dest, Value::Unknown);
                    Step::Next
                }
                None => Step::Next,
            },
        }
    }

    fn fork(&self, target: u32) -> Step {
        let mut taken = self.clone();
        taken.pc = target;
        Step::Branch(Box::new(taken))
    }
}

fn invalid(message: String) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
