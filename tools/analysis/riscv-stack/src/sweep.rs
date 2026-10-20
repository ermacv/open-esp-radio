//! Calls and out-of-function jumps of one function by a linear sweep over its
//! whole extent, independent of which code its control-flow graph reaches.
use crate::image::{Function, function_bytes};
use oer_analysis_riscv_decode::{
    CsrOp, Extension, Extensions, Float, Inst, Instruction, Operand, Reg, Register, decode,
};
use oer_analysis_riscv_model::Ledger;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TransferKind {
    /// A linking call: the callee runs below the caller's frame.
    Call,
    /// A jump out of the function: a tail call, or an indirect jump whose
    /// target is unknown.
    Tail,
}

/// Where an unresolved indirect transfer's target register came from: the
/// last instruction of the linear sweep that wrote it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TargetSource {
    /// A load from an `sp`-relative slot: a function pointer spilled by
    /// `core::hint::black_box`, which stack-slot value tracking resolves.
    StackSlot,
    /// A load through another register: a vtable entry or a stored function
    /// pointer.
    Memory,
    /// Any other register value.
    Register,
}

/// Where an indexed dispatch's table starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBase {
    /// A constant the sweep computed.
    Address(u32),
    /// A register unchanged from the indexing `add` to the transfer, whose
    /// value the value analysis knows at the transfer.
    Register(u8),
}

/// One call or out-of-function jump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transfer {
    pub site: u32,
    /// The target address; `None` for an indirect transfer the sweep and the
    /// value analysis cannot resolve.
    pub target: Option<u32>,
    pub kind: TransferKind,
    /// For an unresolved target, where its register came from.
    pub source: Option<TargetSource>,
    /// For an unresolved target loaded as `table[index]`, the table.
    pub table: Option<TableBase>,
    /// For an unresolved target a word load put in its register, that load.
    pub load: Option<Load>,
}

/// The word load an unresolved target comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Load {
    /// The instruction that defined the target register. It remains a
    /// producer only while no control-flow merge or register write loses it.
    pub site: u32,
    /// The complete value-analysis graph proves this load is the call's
    /// sole predecessor. This is separate from tracking its address base.
    pub exclusive_producer: bool,
    /// The register the load's address is based on, unchanged from the load
    /// to the transfer.
    pub base: u8,
    /// The load's offset from its base register: a vtable slot for a call
    /// through a trait object.
    pub offset: i32,
    /// The address loaded from, when the base register holds a constant.
    pub address: Option<u32>,
}

/// What an instruction writes to an integer register, for [`TargetSource`].
#[derive(Clone, Copy)]
enum Write {
    Load { from_sp: bool },
    Other,
}

/// The integer register `instruction` writes, and how.
fn write(instruction: &Instruction) -> Option<(u8, Write)> {
    let other = |dest: u8| Some((dest, Write::Other));
    match *instruction {
        Instruction::Base(inst) => match inst {
            Inst::Lb { dest, base, .. }
            | Inst::Lbu { dest, base, .. }
            | Inst::Lh { dest, base, .. }
            | Inst::Lhu { dest, base, .. }
            | Inst::Lw { dest, base, .. } => Some((
                dest.0,
                Write::Load {
                    from_sp: base.0 == 2,
                },
            )),
            Inst::Lui { dest, .. }
            | Inst::Auipc { dest, .. }
            | Inst::Jal { dest, .. }
            | Inst::Jalr { dest, .. }
            | Inst::Addi { dest, .. }
            | Inst::Slti { dest, .. }
            | Inst::Sltiu { dest, .. }
            | Inst::Xori { dest, .. }
            | Inst::Ori { dest, .. }
            | Inst::Andi { dest, .. }
            | Inst::Slli { dest, .. }
            | Inst::Srli { dest, .. }
            | Inst::Srai { dest, .. }
            | Inst::Add { dest, .. }
            | Inst::Sub { dest, .. }
            | Inst::Sll { dest, .. }
            | Inst::Slt { dest, .. }
            | Inst::Sltu { dest, .. }
            | Inst::Xor { dest, .. }
            | Inst::Srl { dest, .. }
            | Inst::Sra { dest, .. }
            | Inst::Or { dest, .. }
            | Inst::And { dest, .. }
            | Inst::Mul { dest, .. }
            | Inst::Mulh { dest, .. }
            | Inst::Mulhsu { dest, .. }
            | Inst::Mulhu { dest, .. }
            | Inst::Div { dest, .. }
            | Inst::Divu { dest, .. }
            | Inst::Rem { dest, .. }
            | Inst::Remu { dest, .. }
            | Inst::LrW { dest, .. }
            | Inst::ScW { dest, .. }
            | Inst::AmoW { dest, .. } => other(dest.0),
            _ => None,
        },
        Instruction::Extension(Extension::Integer { dest, .. } | Extension::Csr { dest, .. }) => {
            other(dest)
        }
        Instruction::Extension(Extension::Memory {
            load: true,
            register,
            base,
            ..
        }) => Some((register, Write::Load { from_sp: base == 2 })),
        Instruction::Float(Float::Operation {
            operands: [Some(Register::Integer(dest)), ..],
            ..
        }) => other(dest),
        _ => None,
    }
}

