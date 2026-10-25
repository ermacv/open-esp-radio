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
            caller("oer_ieee80211_mac::receive", PROTOCOL, PANIC),
            caller("core::panicking::panic", PANIC, PANIC_FMT),
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
    assert_eq!(report.check(&contract).unwrap(), Vec::<String>::new());
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
fn a_layer_over_its_budget_fails_and_one_under_it_asks_to_lower_it() {
    let image = program();
    let report = Report {
        elf: CheckedElf::Runtime,
        roots: vec![root(&image, "CPU0", false)],
        names: &image.names,
    };
    let error = report
        .check(&contract(&[(Layer::Hal, 1)]))
        .unwrap_err()
        .to_string();
    assert!(
        error
            .contains("runtime reaches 1 panic sites in the protocols layer, over its budget of 0"),
        "{error}"
    );
    assert!(
        report
            .render(&contract(&[(Layer::Hal, 1)]))
            .contains("FAIL: runtime reaches 1 panic sites in the protocols layer")
    );
    let warnings = report
        .check(&contract(&[(Layer::Hal, 3), (Layer::Protocols, 1)]))
        .unwrap();
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
    let error = report
        .check(&contract_with(&layers, 1))
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "runtime reaches 2 panic sites from its hardware interrupt contexts, over its budget of 1"
    );
    assert_eq!(
        report.check(&contract_with(&layers, 2)).unwrap(),
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
        task.check(&contract_with(&layers, 0)).unwrap(),
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
    let error = report
        .check(&pac(serde_json::json!({ "bootstrap": { "protocols": 1 } })))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("bootstrap reaches 1 panic sites in the pac layer, over its budget of 0"),
        "{error}"
    );
    assert!(
        report
            .check(&pac(
                serde_json::json!({ "bootstrap": { "protocols": 1, "pac": 1 } })
            ))
            .is_ok()
    );
    let unknown = Budget::parse(Some(&serde_json::json!({ "runtime": { "drivers": 1 } })))
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("unknown layer `drivers`"), "{unknown}");
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
        ("oer-firmware-apps-w1", Path::new("firmware/apps/w1")),
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
        ("oer_firmware_apps_w1::main", Layer::Firmware),
        ("core::ptr::drop_in_place<oer_time::Instant>", Layer::Core),
        ("compiler_builtins::mem::memcpy", Layer::Core),
        ("embassy_sync::signal::Signal<M,T>::wait", Layer::Embassy),
        ("esp_radio::wifi::start", Layer::EspHal),
        ("esp32p4::Peripherals::steal", Layer::EspHal),
        ("heapless::vec::Vec<T,_>::push", Layer::ThirdParty),
        ("oer_image_checks::x", Layer::ThirdParty),
        ("memcpy", Layer::Unnamed),
        ("FROM_CPU_INTR3", Layer::Unnamed),
    ] {
        assert_eq!(layers.of(name), layer, "{name}");
    }
}
