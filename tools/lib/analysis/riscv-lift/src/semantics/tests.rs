use super::*;
use oer_riscv_model::{FunctionDecoder, InstructionFlow};

#[test]
fn compressed_and_full_instructions_lift_without_display_parsing() {
    assert_eq!(
        RiscvDecoder.lift(&[0x05, 0x05]),
        SemanticOp::Integer {
            op: IntegerOp::Add,
            dest: 10,
            left: Operand::Register(10),
            right: Operand::Immediate(1)
        }
    );
    assert!(matches!(
        RiscvDecoder.lift(&[0x02, 0x45]),
        SemanticOp::Memory {
            kind: MemoryKind::Load,
            base: 2,
            dest: Some(10),
            width: 4,
            displacement: 0,
            ..
        }
    ));
    assert!(matches!(
        RiscvDecoder.lift(&0x60000537u32.to_le_bytes()),
        SemanticOp::Upper {
            dest: 10,
            value: 0x60000000,
            pc_relative: false
        }
    ));
    assert_eq!(RiscvDecoder.lift(&[0xff; 4]), SemanticOp::Unsupported);
    assert_eq!(RiscvDecoder.lift(&[0x73, 0, 0, 0]), SemanticOp::Unsupported);
}

#[test]
fn csr_accesses_write_only_their_destination() {
    // csrr a3, mhartid; csrw mscratch, sp; csrrw sp, mscratch, sp (LLVM).
    for (word, dest) in [(0xf140_26f3u32, 13), (0x3401_1073, 0), (0x3401_1173, 2)] {
        assert_eq!(
            RiscvDecoder.lift(&word.to_le_bytes()),
            SemanticOp::Opaque { dest },
            "{word:#010x}"
        );
    }
    // mret and wfi are not CSR accesses.
    assert_eq!(
        RiscvDecoder.lift(&0x3020_0073u32.to_le_bytes()),
        SemanticOp::Unsupported
    );
    assert_eq!(
        RiscvDecoder.lift(&0x1050_0073u32.to_le_bytes()),
        SemanticOp::Unsupported
    );
}

/// Encodings and display text from LLVM's assembler.
const STACK_FORMS: &[(u16, &str)] = &[
    (0xb892, "cm.push {ra, s0-s4}, -32"),
    (0xb842, "cm.push {ra}, -16"),
    (0xb8fe, "cm.push {ra, s0-s11}, -112"),
    (0xba52, "cm.pop {ra, s0}, 16"),
    (0xbe66, "cm.popret {ra, s0-s1}, 32"),
    (0xbc72, "cm.popretz {ra, s0-s2}, 16"),
    (0xaca2, "cm.mvsa01 s1, s0"),
    (0xad7e, "cm.mva01s s2, s7"),
];

#[test]
fn stack_forms_decode_as_the_assembler_wrote_them() {
    for (half, text) in STACK_FORMS {
        let decoded = RiscvDecoder.decode(&half.to_le_bytes()).unwrap();
        assert_eq!(decoded.text, *text);
        assert_eq!(decoded.length, 2);
        let returns = text.starts_with("cm.popret");
        assert_eq!(
            decoded.flow == InstructionFlow::Return { base: 1 },
            returns,
            "{text}"
        );
        assert_eq!(
            RiscvDecoder.lift(&half.to_le_bytes()),
            SemanticOp::Unsupported
        );
    }
    // The frame stores s11 at the top and ra at the bottom.
    assert_eq!(
        oer_riscv_decode::list_registers(9).collect::<Vec<_>>(),
        [20, 19, 18, 9, 8, 1]
    );
    // Zcmt table jumps and a register list below {ra} are not decoded.
    assert!(RiscvDecoder.decode(&0xa002u16.to_le_bytes()).is_none());
    assert!(RiscvDecoder.decode(&0xb832u16.to_le_bytes()).is_none());
}