/// The integer register `instruction` writes.
pub(crate) fn destination(instruction: &Instruction) -> Option<u8> {
    write(instruction).map(|(register, _)| register)
}

/// Table lengths beyond this are not taken from a bound.
const MAX_ENTRIES: u32 = 4096;

/// What a register holds on the way to a table dispatch. `entries` is the
/// exact number of table words the index can select, when a bounds check or
/// mask on the index states it.
#[derive(Clone, Copy)]
enum Fact {
    Constant(u32),
    /// An index below `entries`.
    Bounded {
        entries: u32,
    },
    /// An index shifted left by two: a word offset.
    Scaled {
        entries: Option<u32>,
    },
    /// A table base plus a word offset.
    TableAddress {
        base: TableBase,
        entries: Option<u32>,
    },
    /// A word loaded from a table.
    TableEntry {
        base: TableBase,
        entries: Option<u32>,
    },
}

fn bounded(entries: u32) -> Option<Fact> {
    (1..=MAX_ENTRIES)
        .contains(&entries)
        .then_some(Fact::Bounded { entries })
}

/// How `instruction` changes the dispatch facts of the register it writes.
fn fact(
    instruction: &Instruction,
    pc: u32,
    facts: &[Option<Fact>; 32],
    image: &ImageFacts<'_>,
) -> Option<Fact> {
    let get = |r: u8| {
        if r == 0 {
            Some(Fact::Constant(0))
        } else {
            facts[r as usize]
        }
    };
    match *instruction {
        Instruction::Base(Inst::Lui { uimm, .. }) => Some(Fact::Constant(uimm.as_u32())),
        Instruction::Base(Inst::Auipc { uimm, .. }) => {
            Some(Fact::Constant(pc.wrapping_add(uimm.as_u32())))
        }
        Instruction::Base(Inst::Addi { imm, src1, .. }) => match get(src1.0) {
            Some(Fact::Constant(value)) => Some(Fact::Constant(value.wrapping_add(imm.as_u32()))),
            _ => None,
        },
        Instruction::Base(Inst::Andi { imm, .. }) if imm.as_i32() >= 0 => bounded(imm.as_u32() + 1),
        Instruction::Base(Inst::Slli { imm, src1, .. }) if imm.as_u32() == 2 => {
            Some(Fact::Scaled {
                entries: match get(src1.0) {
                    Some(Fact::Bounded { entries }) => Some(entries),
                    _ => None,
                },
            })
        }
        Instruction::Base(Inst::Add { src1, src2, .. }) => {
            let base = |r: u8| match get(r) {
                Some(Fact::Constant(address)) => Some(TableBase::Address(address)),
                Some(Fact::Scaled { .. }) => None,
                _ => Some(TableBase::Register(r)),
            };
            let (base, entries) = match (get(src1.0), get(src2.0)) {
                (Some(Fact::Scaled { entries }), _) => (base(src2.0)?, entries),
                (_, Some(Fact::Scaled { entries })) => (base(src1.0)?, entries),
                // A compiler jump table indexed however the code likes.
                (Some(Fact::Constant(table)), _) | (_, Some(Fact::Constant(table)))
                    if image.is_jump_table(table) =>
                {
                    (TableBase::Address(table), None)
                }
                _ => return None,
            };
            Some(Fact::TableAddress { base, entries })
        }
        Instruction::Base(Inst::Lw { offset, base, .. }) => match get(base.0) {
            // An index offset by a constant still selects an entry of the
            // compiler's table.
            Some(Fact::TableAddress {
                base: TableBase::Address(table),
                entries,
            }) if image.is_jump_table(table) => Some(Fact::TableEntry {
                base: TableBase::Address(table),
                entries,
            }),
            Some(Fact::TableAddress {
                base: TableBase::Address(address),
                entries,
            }) => Some(Fact::TableEntry {
                base: TableBase::Address(address.wrapping_add(offset.as_u32())),
                entries,
            }),
            Some(Fact::TableAddress { base, entries }) if offset.as_i32() == 0 => {
                Some(Fact::TableEntry { base, entries })
            }
            _ => None,
        },
        _ => None,
    }
}

