use super::*;

/// Add tiny DWARF4 scopes to an executable fixture. Its Waker inline range
/// deliberately ends just before the call, as optimized firmware can do.
fn debug_scopes(
    mut elf: Vec<u8>,
    ranges: &[(u32, u32, &str)],
    returns: &[(u32, Option<bool>)],
) -> Vec<u8> {
    fn word(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }
    fn put(bytes: &mut [u8], at: usize, value: usize) {
        bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
    }
    let root_size = functions(&elf).unwrap()[0].size;
    let old_table = word(&elf, 32);
    let count = usize::from(u16::from_le_bytes(elf[48..50].try_into().unwrap()));
    let strings = usize::from(u16::from_le_bytes(elf[50..52].try_into().unwrap()));
    let mut headers = elf[old_table..old_table + count * 40].to_vec();
    let string_header = strings * 40;
    let old_strings = word(&headers, string_header + 16);
    let old_size = word(&headers, string_header + 20);
    let mut names = elf[old_strings..old_strings + old_size].to_vec();
    let mut info = vec![0; 4];
    info.extend_from_slice(&4_u16.to_le_bytes());
    info.extend_from_slice(&0_u32.to_le_bytes());
    info.push(4); // RV32 address size.
    info.push(1); // CU: name, low_pc, high_pc (length).
    info.extend_from_slice(b"producer-scopes\0");
    info.extend_from_slice(&TEXT.to_le_bytes());
    info.extend_from_slice(&0x100_u32.to_le_bytes());
    let mut origins = Vec::new();
    for &(_, _, method) in ranges {
        origins.push(info.len() as u32);
        info.push(2); // Abstract Waker method, with a linkage name.
        info.extend_from_slice(format!("<core::task::wake::Waker>::{method}\0").as_bytes());
    }
    info.push(3); // Concrete root, children follow.
    info.extend_from_slice(b"root\0");
    info.extend_from_slice(&TEXT.to_le_bytes());
    info.extend_from_slice(&root_size.to_le_bytes());
    for (&origin, &(start, end, _)) in origins.iter().zip(ranges) {
        info.push(4); // Inline method: origin, low_pc, high_pc (length).
        info.extend_from_slice(&origin.to_le_bytes());
        info.extend_from_slice(&start.to_le_bytes());
        info.extend_from_slice(&(end - start).to_le_bytes());
    }
    info.push(0); // Root children end.
    for &(address, noreturn) in returns {
        info.push(if noreturn.is_some() { 5 } else { 6 });
        info.extend_from_slice(b"callee\0");
        info.extend_from_slice(&address.to_le_bytes());
        info.extend_from_slice(&12_u32.to_le_bytes());
        if let Some(noreturn) = noreturn {
            info.push(u8::from(noreturn));
        }
    }
    info.push(0); // CU children end.
    let length = (info.len() - 4) as u32;
    info[..4].copy_from_slice(&length.to_le_bytes());
    // DW_AT_name / linkage_name / low_pc / high_pc / abstract_origin;
    // DW_FORM_string / addr / data4 / ref4.
    let abbrev = [
        1, 0x11, 1, 0x03, 0x08, 0x11, 0x01, 0x12, 0x06, 0, 0, 2, 0x2e, 0, 0x6e, 0x08, 0, 0, 3,
        0x2e, 1, 0x03, 0x08, 0x11, 0x01, 0x12, 0x06, 0, 0, 4, 0x1d, 0, 0x31, 0x13, 0x11, 0x01,
        0x12, 0x06, 0, 0,
        // Concrete subprogram with/without DW_AT_noreturn (DW_FORM_flag).
        5, 0x2e, 0, 0x03, 0x08, 0x11, 0x01, 0x12, 0x06, 0x87, 0x01, 0x0c, 0, 0, 6, 0x2e, 0, 0x03,
        0x08, 0x11, 0x01, 0x12, 0x06, 0, 0, 0,
    ];
    for (name, data) in [
        (".debug_abbrev", abbrev.as_slice()),
        (".debug_info", info.as_slice()),
    ] {
        let name_at = names.len();
        names.extend_from_slice(name.as_bytes());
        names.push(0);
        let mut header = vec![0; 40];
        put(&mut header, 0, name_at);
        put(&mut header, 4, 1); // SHT_PROGBITS.
        put(&mut header, 16, elf.len());
        put(&mut header, 20, data.len());
        put(&mut header, 32, 1);
        elf.extend_from_slice(data);
        headers.extend(header);
    }
    put(&mut headers, string_header + 16, elf.len());
    put(&mut headers, string_header + 20, names.len());
    elf.extend(names);
    let table = elf.len();
    elf.extend(headers);
    put(&mut elf, 32, table);
    elf[48..50].copy_from_slice(&((count + 2) as u16).to_le_bytes());
    elf
}

