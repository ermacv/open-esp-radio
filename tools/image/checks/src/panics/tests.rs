use super::*;
use crate::stack::Image;
use crate::stack::tests::{Code, LOAD_RA, RET, SAVE_RA, SP_DOWN_16, SP_UP_16, TEXT, call, image};
use std::path::Path;

const HAL: u32 = TEXT + 28;
const PROTOCOL: u32 = HAL + 24;
const PANIC: u32 = PROTOCOL + 24;
const PANIC_FMT: u32 = PANIC + 24;
const UNREACHED: u32 = PANIC_FMT + 4;

/// A function of 24 bytes at `at` that calls `target`.
fn caller(name: &'static str, at: u32, target: u32) -> Code {
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

/// `root` calls a HAL function and a protocol function, each of which
/// calls `core::panicking::panic`, which calls `panic_fmt`; `unreached`
/// also calls `panic`, but no root reaches it.
fn program() -> Image {
    program_with_panic("core::panicking::panic", PANIC)
}

fn program_with_panic(entry: &'static str, protocol_target: u32) -> Image {
    image(
        &[
            Code {
                name: "root",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    call(TEXT + 8, HAL),
                    call(TEXT + 12, PROTOCOL),
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                ],
                frame: Some(16),
            },
            caller("oer_esp32s31_hal::register::write", HAL, PANIC),
            caller("oer_ieee80211_mac::receive", PROTOCOL, protocol_target),
            caller(entry, PANIC, PANIC_FMT),
            Code {
                name: "core::panicking::panic_fmt",
                words: vec![RET],
                frame: Some(0),
            },
            caller("oer_ieee80211_mac::unreached", UNREACHED, PANIC),
        ],
        &[],
    )
}

fn root(image: &Image, label: &str, hardware: bool) -> Root {
    let bound = image
        .with_bounds(|keep, control| {
            Ok(image
                .analysis
                .bound_with(TEXT, &image.resolutions, keep, control)?)
        })
        .unwrap()
        .0;
    Root::of(
        label,
        hardware,
        &image.analysis,
        &image.names,
        &bound.reached,
    )
}

fn contract(runtime: &[(Layer, usize)]) -> Contract {
    contract_with(runtime, 0)
}

fn contract_with(runtime: &[(Layer, usize)], hardware: usize) -> Contract {
    Contract {
        layers: Layers::of_packages([
            ("oer-esp32s31-hal", Path::new("chips/esp32s31/hal")),
            ("oer-ieee80211-mac", Path::new("protocols/ieee80211/mac")),
        ]),
        budget: Budget {
            runtime: Limits {
                layers: runtime.iter().copied().collect(),
                hardware,
            },
            bootstrap: Limits::default(),
        },
    }
}

#[test]
fn a_reachable_panic_counts_once_in_the_layer_of_its_caller() {
    let image = program();
    let report = Report {
        elf: CheckedElf::Runtime,
        // Two roots reaching the same sites count each site once.
        roots: vec![root(&image, "CPU0", false), root(&image, "T0", false)],
        names: &image.names,
    };
    let contract = contract(&[(Layer::Hal, 1), (Layer::Protocols, 1)]);
    assert_eq!(
        report.counts(&contract),
        BTreeMap::from([(Layer::Hal, 1), (Layer::Protocols, 1)])
    );
    assert_eq!(report.roots[0].sites.len(), 2);
    assert!(
        report.roots[0]
            .sites
            .iter()
            .all(|site| site.entry == PANIC && site.caller != UNREACHED)
    );
    assert_eq!(report.warnings(&contract), Vec::<String>::new());
    let text = report.render(&contract);
    assert!(text.contains("hal\t1\t1"), "{text}");
    assert!(text.contains("protocols\t1\t1"), "{text}");
    assert!(text.contains("CPU0: 2 (hal 1, protocols 1)"), "{text}");
    assert!(
        text.contains("oer_esp32s31_hal::register::write at 0x00001024 calls core::panicking::panic\t[CPU0, T0]"),
        "{text}"
    );
}

