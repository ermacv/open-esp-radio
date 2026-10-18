use super::*;

fn decodes(bytes: &[u8], extensions: Extensions) -> bool {
    decode(bytes, extensions).is_some()
}

#[test]
fn the_extension_set_selects_the_forms() {
    // add a0, a0, a1; c.addi a0, 1; mul a0, a0, a1; lr.w a0, (a0);
    // flw fa0, 0(a0); sh1add a0, a0, a1; cm.push {ra}, -16.
    let add = 0x00b5_0533u32.to_le_bytes();
    let c_addi = 0x0505u16.to_le_bytes();
    let mul = 0x02b5_0533u32.to_le_bytes();
    let lr = 0x1005_252fu32.to_le_bytes();
    let flw = 0x0005_2507u32.to_le_bytes();
    let sh1add = 0x20b5_2533u32.to_le_bytes();
    let push = 0xb842u16.to_le_bytes();
    assert!(decodes(&add, Extensions::NONE));
    for bytes in [&c_addi[..], &mul, &lr, &flw, &sh1add, &push] {
        assert!(!decodes(bytes, Extensions::NONE), "{bytes:02x?}");
        assert!(decodes(bytes, Extensions::ALL), "{bytes:02x?}");
    }
    for bytes in [&c_addi[..], &mul, &lr] {
        assert!(decodes(bytes, Extensions::RV32IMAC));
    }
    for bytes in [&flw[..], &sh1add, &push] {
        assert!(!decodes(bytes, Extensions::RV32IMAC), "{bytes:02x?}");
    }
    // c.flwsp fa0, 0(sp) needs both F and C.
    let c_flwsp = 0x6502u16.to_le_bytes();
    assert!(!decodes(&c_flwsp, Extensions::F));
    assert!(!decodes(&c_flwsp, Extensions::C));
    assert!(decodes(&c_flwsp, Extensions::F.union(Extensions::C)));
}

#[test]
fn compressed_zcb_arithmetic_needs_its_base_extensions() {
    let zcb = Extensions::C.union(Extensions::ZCB);
    // c.zext.b s0 and c.not a2 need only Zcb; c.sext.b s1 and c.zext.h a0
    // need Zbb; c.mul a3, a4 needs M.
    assert!(decodes(&0x9c61u16.to_le_bytes(), zcb));
    assert!(decodes(&0x9e75u16.to_le_bytes(), zcb));
    for (half, needs) in [
        (0x9ce5u16, Extensions::ZBB),
        (0x9d69, Extensions::ZBB),
        (0x9ed9, Extensions::M),
    ] {
        assert!(!decodes(&half.to_le_bytes(), zcb), "{half:#06x}");
        assert!(
            decodes(&half.to_le_bytes(), zcb.union(needs)),
            "{half:#06x}"
        );
    }
    // Their 32-bit counterparts need no C: zext.h a0, a1 is Zbb alone.
    assert!(decodes(&0x0805_c533u32.to_le_bytes(), Extensions::ZBB));
}

#[test]
fn compressed_andi_has_the_signed_six_bit_immediate() {
    // Independently encode every C.ANDI register and immediate and compare
    // with the ordinary ANDI encoding.
    for register in 8u16..16 {
        for signed in -32i32..32 {
            let bits = (signed as u16) & 63;
            let half = 0x8801 | ((register - 8) << 7) | ((bits & 31) << 2) | ((bits & 32) << 7);
            let word = ((signed as u32 & 0xfff) << 20)
                | (u32::from(register) << 15)
                | 0x7013
                | (u32::from(register) << 7);
            let (compressed, two) = decode(&half.to_le_bytes(), Extensions::ALL).unwrap();
            let (full, four) = decode(&word.to_le_bytes(), Extensions::ALL).unwrap();
            assert_eq!(compressed, full);
            assert_eq!((two, four), (2, 4));
            let Instruction::Base(Inst::Andi { imm, .. }) = compressed else {
                panic!("{compressed:?}");
            };
            assert_eq!(imm.as_i32(), signed);
        }
    }
}

#[test]
fn truncated_and_long_encodings_do_not_decode() {
    assert_eq!(decode(&[1, 0], Extensions::ALL).unwrap().1, 2);
    assert!(decode(&[1], Extensions::ALL).is_none());
    assert!(decode(&[0x13, 0, 0], Extensions::ALL).is_none());
    assert!(decode(&[0xff, 0xff, 0xff, 0xff], Extensions::ALL).is_none());
    assert!(decode(&[0, 0], Extensions::ALL).is_none());
}

#[test]
fn reserved_compressed_stack_adjustment_does_not_decode() {
    // C.ADDI16SP with nzimm = 0 is reserved; c.addi16sp sp, 16 and the
    // c.addi sp, 0 HINT are instructions.
    assert!(decode(&0x6101u16.to_le_bytes(), Extensions::ALL).is_none());
    assert!(decode(&0x6141u16.to_le_bytes(), Extensions::ALL).is_some());
    assert!(decode(&0x0101u16.to_le_bytes(), Extensions::ALL).is_some());
}

#[test]
fn zicsr_accesses_decode_with_their_csr_and_source() {
    // LLVM's encodings of csrrs a3, mhartid, zero; csrrw zero, mscratch, sp;
    // csrrci a0, mstatus, 8; csrrc t0, 0x7e1, a1.
    let forms: [(u32, &str); 4] = [
        (0xf140_26f3, "csrrs a3, 0xf14, zero"),
        (0x3401_1073, "csrrw zero, 0x340, sp"),
        (0x3004_7573, "csrrci a0, 0x300, 8"),
        (0x7e15_b2f3, "csrrc t0, 0x7e1, a1"),
    ];
    for (word, text) in forms {
        let bytes = word.to_le_bytes();
        let (instruction, width) = decode(&bytes, Extensions::ALL).unwrap();
        assert_eq!((instruction.to_string().as_str(), width), (text, 4));
        assert!(!decodes(&bytes, Extensions::RV32IMAC));
    }
    assert_eq!(
        decode(&0x3401_1073u32.to_le_bytes(), Extensions::ZICSR)
            .unwrap()
            .0,
        Instruction::Extension(Extension::Csr {
            op: CsrOp::Write,
            dest: 0,
            source: Operand::Register(2),
            csr: 0x340,
        })
    );
    // mret and wfi are not CSR accesses and stay undecoded.
    for word in [0x3020_0073u32, 0x1050_0073] {
        assert!(!decodes(&word.to_le_bytes(), Extensions::ALL));
    }
}