fn fixture(words: Vec<u32>, load: u32, call: u32) -> (Analysis, Dwarf, [u32; 4]) {
    fixture_with(words, &[(load, call, "wake")], &[])
}

fn fixture_with(
    words: Vec<u32>,
    ranges: &[(u32, u32, &str)],
    returns: &[Option<bool>],
) -> (Analysis, Dwarf, [u32; 4]) {
    let root_size = words.len() as u32 * 4;
    let never = TEXT + root_size + 32;
    let elf = debug_scopes(
        executable(&[
            Symbol {
                name: "root",
                words,
                frame: Some(0),
            },
            Symbol {
                name: "clone",
                words: vec![RET],
                frame: Some(0),
            },
            Symbol {
                name: "wake",
                words: vec![SP_DOWN_32, SP_UP_32, RET],
                frame: Some(32),
            },
            Symbol {
                name: "by_ref",
                words: vec![SP_DOWN_16, SP_UP_16, RET],
                frame: Some(16),
            },
            Symbol {
                name: "drop",
                words: vec![RET],
                frame: Some(0),
            },
            Symbol {
                name: "never",
                words: vec![SP_DOWN_32, SP_DOWN_32, 0x0000_006f],
                frame: Some(64),
            },
        ]),
        ranges,
        &returns
            .iter()
            .map(|&proof| (never, proof))
            .collect::<Vec<_>>(),
    );
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dwarf, _) = debug(&elf);
    let targets = [0, 4, 16, 28].map(|offset| TEXT + root_size + offset);
    (analysis, dwarf, targets)
}

const LOAD_WAKE: u32 = 0x0045_2783; // lw a5, 4(a0).

#[test]
fn a_waker_scope_ending_before_jalr_is_proved_by_its_exclusive_load() {
    let (analysis, dwarf, targets) = fixture(vec![LOAD_WAKE, CALL_A5, RET], TEXT, TEXT + 4);
    assert_eq!(
        dwarf
            .inline_chain(TEXT)
            .unwrap()
            .first()
            .map(String::as_str),
        Some("<core::task::wake::Waker>::wake")
    );
    assert_eq!(dwarf.inline_chain(TEXT + 4).unwrap(), ["root"]);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 4).unwrap().targets,
        BTreeSet::from([targets[1]])
    );
    assert_eq!(
        analysis
            .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || Ok(
                ()
            ))
            .unwrap()
            .bytes,
        Some(32)
    );
}

#[test]
fn a_branch_bypassing_the_load_cannot_borrow_its_waker_scope() {
    // beq a0, zero, +8 bypasses the Waker load and enters the call.
    let (analysis, dwarf, targets) = fixture(
        vec![0x0005_0463, LOAD_WAKE, CALL_A5, RET],
        TEXT + 4,
        TEXT + 8,
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 8).is_none());
    assert!(
        analysis
            .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || Ok(
                ()
            ))
            .unwrap()
            .bytes
            .is_none()
    );
}

#[test]
fn a_different_vtable_slot_is_not_the_waker_method_the_scope_names() {
    let (analysis, dwarf, targets) = fixture(vec![0x0085_2783, CALL_A5, RET], TEXT, TEXT + 4);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}

#[test]
fn an_adjusted_function_pointer_does_not_inherit_the_producers_scope() {
    let (analysis, dwarf, targets) = fixture(vec![LOAD_WAKE, 0x0027_80e7, RET], TEXT, TEXT + 4);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}

const LOAD_BY_REF: u32 = 0x0085_2783; // lw a5, 8(a0).
const TAIL_A5: u32 = 0x0007_8067; // jalr zero, 0(a5).
const UNIMP: u32 = 0x0000_0000; // Two compressed unimp instructions.

