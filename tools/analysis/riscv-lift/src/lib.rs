//! ISA-only function decoder and relocation interpretation; no I/O authority.

#![forbid(unsafe_code)]
use oer_analysis_elf::rv32::{self, Role};
pub use oer_analysis_riscv_decode::register_name;
use oer_analysis_riscv_decode::{Extension, Extensions, Inst, Instruction};
use oer_analysis_riscv_model::*;
pub struct RiscvDecoder;
impl FunctionDecoder for RiscvDecoder {
    fn identity(&self) -> &'static str {
        "rv32imafc-zicsr-zba-zbb-zbs-zcb-zcmp/rv-asm-0.2.1/policy-7"
    }
    fn unsupported_flow(&self, bytes: &[u8]) -> UnsupportedFlow {
        // ISA structure only, not CSR/privileged execution support. Zicsr:
        // https://docs.riscv.org/reference/isa/unpriv/zicsr.html
        // Trap returns transfer through architectural CSR state, not a static target.
        if let Ok(raw) = <[u8; 4]>::try_from(bytes) {
            let word = u32::from_le_bytes(raw);
            if matches!(word, 0x30200073 | 0x10200073) {
                return UnsupportedFlow::Indirect;
            }
            if word == 0x10500073
                || ((word & 0x7f) == 0x73 && matches!((word >> 12) & 7, 1 | 2 | 3 | 5 | 6 | 7))
            {
                return UnsupportedFlow::NonControl;
            }
            if matches!(
                word & 0x7f,
                0x03 | 0x07
                    | 0x0f
                    | 0x13
                    | 0x17
                    | 0x23
                    | 0x27
                    | 0x2f
                    | 0x33
                    | 0x37
                    | 0x43
                    | 0x47
                    | 0x4b
                    | 0x4f
                    | 0x53
            ) {
                return UnsupportedFlow::NonControl;
            }
        } else if let Ok(raw) = <[u8; 2]>::try_from(bytes) {
            let half = u16::from_le_bytes(raw);
            if half == 0 || matches!((half & 3, half >> 13), (0, _) | (2, 0..=3) | (2, 5..=7)) {
                return UnsupportedFlow::NonControl;
            }
        }
        UnsupportedFlow::Unknown
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        let (inst, width) = match decode_instruction(bytes)? {
            (Instruction::Base(inst), width) => (inst, width),
            (Instruction::Extension(extension), width) => {
                let flow = match extension {
                    Extension::Pop { ret: Some(_), .. } => InstructionFlow::Return {
                        base: oer_analysis_riscv_decode::RA,
                    },
                    _ => InstructionFlow::Next,
                };
                return Some(DecodedOp {
                    length: width as u8,
                    text: extension.to_string(),
                    flow,
                });
            }
            (Instruction::Float(float), width) => {
                return Some(DecodedOp {
                    length: width as u8,
                    text: float.to_string(),
                    flow: InstructionFlow::Next,
                });
            }
        };
        let flow = match inst {
            Inst::Jal { offset, dest } => InstructionFlow::Jump {
                displacement: offset.as_i32(),
                link: matches!(dest.0, 1 | 5),
            },
            // A jump through a link register that links none returns: the
            // pop hint of the unprivileged ISA's `jalr` table.
            Inst::Jalr { offset, base, dest }
                if offset.as_i32() == 0 && matches!(base.0, 1 | 5) && !matches!(dest.0, 1 | 5) =>
            {
                InstructionFlow::Return { base: base.0 }
            }
            Inst::Jalr { offset, base, dest } => InstructionFlow::Indirect {
                base: base.0,
                offset: offset.as_i32(),
                link: matches!(dest.0, 1 | 5),
            },
            Inst::Beq { offset, .. }
            | Inst::Bne { offset, .. }
            | Inst::Blt { offset, .. }
            | Inst::Bge { offset, .. }
            | Inst::Bltu { offset, .. }
            | Inst::Bgeu { offset, .. } => InstructionFlow::Branch {
                displacement: offset.as_i32(),
            },
            Inst::Ecall | Inst::Ebreak => InstructionFlow::Stop,
            _ => InstructionFlow::Next,
        };
        Some(DecodedOp {
            length: width as u8,
            text: inst.to_string(),
            flow,
        })
    }
    fn reference(
        &self,
        r: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        control: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        control.checkpoint(1)?;
        let mut target = r.target.clone();
        let mut addend = r.addend;
        let mut paired = None;
        let kind = match rv32::kind(r.relocation_type).role {
            Role::Call => ReferenceKind::Call,
            Role::Jump | Role::Branch => ReferenceKind::Branch,
            Role::AbsoluteHigh | Role::AbsoluteLow | Role::PcRelativeHigh | Role::Word => {
                ReferenceKind::Address
            }
            Role::PcRelativeLow => {
                let mut found = None;
                let mut count = 0;
                if r.target.section == Some(section) && r.addend == Some(0) {
                    let start = all.partition_point(|hi| hi.offset < r.target.offset);
                    for hi in all[start..]
                        .iter()
                        .take_while(|hi| hi.offset == r.target.offset)
                    {
                        control.checkpoint(1)?;
                        if hi.offset == r.target.offset
                            && rv32::kind(hi.relocation_type).role == Role::PcRelativeHigh
                        {
                            found = Some(hi);
                            count += 1;
                        }
                    }
                }
                if count == 1 {
                    let hi = found.unwrap();
                    target = hi.target.clone();
                    addend = hi.addend;
                    paired = Some((hi.section, hi.index));
                    ReferenceKind::Address
                } else {
                    ReferenceKind::Unknown
                }
            }
            Role::Hint => ReferenceKind::Metadata,
            Role::Other => ReferenceKind::Unknown,
        };
        let known = kind != ReferenceKind::Unknown
            && (kind == ReferenceKind::Metadata
                || (addend.is_some() && target.definition != SymbolDefinition::Null));
        Ok(NormalizedReference {
            kind,
            target,
            addend,
            paired,
            known,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_analysis_elf::rv32::{R_RISCV_PCREL_HI20, R_RISCV_PCREL_LO12_I};
    fn relocation(offset: u64, kind: u32, target_offset: u64) -> FunctionRelocation {
        FunctionRelocation {
            section: 9,
            index: offset,
            offset,
            relocation_type: kind,
            addend: Some(0),
            target: std::sync::Arc::new(ReferenceTarget {
                binding: 1,
                definition: SymbolDefinition::Section,
                symbol: SymbolId {
                    object: ObjectId {
                        artifact: ArtifactId::of_bytes(b"fixture"),
                        location: ObjectLocation::Standalone,
                    },
                    table: SymbolTableKind::Static,
                    table_section: 8,
                    index: offset + 1,
                },
                name: b"symbol".to_vec(),
                section: Some(1),
                offset: target_offset,
                symbol_type: 0,
            }),
        }
    }
    #[test]
    fn pcrel_lo_uses_label_identity_not_adjacency_or_symbol_name() {
        let lo = relocation(24, R_RISCV_PCREL_LO12_I, 0);
        let mut hi = relocation(0, R_RISCV_PCREL_HI20, 128);
        std::sync::Arc::make_mut(&mut hi.target).name = b"actual_data".to_vec();
        hi.addend = Some(4);
        let other = relocation(20, R_RISCV_PCREL_HI20, 256);
        let all = [hi.clone(), other, lo.clone()];
        let r = RiscvDecoder
            .reference(&lo, &all, 1, &mut || Ok(()))
            .unwrap();
        assert!(r.known);
        assert_eq!(r.target, hi.target);
        assert_eq!(r.addend, Some(4));
        assert_eq!(r.paired, Some((9, 0)));
        let r = RiscvDecoder
            .reference(&lo, &[hi.clone(), hi], 1, &mut || Ok(()))
            .unwrap();
        assert!(!r.known);
    }
    #[test]
    fn compressed_andi_uses_signed_six_bit_immediates_in_display_and_lifting() {
        // Independently encode every C.ANDI register and immediate, and compare
        // with the ordinary ANDI encoding and the ISA's signed numeric value.
        for register in 8u16..16 {
            for signed in -32i32..32 {
                let bits = (signed as u16) & 63;
                let half = 0x8801 | ((register - 8) << 7) | ((bits & 31) << 2) | ((bits & 32) << 7);
                let word = ((signed as u32 & 0xfff) << 20)
                    | (u32::from(register) << 15)
                    | 0x7013
                    | (u32::from(register) << 7);
                let expected = SemanticOp::Integer {
                    op: IntegerOp::And,
                    dest: register as u8,
                    left: Operand::Register(register as u8),
                    right: Operand::Immediate(signed as u32),
                };
                assert_eq!(RiscvDecoder.lift(&half.to_le_bytes()), expected);
                assert_eq!(RiscvDecoder.lift(&word.to_le_bytes()), expected);
                let compressed = RiscvDecoder.decode(&half.to_le_bytes()).unwrap();
                let full = RiscvDecoder.decode(&word.to_le_bytes()).unwrap();
                assert_eq!(compressed.text, full.text);
                assert_eq!(compressed.length, 2);
                assert_eq!(full.length, 4);
            }
        }
        // Ordinary ANDI has twelve immediate bits; it must not be truncated.
        for signed in [-2048i32, -33, 32, 63, 2047] {
            let word = ((signed as u32 & 0xfff) << 20) | 0x57513;
            assert!(matches!(RiscvDecoder.lift(&word.to_le_bytes()),
                SemanticOp::Integer { right: Operand::Immediate(value), .. }
                if value == signed as u32));
        }
    }
    #[test]
    fn only_a_jump_through_a_link_register_at_zero_that_links_none_returns() {
        let flow = |word: u32| RiscvDecoder.decode(&word.to_le_bytes()).unwrap().flow;
        // jr t0; jalr t1, 0(ra)
        assert_eq!(flow(0x0002_8067), InstructionFlow::Return { base: 5 });
        assert_eq!(flow(0x0000_8367), InstructionFlow::Return { base: 1 });
        // jalr zero, 4(ra); jalr ra, 0(t0); jr a5
        for (word, base, offset, link) in [
            (0x0040_8067, 1, 4, false),
            (0x0002_80e7, 5, 0, true),
            (0x0007_8067, 15, 0, false),
        ] {
            assert_eq!(
                flow(word),
                InstructionFlow::Indirect { base, offset, link },
                "{word:#x}"
            );
        }
        // c.jr ra
        assert_eq!(
            RiscvDecoder.decode(&0x8082u16.to_le_bytes()).unwrap().flow,
            InstructionFlow::Return { base: 1 }
        );
    }
    #[test]
    fn decoding_rejects_truncated_long_and_unsupported_encodings() {
        assert_eq!(RiscvDecoder.decode(&[1, 0]).unwrap().length, 2);
        assert!(RiscvDecoder.decode(&[0x13, 0, 0]).is_none());
        assert!(matches!(
            RiscvDecoder.decode(&[0x6f, 1, 0x40, 0]).unwrap().flow,
            InstructionFlow::Jump { link: false, .. }
        ));
        assert!(RiscvDecoder.decode(&[0xff, 0xff, 0xff, 0xff]).is_none());
        assert_eq!(
            RiscvDecoder.decode(&[0x67, 0x80, 0, 0]).unwrap().flow,
            InstructionFlow::Return { base: 1 }
        );
    }
}

pub mod listing;

/// Every form the decoder knows: `rv32imafc` with the bit-manipulation and
/// code-size extensions, without Zcmt.
pub fn decode_instruction(bytes: &[u8]) -> Option<(Instruction, usize)> {
    oer_analysis_riscv_decode::decode(bytes, Extensions::ALL)
}

/// Whether the decoder classifies the next instruction as a base RV32IMAC
/// form, including expanded compressed forms. Extension and floating-point
/// forms are excluded even when they lift to supported integer operations.
/// This classification does not imply that all semantic effects are known.
pub fn is_base_instruction(bytes: &[u8]) -> bool {
    matches!(decode_instruction(bytes), Some((Instruction::Base(_), _)))
}

mod semantics;

/// The structural decoder can identify a non-control instruction even when
/// its integer effects are unsupported. Visit its continuation, with every
/// value killed by `Unsupported`, rather than silently losing an incoming
/// edge (for example the executor loop's `wfi`).
pub struct StructuralRiscvDecoder;
impl FunctionDecoder for StructuralRiscvDecoder {
    fn identity(&self) -> &'static str {
        "rv32/structural-proof-1"
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

impl FunctionSemantics for StructuralRiscvDecoder {
    fn interrupt_wait(&self, bytes: &[u8]) -> bool {
        RiscvDecoder.interrupt_wait(bytes)
    }
    fn scratch_write(&self, bytes: &[u8]) -> Option<Operand> {
        RiscvDecoder.scratch_write(bytes)
    }
    fn semantic_identity(&self) -> &'static str {
        "rv32/conservative-proof-2"
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

/// Opt-in stack-context semantics over the same decoder and value engine.
/// Hints preserve integer registers; waits explicitly lose remembered memory
/// in the evaluator and prove no source contents.
pub struct StackRiscvDecoder;
impl FunctionDecoder for StackRiscvDecoder {
    fn identity(&self) -> &'static str {
        "rv32/stack-context-1"
    }
    fn unsupported_flow(&self, bytes: &[u8]) -> UnsupportedFlow {
        StructuralRiscvDecoder.unsupported_flow(bytes)
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        let hint = if self.interrupt_wait(bytes) {
            Some("wfi")
        } else if bytes.get(..4) == Some(&[0x0f, 0x10, 0, 0]) {
            Some("fence.i")
        } else {
            None
        };
        if let Some(text) = hint {
            return Some(DecodedOp {
                length: 4,
                text: text.into(),
                flow: InstructionFlow::Next,
            });
        }
        StructuralRiscvDecoder.decode(bytes)
    }
    fn reference(
        &self,
        relocation: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        c: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        StructuralRiscvDecoder.reference(relocation, all, section, c)
    }
}
impl FunctionSemantics for StackRiscvDecoder {
    fn semantic_identity(&self) -> &'static str {
        "rv32/stack-context-values-1"
    }
    fn interrupt_wait(&self, bytes: &[u8]) -> bool {
        bytes.get(..4) == Some(&[0x73, 0, 0x50, 0x10])
    }
    fn scratch_write(&self, bytes: &[u8]) -> Option<Operand> {
        StructuralRiscvDecoder.scratch_write(bytes)
    }
    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        StructuralRiscvDecoder.branch(bytes)
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        if self.interrupt_wait(bytes) {
            return SemanticOp::Opaque { dest: 0 };
        }
        if bytes.get(..4) == Some(&[0x0f, 0x10, 0, 0]) {
            return SemanticOp::None;
        }
        StructuralRiscvDecoder.lift(bytes)
    }
    fn value_relocation(
        &self,
        relocation: &FunctionRelocation,
        operation: SemanticOp,
    ) -> ValueRelocation {
        StructuralRiscvDecoder.value_relocation(relocation, operation)
    }
}
