//! End-to-end behavior over synthetic archive revisions.
#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test code: a test may panic"
)]

use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_tools_analysis_elf::{Code, Reference, Site};
use oer_tools_vendor_lineage::{
    archive::{Revision, read_archive},
    correspond::{Evidence, Policy, correlate},
    lineage::{NameClass, trace},
};
use oer_tools_vendor_provenance::fingerprint::Function;

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

/// A function given as its instruction tokens: its fingerprint is their
/// text, and its size two bytes a token unless `size` says otherwise.
fn body(name: &str, tokens: &[u64], calls: &[&str]) -> Function {
    Function {
        member: "0.o".to_owned(),
        name: name.to_owned(),
        size: 2 * tokens.len(),
        code: format!("{tokens:?}"),
        named: format!("{tokens:?} {calls:?}"),
        tokens: tokens.to_vec(),
        calls: calls.iter().map(|callee| (*callee).to_owned()).collect(),
    }
}

/// Only names, equal bodies and the call graph pair.
fn call_graph_alone() -> Policy {
    Policy {
        minimum_ppm: 1_000_001,
        dominant_minimum_ppm: 1_000_001,
        neighbourhood_minimum_ppm: 1_000_001,
        call_graph_minimum_ppm: 0,
        ..Policy::default()
    }
}

/// The pairs of `evidence` between `left` and `right`, by name.
fn paired_by(
    left: &Revision,
    right: &Revision,
    policy: Policy,
    wanted: fn(Evidence) -> bool,
) -> Vec<(String, String)> {
    correlate(left, right, policy)
        .iter()
        .filter(|pair| wanted(pair.evidence))
        .map(|pair| {
            (
                left.functions[pair.left].name.clone(),
                right.functions[pair.right].name.clone(),
            )
        })
        .collect()
}

/// One caller named alike on both sides with the given call lists, and one
/// function of its own body for every other name of either list.
fn caller_of(left_calls: &str, right_calls: &str) -> (Revision, Revision) {
    let side = |label: &str, calls: &str, seed: u64| {
        let calls: Vec<&str> = calls.split_whitespace().collect();
        let mut names = calls.clone();
        names.sort_unstable();
        names.dedup();
        let mut functions = vec![body("caller", &[1, 2, 3], &calls)];
        functions.extend(names.iter().zip(0..).map(|(name, index): (_, u64)| {
            let own = name.bytes().map(u64::from).sum::<u64>() * 100;
            body(name, &[own, own + seed, own + index, own + 3], &[])
        }));
        revision(label, functions)
    };
    (side("left", left_calls, 1), side("right", right_calls, 2))
}

#[test]
fn call_lists_of_different_length_correspond_inside_the_equal_gaps_between_their_anchors() {
    fn voted(evidence: Evidence) -> bool {
        evidence == Evidence::CallGraph
    }
    for (left_calls, right_calls, expected) in [
        // Anchors at both ends: the gap between them is one against two.
        ("a x b", "a y n b", &[][..]),
        // The same, the gap one against one; nothing follows `b` on the left.
        ("a x b", "a y b n", &[("x", "y")][..]),
        // An anchor at the end alone, and at the start alone.
        ("x a", "n y a", &[]),
        ("a x", "a y n", &[]),
        ("x a", "y a n", &[("x", "y")]),
        // An anchor in the middle: the gap before it is equal, the one after is not.
        ("x1 a x2", "y1 a y2 n", &[("x1", "y1")]),
        // Both gaps equal, and a third after a second anchor that is not.
        (
            "x1 a x2 x3 b",
            "y1 a y2 y3 b n",
            &[("x1", "y1"), ("x2", "y2"), ("x3", "y3")],
        ),
        // No anchor: the lists are one gap of two against three.
        ("x1 x2", "y1 y2 y3", &[]),
        // Equal lengths correspond position by position, anchors or none.
        ("x1 x2", "y1 y2", &[("x1", "y1"), ("x2", "y2")]),
        ("x1 a", "a y1", &[]),
        // A callee named twice anchors twice, in order.
        ("a x1 a", "a y1 a n a", &[("x1", "y1")]),
        ("a a x1", "a n a y1", &[("x1", "y1")]),
        // Two alignments of one anchor each: the one that steps past the
        // left callee first is taken, and it leaves no equal gap.
        ("a b x", "b a y n", &[]),
        ("b a x", "a b n y", &[]),
        // An empty list against a longer one.
        ("", "y1", &[]),
    ] {
        let (left, right) = caller_of(left_calls, right_calls);
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(left, right)| ((*left).to_owned(), (*right).to_owned()))
            .collect();
        assert_eq!(
            paired_by(&left, &right, call_graph_alone(), voted),
            expected,
            "{left_calls} | {right_calls}"
        );
    }
}

