use super::*;

#[test]
fn an_external_long_call_does_not_obscure_the_local_frame() {
    let image = executable(&[
        Symbol {
            name: "wrapper",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                0x0000_0097, // auipc ra, 0
                0x0140_80e7, // jalr ra, 20(ra): leaf after the wrapper
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: None,
        },
        leaf(None),
    ]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    let facts = &analysis.functions[&TEXT];
    assert_eq!(
        (facts.frame, facts.source),
        (Some(16), Some(FrameSource::Observed))
    );
    assert!(facts.complete);
    let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert!(bound.unresolved.is_empty());
}

#[test]
fn local_frame_admission_refuses_fall_off_the_function_end() {
    for last in [
        0x0000_0013,                // nop: fall through into the next function
        BRANCH_NEXT,                // the branch's fall-through also leaves the function
        call(TEXT + 12, TEXT + 16), // the call's continuation is outside
    ] {
        let image = executable(&[
            Symbol {
                name: "falls_off",
                words: vec![SP_DOWN_16, SAVE_RA, 0x0000_0013, last],
                frame: None,
            },
            leaf(None),
        ]);
        let analysis = analyze(&image, &[], &[]).unwrap();
        // A fall-through would reach 16 + 32 bytes. Proving only the first
        // function's 16-byte frame must not produce a complete 16-byte bound.
        assert_eq!(analysis.functions[&(TEXT + 16)].frame, Some(32));
        assert_eq!(analysis.functions[&TEXT].frame, None, "{last:#x}");
        let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
        assert_eq!(bound.bytes, None, "{last:#x}");
        assert!(bound.reasons().contains_key(&Reason::Unframed));
    }
}

#[test]
fn local_frame_admission_refuses_a_conflicting_instruction_boundary() {
    let image = executable(&[Symbol {
        name: "conflict",
        words: vec![
            SP_DOWN_16,
            0x0000_0163, // beq zero, zero, 2: into this instruction's middle
            SP_UP_16,
            RET,
        ],
        frame: None,
    }]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    assert_eq!(analysis.functions[&TEXT].frame, None);
    let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(bound.bytes, None);
    assert!(bound.reasons().contains_key(&Reason::Unframed));
}

#[test]
fn local_frame_admission_refuses_a_stop_edge() {
    let image = executable(&[Symbol {
        name: "stop",
        words: vec![SP_DOWN_16, 0x0010_0073 /* ebreak */],
        frame: None,
    }]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    assert_eq!(analysis.functions[&TEXT].frame, None);
    let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(bound.bytes, None);
    assert!(bound.reasons().contains_key(&Reason::Unframed));
}

#[test]
fn a_local_frame_does_not_resolve_an_unknown_callee() {
    let image = executable(&[Symbol {
        name: "callback",
        words: vec![SP_DOWN_16, SAVE_RA, CALL_A5, LOAD_RA, SP_UP_16, RET],
        frame: None,
    }]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    assert_eq!(analysis.functions[&TEXT].frame, Some(16));
    let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 8, Reason::RegisterCall)]);
}

#[test]
fn local_frame_admission_still_refuses_decode_gaps_and_dynamic_sp() {
    for instruction in [0xffff_ffff, 0x0005_0113 /* mv sp, a0 */] {
        let image = executable(&[Symbol {
            name: "incomplete",
            words: vec![SP_DOWN_16, instruction, CALL_A5, SP_UP_16, RET],
            frame: None,
        }]);
        let analysis = analyze(&image, &[], &[]).unwrap();
        assert_eq!(analysis.functions[&TEXT].frame, None);
        assert_eq!(
            analysis
                .bound(TEXT, &mut ledger(), &mut || Ok(()))
                .unwrap()
                .bytes,
            None
        );
    }
}

/// The indexed address is formed before a call and a merge, then retained in
/// s7. Only its word load and the jump share a straight-line instruction slice.
fn saved_indexed_address(clobber: bool) -> (Vec<Symbol>, [u32; 2]) {
    let mut words = vec![
        SP_DOWN_16,
        SAVE_RA,
        0x0171_2423, // sw s7, 8(sp)
        0x0005_4503, // lbu a0, 0(a0)
        0x0025_1513, // slli a0, a0, 2
        0x0000_2637, // lui a2, %hi(RODATA)
        0x00c5_0bb3, // add s7, a0, a2
        0,           // call leaf
        BRANCH_NEXT,
        if clobber { 0x0005_0b93 } else { 0x0000_0013 }, // mv s7, a0 / nop
        0x000b_a703,                                     // lw a4, 0(s7)
        0x0200_0693,                                     // li a3, 32
        0x0010_0613,                                     // li a2, 1
        0x0007_0067,                                     // jr a4
    ];
    let mut arms = [0; 2];
    for arm in &mut arms {
        *arm = TEXT + 4 * words.len() as u32;
        words.extend([LOAD_RA, 0x0081_2b83 /* lw s7, 8(sp) */, SP_UP_16, RET]);
    }
    words[7] = call(TEXT + 7 * 4, TEXT + 4 * words.len() as u32);
    (
        vec![
            Symbol {
                name: "dispatch",
                words,
                frame: None,
            },
            Symbol {
                name: "leaf",
                words: vec![RET],
                frame: None,
            },
        ],
        arms,
    )
}

#[test]
fn a_saved_indexed_address_uses_the_existing_read_only_target_proof() {
    let (symbols, arms) = saved_indexed_address(false);
    let image = labelled(&symbols, &arms);
    assert!(swept(&image).unwrap().jumps.is_empty());
    let analysis = analyze(&image, &[], &[]).unwrap();
    assert_eq!(analysis.functions[&TEXT].frame, Some(16));
    let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(bound.bytes, Some(16));
    assert!(bound.unresolved.is_empty());
}

#[test]
fn a_saved_indexed_address_does_not_admit_unknown_mutable_or_foreign_tables() {
    let (symbols, arms) = saved_indexed_address(false);
    let mut writable = labelled(&symbols, &arms);
    let shoff = u32::from_le_bytes(writable[32..36].try_into().unwrap()) as usize;
    // This fixture's .rodata section is 6: SHF_ALLOC | SHF_WRITE.
    writable[shoff + 6 * 40 + 8..shoff + 6 * 40 + 12].copy_from_slice(&3u32.to_le_bytes());
    for (case, image) in [
        image(&symbols, &arms, false),
        writable,
        labelled(&symbols, &[arms[0], 0x5000]),
        {
            let (clobbered, arms) = saved_indexed_address(true);
            labelled(&clobbered, &arms)
        },
    ]
    .into_iter()
    .enumerate()
    {
        let analysis = analyze(&image, &[], &[]).unwrap();
        let bound = analysis.bound(TEXT, &mut ledger(), &mut || Ok(())).unwrap();
        assert_eq!(bound.bytes, None, "case {case}");
        assert!(
            bound.reasons().contains_key(&Reason::IndirectJump),
            "case {case}"
        );
    }
}