fn merged_tail() -> (Analysis, Dwarf, [u32; 4]) {
    fixture_with(
        vec![0x0005_0663, LOAD_WAKE, 0x0080_006f, LOAD_BY_REF, TAIL_A5],
        &[
            (TEXT + 4, TEXT + 8, "wake"),
            (TEXT + 12, TEXT + 16, "wake_by_ref"),
        ],
        &[],
    )
}

#[test]
fn a_merged_tail_uses_both_slots_of_every_vtable_without_exposing_ordinary_loads() {
    let (analysis, dwarf, first) = merged_tail();
    let facts = &analysis.functions[&TEXT];
    let site = TEXT + 16;
    assert_eq!(dwarf.inline_chain(site).unwrap(), ["root"]);
    assert!(facts.target_loads.is_empty());
    assert!(facts.load_bases.is_empty());
    assert_eq!(
        facts.open_exit_target_loads[&site]
            .iter()
            .map(|load| load.offset)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([4, 8])
    );
    let second = [first[0], first[3], first[3] + 4, first[0]];
    let resolutions = waker_resolutions(
        &analysis,
        &dwarf,
        &[first, second],
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let resolved = resolutions.get(site).unwrap();
    assert_eq!(resolved.facts, BTreeSet::from([Fact::WakerVtables]));
    assert_eq!(
        resolved.targets,
        BTreeSet::from([first[1], first[2], second[1], second[2]])
    );
    assert_eq!(
        analysis
            .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || Ok(
                ()
            ))
            .unwrap()
            .bytes,
        Some(64)
    );
}

#[test]
fn another_unresolved_jump_blocks_the_merged_tail_loads() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0006_0c63,
            0x0005_0663,
            LOAD_WAKE,
            0x0080_006f,
            LOAD_BY_REF,
            TAIL_A5,
            0x0007_0067,
        ], // The entry branch reaches jr a4.
        &[
            (TEXT + 8, TEXT + 12, "wake"),
            (TEXT + 16, TEXT + 20, "wake_by_ref"),
        ],
        &[],
    );
    let facts = &analysis.functions[&TEXT];
    assert!(facts.target_loads.is_empty());
    assert!(facts.open_exit_target_loads.is_empty());
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}

#[test]
fn an_unresolved_call_still_requires_a_closed_graph() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0006_0863, // beq a2, zero, +16 to the call path.
            LOAD_WAKE,
            TAIL_A5,
            RET,
            LOAD_WAKE,
            CALL_A5,
            RET,
        ],
        &[(TEXT + 4, TEXT + 8, "wake"), (TEXT + 16, TEXT + 20, "wake")],
        &[],
    );
    let facts = &analysis.functions[&TEXT];
    assert!(facts.target_loads.is_empty());
    assert!(facts.open_exit_target_loads.contains_key(&(TEXT + 8)));
    assert!(!facts.open_exit_target_loads.contains_key(&(TEXT + 20)));
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 8).is_some());
    assert!(resolutions.get(TEXT + 20).is_none());
}

#[test]
fn a_compressed_tail_uses_the_same_target_load_proof() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![LOAD_WAKE, 0x0001_8782], // c.jr a5; unvisited c.nop.
        &[(TEXT, TEXT + 4, "wake")],
        &[],
    );
    assert!(analysis.functions[&TEXT].target_loads.is_empty());
    assert!(
        analysis.functions[&TEXT]
            .open_exit_target_loads
            .contains_key(&(TEXT + 4))
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 4).unwrap().targets,
        BTreeSet::from([targets[1]])
    );
}

#[test]
fn any_selected_slot_in_any_vtable_inside_the_function_blocks_the_tail() {
    let (analysis, dwarf, targets) = merged_tail();
    let end = TEXT + analysis.functions[&TEXT].function.size;
    for table in 0..2 {
        for slot in [1, 2] {
            for local in [TEXT, TEXT + 4, end - 2] {
                let mut tables = [targets; 2];
                tables[table][slot] = local;
                let resolutions =
                    waker_resolutions(&analysis, &dwarf, &tables, &mut ledger(), &mut || Ok(()))
                        .unwrap();
                assert!(resolutions.get(TEXT + 16).is_none(), "{tables:?}");
            }
        }
    }
    let mut boundary = targets;
    boundary[1] = end;
    let resolutions =
        waker_resolutions(
            &analysis,
            &dwarf,
            &[boundary],
            &mut ledger(),
            &mut || Ok(()),
        )
        .unwrap();
    assert!(resolutions.get(TEXT + 16).unwrap().targets.contains(&end));
}