#[test]
fn compiler_resumed_checks_have_a_separate_row_and_keep_their_locations() {
    let image = program_with_panic(ASYNC_FN_RESUMED, PANIC_FMT);
    let mut contract = contract_with(&[(Layer::Protocols, 1)], 1);
    contract.budget.bootstrap = contract.budget.runtime.clone();
    for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
        let report = Report {
            elf,
            roots: vec![root(&image, "CPU0", true), root(&image, "T0", false)],
            names: &image.names,
        };
        // Both direct callees are discovered, even though only one is budgeted.
        assert_eq!(report.roots[0].sites.len(), 2);
        assert_eq!(
            report.counts(&contract),
            BTreeMap::from([(Layer::Protocols, 1)])
        );
        assert!(report.warnings(&contract).is_empty());
        let text = report.render(&contract);
        for expected in [
            "panic sites (excluding compiler resumed-future checks, distinct over all roots): 1",
            "hal\t0\t0",
            "protocols\t1\t1",
            "hardware\t1\t1",
            "compiler-async-fn-resumed\t1\texcluded",
            "CPU0 (hardware): 1 (protocols 1); compiler-async-fn-resumed 1",
            "T0: 1 (protocols 1); compiler-async-fn-resumed 1",
            "compiler-async-fn-resumed\toer_esp32s31_hal::register::write at 0x00001024 calls core::panicking::panic_const::panic_const_async_fn_resumed\t[CPU0, T0]",
            "protocols\toer_ieee80211_mac::receive at 0x0000103c calls core::panicking::panic_fmt\t[CPU0, T0]",
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
    }
}

#[test]
fn compiler_resumed_checks_do_not_spend_a_zero_budget_or_the_zero_target() {
    let image = program_with_panic(ASYNC_FN_RESUMED, PANIC);
    let report = Report {
        elf: CheckedElf::Runtime,
        roots: vec![root(&image, "CPU0", true), root(&image, "T0", false)],
        names: &image.names,
    };
    let contract = Contract::default();
    assert!(report.counts(&contract).is_empty());
    assert!(report.warnings(&contract).is_empty());
    let text = report.render(&contract);
    assert!(text.contains("distinct over all roots): 0"), "{text}");
    assert!(
        text.contains("compiler-async-fn-resumed\t2\texcluded"),
        "{text}"
    );
    assert!(text.contains("hardware\t0\t0"), "{text}");
    assert!(
        text.contains("CPU0 (hardware): 0; compiler-async-fn-resumed 2"),
        "{text}"
    );
}

#[test]
fn ordinary_panics_are_not_classified_by_a_similar_callee_or_the_caller_name() {
    for entry in [
        "core::panicking::panic",
        "core::panicking::panic_const::panic_const_async_fn_resumed_extra",
        "core::panicking::panic_const::panic_const_async_gen_fn_resumed",
    ] {
        let mut image = program_with_panic(entry, PANIC);
        image
            .names
            .insert(HAL, "oer_esp32s31_hal::panic_const_async_fn_resumed".into());
        let report = Report {
            elf: CheckedElf::Runtime,
            roots: vec![root(&image, "CPU0", true)],
            names: &image.names,
        };
        let contract = contract_with(&[(Layer::Hal, 1), (Layer::Protocols, 1)], 2);
        assert_eq!(report.counts(&contract).values().sum::<usize>(), 2);
        assert!(report.warnings(&contract).is_empty());
        let text = report.render(&contract);
        assert!(
            text.contains("compiler-async-fn-resumed\t0\texcluded"),
            "{text}"
        );
        assert!(
            text.contains("hal\toer_esp32s31_hal::panic_const_async_fn_resumed"),
            "{text}"
        );
        assert!(text.contains("hardware\t2\t2"), "{text}");
    }
}

