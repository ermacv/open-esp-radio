use super::*;
use crate::stack::reachability::Report;
use oer_image_stack::{Fact, Predecessor};

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

#[test]
fn an_independent_image_layout_keeps_chip_observers_and_can_add_its_own() {
    let mut image = graph(true, false);
    image.observed_functions = vec!["core::fmt::write".into()];
    let mut policy = watched();
    policy.observed_functions = vec!["deeper".into()];
    let out = Report::new(&image, &policy).root("CPU0", &bound(&image));
    assert!(out.contains("verdict=reachable\tpaths=2"), "{out}");
    assert!(out.contains("core::fmt::write"), "{out}");
    assert!(out.contains("deeper"), "{out}");
    policy.observed_functions.push("core::fmt::write".into());
    let out = Report::new(&image, &policy).root("CPU0", &bound(&image));
    assert!(out.contains("verdict=reachable\tpaths=2"), "{out}");
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
fn aliases_and_overlapping_selectors_count_one_implementation_and_missing_entries_stay_explicit() {
    let mut image = graph(true, false);
    image
        .analysis
        .functions
        .get_mut(&FORMATTER)
        .unwrap()
        .function
        .names
        .push("app::render".into());
    let mut policy = watched();
    policy
        .observed_functions
        .extend(["app::*".into(), "missing::function".into()]);
    let out = Report::new(&image, &policy).root("CPU0", &bound(&image));
    assert!(out.contains("verdict=reachable\tpaths=1"), "{out}");
    assert!(out.contains("app::render | core::fmt::write"), "{out}");
    assert!(
        out.contains("proven-unreachable\tlinked=0\tselector=missing::function"),
        "{out}"
    );
    let image = graph(false, true);
    let out = Report::new(&image, &policy).root("CPU0", &bound(&image));
    assert!(
        out.contains("unknown\tlinked=0\tselector=missing::function"),
        "{out}"
    );
}

#[test]
fn irq_context_resolutions_and_bootstrap_use_the_same_three_way_report() {
    use oer_image_stack::{Stacks, TableEntry, TrapEntry, Vector};
    let image = graph(false, true);
    let entries = [
        TableEntry {
            core: 0,
            line: 16,
            level: 2,
            class: oer_image_stack::Class::Executor,
            source: 7,
            handler: Some(FORMATTER),
        },
        TableEntry {
            core: 1,
            line: 16,
            level: 2,
            class: oer_image_stack::Class::Executor,
            source: 8,
            handler: Some(TEXT + 28),
        },
    ];
    // Each hart's line calls its entry's handler directly.
    let vector = |hart, handler| Vector {
        hart,
        line: 16,
        entry: TrapEntry {
            entry: TEXT,
            handler,
            frame: 0,
        },
    };
    let inputs = Stacks {
        table: &entries,
        cores: &[0, 1],
        vectors: &[vector(0, FORMATTER), vector(1, TEXT + 28)],
        exception: vector(0, TEXT).entry,
        exception_copies: 1,
        resolutions: &image.resolutions,
    };
    let (harts, _) = image
        .with_bounds(|keep, control| {
            Ok(oer_image_stack::interrupt_stacks(
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