#[test]
fn an_unknown_incoming_producer_keeps_a_tail_unresolved() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0006_0a63,
            0x0005_0663,
            LOAD_WAKE,
            0x0080_006f,
            LOAD_BY_REF,
            TAIL_A5,
        ],
        &[
            (TEXT + 8, TEXT + 12, "wake"),
            (TEXT + 16, TEXT + 20, "wake_by_ref"),
        ],
        &[],
    );
    assert!(analysis.functions[&TEXT].open_exit_target_loads.is_empty());
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.is_empty());
}

#[test]
fn an_object_call_requires_the_same_relocated_upper_on_every_path() {
    use oer_analysis_riscv_analysis::{FunctionInput, PreparedReferences};
    use oer_analysis_riscv_lift::RiscvDecoder;
    use oer_analysis_riscv_model::*;
    struct Behavior;
    impl CallBehavior for Behavior {
        fn noreturn(&self, callee: Callee<'_>) -> bool {
            callee == Callee::Symbol(b"never")
        }
    }
    #[derive(Default)]
    struct Saved(Vec<FunctionRecord>);
    impl FunctionSink for Saved {
        fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
            self.0.push(record.clone());
            Ok(())
        }
    }
    const NOP: u32 = 0x0000_0013;
    const AUIPC_RA: u32 = 0x0000_0097;
    const CALL_RA: u32 = 0x0000_80e7;
    for (words, uppers, call, may_return) in [
        // Both paths execute the same AUIPC before entering JALR.
        (
            vec![0x0005_0463, NOP, AUIPC_RA, CALL_RA, UNIMP],
            vec![8],
            12,
            false,
        ),
        // The review reproducer: one path enters JALR with unknown entry RA.
        (
            vec![0x0005_0663, NOP, AUIPC_RA, CALL_RA, UNIMP],
            vec![8],
            12,
            true,
        ),
        // Both paths carry uppers for the same symbol and addend, but from
        // different physical relocations: the adjacent one is not proven.
        (
            vec![AUIPC_RA, 0x0005_0663, NOP, AUIPC_RA, CALL_RA, UNIMP],
            vec![0, 12],
            16,
            true,
        ),
    ] {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let target = std::sync::Arc::new(ReferenceTarget {
            binding: 1,
            definition: SymbolDefinition::Undefined,
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"object-call-upper"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            },
            name: b"never".to_vec(),
            section: None,
            offset: 0,
            symbol_type: 2,
        });
        let relocations: Vec<_> = uppers
            .into_iter()
            .enumerate()
            .map(|(index, offset)| FunctionRelocation {
                section: 2,
                index: index as u64,
                offset,
                relocation_type: 18,
                addend: Some(0),
                target: target.clone(),
            })
            .collect();
        let refs = PreparedReferences::new(&relocations, 1, &RiscvDecoder, &memory, &mut || Ok(()))
            .unwrap();
        let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let mut saved = Saved::default();
        let summary = oer_analysis_riscv_analysis::research(
            FunctionInput {
                traps: oer_analysis_riscv_analysis::TrapSemantics::Unknown,
                image: None,
                section: 1,
                extent: CodeRange {
                    start: 0,
                    length: bytes.len() as u64,
                },
                bytes: &bytes,
                relocations: &refs,
                data_ranges: &[],
                jumps: &[],
                returns: &NoSummaries,
                call_behavior: &Behavior,
            },
            &RiscvDecoder,
            &memory,
            &mut || Ok(()),
            &mut saved,
            Some(CallAbi::RiscvInteger),
        )
        .unwrap();
        let continuation = saved.0.iter().any(|record| matches!(record,
            FunctionRecord::Edge { from, relation: EdgeKind::PossibleContinuation, .. } if *from == call));
        assert_eq!(continuation, may_return);
        assert_eq!(summary.coverage.decoding, !may_return);
        assert!(saved.0.iter().any(|record| matches!(record,
            FunctionRecord::Edge { from, relation: EdgeKind::Call, .. } if *from == call)));
    }
}

