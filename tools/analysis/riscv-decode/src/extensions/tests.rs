use super::*;
use crate::{Extensions, Instruction, decode};

fn extension(bytes: &[u8]) -> Extension {
    match decode(bytes, Extensions::ALL) {
        Some((Instruction::Extension(extension), _)) => extension,
        other => panic!("{bytes:02x?} decoded to {other:?}"),
    }
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
        let (instruction, width) = decode(&half.to_le_bytes(), Extensions::ALL).unwrap();
        assert_eq!(instruction.to_string(), *text);
        assert_eq!(width, 2);
        assert_eq!(
            matches!(
                instruction,
                Instruction::Extension(Extension::Pop { ret: Some(_), .. })
            ),
            text.starts_with("cm.popret"),
            "{text}"
        );
    }
    // The frame stores s11 at the top and ra at the bottom.
    assert_eq!(list_registers(9).collect::<Vec<_>>(), [20, 19, 18, 9, 8, 1]);
    // Zcmt table jumps and a register list below {ra} are not decoded.
    assert!(decode(&0xa002u16.to_le_bytes(), Extensions::ALL).is_none());
    assert!(decode(&0xb832u16.to_le_bytes(), Extensions::ALL).is_none());
}

#[test]
fn integer_forms_decode_to_their_operations() {
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
            extension(&word.to_le_bytes()),
            Extension::Integer {
                op: *op,
                dest: *dest,
                left: *left,
                right: *right,
            },
            "{word:#010x}"
        );
        assert_eq!(decode(&word.to_le_bytes(), Extensions::ALL).unwrap().1, 4);
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
            extension(&half.to_le_bytes()),
            Extension::Integer {
                op: *op,
                dest: *dest,
                left: *dest,
                right: *right,
            },
            "{half:#06x}"
        );
    }
}

#[test]
fn zcb_memory_forms_decode_to_loads_and_stores() {
    // (LLVM encoding, load, register, base, offset, width, signed)
    let forms: &[(u16, bool, u8, u8, u8, u8, bool)] = &[
        (0x81e8, true, 10, 11, 3, 1, false),
        (0x8430, true, 12, 8, 2, 2, false),
        (0x87d4, true, 13, 15, 0, 2, true),
        (0x88d8, false, 14, 9, 1, 1, false),
        (0x8d3c, false, 15, 10, 2, 2, false),
    ];
    for (half, load, register, base, offset, width, signed) in forms {
        assert_eq!(
            extension(&half.to_le_bytes()),
            Extension::Memory {
                load: *load,
                register: *register,
                base: *base,
                offset: *offset,
                width: *width,
                signed: *signed,
            },
            "{half:#06x}"
        );
    }
}