#[test]
fn a_call_graph_pair_is_the_one_candidate_of_every_voter_and_the_one_voter_s_candidate() {
    fn voted(evidence: Evidence) -> bool {
        evidence == Evidence::CallGraph
    }
    let revisions = |left_calls: [&str; 2], right_calls: [&str; 2], right_twice: bool| {
        let mut right = vec![
            body("first", &[1, 2], &[right_calls[0]]),
            body("second", &[3, 4], &[right_calls[1]]),
            body("y", &[20, 21, 22], &[]),
            body("y2", &[30, 31, 32], &[]),
        ];
        if right_twice {
            right.push(body("y", &[40, 41, 42], &[]));
        }
        (
            revision(
                "left",
                vec![
                    body("first", &[1, 2], &[left_calls[0]]),
                    body("second", &[3, 4], &[left_calls[1]]),
                    body("x", &[50, 51, 52], &[]),
                    body("x2", &[60, 61, 62], &[]),
                ],
            ),
            revision("right", right),
        )
    };
    let pairs = |left_calls, right_calls, right_twice| {
        let (left, right) = revisions(left_calls, right_calls, right_twice);
        paired_by(&left, &right, call_graph_alone(), voted)
    };
    let one = [("x".to_owned(), "y".to_owned())];
    // Two voters for one pair; one voter, the other naming what is paired.
    assert_eq!(pairs(["x", "x"], ["y", "y"], false), one);
    assert_eq!(pairs(["x", "first"], ["y", "first"], false), one);
    // Two candidates for `x`; two voters' functions for `y`.
    assert_eq!(pairs(["x", "x"], ["y", "y2"], false), []);
    assert_eq!(pairs(["x", "x2"], ["y", "y"], false), []);
    // Each its own.
    assert_eq!(
        pairs(["x", "x2"], ["y", "y2"], false),
        [
            ("x".to_owned(), "y".to_owned()),
            ("x2".to_owned(), "y2".to_owned())
        ]
    );
    // A name two functions carry is no candidate, and a name no function
    // carries neither.
    assert_eq!(pairs(["x", "x"], ["y", "y"], true), []);
    assert_eq!(pairs(["x", "x"], ["absent", "absent"], false), []);
    assert_eq!(pairs(["absent", "absent"], ["y", "y"], false), []);
}

#[test]
fn a_similar_pair_is_a_mutual_best_with_a_lead_among_bodies_of_comparable_size() {
    let old: Vec<u64> = (0..20).collect();
    let edit = |changed: usize, by: u64| {
        let mut tokens = old.clone();
        for token in tokens.iter_mut().take(changed) {
            *token += by;
        }
        tokens
    };
    let pairs = |right: Vec<Function>| {
        let left = revision("left", vec![body("r_old", &old, &[])]);
        correlate(&left, &revision("right", right), Policy::default())
            .iter()
            .map(|pair| {
                (
                    pair.right,
                    pair.evidence,
                    pair.identical,
                    pair.similarity_ppm,
                )
            })
            .collect::<Vec<_>>()
    };
    // One candidate above the minimum.
    assert_eq!(
        pairs(vec![body("new", &edit(2, 100), &[])]),
        [(0, Evidence::Similar { ppm: 900_000 }, false, 900_000)]
    );
    // A second as similar: a tie.
    assert_eq!(
        pairs(vec![
            body("new", &edit(2, 100), &[]),
            body("other", &edit(2, 200), &[])
        ]),
        []
    );
    // A second less than the margin behind: one more token changes it from
    // nineteen of twenty to thirty-eight of forty-one.
    let mut longer = edit(1, 200);
    longer.push(300);
    assert_eq!(
        pairs(vec![
            body("new", &edit(1, 100), &[]),
            body("other", &longer, &[])
        ]),
        []
    );
    // A second a margin behind.
    assert_eq!(
        pairs(vec![
            body("new", &edit(1, 100), &[]),
            body("other", &edit(2, 200), &[])
        ]),
        [(0, Evidence::Similar { ppm: 950_000 }, false, 950_000)]
    );
    // The candidate's size: twice the other's and sixteen bytes is
    // comparable, a byte more is not.
    let sized = |size: usize| Function {
        size,
        ..body("new", &edit(2, 100), &[])
    };
    assert_eq!(pairs(vec![sized(96)]).len(), 1);
    assert_eq!(pairs(vec![sized(97)]), []);
    assert_eq!(pairs(vec![sized(12)]).len(), 1);
    assert_eq!(pairs(vec![sized(11)]), []);
    // No candidate at all.
    assert_eq!(pairs(vec![]), []);
}