#[test]
fn a_noreturn_panic_branch_preserves_the_waker_proof_and_the_callees_stack() {
    // The panic target has a 64-byte frame, deeper than either Waker slot.
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0005_0863, // beq a0, zero, +16 to the panic branch.
            LOAD_WAKE,
            CALL_A5,
            RET,
            call(TEXT + 16, TEXT + 24 + 32),
            UNIMP,
        ],
        &[(TEXT + 4, TEXT + 8, "wake")],
        &[Some(true)],
    );
    let root = &analysis.functions[&TEXT];
    assert!(root.target_loads.contains_key(&(TEXT + 8)));
    assert!(
        root.transfers
            .iter()
            .any(|transfer| { transfer.site == TEXT + 16 && transfer.target == Some(TEXT + 56) })
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 8).unwrap().targets,
        BTreeSet::from([targets[1]])
    );
    let bound = analysis
        .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || {
            Ok(())
        })
        .unwrap();
    assert_eq!(bound.bytes, Some(64));
    assert!(bound.path.contains(&(TEXT + 56, 64)), "{bound:?}");
}

#[test]
fn missing_false_or_ambiguous_noreturn_facts_keep_the_continuation() {
    for proof in [
        vec![],
        vec![None],
        vec![Some(false)],
        vec![Some(true), None],
        vec![Some(true), Some(false)],
    ] {
        let (analysis, dwarf, targets) = fixture_with(
            vec![
                0x0005_0863,
                LOAD_WAKE,
                CALL_A5,
                RET,
                call(TEXT + 16, TEXT + 56),
                UNIMP,
            ],
            &[(TEXT + 4, TEXT + 8, "wake")],
            &proof,
        );
        assert!(
            analysis.functions[&TEXT].target_loads.is_empty(),
            "{proof:?}"
        );
        let resolutions =
            waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(()))
                .unwrap();
        assert!(resolutions.get(TEXT + 8).is_none(), "{proof:?}");
    }
}

#[test]
fn unimp_reachable_around_a_noreturn_call_still_blocks_the_waker_proof() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0005_0a63, // beq a0, zero, +20 directly to unimp.
            0x0005_8663, // beq a1, zero, +12 to the panic call.
            LOAD_WAKE,
            CALL_A5,
            call(TEXT + 16, TEXT + 60),
            UNIMP,
            RET,
        ],
        &[(TEXT + 8, TEXT + 12, "wake")],
        &[Some(true)],
    );
    assert!(analysis.functions[&TEXT].target_loads.is_empty());
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 12).is_none());
}

#[test]
fn merging_wake_and_wake_by_ref_preserves_both_targets() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0005_0663, // beq a0, zero, +12.
            LOAD_WAKE,
            0x0080_006f, // j +8 to the shared call.
            LOAD_BY_REF,
            CALL_A5,
            RET,
        ],
        &[
            (TEXT + 4, TEXT + 8, "wake"),
            (TEXT + 12, TEXT + 16, "wake_by_ref"),
        ],
        &[],
    );
    assert_eq!(
        analysis.functions[&TEXT].target_loads[&(TEXT + 16)].len(),
        2
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 16).unwrap().targets,
        BTreeSet::from([targets[1], targets[2]])
    );
}

#[test]
fn merged_waker_loads_outside_the_call_scope_use_both_slots_of_every_vtable() {
    let (analysis, dwarf, first) = fixture_with(
        vec![
            0x0005_0663, // beq a0, zero, +12.
            LOAD_WAKE,
            0x0080_006f, // j +8 to the shared call.
            LOAD_BY_REF,
            CALL_A5,
            RET,
        ],
        &[
            (TEXT + 4, TEXT + 8, "wake"),
            (TEXT + 12, TEXT + 16, "wake_by_ref"),
        ],
        &[],
    );
    let site = TEXT + 16;
    assert_eq!(dwarf.inline_chain(site).unwrap(), ["root"]);
    // Reaching loads are admitted only when the local observation is closed.
    let loads = &analysis.functions[&TEXT].target_loads[&site];
    assert_eq!(
        loads
            .iter()
            .map(|load| load.offset)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([4, 8])
    );
    // Four distinct real functions occupy slots 1/2 across the two tables;
    // clone (slot 0) is deliberately shared and must never be selected.
    let second = [first[0], first[3], first[3] + 4, first[0]];
    let resolutions = waker_resolutions(
        &analysis,
        &dwarf,
        &[first, second],
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let resolution = resolutions.get(site).unwrap();
    assert_eq!(resolution.facts, BTreeSet::from([Fact::WakerVtables]));
    assert_eq!(
        resolution.targets,
        BTreeSet::from([first[1], first[2], second[1], second[2]])
    );
}

#[test]
fn an_unknown_producer_at_a_waker_merge_keeps_the_call_unresolved() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0006_0c63, // beq a2, zero, +24 bypasses both loads.
            0x0005_0663, // beq a0, zero, +12.
            LOAD_WAKE,
            0x00c0_006f, // j +12 to the shared call.
            LOAD_BY_REF,
            0x0000_0013, // nop.
            CALL_A5,
            RET,
        ],
        &[
            (TEXT + 8, TEXT + 12, "wake"),
            (TEXT + 16, TEXT + 20, "wake_by_ref"),
        ],
        &[],
    );
    assert!(analysis.functions[&TEXT].target_loads.is_empty());
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 24).is_none());
}

