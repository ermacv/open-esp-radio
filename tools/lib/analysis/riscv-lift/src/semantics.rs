//! RV32 instruction lifting. Compressed forms are expanded by the pinned decoder.
use super::*;
impl FunctionSemantics for RiscvDecoder {
    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        let (Instruction::Base(inst), _) = decode_instruction(bytes)? else {
            return None;
        };
        let (test, a, b) = match inst {
            Inst::Beq { src1, src2, .. } => (BranchTest::Eq, src1, src2),
            Inst::Bne { src1, src2, .. } => (BranchTest::Ne, src1, src2),
            Inst::Blt { src1, src2, .. } => (BranchTest::Lt, src1, src2),
            Inst::Bge { src1, src2, .. } => (BranchTest::Ge, src1, src2),
            Inst::Bltu { src1, src2, .. } => (BranchTest::Ltu, src1, src2),
            Inst::Bgeu { src1, src2, .. } => (BranchTest::Geu, src1, src2),
            _ => return None,
        };
        Some((test, Operand::Register(a.0), Operand::Register(b.0)))
    }

    fn semantic_identity(&self) -> &'static str {
        "rv32imafc-zicsr-zba-zbb-zbs-zcb-zcmp/values-10/rv-asm-0.2.1"
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        let inst = match decode_instruction(bytes) {
            Some((Instruction::Base(inst), _)) => inst,
            Some((Instruction::Extension(extension), _)) => return lift_extension(extension),
            Some((Instruction::Float(float), _)) => return lift_float(float),
            None => return SemanticOp::Unsupported,
        };
        use Operand::{Immediate as Imm, Register as Reg};
        match inst {
            Inst::Lui { uimm, dest } => SemanticOp::Upper {
                dest: dest.0,
                value: uimm.as_u32(),
                pc_relative: false,
            },
            Inst::Auipc { uimm, dest } => SemanticOp::Upper {
                dest: dest.0,
                value: uimm.as_u32(),
                pc_relative: true,
            },
            Inst::Jal { dest, .. } | Inst::Jalr { dest, .. } => SemanticOp::Link { dest: dest.0 },
            Inst::Addi { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Slti { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Lt,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Sltiu { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Ltu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Xori { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Xor,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Ori { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Or,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Andi { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::And,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Slli { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Srli { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Shr,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Srai { imm, dest, src1 } => SemanticOp::Integer {
                op: IntegerOp::Sar,
                dest: dest.0,
                left: Reg(src1.0),
                right: Imm(imm.as_u32()),
            },
            Inst::Add { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sub { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Sub,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Slt { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Lt,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sltu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Ltu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Xor { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Xor,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Or { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Or,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::And { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::And,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sll { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Srl { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Shr,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Sra { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Sar,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mul { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mul,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulh { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulh,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulhsu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulhsu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Mulhu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Mulhu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Div { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Div,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Divu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Divu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Rem { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Rem,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Remu { dest, src1, src2 } => SemanticOp::Integer {
                op: IntegerOp::Remu,
                dest: dest.0,
                left: Reg(src1.0),
                right: Reg(src2.0),
            },
            Inst::Lb { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: true,
            },
            Inst::Lbu { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Lh { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: true,
            },
            Inst::Lhu { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Lw { offset, dest, base } => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: base.0,
                displacement: offset.as_i32(),
                width: 4,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::Sb { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 1,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::Sh { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 2,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::Sw { offset, src, base } => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: base.0,
                displacement: offset.as_i32(),
                width: 4,
                dest: None,
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::LrW { dest, addr, .. } => SemanticOp::Memory {
                kind: MemoryKind::LoadReserved,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: None,
                swap: false,
                signed: false,
            },
            Inst::ScW {
                dest, addr, src, ..
            } => SemanticOp::Memory {
                kind: MemoryKind::StoreConditional,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: Some(src.0),
                swap: false,
                signed: false,
            },
            Inst::AmoW {
                op,
                dest,
                addr,
                src,
                ..
            } => SemanticOp::Memory {
                kind: MemoryKind::Atomic,
                base: addr.0,
                displacement: 0,
                width: 4,
                dest: Some(dest.0),
                source: Some(src.0),
                swap: op == oer_riscv_decode::AmoOp::Swap,
                signed: false,
            },
            Inst::Beq { .. }
            | Inst::Bne { .. }
            | Inst::Blt { .. }
            | Inst::Bge { .. }
            | Inst::Bltu { .. }
            | Inst::Bgeu { .. } => SemanticOp::None,
            Inst::Fence { fence } => {
                let bits = |s: oer_riscv_decode::FenceSet| {
                    u8::from(s.device_input) * 8
                        + u8::from(s.device_output) * 4
                        + u8::from(s.memory_read) * 2
                        + u8::from(s.memory_write)
                };
                SemanticOp::Fence {
                    fm: fence.fm,
                    predecessor: bits(fence.pred),
                    successor: bits(fence.succ),
                }
            }
            _ => SemanticOp::Unsupported,
        }
    }
    fn value_relocation(&self, r: &FunctionRelocation, operation: SemanticOp) -> ValueRelocation {
        let store = matches!(
            operation,
            SemanticOp::Memory {
                kind: MemoryKind::Store | MemoryKind::FloatStore,
                ..
            }
        );
        let kind = rv32::kind(r.relocation_type);
        let low = matches!(kind.role, Role::AbsoluteLow | Role::PcRelativeLow);
        if low && kind.field == rv32::Field::Store && !store
            || low
                && kind.field == rv32::Field::Immediate
                && !matches!(
                    operation,
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        right: Operand::Immediate(_),
                        ..
                    } | SemanticOp::Memory {
                        kind: MemoryKind::Load | MemoryKind::FloatLoad,
                        ..
                    }
                )
        {
            return ValueRelocation::Unsupported;
        }
        match kind.role {
            Role::Hint | Role::Branch | Role::Jump => ValueRelocation::Ignore,
            Role::Call => ValueRelocation::CallUpper,
            Role::AbsoluteHigh => ValueRelocation::UpperAbsolute,
            Role::PcRelativeHigh => ValueRelocation::UpperPcRelative,
            Role::AbsoluteLow => ValueRelocation::LowerAbsolute,
            Role::PcRelativeLow => ValueRelocation::LowerPcRelative,
            Role::Word | Role::Other => ValueRelocation::Unsupported,
        }
    }
}

/// Integer and Zcb memory forms lift to single operations. The Zcmp stack
/// forms move several registers at once and have no single-operation lift.
fn lift_extension(extension: Extension) -> SemanticOp {
    match extension {
        Extension::Integer {
            op,
            dest,
            left,
            right,
        } => SemanticOp::Integer {
            op: integer_op(op),
            dest,
            left: Operand::Register(left),
            right: match right {
                oer_riscv_decode::Operand::Register(r) => Operand::Register(r),
                oer_riscv_decode::Operand::Immediate(v) => Operand::Immediate(v),
            },
        },
        Extension::Memory {
            load,
            register,
            base,
            offset,
            width,
            signed,
        } => SemanticOp::Memory {
            kind: if load {
                MemoryKind::Load
            } else {
                MemoryKind::Store
            },
            base,
            displacement: i32::from(offset),
            width,
            dest: load.then_some(register),
            source: (!load).then_some(register),
            swap: false,
            signed,
        },
        Extension::Push { .. }
        | Extension::Pop { .. }
        | Extension::MoveToSaved { .. }
        | Extension::MoveFromSaved { .. } => SemanticOp::Unsupported,
        // A CSR access's only integer-register effect is the old CSR value,
        // outside the integer model.
        Extension::Csr { dest, .. } => SemanticOp::Opaque { dest },
    }
}

/// The domain operation of a decoded extension operation.
fn integer_op(op: oer_riscv_decode::ExtensionOp) -> IntegerOp {
    use oer_riscv_decode::ExtensionOp as E;
    match op {
        E::And => IntegerOp::And,
        E::Xor => IntegerOp::Xor,
        E::Mul => IntegerOp::Mul,
        E::ShiftAdd1 => IntegerOp::ShiftAdd1,
        E::ShiftAdd2 => IntegerOp::ShiftAdd2,
        E::ShiftAdd3 => IntegerOp::ShiftAdd3,
        E::AndNot => IntegerOp::AndNot,
        E::OrNot => IntegerOp::OrNot,
        E::XorNot => IntegerOp::XorNot,
        E::Min => IntegerOp::Min,
        E::Minu => IntegerOp::Minu,
        E::Max => IntegerOp::Max,
        E::Maxu => IntegerOp::Maxu,
        E::RotateLeft => IntegerOp::RotateLeft,
        E::RotateRight => IntegerOp::RotateRight,
        E::CountLeadingZeros => IntegerOp::CountLeadingZeros,
        E::CountTrailingZeros => IntegerOp::CountTrailingZeros,
        E::PopCount => IntegerOp::PopCount,
        E::SignExtendByte => IntegerOp::SignExtendByte,
        E::SignExtendHalf => IntegerOp::SignExtendHalf,
        E::OrCombineBytes => IntegerOp::OrCombineBytes,
        E::ByteReverse => IntegerOp::ByteReverse,
        E::BitSet => IntegerOp::BitSet,
        E::BitClear => IntegerOp::BitClear,
        E::BitInvert => IntegerOp::BitInvert,
        E::BitExtract => IntegerOp::BitExtract,
    }
}

/// FP loads and stores address memory through an integer base; an
/// operation's only integer effect is an integer destination.
fn lift_float(float: oer_riscv_decode::Float) -> SemanticOp {
    let memory = |kind, base, displacement| SemanticOp::Memory {
        kind,
        base,
        displacement,
        width: 4,
        dest: None,
        source: None,
        swap: false,
        signed: false,
    };
    match float {
        oer_riscv_decode::Float::Load { base, offset, .. } => {
            memory(MemoryKind::FloatLoad, base, offset)
        }
        oer_riscv_decode::Float::Store { base, offset, .. } => {
            memory(MemoryKind::FloatStore, base, offset)
        }
        oer_riscv_decode::Float::Operation {
            operands: [Some(oer_riscv_decode::Register::Integer(dest)), ..],
            ..
        } => SemanticOp::Opaque { dest },
        oer_riscv_decode::Float::Operation { .. } => SemanticOp::None,
    }
}

#[cfg(test)]
mod tests;
