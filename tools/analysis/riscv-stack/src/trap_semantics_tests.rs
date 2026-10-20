use super::*;
use oer_analysis_riscv_analysis::{
    FunctionInput, PreparedReferences, research, writes::closes_local_graph,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;

#[test]
fn trap_encodings_end_only_paths_with_terminal_image_semantics() {
    struct Saved(Vec<FunctionRecord>);
    impl FunctionSink for Saved {
        fn record(&mut self, r: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
            self.0.push(r.clone());
            Ok(())
        }
    }
    for trap in [vec![0, 0], 0xc000_1073u32.to_le_bytes().to_vec()] {
        let width = trap.len() as u64;
        let mut bytes = trap;
        bytes.extend_from_slice(&RET.to_le_bytes());
        assert_eq!(
            RiscvDecoder.decode(&bytes).unwrap().flow,
            InstructionFlow::Trap
        );
        for traps in [TrapSemantics::Unknown, TrapSemantics::Terminal] {
            let memory = WorkingMemory::new(TEST_MEMORY).unwrap();
            let mut saved = Saved(Vec::new());
            let summary = research(
                FunctionInput {
                    traps,
                    image: None,
                    section: 1,
                    extent: CodeRange {
                        start: 0,
                        length: bytes.len() as u64,
                    },
                    bytes: &bytes,
                    relocations: &PreparedReferences::empty(),
                    data_ranges: &[],
                    jumps: &[],
                    returns: &NoReturns,
                    call_behavior: &UnknownCallBehavior,
                },
                &RiscvDecoder,
                &memory,
                &mut || Ok(()),
                &mut saved,
                Some(CallAbi::RiscvInteger),
            )
            .unwrap();
            let terminal = traps == TrapSemantics::Terminal;
            assert_eq!(saved.0.iter().all(closes_local_graph), terminal);
            assert_eq!(
                summary.coverage.decoding && summary.coverage.control_flow,
                terminal
            );
            assert_eq!(
                saved.0.iter().any(|r| matches!(
                    r,
                    FunctionRecord::Gap {
                        start: 0,
                        reason: GapReason::UnsupportedInstruction,
                        ..
                    }
                )),
                !terminal
            );
            assert_eq!(
                saved.0.iter().any(|r| matches!(
                    r,
                    FunctionRecord::Edge {
                        from: 0,
                        target: None,
                        relation: EdgeKind::Trap,
                        ..
                    }
                )),
                terminal
            );
            assert!(!saved.0.iter().any(
                |r| matches!(r, FunctionRecord::Instruction {offset, ..} if *offset >= width)
            ));
        }
    }
    for word in [0x0010_0073u32, 0x0000_0073] {
        assert_eq!(
            RiscvDecoder.decode(&word.to_le_bytes()).unwrap().flow,
            InstructionFlow::Stop
        );
    }
    assert_eq!(
        RiscvDecoder.decode(&[1, 0]).unwrap().flow,
        InstructionFlow::Next
    );
}

fn fixture(handlers: &[&[u32]], valid: bool) -> (Vec<u8>, Vec<(u32, Option<u32>)>) {
    let entry_words = PLATFORM_ENTRY.len() + 1;
    let mut at = TEXT + handlers.len() as u32 * entry_words as u32 * 4;
    let mut symbols = Vec::new();
    let mut entries = Vec::new();
    for (hart, handler) in handlers.iter().enumerate() {
        let address = TEXT + hart as u32 * entry_words as u32 * 4;
        let mut words = PLATFORM_ENTRY.to_vec();
        if !valid {
            words[0] = 0x0011_2023;
        }
        words.push(call(address + PLATFORM_ENTRY.len() as u32 * 4, at));
        symbols.push(Symbol {
            name: if hart == 0 { "entry0" } else { "entry1" },
            words,
            frame: None,
        });
        entries.push((hart as u32, Some(address)));
        at += handler.len() as u32 * 4;
    }
    for (hart, handler) in handlers.iter().enumerate() {
        symbols.push(Symbol {
            name: if hart == 0 { "handler0" } else { "handler1" },
            words: handler.to_vec(),
            frame: Some(0),
        });
    }
    if symbols.is_empty() {
        symbols.push(Symbol {
            name: "unused",
            words: vec![RET],
            frame: Some(0),
        });
    }
    (executable(&symbols), entries)
}

fn proof(handlers: &[&[u32]], valid: bool) -> TrapFact {
    let (elf, entries) = fixture(handlers, valid);
    with_facts(&elf, |image| {
        let unknown = analyze(&elf, &[], &[])?;
        crate::prove_traps(
            image,
            &unknown,
            &entries,
            &UnknownCallBehavior,
            &mut ledger(),
            &mut || Ok(()),
        )
    })
    .unwrap()
}

#[test]
fn every_hart_requires_a_checked_entry_and_a_nonreturning_handler_without_level_drops() {
    let forever = &[0x0000_006f][..];
    let returns = &[RET][..];
    assert_eq!(proof(&[forever], true).semantics, TrapSemantics::Terminal);
    assert!(matches!(
        proof(&[returns], true).failure,
        Some(TrapFailure::Returns { .. })
    ));
    assert!(matches!(
        proof(&[&[0x3020_0073]], true).failure,
        Some(TrapFailure::LevelDrop { .. })
    ));
    assert!(matches!(
        proof(&[forever], false).failure,
        Some(TrapFailure::Entry { .. })
    ));
    assert_eq!(
        proof(&[forever, forever], true).semantics,
        TrapSemantics::Terminal
    );
    assert!(matches!(
        proof(&[forever, returns], true).failure,
        Some(TrapFailure::Returns { hart: 1, .. })
    ));
    assert!(matches!(
        proof(&[returns, forever], true).failure,
        Some(TrapFailure::Returns { hart: 0, .. })
    ));
    // The proof cannot use terminal unimp in the handler to justify itself.
    assert!(matches!(
        proof(&[&[0]], true).failure,
        Some(TrapFailure::Returns { .. })
    ));
    for word in [0x1020_0073, 0x7b20_0073, 0x3450_1073, 0xfb10_1073] {
        assert!(
            matches!(
                proof(&[&[word, 0x0000_006f]], true).failure,
                Some(TrapFailure::LevelDrop { .. })
            ),
            "{word:#x}"
        );
    }
}

#[test]
fn missing_entries_and_a_reachable_return_beside_a_loop_fail_closed() {
    assert!(matches!(
        proof(&[], true).failure,
        Some(TrapFailure::Entry { .. })
    ));
    // beq a0,zero,+8; j .; ret.
    assert!(matches!(
        proof(&[&[0x0005_0463, 0x0000_006f, RET]], true).failure,
        Some(TrapFailure::Returns { .. })
    ));
}

#[test]
fn value_proven_external_tails_are_rechecked_by_the_common_graph() {
    let tail = &[0x0000_0317, 0x0083_0067][..]; // auipc t1,0; jr 8(t1).
    assert_eq!(
        proof(&[tail, &[0x0000_006f]], true).semantics,
        TrapSemantics::Terminal
    );
    assert!(matches!(
        proof(&[tail, &[RET]], true).failure,
        Some(TrapFailure::Returns { hart: 0, .. })
    ));
    // A tail into a level-lowering callee is also in the exception bound.
    assert!(matches!(
        proof(&[tail, &[0x3020_0073]], true).failure,
        Some(TrapFailure::LevelDrop { hart: 0, .. })
    ));
}

#[test]
fn the_handlers_own_noreturn_flag_cannot_hide_a_return_or_trap() {
    struct ClaimsNoReturn;
    impl CallBehavior for ClaimsNoReturn {
        fn noreturn(&self, _: Callee<'_>) -> bool {
            true
        }
    }
    for words in [&[RET][..], &[0][..]] {
        let (elf, entries) = fixture(&[words], true);
        with_facts(&elf, |image| {
            let unknown = analyze(&elf, &[], &[])?;
            let fact = crate::prove_traps(
                image,
                &unknown,
                &entries,
                &ClaimsNoReturn,
                &mut ledger(),
                &mut || Ok(()),
            )?;
            assert!(matches!(fact.failure, Some(TrapFailure::Returns { .. })));
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn missing_hart_entries_and_terminal_input_never_establish_the_fact() {
    let (elf, mut entries) = fixture(&[&[0x0000_006f]], true);
    entries.push((1, None));
    with_facts(&elf, |image| {
        let mut unknown = analyze(&elf, &[], &[])?;
        let fact = crate::prove_traps(
            image,
            &unknown,
            &entries,
            &UnknownCallBehavior,
            &mut ledger(),
            &mut || Ok(()),
        )?;
        assert!(matches!(
            fact.failure,
            Some(TrapFailure::Entry { hart: Some(1), .. })
        ));
        unknown.traps = TrapSemantics::Terminal;
        assert_eq!(
            crate::prove_traps(
                image,
                &unknown,
                &entries,
                &UnknownCallBehavior,
                &mut ledger(),
                &mut || Ok(())
            )
            .unwrap_err()
            .code,
            ErrorCode::Integrity
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn trap_proof_preserves_cancellation_and_memory_errors() {
    let (elf, entries) = fixture(&[&[0x0000_006f]], true);
    with_facts(&elf, |image| {
        let unknown = analyze(&elf, &[], &[])?;
        let mut cancelled = || Err(Error::new(ErrorCode::Cancelled, "test cancellation"));
        assert_eq!(
            crate::prove_traps(
                image,
                &unknown,
                &entries,
                &UnknownCallBehavior,
                &mut ledger(),
                &mut cancelled
            )
            .unwrap_err()
            .code,
            ErrorCode::Cancelled
        );
        let tiny = WorkingMemory::new(1)?;
        let mut keep = kept(&tiny);
        assert_eq!(
            crate::prove_traps(
                image,
                &unknown,
                &entries,
                &UnknownCallBehavior,
                &mut keep,
                &mut || Ok(())
            )
            .unwrap_err()
            .code,
            ErrorCode::ResourceLimited
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_conservative_sweep_does_not_carry_table_facts_across_a_trap() {
    let elf = labelled(
        &[
            Symbol {
                name: "root",
                words: vec![0x0000_2537, 0, 0x0005_2783, 0x0007_80e7, RET],
                frame: Some(0),
            },
            Symbol {
                name: "target",
                words: vec![RET],
                frame: Some(0),
            },
        ],
        &[TEXT + 20],
    );
    let swept = swept(&elf).unwrap();
    assert!(
        swept
            .transfers
            .iter()
            .any(|t| t.site == TEXT + 12 && t.target.is_none())
    );
}