#[test]
fn a_proven_auipc_jalr_target_uses_its_noreturn_fact() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0005_0863,
            LOAD_WAKE,
            CALL_A5,
            RET,
            0x0000_0097, // auipc ra, 0.
            0x02c0_80e7, // jalr ra, 44(ra): the non-returning callee.
            UNIMP,
        ],
        &[(TEXT + 4, TEXT + 8, "wake")],
        &[Some(true)],
    );
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert_eq!(
        resolutions.get(TEXT + 8).unwrap().targets,
        BTreeSet::from([targets[1]])
    );
    assert_eq!(
        analysis
            .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || Ok(
                ()
            ))
            .unwrap()
            .bytes,
        Some(64)
    );
}

#[test]
fn a_path_bypassing_auipc_does_not_prove_the_jalr_target_noreturn() {
    let (analysis, dwarf, targets) = fixture_with(
        vec![
            0x0005_8a63, // beq a1, zero, +20 bypasses auipc.
            0x0005_0663, // beq a0, zero, +12 to auipc.
            LOAD_WAKE,
            CALL_A5,
            0x0000_0097, // auipc ra, 0.
            0x0300_80e7, // jalr ra, 48(ra).
            UNIMP,
            RET,
        ],
        &[(TEXT + 8, TEXT + 12, "wake")],
        &[Some(true)],
    );
    assert!(analysis.functions[&TEXT].target_loads.is_empty());
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[targets], &mut ledger(), &mut || Ok(())).unwrap();
    assert!(resolutions.get(TEXT + 12).is_none());
}

#[test]
fn a_jump_table_edge_to_unimp_survives_noreturn_pruning() {
    // A coroutine-state table can name the trap after a panic call too.
    // This is independent evidence of reachability, just as a direct branch.
    let elf = debug_scopes(
        labelled(
            &[
                Symbol {
                    name: "root",
                    words: vec![
                        0x0006_0c63, // beq a2, zero, +24 to table dispatch.
                        LOAD_WAKE,
                        CALL_A5,
                        RET,
                        call(TEXT + 16, TEXT + 44),
                        UNIMP,
                        0x0000_2537, // lui a0, 2: RODATA.
                        0x0025_9593, // slli a1, a1, 2.
                        0x00b5_0533, // add a0, a0, a1.
                        0x0005_2503, // lw a0, 0(a0).
                        0x0005_0067, // jr a0.
                    ],
                    frame: Some(0),
                },
                Symbol {
                    name: "never",
                    words: vec![0x0000_006f],
                    frame: Some(0),
                },
            ],
            &[TEXT + 20, TEXT + 4],
        ),
        &[(TEXT + 4, TEXT + 8, "wake")],
        &[(TEXT + 44, Some(true))],
    );
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].target_loads.is_empty());
    let (dwarf, _) = debug(&elf);
    let resolutions =
        waker_resolutions(&analysis, &dwarf, &[[TEXT; 4]], &mut ledger(), &mut || {
            Ok(())
        })
        .unwrap();
    assert!(resolutions.get(TEXT + 8).is_none());
}