#[test]
fn integer_forms_lift_to_their_operations() {
    use IntegerOp::*;
    use Operand::{Immediate as I, Register as R};
    // (LLVM encoding, operation, destination, left, right)
    let words: &[(u32, IntegerOp, u8, u8, Operand)] = &[
        (0x20c5a533, ShiftAdd1, 10, 11, R(12)),
        (0x20c5c533, ShiftAdd2, 10, 11, R(12)),
        (0x20f4e2b3, ShiftAdd3, 5, 9, R(15)),
        (0x40c5f533, AndNot, 10, 11, R(12)),
        (0x40c5e533, OrNot, 10, 11, R(12)),
        (0x40c5c533, XorNot, 10, 11, R(12)),
        (0x0ac5c533, Min, 10, 11, R(12)),
        (0x0ac5d533, Minu, 10, 11, R(12)),
        (0x0ac5e533, Max, 10, 11, R(12)),
        (0x0ac5f533, Maxu, 10, 11, R(12)),
        (0x60c59533, RotateLeft, 10, 11, R(12)),
        (0x60c5d533, RotateRight, 10, 11, R(12)),
        (0x6075d513, RotateRight, 10, 11, I(7)),
        (0x60059513, CountLeadingZeros, 10, 11, I(0)),
        (0x60159513, CountTrailingZeros, 10, 11, I(0)),
        (0x60259513, PopCount, 10, 11, I(0)),
        (0x60459513, SignExtendByte, 10, 11, I(0)),
        (0x60559513, SignExtendHalf, 10, 11, I(0)),
        (0x0805c533, And, 10, 11, I(0xffff)),
        (0x2875d513, OrCombineBytes, 10, 11, I(0)),
        (0x6985d513, ByteReverse, 10, 11, I(0)),
        (0x28c59533, BitSet, 10, 11, R(12)),
        (0x29f59513, BitSet, 10, 11, I(31)),
        (0x48c59533, BitClear, 10, 11, R(12)),
        (0x48359513, BitClear, 10, 11, I(3)),
        (0x68c59533, BitInvert, 10, 11, R(12)),
        (0x68459513, BitInvert, 10, 11, I(4)),
        (0x48c5d533, BitExtract, 10, 11, R(12)),
        (0x4855d513, BitExtract, 10, 11, I(5)),
    ];
    for (word, op, dest, left, right) in words {
        assert_eq!(
            RiscvDecoder.lift(&word.to_le_bytes()),
            SemanticOp::Integer {
                op: *op,
                dest: *dest,
                left: Operand::Register(*left),
                right: *right,
            },
            "{word:#010x}"
        );
        assert_eq!(RiscvDecoder.decode(&word.to_le_bytes()).unwrap().length, 4);
    }
    let halves: &[(u16, IntegerOp, u8, Operand)] = &[
        (0x9c61, And, 8, I(0xff)),
        (0x9ce5, SignExtendByte, 9, I(0)),
        (0x9d69, And, 10, I(0xffff)),
        (0x9ded, SignExtendHalf, 11, I(0)),
        (0x9e75, Xor, 12, I(u32::MAX)),
        (0x9ed9, Mul, 13, R(14)),
    ];
    for (half, op, dest, right) in halves {
        assert_eq!(
            RiscvDecoder.lift(&half.to_le_bytes()),
            SemanticOp::Integer {
                op: *op,
                dest: *dest,
                left: Operand::Register(*dest),
                right: *right,
            },
            "{half:#06x}"
        );
    }
}

#[test]
fn zcb_memory_forms_lift_to_loads_and_stores() {
    // (LLVM encoding, load, register, base, offset, width, signed)
    let forms: &[(u16, bool, u8, u8, i32, u8, bool)] = &[
        (0x81e8, true, 10, 11, 3, 1, false),
        (0x8430, true, 12, 8, 2, 2, false),
        (0x87d4, true, 13, 15, 0, 2, true),
        (0x88d8, false, 14, 9, 1, 1, false),
        (0x8d3c, false, 15, 10, 2, 2, false),
    ];
    for (half, load, register, base, offset, width, signed) in forms {
        let SemanticOp::Memory {
            kind,
            base: b,
            displacement,
            width: w,
            dest,
            source,
            signed: s,
            ..
        } = RiscvDecoder.lift(&half.to_le_bytes())
        else {
            panic!("{half:#06x} is not a memory operation");
        };
        assert_eq!(kind == MemoryKind::Load, *load);
        assert_eq!((b, displacement, w, s), (*base, *offset, *width, *signed));
        assert_eq!(if *load { dest } else { source }, Some(*register));
    }
}

#[test]
fn operations_follow_the_extension_definitions() {
    use IntegerOp::*;
    for (op, a, b, expected) in [
        (ShiftAdd3, 1, 2, 10),
        (XorNot, 0xf0, 0x0f, 0xffff_ff00),
        (Min, u32::MAX, 1, u32::MAX),
        (Maxu, u32::MAX, 1, u32::MAX),
        (RotateRight, 1, 33, 0x8000_0000),
        (CountLeadingZeros, 0, 0, 32),
        (CountTrailingZeros, 0, 0, 32),
        (SignExtendHalf, 0x8000, 0, 0xffff_8000),
        (OrCombineBytes, 0x0100_8000, 0, 0xff00_ff00),
        (BitClear, u32::MAX, 32, 0xffff_fffe),
        (BitExtract, 0x8000_0000, 31, 1),
    ] {
        assert_eq!(op.evaluate(a, b), expected, "{op:?}");
    }
}

#[test]
fn loads_and_stores_lift_to_float_memory_without_integer_registers() {
    assert_eq!(
        RiscvDecoder.lift(&[0x07, 0x25, 0xc1, 0xff]),
        SemanticOp::Memory {
            kind: MemoryKind::FloatLoad,
            base: 2,
            displacement: -4,
            width: 4,
            dest: None,
            source: None,
            swap: false,
            signed: false,
        }
    );
    assert_eq!(
        RiscvDecoder.lift(&[0xbe, 0xe0]),
        SemanticOp::Memory {
            kind: MemoryKind::FloatStore,
            base: 2,
            displacement: 64,
            width: 4,
            dest: None,
            source: None,
            swap: false,
            signed: false,
        }
    );
}

#[test]
fn only_an_integer_destination_is_an_integer_effect() {
    // fmv.x.w a0, fa0 and fcvt.w.s a0, fa0 write a0; fadd.s and fmv.w.x do not.
    assert_eq!(
        RiscvDecoder.lift(&[0x53, 0x05, 0x05, 0xe0]),
        SemanticOp::Opaque { dest: 10 }
    );
    assert_eq!(
        RiscvDecoder.lift(&[0x53, 0x15, 0x05, 0xc0]),
        SemanticOp::Opaque { dest: 10 }
    );
    assert_eq!(
        RiscvDecoder.lift(&[0x53, 0xf0, 0x20, 0x00]),
        SemanticOp::None
    );
    assert_eq!(
        RiscvDecoder.lift(&[0x53, 0x05, 0x00, 0xf0]),
        SemanticOp::None
    );
}
