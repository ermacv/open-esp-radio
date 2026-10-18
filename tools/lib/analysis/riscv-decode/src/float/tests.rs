use super::*;
use crate::{Extensions, Instruction, decode};

/// Encodings and display text from LLVM 23's assembler
/// (`llvm-mc -triple=riscv32 -mattr=+f,+c -show-encoding`), which prints the
/// compressed loads and stores in their expanded form.
const FORMS: &[(&[u8], &str)] = &[
    (&[0x07, 0x25, 0xc1, 0xff], "flw fa0, -4(sp)"),
    (&[0x27, 0xae, 0x97, 0x7e], "fsw fs1, 2044(a5)"),
    (&[0xfe, 0x70], "flw ft1, 252(sp)"),
    (&[0xbe, 0xe0], "fsw fa5, 64(sp)"),
    (&[0x74, 0x7f], "flw fa3, 124(a4)"),
    (&[0xc0, 0xe0], "fsw fs0, 4(s1)"),
    (&[0x43, 0x95, 0xc5, 0x68], "fmadd.s fa0, fa1, fa2, fa3, rtz"),
    (&[0x4f, 0xf5, 0xc5, 0x68], "fnmadd.s fa0, fa1, fa2, fa3"),
    (&[0x53, 0xf0, 0x20, 0x00], "fadd.s ft0, ft1, ft2"),
    (&[0x53, 0x85, 0x05, 0x58], "fsqrt.s fa0, fa1, rne"),
    (&[0x53, 0x15, 0x05, 0xc0], "fcvt.w.s a0, fa0, rtz"),
    (&[0x53, 0xf5, 0x15, 0xd0], "fcvt.s.wu fa0, a1"),
    (&[0x53, 0x25, 0xb5, 0xa0], "feq.s a0, fa0, fa1"),
    (&[0xd3, 0x12, 0x10, 0xa0], "flt.s t0, ft0, ft1"),
    (&[0xd3, 0x85, 0xc5, 0xa0], "fle.s a1, fa1, fa2"),
    (&[0x53, 0x05, 0x05, 0xe0], "fmv.x.w a0, fa0"),
    (&[0x53, 0x05, 0x00, 0xf0], "fmv.w.x fa0, zero"),
    (&[0x53, 0x16, 0x06, 0xe0], "fclass.s a2, fa2"),
    (&[0x53, 0x95, 0xc5, 0x20], "fsgnjn.s fa0, fa1, fa2"),
    (&[0x53, 0x95, 0xc5, 0x28], "fmax.s fa0, fa1, fa2"),
];

#[test]
fn single_precision_forms_decode_as_the_assembler_wrote_them() {
    for (bytes, text) in FORMS {
        let (instruction, width) = decode(bytes, Extensions::ALL).unwrap();
        assert!(matches!(instruction, Instruction::Float(_)), "{text}");
        assert_eq!(instruction.to_string(), *text);
        assert_eq!(width, bytes.len());
    }
}

#[test]
fn only_an_integer_destination_names_an_integer_register() {
    // fmv.x.w a0, fa0 writes a0; fadd.s ft0, ft1, ft2 writes no integer register.
    let destination = |bytes: &[u8]| match decode(bytes, Extensions::ALL) {
        Some((Instruction::Float(Float::Operation { operands, .. }), _)) => operands[0],
        other => panic!("{other:?}"),
    };
    assert_eq!(
        destination(&[0x53, 0x05, 0x05, 0xe0]),
        Some(Register::Integer(10))
    );
    assert_eq!(
        destination(&[0x53, 0xf0, 0x20, 0x00]),
        Some(Register::Float(0))
    );
    assert_eq!(
        decode(&[0x07, 0x25, 0xc1, 0xff], Extensions::ALL).map(|(i, _)| i),
        Some(Instruction::Float(Float::Load {
            dest: 10,
            base: 2,
            offset: -4
        }))
    );
}

#[test]
fn reserved_rounding_modes_and_other_formats_stay_undecoded() {
    // fadd.s with rm = 5 and 6 (reserved), fadd.d (fmt = D), fld and c.fld.
    for bytes in [
        &[0x53, 0xd0, 0x20, 0x00][..],
        &[0x53, 0xe0, 0x20, 0x00],
        &[0x53, 0x70, 0x20, 0x02],
        &[0x07, 0x35, 0xc1, 0xff],
        &[0x00, 0x20],
    ] {
        assert!(decode(bytes, Extensions::ALL).is_none(), "{bytes:02x?}");
    }
}