#[test]
fn a_layer_over_its_budget_warns_and_one_under_it_asks_to_lower_it() {
    let image = program();
    let report = Report {
        elf: CheckedElf::Runtime,
        roots: vec![root(&image, "CPU0", false)],
        names: &image.names,
    };
    let warnings = report.warnings(&contract(&[(Layer::Hal, 1)]));
    let error = &warnings[0];
    assert!(
        error
            .contains("runtime reaches 1 panic sites in the protocols layer, over its budget of 0"),
        "{error}"
    );
    assert!(
        report
            .render(&contract(&[(Layer::Hal, 1)]))
            .contains("warning: runtime reaches 1 panic sites in the protocols layer")
    );
    let warnings = report.warnings(&contract(&[(Layer::Hal, 3), (Layer::Protocols, 1)]));
    assert_eq!(
        warnings,
        [
            "runtime reaches 1 panic sites in the hal layer, under its budget of 3: lower \
          `panic-budget.runtime.hal` to 1"
        ]
    );
}

#[test]
fn the_hardware_contexts_have_their_own_budget_over_all_layers() {
    let image = program();
    let report = Report {
        elf: CheckedElf::Runtime,
        roots: vec![
            root(&image, "hart0-P3", true),
            root(&image, "hart0-P1", true),
        ],
        names: &image.names,
    };
    let layers = [(Layer::Hal, 1), (Layer::Protocols, 1)];
    let warnings = report.warnings(&contract_with(&layers, 1));
    let error = warnings[0].as_str();
    assert_eq!(
        error,
        "runtime reaches 2 panic sites from its hardware interrupt contexts, over its budget of 1"
    );
    assert_eq!(
        report.warnings(&contract_with(&layers, 2)),
        Vec::<String>::new()
    );
    assert!(
        report
            .render(&contract_with(&layers, 2))
            .contains("hardware\t2\t2")
    );
    // A non-hardware root reaching the same sites takes nothing from it.
    let task = Report {
        elf: CheckedElf::Runtime,
        roots: vec![root(&image, "T0", false)],
        names: &image.names,
    };
    assert_eq!(
        task.warnings(&contract_with(&layers, 0)),
        Vec::<String>::new()
    );
}

#[test]
fn the_pac_layer_is_budgeted_like_the_others() {
    let image = program();
    let report = Report {
        elf: CheckedElf::Bootstrap,
        roots: vec![root(&image, "bootstrap", false)],
        names: &image.names,
    };
    let pac = |declared: serde_json::Value| Contract {
        layers: Layers::of_packages([
            ("oer-esp32s31-hal", Path::new("chips/esp32s31/pac")),
            ("oer-ieee80211-mac", Path::new("protocols/ieee80211/mac")),
        ]),
        budget: Budget::parse(Some(&declared)).unwrap(),
    };
    let warnings = report.warnings(&pac(serde_json::json!({ "bootstrap": { "protocols": 1 } })));
    let error = &warnings[0];
    assert!(
        error.contains("bootstrap reaches 1 panic sites in the pac layer, over its budget of 0"),
        "{error}"
    );
    assert!(
        report
            .warnings(&pac(
                serde_json::json!({ "bootstrap": { "protocols": 1, "pac": 1 } })
            ))
            .is_empty()
    );
    let unknown = Budget::parse(Some(&serde_json::json!({ "runtime": { "drivers": 1 } })))
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("unknown layer `drivers`"), "{unknown}");
}