/// The direct branch and jump targets inside the function: where paths merge,
/// so no register fact survives. The set is admitted in `ledger`.
fn merges(
    bytes: &[u8],
    start: u32,
    end: u32,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeSet<u32>> {
    let mut targets = BTreeSet::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let pc = start + offset as u32;
        let Some((instruction, length)) = decode(&bytes[offset..], Extensions::ALL) else {
            offset += if bytes[offset] & 3 == 3 { 4 } else { 2 };
            continue;
        };
        let displacement = match instruction {
            Instruction::Base(
                Inst::Jal { offset, .. }
                | Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. },
            ) => Some(offset.as_u32()),
            _ => None,
        };
        if let Some(target) = displacement.map(|d| pc.wrapping_add(d))
            && target >= start
            && target < end
        {
            ledger.add(&mut targets, target, control)?;
        }
        offset += length;
    }
    Ok(targets)
}

/// The fact an index takes on the fall-through path of a bounds check:
/// `bltu limit, index` falls through when `index <= limit`, `bgeu index,
/// limit` when `index < limit`.
fn fall_through(instruction: &Instruction, facts: &[Option<Fact>; 32]) -> Option<(u8, Fact)> {
    let constant = |r: u8| match (r, facts[r as usize]) {
        (0, _) => Some(0),
        (_, Some(Fact::Constant(value))) => Some(value),
        _ => None,
    };
    let (index, entries) = match *instruction {
        Instruction::Base(Inst::Bltu { src1, src2, .. }) => {
            (src2.0, constant(src1.0)?.checked_add(1)?)
        }
        Instruction::Base(Inst::Bgeu { src1, src2, .. }) => (src1.0, constant(src2.0)?),
        _ => return None,
    };
    (index != 0).then_some((index, bounded(entries)?))
}

/// The upper-immediate value `auipc`/`lui` gave a register, as the target base
/// of the immediately following `jalr`.
fn upper(instruction: &Instruction, pc: u32) -> Option<(u8, u32)> {
    match instruction {
        Instruction::Base(Inst::Lui { uimm, dest }) => Some((dest.0, uimm.as_u32())),
        Instruction::Base(Inst::Auipc { uimm, dest }) => {
            Some((dest.0, pc.wrapping_add(uimm.as_u32())))
        }
        _ => None,
    }
}

/// An instruction that can lower the hart's interrupt level (CLIC
/// `mintstatus.mil`) while it runs: the only way an interrupt of a level
/// can preempt a handler of the same level. Raising or lowering the
/// threshold or setting `mstatus.MIE` cannot: an interrupt preempts only
/// above both the threshold and `mil`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LevelDrop {
    /// `mret`, `sret` or `dret`: restores `mil` from `mcause.mpil`.
    Return,
    /// A write of `mnxti` (0x345): takes the next pending interrupt and sets
    /// `mil` to its level.
    NextInterrupt,
    /// A write of `mintstatus` (0xfb1).
    Status,
}

impl LevelDrop {
    const MNXTI: u16 = 0x345;
    const MINTSTATUS: u16 = 0xfb1;

    /// The drop an undecoded 32-bit word is.
    fn of_word(word: u32) -> Option<Self> {
        matches!(word, 0x3020_0073 | 0x1020_0073 | 0x7b20_0073).then_some(Self::Return)
    }

    /// The drop a CSR access is: a write of `mnxti` or `mintstatus` (a read,
    /// `csrrs`/`csrrc` of `x0` or 0, has no effect on either).
    pub(crate) fn of_csr(csr: u16, op: CsrOp, source: Operand) -> Option<Self> {
        let writes = match (op, source) {
            (CsrOp::Write, _) => true,
            (_, Operand::Register(register)) => register != 0,
            (_, Operand::Immediate(value)) => value != 0,
        };
        match csr {
            Self::MNXTI if writes => Some(Self::NextInterrupt),
            Self::MINTSTATUS if writes => Some(Self::Status),
            _ => None,
        }
    }
}

