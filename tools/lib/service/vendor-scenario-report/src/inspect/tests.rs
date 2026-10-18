use super::*;

/// Little-endian code of RISC-V instruction words written as the
/// disassembler prints them: eight hex digits for a full instruction, four
/// for a compressed one.
fn code(words: &[&str]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| {
            let value = u32::from_str_radix(word, 16).unwrap();
            let bytes = value.to_le_bytes();
            bytes[..word.len() / 2].to_vec()
        })
        .collect()
}

fn corpus(name: &str, code: Vec<u8>) -> Corpus {
    Corpus {
        functions: vec![Function {
            origin: "test".into(),
            name: name.into(),
            entry: 0,
            code,
            references: BTreeMap::new(),
            linked: true,
        }],
        names: BTreeMap::new(),
        registers: Registers::default(),
        missing: vec![],
    }
}

fn accesses(corpus: &Corpus) -> Vec<Access> {
    corpus
        .walk(&corpus.functions[0])
        .into_iter()
        .filter_map(|step| step.access)
        .collect()
}

#[test]
fn a_read_modify_write_that_ors_a_constant_sets_those_bits() {
    // libpp pwr_hal_set_lpclk_sync_enable: lui/addi the word, lw, lui the
    // bit, or, sw, ret.
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    assert_eq!(
        accesses(&corpus),
        [
            Access {
                location: Location::Absolute(0x2010_d830),
                width: 4,
                effect: Effect::Read,
            },
            Access {
                location: Location::Absolute(0x2010_d830),
                width: 4,
                effect: Effect::Modify {
                    clear: 0,
                    set: 0x0800_0000,
                    value: 0,
                },
            },
        ]
    );
}

#[test]
fn a_floating_point_load_reads_and_its_store_writes_an_unknown_word() {
    // lui/addi the word, flw fa0, 0(a5), fsw fa0, 4(a5), ret.
    let corpus = corpus(
        "float",
        code(&["2010e7b7", "83078793", "0007a507", "00a7a227", "8082"]),
    );
    assert_eq!(
        accesses(&corpus),
        [
            Access {
                location: Location::Absolute(0x2010_d830),
                width: 4,
                effect: Effect::Read,
            },
            Access {
                location: Location::Absolute(0x2010_d834),
                width: 4,
                effect: Effect::Unknown { bits: u32::MAX },
            },
        ]
    );
}

#[test]
fn a_masked_argument_or_into_a_cleared_field_is_a_value_field() {
    // libcoexist hal_set_extern_pti: clear bits 15:12 through an and-mask,
    // then insert (argument << 4) & 0xff into bits 7:4.
    let corpus = corpus(
        "set_pti",
        code(&[
            "2010f7b7", "49c78793", "4398", "76c5", "16fd", "8f75", "c398", "4398", "0592",
            "0ff5f593", "f0f77713", "8dd9", "c38c", "8082",
        ]),
    );
    let writes: Vec<Effect> = accesses(&corpus)
        .into_iter()
        .filter(|access| access.effect != Effect::Read)
        .map(|access| access.effect)
        .collect();
    assert_eq!(
        writes,
        [
            Effect::Modify {
                clear: 0xf000,
                set: 0,
                value: 0,
            },
            Effect::Modify {
                clear: 0,
                set: 0,
                value: 0xf0,
            },
        ]
    );
}

#[test]
fn xref_lists_only_accesses_inside_the_range() {
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    let inside = xref(&corpus, 0x2010_d830, 0x2010_d834);
    assert_eq!(inside.lines().count(), 2);
    assert!(inside.contains("W test::sync_enable+0x10  sets"));
    assert!(xref(&corpus, 0x2010_d834, 0x2010_d840).is_empty());
}

#[test]
fn show_prints_every_definition_with_folded_constants() {
    let corpus = corpus(
        "sync_enable",
        code(&[
            "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
        ]),
    );
    let text = show(&corpus, "sync_enable").unwrap();
    assert!(text.starts_with("== test::sync_enable at 0x0, 0x14 bytes"));
    assert!(text.contains("= 0x2010d830"));
    assert!(show(&corpus, "absent").is_none());
}

#[test]
fn a_store_through_a_loaded_pointer_is_a_field_of_the_argument() {
    // lw a5, 52(a0); lw a4, 0(a5); lui a3, 0x400; or a4, a4, a3;
    // sw a4, 0(a5); ret: set bit 22 of the word the pointer at a0+0x34
    // addresses.
    let corpus = corpus(
        "set_flag",
        code(&[
            "03452783", "0007a703", "004006b7", "00d76733", "00e7a023", "8082",
        ]),
    );
    let text = fields(&corpus, &[0x34, 0]);
    assert!(text.contains("a0->0x34->0x0 W test::set_flag+0x10  sets bits 22"));
    assert!(fields(&corpus, &[0x38, 0]).is_empty());
}

