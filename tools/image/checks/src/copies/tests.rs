use super::*;
use crate::stack::tests::{Code, LOAD_RA, RET, SAVE_RA, SP_DOWN_16, SP_UP_16, TEXT, call, image};

const LARGE: u32 = TEXT;
const SMALL: u32 = LARGE + 28;
const VARIABLE: u32 = SMALL + 28;
const MEMCPY: u32 = VARIABLE + 28;

/// `addi a2, zero, bytes`.
fn li_a2(bytes: u32) -> u32 {
    (bytes << 20) | (12 << 7) | 0x13
}

/// A function of 28 bytes at `at` that runs `set` and calls `target`.
fn caller(name: &'static str, at: u32, set: u32, target: u32) -> Code {
    Code {
        name,
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            set,
            call(at + 12, target),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    }
}

/// No source: every site is named by its caller alone.
struct Nowhere;
impl Locate for Nowhere {
    fn locate(&self, _: u32) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn constant_and_variable_copies_are_reported_by_size_and_caller() {
    // `addi a2, a0, 0`: the size the caller was given.
    const A2_FROM_A0: u32 = (10 << 15) | (12 << 7) | 0x13;
    let program = [
        caller("oer_station::poll_action", LARGE, li_a2(1240), MEMCPY),
        caller("oer_driver::poll", SMALL, li_a2(64), MEMCPY),
        caller("oer_net::copy_payload", VARIABLE, A2_FROM_A0, MEMCPY),
        Code {
            name: "compiler_builtins::mem::memcpy",
            words: vec![RET],
            frame: Some(0),
        },
    ];
    let image = image(&program, &[]);
    let sites = sites(&image.analysis, &image.names);
    let sizes: Vec<(String, Option<u32>)> = sites
        .iter()
        .map(|site| (image.names[&site.caller].clone(), site.bytes))
        .collect();
    assert_eq!(
        sizes,
        [
            ("oer_station::poll_action".to_owned(), Some(1240)),
            ("oer_driver::poll".to_owned(), Some(64)),
            ("oer_net::copy_payload".to_owned(), None),
        ]
    );
    let report = render(&sites, &image.names, &Nowhere, Path::new("/root"));
    assert!(
        report.contains("constant >= 512 B\t1 (1240 B in all)"),
        "{report}"
    );
    assert!(report.contains("constant < 512 B\t1"), "{report}");
    assert!(report.contains("\nvariable\t1"), "{report}");
    assert!(
        report.contains("1240\t0x0000100c\toer_station::poll_action\t"),
        "{report}"
    );
    assert!(
        !report.contains("\t0x00001028\toer_driver::poll"),
        "{report}"
    );
    assert!(report.contains("1\toer_net::copy_payload"), "{report}");
}

#[test]
fn copy_entries_are_the_builtins_and_their_c_symbols() {
    for name in [
        "memcpy",
        "memmove",
        "compiler_builtins::mem::memcpy",
        "compiler_builtins::mem::memcpy::h0123456789abcdef",
    ] {
        assert!(copy_entry(name), "{name}");
    }
    for name in [
        "memset",
        "core::ptr::copy_nonoverlapping",
        "oer_net::memcpy_like",
    ] {
        assert!(!copy_entry(name), "{name}");
    }
}
