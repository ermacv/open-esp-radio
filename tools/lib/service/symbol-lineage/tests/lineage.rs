//! End-to-end behavior over synthetic archive revisions.

use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_elf::{Code, Reference, Site};
use oer_symbol_lineage::{
    archive::{Revision, read_archive},
    correspond::{Evidence, Policy, correlate},
    lineage::{NameClass, trace},
};
use oer_vendor_provenance::fingerprint::Function;

const R_RISCV_CALL: u32 = 18;
const RET: [u8; 2] = [0x82, 0x80];

/// `auipc ra, 0 ; jalr ra` followed by distinct filler and `ret`.
fn caller_bytes(filler: u8) -> Vec<u8> {
    let mut bytes = vec![0x97, 0x00, 0x00, 0x00, 0xe7, 0x80, 0x00, 0x00];
    bytes.extend([0x01, filler]);
    bytes.extend(RET);
    bytes
}

fn function(name: &str, bytes: &[u8], callees: &[&str]) -> Function {
    let sites = callees
        .iter()
        .enumerate()
        .map(|(index, callee)| Site {
            offset: index * 8,
            r_type: R_RISCV_CALL,
            reference: Reference::Symbol((*callee).to_owned()),
        })
        .collect();
    Function::of(&Code {
        member: "0.o".to_owned(),
        name,
        bytes,
        sites,
    })
}

fn revision(label: &str, functions: Vec<Function>) -> Revision {
    Revision {
        label: label.to_owned(),
        sha256: label.to_owned(),
        functions,
    }
}

fn leaf(seed: u8, length: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..length)
        .map(|index| seed.wrapping_add((index as u8).wrapping_mul(7)))
        .collect();
    bytes.extend(RET);
    bytes
}

fn tokens(name: &str) -> String {
    format!("r_sym_bt_{name}")
}

#[test]
fn source_names_cross_the_obfuscation_revision() {
    let callee_old = leaf(0x10, 40);
    let mut callee_new = callee_old.clone();
    callee_new[5] ^= 0xff;
    let named = revision(
        "named",
        vec![
            function("r_caller", &caller_bytes(1), &["r_changed_callee"]),
            function("r_changed_callee", &callee_old, &[]),
            function("r_stable_name", &leaf(0x40, 12), &[]),
        ],
    );
    let obfuscated = revision(
        "obfuscated",
        vec![
            function(
                &tokens("Caller0Token0Aa1Bb2Cc"),
                &caller_bytes(1),
                &[&tokens("Callee0Token0Dd3Ee4Ff")],
            ),
            function(&tokens("Callee0Token0Dd3Ee4Ff"), &callee_new, &[]),
            function("r_stable_name", &leaf(0x40, 12), &[]),
        ],
    );
    let pairs = correlate(&named, &obfuscated, Policy::default());
    let evidence = |right: usize| {
        pairs
            .iter()
            .find(|pair| pair.right == right)
            .map(|pair| pair.evidence)
    };
    assert_eq!(evidence(0), Some(Evidence::ExactBody));
    assert_eq!(evidence(1), Some(Evidence::CallGraph));
    assert_eq!(evidence(2), Some(Evidence::SameName));

    let lineage = trace(
        &[named, obfuscated],
        Policy::default(),
        &NameClass::Prefixes(vec!["r_sym_".into()]),
    );
    let names: Vec<_> = lineage
        .functions
        .iter()
        .map(|f| f.source_name.as_deref())
        .collect();
    assert_eq!(
        names,
        [
            Some("r_caller"),
            Some("r_changed_callee"),
            Some("r_stable_name")
        ]
    );
    assert_eq!(
        lineage.functions[1].steps[0].previous_name,
        "r_changed_callee"
    );
}

#[test]
fn identical_duplicate_bodies_stay_unpaired() {
    let body = leaf(0x20, 16);
    let left = revision(
        "left",
        vec![function("r_a", &body, &[]), function("r_b", &body, &[])],
    );
    let right = revision(
        "right",
        vec![
            function(&tokens("Dup0Token0Aa1Bb2Cc3Dd"), &body, &[]),
            function(&tokens("Dup1Token0Ee4Ff5Gg6Hh"), &body, &[]),
        ],
    );
    assert!(correlate(&left, &right, Policy::default()).is_empty());
}