#[test]
fn a_value_both_paths_agree_on_survives_the_join() {
    // beq a1, zero, +8; addi a2, a2, 1; sw zero, 4(a0); ret: the store
    // after the join still addresses the first argument.
    let corpus = corpus(
        "join",
        code(&["00058463", "00160613", "00052223", "00008067"]),
    );
    assert!(fields(&corpus, &[4]).contains("a0->0x4 W test::join+0x8  = 0x0"));
}

#[test]
fn a_relocated_low_part_names_the_symbol_once() {
    // lui a5, %hi(g+0xc); lw a4, %lo(g+0xc)(a5); ret: the load reads
    // g+0xc, not g+0x18.
    let mut corpus = corpus("load", code(&["000007b7", "00c7a703", "8082"]));
    let relocation = |kind| Reference {
        kind,
        target: "g".into(),
        addend: 0xc,
        literal: None,
    };
    corpus.functions[0].references = BTreeMap::from([
        (0, relocation(oer_elf::rv32::R_RISCV_HI20)),
        (4, relocation(oer_elf::rv32::R_RISCV_LO12_I)),
    ]);
    let text = fields(&corpus, &[0xc]);
    assert!(text.contains("&g->0xc R test::load+0x4"));
    assert!(fields(&corpus, &[0x18]).is_empty());
}

#[test]
fn a_printed_register_field_is_tied_to_its_conversion() {
    // lui/addi 0x2010d830; lw a1, 0(a5); srli a1, a1, 4; andi a1, a1, 0xf;
    // lui/addi a0, "gain=%d\n"; call printf; ret: bits 7:4 of the register
    // are the first conversion.
    let mut corpus = corpus(
        "gauge",
        code(&[
            "2010e7b7", "83078793", "0007a583", "0045d593", "00f5f593", "00000537", "00050513",
            "00000097", "000080e7", "8082",
        ]),
    );
    let format = |kind| Reference {
        kind,
        target: ".rodata".into(),
        addend: 0,
        literal: Some("gain=%d\n".into()),
    };
    corpus.functions[0].references = BTreeMap::from([
        (0x14, format(oer_elf::rv32::R_RISCV_HI20)),
        (0x18, format(oer_elf::rv32::R_RISCV_LO12_I)),
        (
            0x1c,
            Reference {
                kind: oer_elf::rv32::R_RISCV_CALL_PLT,
                target: "printf".into(),
                addend: 0,
                literal: None,
            },
        ),
    ]);
    let text = prints(&corpus, 0x2010_d830, 0x2010_d834);
    assert!(
        text.contains("bits 4,5,6,7 >> 4 -> \"gain=%d\" test::gauge+0x20"),
        "{text}"
    );
    assert!(prints(&corpus, 0x2010_d834, 0x2010_d838).is_empty());
}

#[test]
fn conversions_carry_the_text_since_the_previous_one() {
    assert_eq!(
        conversions("a=%d 100%% b=%08lx\n"),
        ["a=%d", " 100%% b=%08lx"]
    );
}

#[test]
fn a_tail_call_prints_like_a_call() {
    // As a_printed_register_field_is_tied_to_its_conversion, but the print
    // is a tail call: auipc t1; jalr zero, 0(t1).
    let text = prints_ending_with("00000317", "00030067");
    assert!(
        text.contains("bits 4,5,6,7 >> 4 -> \"gain=%d\" test::gauge+0x20"),
        "{text}"
    );
}

#[test]
fn only_a_jump_the_decoder_does_not_mark_a_return_is_a_tail_call() {
    // jr t0 returns, so it prints nothing; jalr zero, 4(ra) is a jump.
    assert_eq!(prints_ending_with("00000013", "00028067"), "");
    let text = prints_ending_with("00000013", "00408067");
    assert!(text.contains("test::gauge+0x20"), "{text}");
}

/// The prints of a_printed_register_field_is_tied_to_its_conversion's
/// setup ended by the instructions `before` and `jump`.
fn prints_ending_with(before: &str, jump: &str) -> String {
    let mut corpus = corpus(
        "gauge",
        code(&[
            "2010e7b7", "83078793", "0007a583", "0045d593", "00f5f593", "00000537", "00050513",
            before, jump,
        ]),
    );
    let format = |kind| Reference {
        kind,
        target: ".rodata".into(),
        addend: 0,
        literal: Some("gain=%d\n".into()),
    };
    corpus.functions[0].references = BTreeMap::from([
        (0x14, format(oer_elf::rv32::R_RISCV_HI20)),
        (0x18, format(oer_elf::rv32::R_RISCV_LO12_I)),
    ]);
    prints(&corpus, 0x2010_d830, 0x2010_d834)
}
