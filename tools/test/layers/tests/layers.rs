//! Structural rules read from package manifests: names, the unsafe boundary
//! and dependency direction for the Wi-Fi chip backends.

/// The name of the package at `path`: `oer-` and the path below its root,
/// with `firmware-` kept for images and `tool-` for the programs of `tools/`.
fn expected_name(path: &str, executable: bool) -> String {
    let parts = path.split('/').collect::<Vec<_>>();
    let parts = match parts.as_slice() {
        ["firmware", rest @ ..] => [&["oer", "firmware"][..], rest].concat(),
        ["tools", rest @ ..] if executable => [&["oer", "tool"][..], rest].concat(),
        [_, rest @ ..] => [&["oer"][..], rest].concat(),
        [] => Vec::new(),
    };
    parts.join("-")
}

#[test]
fn the_name_follows_the_path() {
    for (path, executable, name) in [
        ("portable/time", false, "oer-time"),
        ("protocols/ieee80211/sta", false, "oer-ieee80211-sta"),
        (
            "chips/esp32s31/ieee80211/driver",
            false,
            "oer-esp32s31-ieee80211-driver",
        ),
        ("firmware/apps/w1", true, "oer-firmware-apps-w1"),
        ("tools/image/build", false, "oer-image-build"),
        ("tools/image/cli", true, "oer-tool-image-cli"),
        (
            "tools/image/large-values",
            true,
            "oer-tool-image-large-values",
        ),
    ] {
        assert_eq!(expected_name(path, executable), name);
    }
}

#[test]
fn compiler_imports_stay_in_the_reporter_and_its_private_bridge() {
    let manifest: toml::Value =
        toml::from_str(include_str!("../../../image/large-values/Cargo.toml"))
            .expect("the compiler reporter has a manifest");
    assert_eq!(
        manifest["package"]["name"].as_str(),
        Some(expected_name("tools/image/large-values", true).as_str())
    );
    let main = include_str!("../../../image/large-values/src/main.rs");
    assert!(main.contains("#![forbid(unsafe_code)]"));
    assert!(main.contains("#![feature(rustc_private)]"));
    for source in [
        main,
        include_str!("../../../image/large-values/src/report.rs"),
        include_str!("../../../image/large-values/build.rs"),
    ] {
        assert!(!source.contains("rustc_internal::"));
        for line in source
            .lines()
            .filter(|line| line.starts_with("extern crate rustc_"))
        {
            assert_eq!(line, "extern crate rustc_public;");
        }
    }
    let bridge = include_str!("../../../image/large-values/src/bridge.rs");
    assert!(bridge.contains("extern crate rustc_driver;"));
    assert!(!bridge.contains(".layout()") && !bridge.contains("instance_mir"));
}

/// The packages below the unsafe boundary (`docs/unsafe.md`), by directory
/// or, ending in `/`, by domain: the generated and the semantic PACs, the
/// HAL, the platform, executor and adapter bindings, the volatile
/// register-access core, the cores of memory the hardware shares, the host's operating-system bindings, the C-driver
/// stand, and the bootstrap and probe images. Every other package forbids
/// unsafe code.
const BELOW_THE_BOUNDARY: &[&str] = &[
    "chips/esp32s31/pac-raw",
    "chips/esp32s31/pac",
    "chips/esp32s31/hal",
    "chips/esp32s31/board",
    "chips/espressif/staged-bootstrap",
    "chips/espressif/staged-layout",
    "chips/espressif/staged-runtime",
    "chips/espressif/executor",
    "chips/espressif/esp-hal/",
    "portable/embassy/",
    "portable/memory",
    "portable/mmio",
    "portable/interrupt-table",
    "tools/os/process",
    "tools/os/serial",
    "tools/vendor/ieee802154-esp32s31",
    "firmware/bootstrap/",
    "firmware/probes/",
];

fn below_the_boundary(package: &str) -> bool {
    BELOW_THE_BOUNDARY
        .iter()
        .any(|entry| match entry.strip_suffix('/') {
            Some(domain) => package.starts_with(entry) || package == domain,
            None => package == *entry,
        })
}

#[test]
fn the_boundary_is_named_by_package_or_domain() {
    for package in [
        "chips/esp32s31/hal",
        "chips/espressif/staged-runtime",
        "portable/embassy/net",
        "tools/os/serial",
        "firmware/probes/esp32s31-radio",
    ] {
        assert!(below_the_boundary(package), "{package}");
    }
    for package in [
        "chips/esp32s31/bluetooth/driver",
        "chips/esp32s31/phy",
        "portable/trace",
        "tools/os/durable",
        "tools/hil/link",
        "firmware/apps/w1",
        "firmware/hil/esp32s31",
        "tools/lab/fixture-install",
    ] {
        assert!(!below_the_boundary(package), "{package}");
    }
}
