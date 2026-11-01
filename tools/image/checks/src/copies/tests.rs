use super::*;
use crate::stack::Image;
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

/// A function of 24 bytes at `at` that calls `target`.
fn root_code(name: &'static str, at: u32, target: u32) -> Code {
    Code {
        name,
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(at + 8, target),
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

/// The root `label` at `entry`, over the functions its bound reaches.
fn root(image: &Image, label: &str, entry: u32) -> Root {
    let bound = image
        .with_bounds(|keep, control| {
            Ok(image
                .analysis
                .bound_with(entry, &image.resolutions, keep, control)?)
        })
        .unwrap()
        .0;
    Root::of(label, false, &image.analysis, &image.names, &bound.reached)
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
    let report = render(&sites, &image.names, &Nowhere, Path::new("/root"), None);
    assert!(
        report.contains("constant >= 512 B\t1 (1240 B in all)"),
        "{report}"
    );
    assert!(report.contains("constant < 512 B\t1"), "{report}");
    assert!(report.contains("\nvariable\t1"), "{report}");
    assert!(report.contains("\nroots: not analysed"), "{report}");
    assert!(
        report.contains("1240\t0x0000100c\toer_station::poll_action\t?\t"),
        "{report}"
    );
    assert!(
        !report.contains("\t0x00001028\toer_driver::poll"),
        "{report}"
    );
    assert!(report.contains("1\toer_net::copy_payload\t?"), "{report}");
}

/// Two roots: `hart0-P2` reaches one copy, the task root another;
/// `unreached` copies too, but no root reaches it.
fn two_roots() -> (Image, Vec<Root>) {
    const ROOT_IRQ: u32 = TEXT;
    const ROOT_TASK: u32 = ROOT_IRQ + 24;
    const LARGE: u32 = ROOT_TASK + 24;
    const SMALL: u32 = LARGE + 28;
    const UNREACHED: u32 = SMALL + 28;
    const MEMMOVE: u32 = UNREACHED + 28;
    let image = image(
        &[
            root_code("irq_root", ROOT_IRQ, LARGE),
            root_code("task_root", ROOT_TASK, SMALL),
            caller("oer_station::poll_action", LARGE, li_a2(1240), MEMMOVE),
            caller("oer_driver::poll", SMALL, li_a2(640), MEMMOVE),
            caller("oer_driver::unreached", UNREACHED, li_a2(2000), MEMMOVE),
            Code {
                name: "memmove",
                words: vec![RET],
                frame: Some(0),
            },
        ],
        &[],
    );
    let roots = vec![
        root(&image, "hart0-P2", ROOT_IRQ),
        root(&image, "hart0 task", ROOT_TASK),
    ];
    (image, roots)
}

#[test]
fn each_site_and_each_root_s_totals_name_the_roots_that_reach_it() {
    let (image, roots) = two_roots();
    let sites = sites(&image.analysis, &image.names);
    assert_eq!(sites.len(), 3);
    let report = render(
        &sites,
        &image.names,
        &Nowhere,
        Path::new("/root"),
        Some(&roots),
    );
    assert!(
        report.contains("\t0x0000103c\toer_station::poll_action\thart0-P2\t"),
        "{report}"
    );
    assert!(
        report.contains("\t0x00001058\toer_driver::poll\thart0 task\t"),
        "{report}"
    );
    assert!(
        report.contains("\t0x00001074\toer_driver::unreached\t-\t"),
        "{report}"
    );
    assert!(report.contains("\nhart0-P2\t1\t1\t1240\t0\n"), "{report}");
    assert!(report.contains("\nhart0 task\t1\t1\t640\t0\n"), "{report}");
    assert!(report.contains("\n-\t1\t1\t2000\t0\n"), "{report}");
}

#[test]
fn the_machine_readable_sites_hold_return_addresses_code_ranges_and_roots() {
    let (image, roots) = two_roots();
    let sites = sites(&image.analysis, &image.names);
    let copies = copies(&image, &sites, &Nowhere, Path::new("/root"), Some(&roots)).unwrap();
    assert_eq!(copies.schema, SCHEMA);
    assert_eq!(
        copies.roots.as_deref(),
        Some(&["hart0-P2".to_owned(), "hart0 task".to_owned()][..])
    );
    let memmove = image.symbol("memmove").unwrap();
    assert_eq!(
        copies.entries,
        [super::Code {
            name: "memmove".to_owned(),
            address: memmove,
            size: 4,
        }]
    );
    let large = &copies.sites[0];
    assert_eq!(large.caller.name, "oer_station::poll_action");
    assert_eq!((large.caller.address, large.caller.size), (TEXT + 48, 28));
    assert!(large.caller.contains(large.site) && !large.caller.contains(TEXT + 76));
    // A `jal` of four bytes returns past itself.
    assert_eq!(large.returns, Some(large.site + 4));
    assert_eq!((large.entry.as_str(), large.bytes), ("memmove", Some(1240)));
    assert_eq!(large.roots, ["hart0-P2"]);
    assert!(copies.sites[2].roots.is_empty());

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RUNTIME_JSON);
    std::fs::write(&path, serde_json::to_vec(&copies).unwrap()).unwrap();
    assert_eq!(read(&path).unwrap(), copies);
    let mut other = copies.clone();
    other.schema = SCHEMA + 1;
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    assert!(read(&path).is_err());
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