impl std::fmt::Display for LevelDrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Return => "a trap return",
            Self::NextInterrupt => "a write of mnxti",
            Self::Status => "a write of mintstatus",
        })
    }
}

/// Every call and out-of-function jump of `function` in `elf`.
/// What a sweep finds in one function.
pub(crate) struct Swept {
    pub transfers: Vec<Transfer>,
    /// Indirect jumps through a table of known entries, with every entry.
    pub jumps: Vec<(u32, Vec<u32>)>,
    /// Instructions that can lower the interrupt level, by site.
    pub level_drops: Vec<(u32, LevelDrop)>,
    /// Floating-point instructions and accesses of the floating-point CSRs
    /// (`fflags`, `frm`, `fcsr`): each traps while `mstatus.FS` is off.
    pub float_sites: Vec<u32>,
}

pub(crate) fn transfers(
    image: &ImageFacts<'_>,
    function: &Function,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Swept> {
    let bytes = function_bytes(image.file(), function)?;
    let (start, end) = (function.address, function.address + function.size);
    let outside = |target: u32| target < start || target >= end;
    let mut transfers = Vec::new();
    let mut jumps = Vec::new();
    let mut offset = 0usize;
    let mut previous: Option<(u8, u32)> = None;
    let mut writes = [None::<Write>; 32];
    let merges = merges(bytes, start, end, ledger, control)?;
    let mut facts = [None::<Fact>; 32];
    let mut loads = [None::<Load>; 32];
    let mut level_drops = Vec::new();
    let mut float_sites = Vec::new();
    while offset < bytes.len() {
        let pc = start + offset as u32;
        if merges.contains(&pc) {
            facts = [None; 32];
            loads = [None; 32];
        }
        let Some((instruction, length)) = decode(&bytes[offset..], Extensions::ALL) else {
            // An encoding outside the decoder's set (a trap return, say) still
            // has its length in its low bits; it carries no direct transfer,
            // and the value analysis classifies it.
            if let Some(word) = bytes.get(offset..offset + 4)
                && let Some(drop) =
                    LevelDrop::of_word(u32::from_le_bytes(word.try_into().expect("4 bytes")))
            {
                ledger.push(&mut level_drops, (pc, drop), control)?;
            }
            offset += if bytes[offset] & 3 == 3 { 4 } else { 2 };
            previous = None;
            facts = [None; 32];
            loads = [None; 32];
            continue;
        };
        if let Instruction::Extension(Extension::Csr {
            op, source, csr, ..
        }) = instruction
            && let Some(drop) = LevelDrop::of_csr(csr, op, source)
        {
            ledger.push(&mut level_drops, (pc, drop), control)?;
        }
        if matches!(
            instruction,
            Instruction::Float(_)
                | Instruction::Extension(Extension::Csr {
                    csr: 0x001..=0x003,
                    ..
                })
        ) {
            ledger.push(&mut float_sites, pc, control)?;
        }
        let resolved = |target: u32, kind| Transfer {
            site: pc,
            target: Some(target),
            kind,
            source: None,
            table: None,
            load: None,
        };
        match instruction {
            Instruction::Base(Inst::Jal {
                offset: displacement,
                dest,
            }) => {
                let target = pc.wrapping_add(displacement.as_u32());
                if matches!(dest.0, 1 | 5) {
                    ledger.push(
                        &mut transfers,
                        resolved(target, TransferKind::Call),
                        control,
                    )?;
                } else if outside(target) {
                    ledger.push(
                        &mut transfers,
                        resolved(target, TransferKind::Tail),
                        control,
                    )?;
                }
            }
            Instruction::Base(Inst::Jalr {
                offset: displacement,
                base,
                dest,
            }) => {
                let link = matches!(dest.0, 1 | 5);
                // The link's relocations name a compiler table exactly; a
                // bounds check or mask only bounds the index.
                let dispatched = match facts[base.0 as usize] {
                    Some(Fact::TableEntry {
                        base: TableBase::Address(table),
                        entries,
                    }) if displacement.as_i32() == 0 => {
                        match crate::image::jump_table(image, table, ledger, control)? {
                            Some(entries) => Some(entries),
                            None => match entries {
                                Some(entries) => crate::image::words(
                                    image,
                                    table,
                                    u64::from(entries),
                                    ledger,
                                    control,
                                )?,
                                None => None,
                            },
                        }
                    }
                    _ => None,
                };
                let kind = if link {
                    TransferKind::Call
                } else {
                    TransferKind::Tail
                };
                match previous {
                    Some((register, value)) if register == base.0 => {
                        let target = value.wrapping_add(displacement.as_u32()) & !1;
                        if link || outside(target) {
                            ledger.push(&mut transfers, resolved(target, kind), control)?;
                        }
                    }
                    // `ret`: a return, not a transfer.
                    _ if dest.0 == 0 && matches!(base.0, 1 | 5) && displacement.as_i32() == 0 => {}
                    // A table of exactly known length: each entry is a
                    // call, or a jump out of the function; the sweep
                    // already covers jumps inside it.
                    _ if dispatched.is_some() => {
                        if !link {
                            let mut entries = Vec::new();
                            for &entry in dispatched.iter().flatten() {
                                ledger.push(&mut entries, entry, control)?;
                            }
                            ledger.push(&mut jumps, (pc, entries), control)?;
                        }
                        let mut targets = Vec::new();
                        // A Rust function pointer is never null: a zero
                        // entry is an empty `Option<fn>` slot.
                        for &word in dispatched.iter().flatten() {
                            let target = word & !1;
                            if word != 0 && (link || outside(target)) {
                                ledger.push(&mut targets, target, control)?;
                            }
                        }
                        targets.sort_unstable();
                        targets.dedup();
                        for target in targets {
                            ledger.push(&mut transfers, resolved(target, kind), control)?;
                        }
                    }
                    _ => ledger.push(
                        &mut transfers,
                        Transfer {
                            site: pc,
                            target: None,
                            kind,
                            source: Some(match writes[base.0 as usize] {
                                Some(Write::Load { from_sp: true }) => TargetSource::StackSlot,
                                Some(Write::Load { from_sp: false }) => TargetSource::Memory,
                                _ => TargetSource::Register,
                            }),
                            table: match facts[base.0 as usize] {
                                Some(Fact::TableEntry { base, .. }) => Some(base),
                                _ => None,
                            },
                            load: (displacement.as_i32() == 0)
                                .then_some(loads[base.0 as usize])
                                .flatten(),
                        },
                        control,
                    )?,
                }
            }
            // cm.popret and cm.popretz return through ra.
            Instruction::Extension(Extension::Pop { ret: Some(_), .. }) => {}
            _ => {}
        }
        previous = upper(&instruction, pc);
        let next = fact(&instruction, pc, &facts, image);
        let load = match instruction {
            Instruction::Base(Inst::Lw { offset, base, .. }) => Some(Load {
                site: pc,
                exclusive_producer: false,
                base: base.0,
                offset: offset.as_i32(),
                address: match facts[base.0 as usize] {
                    Some(Fact::Constant(value)) => Some(value.wrapping_add(offset.as_u32())),
                    _ => None,
                },
            }),
            _ => None,
        };
        if let Some((register, how)) = write(&instruction) {
            writes[register as usize] = Some(how);
            // A load based on this register no longer names its address,
            // unless the address was known when it loaded.
            for entry in &mut loads {
                if entry.is_some_and(|load| load.base == register && load.address.is_none()) {
                    *entry = None;
                }
            }
            loads[register as usize] = load;
            // A fact indexed off this register no longer holds.
            for fact in &mut facts {
                if let Some(
                    Fact::TableAddress {
                        base: TableBase::Register(r),
                        ..
                    }
                    | Fact::TableEntry {
                        base: TableBase::Register(r),
                        ..
                    },
                ) = *fact
                    && r == register
                {
                    *fact = None;
                }
            }
            facts[register as usize] = next;
        }
        if let Some((index, fact)) = fall_through(&instruction, &facts) {
            facts[index as usize] = Some(fact);
        }
        // Code after a trap or unconditional transfer needs independent facts.
        if matches!(
            instruction,
            Instruction::Unimp
                | Instruction::Base(Inst::Jal { dest: Reg(0), .. } | Inst::Jalr { .. })
        ) {
            facts = [None; 32];
            loads = [None; 32];
        }
        offset += length;
    }
    Ok(Swept {
        transfers,
        jumps,
        level_drops,
        float_sites,
    })
}