#[test]
fn every_category_and_hardware_excess_is_reported_without_a_failed_check() {
    let (directory, policy) =
        crate::stack::tests::policy_with("schema = 6\nmax_move_bytes = 4096\n", "");
    for (path, text) in oer_espressif_facts::tracked::FILES {
        let path = directory.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let profile = oer_chip_profile::Profile::load(directory.path(), "esp32s31").unwrap();
    let mut image = program();
    image.stack_sizes.insert(UNREACHED, 4097);
    let mut contract = Contract::default();
    let mut names = BTreeMap::new();
    let mut sites = BTreeSet::new();
    for (index, layer) in Layer::ALL.into_iter().enumerate() {
        let caller = u32::try_from(index).unwrap();
        let name = if layer == Layer::Unnamed {
            "_start".to_owned()
        } else {
            let name = format!("layer_{index}");
            contract.layers.crates.insert(name.clone(), layer);
            format!("{name}::may_panic")
        };
        names.insert(caller, name);
        sites.insert(Site {
            caller,
            site: caller,
            entry: PANIC,
        });
    }
    image.names.extend(names);
    let roots = vec![Root {
        label: "hardware".into(),
        hardware: true,
        sites,
    }];
    let input = crate::CheckInput {
        root: directory.path(),
        profile: &profile,
        policy: &policy,
        elf: Path::new("unused.elf"),
        flat: None,
        output: directory.path(),
        task_stack_proven: true,
        critical_section_hart: 0,
        stage_two_elf: None,
        panics: Some(&contract),
    };
    let gates = crate::Gates::all(crate::interrupts::Required::Product);
    for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
        let mut outcome = crate::CheckOutcome::default();
        gates.frames_of(&image, &input, elf, &mut outcome);
        gates.panics_of(&image, &input, elf, Ok(roots.clone()), &mut outcome);
        assert!(outcome.results.iter().all(|(_, result)| result.passes()));
        assert_eq!(outcome.warnings.len(), Layer::ALL.len() + 2);
        let name = if elf == CheckedElf::Runtime {
            RUNTIME_REPORT
        } else {
            BOOTSTRAP_REPORT
        };
        let text = std::fs::read_to_string(directory.path().join(name)).unwrap();
        for layer in Layer::ALL {
            assert!(text.contains(&format!("{layer}\t1\t0")), "{text}");
        }
        assert!(text.contains("hardware\t10\t0"), "{text}");
        assert!(text.contains("_start at 0x00000009"), "{text}");
        assert!(!text.contains("FAIL:"), "{text}");
    }
}

#[test]
fn a_function_takes_the_layer_of_the_crate_its_name_starts_with() {
    let layers = Layers::of_packages([
        ("oer-esp32s31-pac", Path::new("chips/esp32s31/pac")),
        ("oer-esp32s31-pac-raw", Path::new("chips/esp32s31/pac-raw")),
        (
            "oer-espressif-ieee80211-pac",
            Path::new("chips/espressif/ieee80211-pac"),
        ),
        ("oer-esp32s31-hal", Path::new("chips/esp32s31/hal")),
        (
            "oer-espressif-executor",
            Path::new("chips/espressif/executor"),
        ),
        ("oer-time", Path::new("portable/time")),
        ("oer-ieee80211-sta", Path::new("protocols/ieee80211/sta")),
        (
            "oer-firmware-apps-boot-smoke",
            Path::new("firmware/apps/boot-smoke"),
        ),
        ("oer-image-checks", Path::new("tools/image/checks")),
    ]);
    for (name, layer) in [
        ("oer_esp32s31_pac::Block::read", Layer::Pac),
        ("oer_esp32s31_pac_raw::x", Layer::Pac),
        ("oer_espressif_ieee80211_pac::x", Layer::Pac),
        (
            "<oer_esp32s31_hal::Uart as core::fmt::Write>::write_str",
            Layer::Hal,
        ),
        ("oer_espressif_executor::poll", Layer::Chips),
        (
            "<&oer_time::Instant as core::fmt::Debug>::fmt",
            Layer::Portable,
        ),
        ("oer_ieee80211_sta::Station::step", Layer::Protocols),
        ("oer_firmware_apps_boot_smoke::main", Layer::Firmware),
        ("core::ptr::drop_in_place<oer_time::Instant>", Layer::Core),
        ("compiler_builtins::mem::memcpy", Layer::Core),
        ("embassy_sync::signal::Signal<M,T>::wait", Layer::Embassy),
        ("esp_radio::wifi::start", Layer::ThirdParty),
        ("esp32p4::Peripherals::steal", Layer::ThirdParty),
        ("heapless::vec::Vec<T,_>::push", Layer::ThirdParty),
        ("oer_image_checks::x", Layer::ThirdParty),
        ("memcpy", Layer::Unnamed),
        ("FROM_CPU_INTR3", Layer::Unnamed),
    ] {
        assert_eq!(layers.of(name), layer, "{name}");
    }
}