#[test]
fn every_possible_callee_must_be_proven_noreturn() {
    for second in [None, Some(false), Some(true)] {
        let elf = debug_scopes(
            executable(&[
                Symbol {
                    name: "root",
                    words: vec![
                        0x0000_17b7, // lui a5, 1.
                        0x0005_0663, // beq a0, zero, +12.
                        0x0207_8793, // addi a5, a5, 32: first callee.
                        0x0080_006f, // j +8 to call.
                        0x0247_8793, // addi a5, a5, 36: second callee.
                        CALL_A5,
                        CALL_A5, // Observed only if the first call may return.
                        UNIMP,
                    ],
                    frame: Some(0),
                },
                Symbol {
                    name: "first",
                    words: vec![0x0000_006f],
                    frame: Some(0),
                },
                Symbol {
                    name: "second",
                    words: vec![0x0000_006f],
                    frame: Some(0),
                },
            ]),
            &[],
            &[(TEXT + 32, Some(true)), (TEXT + 36, second)],
        );
        let analysis = analyze(&elf, &[], &[]).unwrap();
        let root = &analysis.functions[&TEXT];
        assert_eq!(
            root.site_depths.contains_key(&(TEXT + 24)),
            second != Some(true),
            "{second:?}"
        );
        let targets: BTreeSet<_> = root
            .transfers
            .iter()
            .filter(|transfer| transfer.site == TEXT + 20)
            .filter_map(|transfer| transfer.target)
            .collect();
        assert_eq!(targets, BTreeSet::from([TEXT + 32, TEXT + 36]));
    }
}

#[test]
fn terminal_table_trap_allows_merged_waker_slots_one_and_two() {
    let elf = debug_scopes(
        labelled(
            &[
                Symbol {
                    name: "root",
                    words: vec![
                        0x0006_0e63, // beq a2,zero,+28 to state dispatch.
                        0x0005_8663, // beq a1,zero,+12 to wake_by_ref.
                        LOAD_WAKE,
                        0x0080_006f, // j +8 to merged call.
                        LOAD_BY_REF,
                        CALL_A5,
                        RET,
                        0x0000_2537, // lui a0,2: jump table.
                        0x0025_9593, // slli a1,a1,2.
                        0x00b5_0533, // add a0,a0,a1.
                        0x0005_2503, // lw a0,0(a0).
                        0x0005_0067, // jr a0.
                        UNIMP,
                    ],
                    frame: Some(0),
                },
                Symbol {
                    name: "clone",
                    words: vec![RET],
                    frame: Some(0),
                },
                Symbol {
                    name: "wake1",
                    words: vec![RET],
                    frame: Some(0),
                },
                Symbol {
                    name: "ref1",
                    words: vec![RET],
                    frame: Some(0),
                },
                Symbol {
                    name: "wake2",
                    words: vec![RET],
                    frame: Some(0),
                },
                Symbol {
                    name: "ref2",
                    words: vec![RET],
                    frame: Some(0),
                },
            ],
            &[TEXT + 4, TEXT + 48],
        ),
        &[
            (TEXT + 8, TEXT + 12, "wake"),
            (TEXT + 16, TEXT + 20, "wake_by_ref"),
        ],
        &[],
    );
    let (dwarf, _) = debug(&elf);
    let tables = [
        [TEXT + 52, TEXT + 56, TEXT + 60, TEXT + 52],
        [TEXT + 52, TEXT + 64, TEXT + 68, TEXT + 52],
    ];
    with_facts(&elf, |image| {
        for traps in [TrapSemantics::Unknown, TrapSemantics::Terminal] {
            let analysis = crate::analyze_with_traps(
                image,
                &[],
                &[],
                &dwarf,
                traps,
                &mut ledger(),
                &mut || Ok(()),
            )?;
            let site = TEXT + 20;
            let resolutions =
                waker_resolutions(&analysis, &dwarf, &tables, &mut ledger(), &mut || Ok(()))?;
            if traps == TrapSemantics::Terminal {
                assert_eq!(
                    analysis.functions[&TEXT].target_loads[&site]
                        .iter()
                        .map(|l| l.offset)
                        .collect::<BTreeSet<_>>(),
                    BTreeSet::from([4, 8])
                );
                assert_eq!(
                    resolutions.get(site).unwrap().targets,
                    BTreeSet::from([TEXT + 56, TEXT + 60, TEXT + 64, TEXT + 68])
                );
            } else {
                assert!(analysis.functions[&TEXT].target_loads.is_empty());
                assert!(resolutions.get(site).is_none());
            }
        }
        Ok(())
    })
    .unwrap();
}
