use super::*;

fn immediate(opcode: u32, funct3: u32, dest: u32, base: u32, value: i32) -> u32 {
    ((value as u32 & 0xfff) << 20) | (base << 15) | (funct3 << 12) | (dest << 7) | opcode
}

fn store(funct3: u32, source: u32, base: u32, offset: u32) -> u32 {
    ((offset >> 5) << 25)
        | (source << 20)
        | (base << 15)
        | (funct3 << 12)
        | ((offset & 31) << 7)
        | 0x23
}

fn branch_nonzero(register: u32, offset: u32) -> u32 {
    ((offset >> 12 & 1) << 31)
        | ((offset >> 5 & 63) << 25)
        | (register << 15)
        | (1 << 12)
        | ((offset >> 1 & 15) << 8)
        | ((offset >> 11 & 1) << 7)
        | 0x63
}

/// A byte in a stack argument changes the recursive arm to the terminal
/// arm. The extra call is included even when only the terminal arm uses it.
fn byte_phase(tag: i32, child: bool) -> Symbol {
    let mut words = vec![
        SP_DOWN_32,
        store(2, 1, 2, 28),
        immediate(3, 4, 12, 11, 7), // lbu a2, 7(a1)
        branch_nonzero(12, 20),
        immediate(0x13, 0, 12, 0, tag),
        store(0, 12, 2, 9),
        immediate(0x13, 0, 11, 2, 2),
        call(TEXT + 28, TEXT),
    ];
    if child {
        words.push(call(TEXT + 32, TEXT + 48));
    }
    words.extend([immediate(3, 2, 1, 2, 28), SP_UP_32, RET]);
    Symbol {
        name: "arbitrary_phase",
        words,
        frame: Some(32),
    }
}

fn tagged_dispatch(tag: i32, width: u32) -> Vec<u8> {
    let symbol = Symbol {
        name: "arbitrary_dispatch",
        words: vec![
            SP_DOWN_32,
            store(2, 1, 2, 28),
            immediate(3, 2, 12, 11, 0),
            immediate(0x13, 1, 12, 12, 2), // slli a2, a2, 2
            0x0000_26b7,                   // lui a3, 2
            0x00d6_0633,                   // add a2, a2, a3
            immediate(3, 2, 12, 12, 0),
            immediate(0x67, 0, 0, 12, 0),
            immediate(0x13, 0, 12, 0, tag),
            store(width, 12, 2, 0),
            immediate(0x13, 0, 11, 2, 0),
            call(TEXT + 44, TEXT),
            immediate(3, 2, 1, 2, 28),
            SP_UP_32,
            RET,
        ],
        frame: Some(32),
    };
    labelled(&[symbol], &[TEXT + 32, TEXT + 32, TEXT + 48])
}

#[test]
fn a_recursive_stack_byte_selects_a_terminal_branch() {
    let elf = executable(&[byte_phase(1, false)]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].terminal_recursive_callees);
    let bound = analysis
        .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
        .unwrap();
    assert_eq!(bound.bytes, Some(64));
    assert_eq!(bound.path, [(TEXT, 32), (TEXT, 32)]);
}

#[test]
fn a_recursive_stack_word_selects_a_terminal_jump_table_arm() {
    let analysis = analyze(&tagged_dispatch(2, 2), &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        Some(64)
    );
}

#[test]
fn a_tag_that_still_recurses_does_not_prove_a_bound() {
    for elf in [executable(&[byte_phase(0, false)]), tagged_dispatch(0, 2)] {
        let analysis = analyze(&elf, &[], &[]).unwrap();
        assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
        assert_eq!(
            analysis
                .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
                .unwrap()
                .bytes,
            None
        );
    }
}

#[test]
fn a_partial_payload_write_leaves_the_unwritten_bytes_unknown() {
    let analysis = analyze(&tagged_dispatch(2, 1), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn every_unfolded_frame_keeps_its_nonrecursive_callees() {
    let elf = executable(&[byte_phase(1, true), leaf(Some(96))]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        Some(160)
    );
    // A second root must use the ordinary entry context, not a memoized
    // terminal invocation with only one frame.
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        Some(160)
    );
}

#[test]
fn a_proven_direct_recursion_does_not_hide_another_cycle() {
    let helper_at = TEXT + 48;
    let helper = Symbol {
        name: "helper_cycle",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(helper_at + 8, TEXT),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    };
    let analysis = analyze(&executable(&[byte_phase(1, true), helper]), &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].terminal_recursive_callees);
    let bound = analysis
        .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
        .unwrap();
    assert_eq!(bound.bytes, None);
    assert!(bound.unresolved.contains(&(TEXT, Reason::Recursion)));
}

#[test]
fn an_indirect_self_call_is_not_a_proven_terminal_edge() {
    let mut symbol = byte_phase(1, true);
    symbol.words[8] = CALL_A5;
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].terminal_recursive_callees);
    let mut resolutions = Resolutions::new();
    resolutions.add(TEXT + 32, Fact::FieldType, [TEXT]);
    let bound = analysis
        .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || {
            Ok(())
        })
        .unwrap();
    assert_eq!(bound.bytes, None);
    assert!(bound.unresolved.contains(&(TEXT, Reason::Recursion)));
}

#[test]
fn an_unknown_alias_store_discards_a_recursive_payload() {
    let mut symbol = byte_phase(1, false);
    symbol.words[6] = store(2, 0, 10, 0); // sw zero, 0(a0), may alias payload
    // Set a1=sp+2 just before the recursive call; its source bytes have
    // already been killed by the store through an unknown address.
    symbol.words.insert(7, immediate(0x13, 0, 11, 2, 2));
    symbol.words[8] = call(TEXT + 32, TEXT);
    symbol.words[3] = branch_nonzero(12, 24);
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn a_path_bypassing_the_payload_store_cannot_borrow_another_paths_fact() {
    let mut symbol = byte_phase(1, false);
    symbol.words.insert(5, branch_nonzero(10, 8));
    symbol.words[8] = call(TEXT + 32, TEXT);
    symbol.words[3] = branch_nonzero(12, 24);
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn a_later_byte_store_replaces_the_terminal_tag() {
    let mut symbol = byte_phase(1, false);
    symbol.words.insert(6, store(0, 0, 2, 9));
    symbol.words[8] = call(TEXT + 32, TEXT);
    symbol.words[3] = branch_nonzero(12, 24);
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn a_local_cycle_leaves_the_recursive_specialization_unknown() {
    let mut symbol = byte_phase(1, false);
    symbol.words[5] = 0x0000_006f; // jal zero, 0
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn the_local_instruction_limit_never_turns_into_a_proof() {
    let mut symbol = byte_phase(1, false);
    symbol.words.extend([0x0000_0013; 1024]);
    let analysis = analyze(&executable(&[symbol]), &[], &[]).unwrap();
    assert!(!analysis.functions[&TEXT].terminal_recursive_callees);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn recursive_specialization_obeys_the_work_budget() {
    let elf = executable(&[byte_phase(1, false)]);
    with_facts(&elf, |facts| {
        let mut work = ledger();
        let error = crate::recursion::terminal_callees(
            facts,
            &facts.functions()[0],
            &[Transfer {
                site: TEXT + 28,
                target: Some(TEXT),
                kind: TransferKind::Call,
                source: None,
                table: None,
                load: None,
            }],
            &[],
            &mut work,
            &mut Units(1),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        Ok(())
    })
    .unwrap();
}