#[test]
fn a_dominant_pair_exceeds_its_runner_up_by_the_policy_s_ratio() {
    let old: Vec<u64> = (0..20).collect();
    let edit = |changed: usize| {
        let mut tokens = old.clone();
        for token in tokens.iter_mut().take(changed) {
            *token += 100;
        }
        tokens
    };
    let pairs = |dominance_ratio: u32| {
        let left = revision("left", vec![body("r_old", &old, &[])]);
        let right = revision(
            "right",
            vec![body("new", &edit(6), &[]), body("other", &edit(12), &[])],
        );
        let policy = Policy {
            dominance_ratio,
            ..Policy::default()
        };
        correlate(&left, &right, policy)
            .iter()
            .map(|pair| (pair.right, pair.evidence))
            .collect::<Vec<_>>()
    };
    let dominant = [(
        0,
        Evidence::Dominant {
            ppm: 700_000,
            runner_up_ppm: 400_000,
        },
    )];
    // Seven against four tenths: once and by nothing, not twice.
    assert_eq!(pairs(2), []);
    assert_eq!(pairs(1), dominant);
    assert_eq!(pairs(0), dominant);
    assert_eq!(pairs(u32::MAX), []);
}

#[test]
fn the_neighbourhood_counts_every_relation_that_expects_a_candidate() {
    // Two getters of one body, each called by one caller and calling one
    // callee of its own; the callers' and the callees' names stay.
    let getter = [7, 8, 9, 10, 11, 12];
    let left = revision(
        "left",
        vec![
            body("caller_a", &[1, 2, 3], &["get_a"]),
            body("caller_b", &[4, 5, 6], &["get_b"]),
            body("get_a", &getter, &["leaf_a"]),
            body("get_b", &getter, &["leaf_b"]),
            body("leaf_a", &[20, 21], &[]),
            body("leaf_b", &[30, 31], &[]),
        ],
    );
    let right = |callers: [&[&str]; 2]| {
        revision(
            "right",
            vec![
                body("caller_a", &[1, 2, 3], callers[0]),
                body("caller_b", &[4, 5, 6], callers[1]),
                body("new_a", &getter, &["leaf_a"]),
                body("new_b", &getter, &["leaf_b"]),
                body("leaf_a", &[20, 21], &[]),
                body("leaf_b", &[30, 31], &[]),
            ],
        )
    };
    let neighbours = |right: &Revision| {
        correlate(&left, right, Policy::default())
            .iter()
            .filter_map(|pair| match pair.evidence {
                Evidence::Neighbourhood { support, ppm } => {
                    Some((pair.left, pair.right, support, ppm))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // The caller's list changed length, so no position votes: the caller
    // and the callee each expect the getter.
    assert_eq!(
        neighbours(&right([&["new_a", "new_a"], &["new_b", "new_b"]])),
        [(2, 2, 2, 1_000_000), (3, 3, 2, 1_000_000)]
    );
    // Both callers call both getters: the callee alone tells them apart.
    assert_eq!(
        neighbours(&right([
            &["new_a", "new_b", "new_a"],
            &["new_b", "new_a", "new_b"]
        ])),
        [(2, 2, 2, 1_000_000), (3, 3, 2, 1_000_000)]
    );
}

#[test]
fn equally_supported_candidates_of_one_body_stay_unpaired() {
    let getter = [7, 8, 9, 10, 11, 12];
    let left = revision(
        "left",
        vec![
            body("caller", &[1, 2, 3], &["get_a", "get_b"]),
            body("get_a", &getter, &[]),
            body("get_b", &getter, &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            body("caller", &[1, 2, 3], &["new_a", "new_b", "new_a"]),
            body("new_a", &getter, &[]),
            body("new_b", &getter, &[]),
        ],
    );
    let pairs = correlate(&left, &right, Policy::default());
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    assert_eq!((pairs[0].left, pairs[0].evidence), (0, Evidence::SameName));
}

#[test]
fn a_step_s_summary_counts_its_pairs_by_evidence_and_what_stays_unpaired() {
    let old: Vec<u64> = (0..20).collect();
    let mut edited = old.clone();
    edited[0] = 100;
    edited[1] = 101;
    let getter = [7, 8, 9, 10, 11, 12];
    let left = revision(
        "left",
        vec![
            body("named", &[1, 2, 3], &["voted"]),
            body("exact", &[4, 5, 6, 7], &[]),
            body("voted", &[40, 41, 42, 43], &[]),
            body("similar", &old, &[]),
            body("get_a", &getter, &["leaf"]),
            body("get_b", &getter, &[]),
            body("leaf", &[20, 21], &[]),
            body("caller", &[30, 31], &["get_a"]),
            body("gone", &[90, 91, 92, 93, 94, 95, 96, 97], &[]),
        ],
    );
    let right = revision(
        "right",
        vec![
            body("named", &[1, 2, 3], &["g_voted"]),
            body("g_exact", &[4, 5, 6, 7], &[]),
            body("g_voted", &[40, 41, 99, 43], &[]),
            body("g_similar", &edited, &[]),
            body("g_a", &getter, &["leaf"]),
            body("g_b", &getter, &[]),
            body("leaf", &[20, 22], &[]),
            body("caller", &[30, 31], &["g_a", "g_a"]),
            body("g_new", &[80, 81, 82, 83, 84, 85, 86, 87, 88, 89], &[]),
            body("g_newer", &[70, 71], &[]),
        ],
    );
    let lineage = trace(
        &[left, right],
        Policy::default(),
        &NameClass::Prefixes(vec!["g_".to_owned()]),
    );
    let [step] = lineage.steps.as_slice() else {
        panic!("one step, got {:?}", lineage.steps);
    };
    assert_eq!(
        (
            step.from.as_str(),
            step.to.as_str(),
            step.same_name,
            step.exact_body,
            step.call_graph,
            step.similar,
            step.dominant,
            step.neighbourhood
        ),
        ("left", "right", 3, 2, 1, 1, 0, 1)
    );
    assert_eq!(
        (
            step.changed_bodies,
            step.dissimilar,
            step.unpaired_left,
            step.unpaired_right
        ),
        (3, 0, 1, 2)
    );
    let recovered: Vec<_> = lineage
        .functions
        .iter()
        .map(|function| {
            (
                function.name.as_str(),
                function.source_name.as_deref(),
                function.origin.as_deref(),
                function.steps.len(),
                function.chain.len(),
            )
        })
        .collect();
    assert_eq!(
        recovered,
        [
            ("named", Some("named"), Some("left"), 1, 1),
            ("g_exact", Some("exact"), Some("left"), 1, 1),
            ("g_voted", Some("voted"), Some("left"), 1, 1),
            ("g_similar", Some("similar"), Some("left"), 1, 1),
            ("g_a", Some("get_a"), Some("left"), 1, 1),
            ("g_b", Some("get_b"), Some("left"), 1, 1),
            ("leaf", Some("leaf"), Some("left"), 1, 1),
            ("caller", Some("caller"), Some("left"), 1, 1),
            ("g_new", None, None, 0, 0),
            ("g_newer", None, None, 0, 0),
        ]
    );
    assert_eq!(
        (
            lineage.revisions[0].functions,
            lineage.revisions[1].functions
        ),
        (9, 10)
    );
}

#[test]
fn a_plain_name_in_the_newer_revision_is_kept_and_a_broken_chain_recovers_nothing() {
    let class = NameClass::Prefixes(vec!["g_".to_owned()]);
    let revisions = [
        revision(
            "r0",
            vec![
                body("renamed", &[1, 2, 3, 4], &[]),
                body("kept", &[5, 6, 7, 8], &[]),
                body("lost", &[9, 10, 11, 12], &[]),
            ],
        ),
        revision(
            "r1",
            vec![
                // A plain name that differs from the older one stands.
                body("plain", &[1, 2, 3, 4], &[]),
                body("kept", &[5, 6, 7, 8], &[]),
            ],
        ),
        revision(
            "r2",
            vec![
                body("g_plain", &[1, 2, 3, 4], &[]),
                body("g_kept", &[5, 6, 7, 8], &[]),
                body("g_lost", &[9, 10, 11, 12], &[]),
            ],
        ),
    ];
    let lineage = trace(&revisions, Policy::default(), &class);
    let recovered: Vec<_> = lineage
        .functions
        .iter()
        .map(|function| {
            (
                function.source_name.as_deref(),
                function.origin.as_deref(),
                function
                    .steps
                    .iter()
                    .map(|step| (step.to.as_str(), step.previous_name.as_str()))
                    .collect::<Vec<_>>(),
                function
                    .chain
                    .iter()
                    .map(|step| step.previous_name.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        recovered,
        [
            (
                Some("plain"),
                Some("r1"),
                vec![("r2", "plain")],
                vec!["renamed", "plain"]
            ),
            (
                Some("kept"),
                Some("r0"),
                vec![("r1", "kept"), ("r2", "kept")],
                vec!["kept", "kept"]
            ),
            (None, None, vec![], vec![]),
        ]
    );
    assert_eq!(
        (
            lineage.steps[0].unpaired_left,
            lineage.steps[0].unpaired_right
        ),
        (1, 0)
    );
    assert_eq!(
        (
            lineage.steps[1].unpaired_left,
            lineage.steps[1].unpaired_right
        ),
        (0, 1)
    );
    // No revision, and one.
    assert!(trace(&[], Policy::default(), &class).functions.is_empty());
    let one = trace(&revisions[..1], Policy::default(), &class);
    assert_eq!((one.steps.len(), one.functions.len()), (0, 3));
}

/// FNV-1a over `bytes`, continued from `hash`.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A sequence of numbers that is the same on every run.
struct Numbers(u64);

impl Numbers {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound as u64) as usize
    }

    fn one_in(&mut self, chances: usize) -> bool {
        self.below(chances) == 0
    }
}

/// A family of functions that call each other, and revisions drawn from it:
/// a revision drops, renames and edits functions, changes call lists and
/// adds functions of its own.
struct Family {
    tokens: Vec<Vec<u64>>,
    calls: Vec<Vec<usize>>,
}

impl Family {
    fn new(numbers: &mut Numbers) -> Self {
        let count = 4 + numbers.below(7);
        let tokens = (0..count)
            .map(|_| {
                (0..4 + numbers.below(18))
                    .map(|_| numbers.below(40) as u64)
                    .collect()
            })
            .collect();
        let calls = (0..count)
            .map(|_| {
                (0..numbers.below(6))
                    .map(|_| numbers.below(count))
                    .collect()
            })
            .collect();
        Self { tokens, calls }
    }

    /// The first revision: the family under its names, some functions
    /// missing, some twice.
    fn named(&self, numbers: &mut Numbers) -> Revision {
        let mut functions = vec![];
        for (id, tokens) in self.tokens.iter().enumerate() {
            let calls: Vec<String> = self.calls[id].iter().map(|id| format!("f{id}")).collect();
            let calls: Vec<&str> = calls.iter().map(String::as_str).collect();
            if !numbers.one_in(8) {
                functions.push(body(&format!("f{id}"), tokens, &calls));
            }
            if numbers.one_in(6) {
                functions.push(body(&format!("d{id}"), tokens, &calls));
            }
        }
        revision("named", functions)
    }

    /// A later revision under `prefix` for the names it changes.
    fn later(&self, label: &str, prefix: &str, numbers: &mut Numbers) -> Revision {
        let names: Vec<String> = (0..self.tokens.len())
            .map(|id| {
                if numbers.one_in(2) {
                    format!("f{id}")
                } else {
                    format!("{prefix}{id}")
                }
            })
            .collect();
        let mut functions = vec![];
        for (id, tokens) in self.tokens.iter().enumerate() {
            let mut tokens = tokens.clone();
            if numbers.one_in(2) {
                for _ in 0..1 + numbers.below(tokens.len() / 2 + 1) {
                    let at = numbers.below(tokens.len());
                    match numbers.below(3) {
                        0 => tokens[at] = 40 + numbers.below(40) as u64,
                        1 => tokens.insert(at, 40 + numbers.below(40) as u64),
                        _ if tokens.len() > 2 => {
                            tokens.remove(at);
                        }
                        _ => {}
                    }
                }
            }
            let mut calls: Vec<String> =
                self.calls[id].iter().map(|id| names[*id].clone()).collect();
            if numbers.one_in(3) {
                let at = numbers.below(calls.len() + 1);
                calls.insert(at, names[numbers.below(names.len())].clone());
            }
            if numbers.one_in(5) && !calls.is_empty() {
                calls.remove(numbers.below(calls.len()));
            }
            let calls: Vec<&str> = calls.iter().map(String::as_str).collect();
            if !numbers.one_in(8) {
                functions.push(body(&names[id], &tokens, &calls));
            }
            if numbers.one_in(8) {
                functions.push(body(&format!("{prefix}d{id}"), &tokens, &calls));
            }
        }
        if numbers.one_in(3) {
            let tokens: Vec<u64> = (0..4 + numbers.below(18))
                .map(|_| numbers.below(40) as u64)
                .collect();
            let callee = names[numbers.below(names.len())].clone();
            functions.push(body(&format!("{prefix}n"), &tokens, &[&callee]));
        }
        revision(label, functions)
    }
}

/// What `correlate` and `trace` give for fifteen hundred drawn families of
/// three revisions under three policies, held as one digest with the number of
/// pairs of each evidence: the correspondence and the lineage of this crate
/// as it stands.
#[test]
fn the_correspondence_and_the_lineage_of_a_space_of_revisions_are_held() {
    let policies = [
        Policy::default(),
        Policy {
            minimum_ppm: 600_000,
            margin_ppm: 10_000,
            call_graph_minimum_ppm: 300_000,
            dominant_minimum_ppm: 300_000,
            dominance_ratio: 0,
            neighbourhood_minimum_ppm: 300_000,
        },
        Policy {
            minimum_ppm: 950_000,
            margin_ppm: 200_000,
            call_graph_minimum_ppm: 800_000,
            dominant_minimum_ppm: 700_000,
            dominance_ratio: 3,
            neighbourhood_minimum_ppm: 900_000,
        },
    ];
    let class = NameClass::Prefixes(vec!["g".to_owned(), "h".to_owned()]);
    let mut numbers = Numbers(0x1eaf);
    let mut digest = 0xcbf2_9ce4_8422_2325;
    let mut evidence = [0_usize; 6];
    for _ in 0..1500 {
        let family = Family::new(&mut numbers);
        let revisions = [
            family.named(&mut numbers),
            family.later("second", "g", &mut numbers),
            family.later("third", "h", &mut numbers),
        ];
        for policy in policies {
            for pair in revisions.windows(2) {
                let pairs = correlate(&pair[0], &pair[1], policy);
                for pair in &pairs {
                    evidence[match pair.evidence {
                        Evidence::SameName => 0,
                        Evidence::ExactBody => 1,
                        Evidence::CallGraph => 2,
                        Evidence::Similar { .. } => 3,
                        Evidence::Dominant { .. } => 4,
                        Evidence::Neighbourhood { .. } => 5,
                    }] += 1;
                }
                digest = fnv(digest, format!("{pairs:?}\n").as_bytes());
            }
            let lineage = trace(&revisions, policy, &class);
            digest = fnv(digest, format!("{lineage:?}\n").as_bytes());
        }
    }
    assert_eq!(evidence, [18417, 8439, 7852, 4084, 6677, 760]);
    assert_eq!(digest, 0x060e_c873_bc6d_611e, "{digest:#018x}");
}
