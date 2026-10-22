use super::*;
use crate::stack::reachability::Report;
use oer_analysis_riscv_stack::{Fact, Predecessor};

const FORMATTER: u32 = TEXT + 40;

fn graph(formatting: bool, hole: bool) -> Image {
    image(
        &[
            Code {
                name: "root",
                frame: Some(16),
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    call(TEXT + 8, TEXT + 28),
                    if formatting {
                        call(TEXT + 12, FORMATTER)
                    } else if hole {
                        CALL_A5
                    } else {
                        0x0000_0013
                    },
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                ],
            },
            Code {
                name: "deeper",
                frame: Some(32),
                words: vec![SP_DOWN_32, SP_UP_32, RET],
            },
            Code {
                name: "core::fmt::write",
                frame: Some(0),
                words: vec![RET],
            },
        ],
        &[],
    )
}

fn watched() -> StackPolicy {
    toml::from_str("schema=5\nmax_move_bytes=4096\nobserved_functions=['core::fmt::write']")
        .unwrap()
}

fn bound(image: &Image) -> Bound {
    image
        .with_bounds(|keep, control| {
            Ok(image
                .analysis
                .bound_with(TEXT, &image.resolutions, keep, control)?)
        })
        .unwrap()
        .0
}

#[test]
fn a_shorter_branch_has_an_actual_witness_even_when_the_critical_path_omits_it() {
    let image = graph(true, false);
    let bound = bound(&image);
    assert!(!bound.path.iter().any(|&(address, _)| address == FORMATTER));
    assert_eq!(
        bound.witness(FORMATTER),
        Some(vec![(
            FORMATTER,
            Predecessor {
                caller: TEXT,
                site: TEXT + 12
            }
        )])
    );
    assert_eq!(bound.witness(TEXT), Some(Vec::new()));
    let policy = watched();
    let out = Report::new(&image, &policy).root("CPU0", &bound);
    assert!(
        out.contains("root\tCPU0\tverdict=reachable\tpaths=1\tholes=0"),
        "{out}"
    );
    assert!(
        out.contains("0x00001000 --0x0000100c--> 0x00001028 core::fmt::write"),
        "{out}"
    );
}

#[test]
fn only_complete_roots_prove_unreachability_and_holes_never_supply_witnesses() {
    let policy = watched();
    for hole in [false, true] {
        let image = graph(false, hole);
        let bound = bound(&image);
        assert_eq!(bound.witness(FORMATTER), None);
        let out = Report::new(&image, &policy).root("bootstrap", &bound);
        let expected = if hole {
            "unknown"
        } else {
            "proven-unreachable"
        };
        assert!(
            out.contains(&format!("root\tbootstrap\tverdict={expected}\tpaths=0")),
            "{out}"
        );
        assert_eq!(out.contains("  hole\t"), hole, "{out}");
        assert!(!out.contains("witness:"), "{out}");
    }
}

#[test]
fn resolved_edges_get_witnesses_but_supplemental_targets_of_direct_edges_do_not() {
    let mut image = graph(false, true);
    image
        .resolutions
        .add(TEXT + 12, Fact::FieldType, [FORMATTER]);
    image
        .resolutions
        .add(TEXT + 8, Fact::FieldType, [FORMATTER]);
    let bound = bound(&image);
    assert_eq!(
        bound.witness(FORMATTER),
        Some(vec![(
            FORMATTER,
            Predecessor {
                caller: TEXT,
                site: TEXT + 12
            }
        )])
    );
    assert!(bound.unresolved.is_empty());
}

#[test]
fn irq_context_resolutions_and_bootstrap_use_the_same_three_way_report() {
    use oer_analysis_riscv_stack::{Stacks, TableEntry, TrapEntry};
    let mut image = graph(false, true);
    image
        .analysis
        .functions
        .get_mut(&TEXT)
        .unwrap()
        .table_bases
        .insert(TEXT + 12, 0x3000);
    let entries = [TableEntry {
        core: 0,
        level: 2,
        source: 7,
        handler: Some(FORMATTER),
    }];
    let vector = TrapEntry {
        entry: TEXT,
        handler: TEXT,
        frame: 0,
    };
    let inputs = Stacks {
        table: &entries,
        cores: &[0, 1],
        always: &[2],
        vectors: &[vector],
        exception: vector,
        sources: 0x3000,
        resolutions: &image.resolutions,
    };
    let (harts, _) = image
        .with_bounds(|keep, control| {
            Ok(oer_analysis_riscv_stack::interrupt_stacks(
                &image.analysis,
                &inputs,
                keep,
                control,
            )?)
        })
        .unwrap();
    let policy = watched();
    let report = Report::new(&image, &policy);
    let first = report.root("hart0-P2", &harts[0].levels[0].bound);
    let second = report.root("hart1-P2", &harts[1].levels[0].bound);
    assert!(first.contains("verdict=reachable\tpaths=1"), "{first}");
    assert!(!second.contains("verdict=reachable"), "{second}");
    assert_eq!(harts[1].levels[0].bound.witness(FORMATTER), None);
    let bootstrap = report.root("bootstrap", &harts[0].levels[0].bound);
    assert!(bootstrap.contains("verdict=reachable\tpaths=1"));
}