#[test]
fn call_graph_votes_need_a_similar_body() {
    let left = revision(
        "left",
        vec![
            function("r_caller", &caller_bytes(2), &["r_callee"]),
            function("r_callee", &leaf(0x30, 40), &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            function(
                &tokens("Caller0Token0Aa1Bb2Cc"),
                &caller_bytes(2),
                &[&tokens("Stub0Token0Dd3Ee4Ff5")],
            ),
            function(&tokens("Stub0Token0Dd3Ee4Ff5"), &RET, &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].right, 0);
}

#[test]
fn names_carry_through_every_revision_and_new_functions_stay_unknown() {
    let body = leaf(0x50, 24);
    let token = tokens("Kept0Token0Aa1Bb2Cc3D");
    let revisions = [
        revision("r0", vec![function("r_kept", &body, &[])]),
        revision("r1", vec![function(&token, &body, &[])]),
        revision(
            "r2",
            vec![
                function(&token, &body, &[]),
                function(&tokens("New0Token0Ee4Ff5Gg6Hh"), &leaf(0x70, 30), &[]),
            ],
        ),
    ];
    let lineage = trace(&revisions, Policy::default(), &NameClass::Heuristic);
    assert_eq!(lineage.functions[0].source_name.as_deref(), Some("r_kept"));
    assert_eq!(lineage.functions[0].origin.as_deref(), Some("r0"));
    assert_eq!(lineage.functions[0].steps.len(), 2);
    assert_eq!(lineage.functions[1].source_name, None);
}

#[test]
fn archive_reader_extracts_functions_and_their_calls() {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".text.r_caller".to_vec(), SectionKind::Text);
    let bytes = caller_bytes(3);
    object.append_section_data(section, &bytes, 2);
    object.add_symbol(Symbol {
        name: b"r_caller".to_vec(),
        value: 0,
        size: bytes.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    let callee = object.add_symbol(Symbol {
        name: b"r_callee".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            section,
            Relocation {
                offset: 0,
                symbol: callee,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: R_RISCV_CALL,
                },
            },
        )
        .unwrap();
    let elf = object.write().unwrap();
    let mut archive = b"!<arch>\n".to_vec();
    archive.extend(
        format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "0.o/",
            0,
            0,
            0,
            644,
            elf.len()
        )
        .as_bytes(),
    );
    archive.extend(&elf);
    if elf.len() % 2 == 1 {
        archive.push(b'\n');
    }

    let revision = read_archive("synthetic", &archive).unwrap();
    assert_eq!(revision.functions.len(), 1);
    let function = &revision.functions[0];
    assert_eq!(
        (function.member.as_str(), function.name.as_str()),
        ("0.o", "r_caller")
    );
    assert_eq!(function.calls, ["r_callee"]);
    assert_eq!(function.size, bytes.len());
}

/// A caller body with `calls` call sites, each followed by distinct filler.
fn calls_bytes(calls: usize, filler: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..calls {
        bytes.extend([0x97, 0x00, 0x00, 0x00, 0xe7, 0x80, 0x00, 0x00]);
    }
    bytes.extend([0x01, filler]);
    bytes.extend(RET);
    bytes
}

#[test]
fn call_lists_of_different_length_vote_inside_equal_gaps_between_anchors() {
    // The caller gains one call after the changed callee; the stable callee
    // anchors the alignment and the gap before it pairs the changed callee.
    let changed_old = leaf(0x10, 40);
    let mut changed_new = changed_old.clone();
    changed_new[7] ^= 0xff;
    let left = revision(
        "left",
        vec![
            function("r_caller", &calls_bytes(2, 1), &["r_changed", "r_stable"]),
            function("r_changed", &changed_old, &[]),
            function("r_stable", &leaf(0x60, 20), &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            function(
                "r_caller",
                &calls_bytes(3, 1),
                &[
                    &tokens("Changed0Token0Aa1Bb2Cc"),
                    "r_stable",
                    &tokens("Added0Token0Dd3Ee4Ff5"),
                ],
            ),
            function(&tokens("Changed0Token0Aa1Bb2Cc"), &changed_new, &[]),
            function("r_stable", &leaf(0x60, 20), &[]),
            function(&tokens("Added0Token0Dd3Ee4Ff5"), &leaf(0x90, 6), &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    let changed = pairs.iter().find(|pair| pair.right == 1).unwrap();
    assert_eq!((changed.left, changed.evidence), (1, Evidence::CallGraph));
    assert!(pairs.iter().all(|pair| pair.right != 3));
}

/// `leaf` with one byte changed every `stride` bytes. The change keeps the
/// low two bits, so instruction lengths and boundaries stay and each edit
/// changes one instruction token.
fn edited(seed: u8, length: usize, stride: usize) -> Vec<u8> {
    let mut bytes = leaf(seed, length);
    for index in (0..length).step_by(stride) {
        bytes[index] ^= 0x50;
    }
    bytes
}

#[test]
fn a_dominant_mutual_best_pairs_below_the_similarity_minimum() {
    let old = leaf(0x21, 64);
    let new = edited(0x21, 64, 12);
    let left = revision("left", vec![function("r_rewritten", &old, &[])]);
    let right = revision(
        "right",
        vec![
            function(&tokens("Rewritten0Token0Aa1Bb2"), &new, &[]),
            function(&tokens("Unrelated0Token0Cc3Dd4"), &leaf(0xc7, 64), &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    let [pair] = pairs.as_slice() else {
        panic!("expected one pair, got {pairs:?}");
    };
    assert_eq!(pair.right, 0);
    let Evidence::Dominant { ppm, runner_up_ppm } = pair.evidence else {
        panic!("expected dominance, got {:?}", pair.evidence);
    };
    assert!(ppm < Policy::default().minimum_ppm);
    assert!(u64::from(ppm) >= 2 * u64::from(runner_up_ppm));
}

#[test]
fn a_close_runner_up_blocks_dominance() {
    let old = leaf(0x21, 64);
    let left = revision("left", vec![function("r_rewritten", &old, &[])]);
    let right = revision(
        "right",
        vec![
            function(
                &tokens("Rewritten0Token0Aa1Bb2"),
                &edited(0x21, 64, 12),
                &[],
            ),
            function(
                &tokens("Rewritten1Token0Cc3Dd4"),
                &edited(0x21, 64, 10),
                &[],
            ),
        ],
    );
    assert!(correlate(&left, &right, Policy::default()).is_empty());
}

#[test]
fn the_neighbourhood_pairs_identical_bodies_where_paired_callers_expect_them() {
    // Two identical getters, each called by one caller whose call count
    // changes: only the neighbourhood tells them apart.
    let getter = leaf(0x33, 10);
    let left = revision(
        "left",
        vec![
            function("r_first_caller", &calls_bytes(1, 1), &["r_first_getter"]),
            function("r_second_caller", &calls_bytes(1, 2), &["r_second_getter"]),
            function("r_first_getter", &getter, &[]),
            function("r_second_getter", &getter, &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            function(
                "r_first_caller",
                &calls_bytes(2, 1),
                &[
                    &tokens("First0Getter0Aa1Bb2Cc"),
                    &tokens("First0Getter0Aa1Bb2Cc"),
                ],
            ),
            function(
                "r_second_caller",
                &calls_bytes(2, 2),
                &[
                    &tokens("Second0Getter0Dd3Ee4F"),
                    &tokens("Second0Getter0Dd3Ee4F"),
                ],
            ),
            function(&tokens("First0Getter0Aa1Bb2Cc"), &getter, &[]),
            function(&tokens("Second0Getter0Dd3Ee4F"), &getter, &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    let partner = |right: usize| {
        pairs
            .iter()
            .find(|pair| pair.right == right)
            .map(|pair| (pair.left, pair.evidence))
    };
    assert!(matches!(
        partner(2),
        Some((2, Evidence::Neighbourhood { support: 1, .. }))
    ));
    assert!(matches!(
        partner(3),
        Some((3, Evidence::Neighbourhood { support: 1, .. }))
    ));
}

#[test]
fn the_pairing_chain_crosses_revisions_without_source_names() {
    let body = leaf(0x44, 20);
    let first = tokens("First0Token0Aa1Bb2Cc3");
    let second = tokens("Second0Token0Dd4Ee5Ff");
    let lineage = trace(
        &[
            revision("r0", vec![function(&first, &body, &[])]),
            revision("r1", vec![function(&second, &body, &[])]),
        ],
        Policy::default(),
        &NameClass::Prefixes(vec!["r_sym_".into()]),
    );
    let function = &lineage.functions[0];
    assert_eq!(function.source_name, None);
    assert!(function.steps.is_empty());
    assert_eq!(function.chain.len(), 1);
    assert_eq!(function.chain[0].previous_name, first);
}
