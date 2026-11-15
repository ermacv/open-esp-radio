//! Structural rules read from package manifests and sources: names, the
//! unsafe boundary, layer roles and their dependencies, protocol core/runtime
//! isolation from backends, and the HAL's atoms (`README.md`).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// The name of the package at `path`: `oer-` and its whole path, the root
/// included; the application entry (`oer/`) is `oer`.
fn expected_name(path: &str) -> String {
    if path == "oer" {
        return "oer".to_owned();
    }
    format!("oer-{}", path.replace('/', "-"))
}

#[test]
fn the_name_follows_the_path() {
    for (path, name) in [
        ("portable/time", "oer-portable-time"),
        (
            "protocols/ieee80211/core/roles/sta",
            "oer-protocols-ieee80211-core-roles-sta",
        ),
        (
            "chips/esp32s31/radio/ieee80211/backend",
            "oer-chips-esp32s31-radio-ieee80211-backend",
        ),
        (
            "hil/probes/esp32s31-boot-smoke",
            "oer-hil-probes-esp32s31-boot-smoke",
        ),
        ("hil/scenarios/aes", "oer-hil-scenarios-aes"),
        ("hil/services/aes", "oer-hil-services-aes"),
        ("hil/services/load", "oer-hil-services-load"),
        ("hil/scenarios/wifi-load", "oer-hil-scenarios-wifi-load"),
        (
            "hil/scenarios/wifi-station",
            "oer-hil-scenarios-wifi-station",
        ),
        ("hil/services/wifi-station", "oer-hil-services-wifi-station"),
        (
            "hil/scenarios/wifi-access-point",
            "oer-hil-scenarios-wifi-access-point",
        ),
        (
            "hil/services/wifi-access-point",
            "oer-hil-services-wifi-access-point",
        ),
        ("tools/image/build", "oer-tools-image-build"),
        ("tools/image/bundle", "oer-tools-image-bundle"),
        ("tools/vendor/scenario", "oer-tools-vendor-scenario"),
        ("tools/vendor/scenario-cli", "oer-tools-vendor-scenario-cli"),
        ("tools/image/cli", "oer-tools-image-cli"),
        ("tools/image/large-values", "oer-tools-image-large-values"),
    ] {
        assert_eq!(expected_name(path), name);
    }
}

#[test]
fn compiler_imports_stay_in_the_reporter_and_its_private_bridge() {
    let manifest: toml::Value =
        toml::from_str(include_str!("../../../image/large-values/Cargo.toml"))
            .expect("the compiler reporter has a manifest");
    assert_eq!(
        manifest["package"]["name"].as_str(),
        Some(expected_name("tools/image/large-values").as_str())
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
/// HAL, the platform and executor bindings, the volatile
/// register-access core, the cores of memory the hardware shares, the host's operating-system bindings, the C-driver
/// stand, and the bootstrap and probe images. Every other package forbids
/// unsafe code.
const BELOW_THE_BOUNDARY: &[&str] = &[
    "chips/esp32s31/pac",
    "chips/esp32s31/hal",
    "chips/esp32s31/platform/board",
    "chips/family/platform/bootstrap",
    "chips/family/platform/layout",
    "chips/family/platform/runtime",
    "chips/family/platform/executor",
    "portable/memory",
    "portable/mmio",
    "portable/riscv",
    "portable/waker",
    "chips/family/platform/interrupt-table",
    "tools/os/process",
    "tools/os/serial",
    "tools/vendor/ieee802154-esp32s31",
    "chips/esp32s31/platform/bootstrap",
    "hil/probes/",
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
        "chips/family/platform/runtime",
        "tools/os/serial",
        "hil/probes/esp32s31-radio",
    ] {
        assert!(below_the_boundary(package), "{package}");
    }
    for package in [
        "chips/esp32s31/radio/bluetooth/backend",
        "chips/esp32s31/radio/phy",
        "portable/trace",
        "portable/embassy/net",
        "portable/embassy/time",
        "portable/interrupt-table",
        "tools/os/durable",
        "tools/hil/link",
        "examples/wifi-station",
        "hil/images/aes",
        "tools/lab/fixture-install",
    ] {
        assert!(!below_the_boundary(package), "{package}");
    }
}

// The metadata-derived Espressif value/algorithm crates in docs/architecture.md.
const ESPRESSIF_SANS_IO: &[&str] = &[
    "chips/family/drivers/regi2c",
    "chips/family/boot",
    "chips/family/radio/coex",
    "chips/family/drivers/efuse",
    "chips/family/facts",
    "chips/family/radio/ieee80211/policy",
    "chips/family/radio/ieee802154/engine",
    "chips/family/radio/ieee802154/values",
    "chips/family/radio/modem-clock",
    "chips/family/radio/power",
    "chips/family/radio/phy",
    "chips/family/radio/bluetooth/ports",
    "chips/family/drivers/system-timer",
];

fn sans_io(package: &str) -> bool {
    package.starts_with("protocols/")
        || (package.starts_with("portable/")
            && ![
                "portable/mmio",
                "portable/riscv",
                "portable/memory",
                "portable/waker",
            ]
            .contains(&package))
        || ESPRESSIF_SANS_IO.contains(&package)
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root")
}

fn read_toml(path: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(path).expect("read TOML"))
        .expect("valid repository TOML")
}

fn packages(root: &Path) -> BTreeMap<String, toml::Value> {
    let manifest = read_toml(&root.join("Cargo.toml"));
    manifest["workspace"]["members"]
        .as_array()
        .expect("explicit workspace members")
        .iter()
        .map(|member| {
            let path = member.as_str().expect("member path");
            (
                path.to_owned(),
                read_toml(&root.join(path).join("Cargo.toml")),
            )
        })
        .collect()
}

// Library and binary crate roots, including explicit paths and Cargo's auto bins.
fn crate_roots(directory: &Path, manifest: &toml::Value) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    let library = manifest
        .get("lib")
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    if directory.join(library).is_file() {
        roots.insert(directory.join(library));
    }
    if manifest["package"]
        .get("autobins")
        .and_then(toml::Value::as_bool)
        != Some(false)
    {
        if directory.join("src/main.rs").is_file() {
            roots.insert(directory.join("src/main.rs"));
        }
        if let Ok(entries) = std::fs::read_dir(directory.join("src/bin")) {
            for entry in entries {
                let path = entry.expect("bin entry").path();
                if path.is_dir() && path.join("main.rs").is_file() {
                    roots.insert(path.join("main.rs"));
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    roots.insert(path);
                }
            }
        }
    }
    for bin in manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = bin.get("path").and_then(toml::Value::as_str) {
            roots.insert(directory.join(path));
        }
    }
    roots
}

// Inspect unconditional inner attributes only: comments, strings, nested modules
// and cfg_attr(test, ...) must not establish a production crate policy.
fn root_lint(source: &str, level: &str, lint: &str) -> bool {
    let tokens = source
        .parse::<TokenStream>()
        .expect("Rust root tokenizes")
        .into_iter()
        .collect::<Vec<_>>();
    tokens.windows(3).any(|window| {
        let [
            TokenTree::Punct(hash),
            TokenTree::Punct(bang),
            TokenTree::Group(attribute),
        ] = window
        else {
            return false;
        };
        if hash.as_char() != '#'
            || bang.as_char() != '!'
            || attribute.delimiter() != Delimiter::Bracket
        {
            return false;
        }
        let mut parts = attribute.stream().into_iter();
        let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(arguments))) =
            (parts.next(), parts.next())
        else {
            return false;
        };
        name == level
            && arguments.delimiter() == Delimiter::Parenthesis
            && arguments
                .stream()
                .to_string()
                .split(',')
                .any(|argument| argument.split_whitespace().collect::<String>() == lint)
    })
}

#[test]
fn every_crate_above_the_unsafe_boundary_forbids_unsafe_code() {
    let root = root();
    let mut missing = Vec::new();
    for (package, manifest) in packages(&root) {
        if below_the_boundary(&package) && !sans_io(&package) {
            continue;
        }
        let roots = crate_roots(&root.join(&package), &manifest);
        assert!(!roots.is_empty(), "{package}: no library or binary roots");
        for path in roots {
            let source = std::fs::read_to_string(&path).expect("crate root");
            if !root_lint(&source, "forbid", "unsafe_code") {
                missing.push(
                    path.strip_prefix(&root)
                        .expect("root is below repository")
                        .display()
                        .to_string(),
                );
            }
        }
    }
    assert!(
        missing.is_empty(),
        "missing #![forbid(unsafe_code)]:\n{}",
        missing.join("\n")
    );
}

#[test]
fn root_policy_is_an_unconditional_inner_attribute() {
    assert!(root_lint(
        "#![forbid(unused, unsafe_code)]",
        "forbid",
        "unsafe_code"
    ));
    assert!(root_lint(
        "#![deny(clippy::disallowed_methods)]",
        "deny",
        "clippy::disallowed_methods"
    ));
    for source in [
        "// #![forbid(unsafe_code)]",
        "const DOC: &str = \"#![forbid(unsafe_code)]\";",
        "mod nested { #![forbid(unsafe_code)] }",
        "#![cfg_attr(test, forbid(unsafe_code))]",
        "#[forbid(unsafe_code)] fn f() {}",
    ] {
        assert!(!root_lint(source, "forbid", "unsafe_code"));
    }
}

fn normal_dependencies(manifest: &toml::Value, workspace: &toml::Value) -> BTreeSet<String> {
    scoped_dependencies(manifest, workspace, "dependencies")
}

/// The packages of a manifest's `table` (`dependencies` or
/// `build-dependencies`), including optional, target-specific, aliased and
/// inherited entries.
fn scoped_dependencies(
    manifest: &toml::Value,
    workspace: &toml::Value,
    table: &str,
) -> BTreeSet<String> {
    let targets = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values());
    core::iter::once(manifest)
        .chain(targets)
        .filter_map(|scope| scope.get(table).and_then(toml::Value::as_table))
        .flat_map(|dependencies| dependencies.iter())
        .map(|(alias, spec)| {
            let spec = if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                workspace
                    .get("dependencies")
                    .and_then(|dependencies| dependencies.get(alias))
                    .expect("inherited dependency exists")
            } else {
                spec
            };
            spec.get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(alias)
                .to_owned()
        })
        .collect()
}

fn forbidden_dependency(name: &str) -> bool {
    name.split('-')
        .any(|part| ["fatal", "reset", "hal", "pac", "mmio", "riscv"].contains(&part))
        || name.contains("retained-record")
}

fn forbidden_paths(
    graph: &BTreeMap<String, BTreeSet<String>>,
    start: &str,
    forbidden: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut pending = vec![vec![start.to_owned()]];
    let mut visited = BTreeSet::new();
    let mut found = Vec::new();
    while let Some(path) = pending.pop() {
        let name = path.last().expect("nonempty dependency path");
        if !visited.insert(name.clone()) {
            continue;
        }
        if name != start && forbidden(name) {
            found.push(path.join(" -> "));
        } else if let Some(dependencies) = graph.get(name) {
            for dependency in dependencies {
                let mut next = path.clone();
                next.push(dependency.clone());
                pending.push(next);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn sans_io_has_no_normal_dependency_on_platform_or_terminal_fault_owners() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let packages = packages(&root);
    let graph = packages
        .values()
        .map(|manifest| {
            (
                manifest["package"]["name"]
                    .as_str()
                    .expect("package name")
                    .to_owned(),
                normal_dependencies(manifest, &workspace["workspace"]),
            )
        })
        .collect();
    let violations = packages
        .iter()
        .filter(|(path, _)| sans_io(path))
        .flat_map(|(_, manifest)| {
            forbidden_paths(
                &graph,
                manifest["package"]["name"].as_str().expect("name"),
                forbidden_dependency,
            )
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "forbidden normal dependencies:\n{}",
        violations.join("\n")
    );
}

#[test]
fn ieee802154_engine_has_no_terminal_custody_or_retained_policy() {
    let root = root();
    let packages = packages(&root);
    let workspace = read_toml(&root.join("Cargo.toml"));
    let graph = packages
        .values()
        .map(|manifest| {
            (
                manifest["package"]["name"]
                    .as_str()
                    .expect("package name")
                    .to_owned(),
                normal_dependencies(manifest, &workspace["workspace"]),
            )
        })
        .collect();
    assert!(
        forbidden_paths(&graph, "oer-chips-family-radio-ieee802154-engine", |name| {
            forbidden_dependency(name) || name == "oer-chips-family-platform-runtime"
        })
        .is_empty()
    );
    let mut pending = vec![root.join("chips/family/radio/ieee802154/engine/src")];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(path)
                    .expect("engine sources")
                    .map(|entry| entry.expect("source entry").path()),
            );
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path).expect("source text");
            for terminal in [
                "Quarantined",
                "FaultRecord",
                "retained_static",
                "reset_recording",
                "fatal::",
            ] {
                assert!(
                    !source.contains(terminal),
                    "{} contains {terminal}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn normal_dependency_graph_includes_aliases_inheritance_targets_and_optional_edges() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
alias = { workspace = true }
bridge = { version = "1", optional = true }
[dev-dependencies]
oer-portable-mmio = "1"
[build-dependencies]
oer-portable-riscv = "1"
[target.'cfg(target_os = "none")'.dependencies]
other = { package = "oer-chip-hal", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value =
        toml::from_str("[dependencies]\nalias = { package = 'oer-fatal', version = '1' }")
            .expect("workspace");
    assert_eq!(
        normal_dependencies(&manifest, &workspace),
        ["bridge", "oer-chip-hal", "oer-fatal"]
            .map(str::to_owned)
            .into()
    );
    let graph = BTreeMap::from([
        ("core".into(), BTreeSet::from(["bridge".into()])),
        (
            "bridge".into(),
            BTreeSet::from(["core".into(), "oer-retained-record".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "core", forbidden_dependency),
        ["core -> bridge -> oer-retained-record"]
    );
    assert!(forbidden_paths(&graph, "unrelated", forbidden_dependency).is_empty());
}

fn lint_paths(config: &toml::Value, key: &str) -> BTreeSet<String> {
    config
        .get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|entry| {
            entry
                .as_str()
                .or_else(|| entry.get("path").and_then(toml::Value::as_str))
                .expect("lint entry has a path")
                .to_owned()
        })
        .collect()
}

fn missing_lints(config: &toml::Value, required: &toml::Value, key: &str) -> BTreeSet<String> {
    lint_paths(required, key)
        .difference(&lint_paths(config, key))
        .cloned()
        .collect()
}

fn clippy_configs(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source tree") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            if ![".git", "target"].contains(&entry.file_name().to_string_lossy().as_ref()) {
                clippy_configs(&path, found);
            }
        } else if entry.file_name() == "clippy.toml" {
            found.push(path);
        }
    }
}

#[test]
fn every_clippy_config_retains_all_common_method_bans() {
    let root = root();
    let common = read_toml(&root.join("clippy.toml"));
    let mut configs = Vec::new();
    clippy_configs(&root, &mut configs);
    let missing = configs
        .into_iter()
        .filter_map(|path| {
            let missing = missing_lints(&read_toml(&path), &common, "disallowed-methods");
            (!missing.is_empty()).then(|| format!("{}: {missing:?}", path.display()))
        })
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "lost common method bans:\n{}",
        missing.join("\n")
    );
}

fn requires_sans_io_catalogue(source: &str) -> bool {
    ["clippy::disallowed_methods", "clippy::disallowed_macros"]
        .iter()
        .all(|lint| root_lint(source, "deny", lint))
}

#[test]
fn both_explicit_root_denies_mark_a_cleaned_sans_io_crate() {
    for source in [
        "#![deny(clippy::disallowed_methods, clippy::disallowed_macros)]",
        "#![deny(clippy::disallowed_methods)] #![deny(clippy::disallowed_macros)]",
    ] {
        assert!(requires_sans_io_catalogue(source));
    }
    for source in [
        "",
        "#![deny(clippy::disallowed_methods)]",
        "#![deny(clippy::disallowed_macros)]",
        "#![deny(clippy::disallowed_macros)] // #![deny(clippy::disallowed_methods)]",
        "#![deny(clippy::disallowed_macros)] mod nested { #![deny(clippy::disallowed_methods)] }",
    ] {
        assert!(!requires_sans_io_catalogue(source));
    }
}

#[test]
fn sans_io_crate_root_denies_require_the_complete_canonical_catalogue() {
    let root = root();
    let required: toml::Value =
        toml::from_str(include_str!("../sans-io-lints.toml")).expect("canonical lint data");
    let mut violations = Vec::new();
    for (package, manifest) in packages(&root)
        .iter()
        .filter(|(package, _)| sans_io(package))
    {
        let opted_in = crate_roots(&root.join(package), manifest)
            .into_iter()
            .any(|path| {
                let source = std::fs::read_to_string(path).expect("crate root");
                requires_sans_io_catalogue(&source)
            });
        if !opted_in {
            continue;
        }
        let path = root.join(package).join("clippy.toml");
        let config = if path.is_file() {
            read_toml(&path)
        } else {
            read_toml(&root.join("clippy.toml"))
        };
        for key in ["disallowed-methods", "disallowed-macros"] {
            let missing = missing_lints(&config, &required, key);
            if !missing.is_empty() {
                violations.push(format!(
                    "{package}: {key}: {} missing, e.g. {:?}",
                    missing.len(),
                    missing.iter().take(3).collect::<Vec<_>>()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "incomplete sans-IO catalogue:\n{}",
        violations.join("\n")
    );
}

#[test]
fn lint_catalogue_comparison_accepts_tables_and_strings_and_rejects_missing_paths() {
    let common: toml::Value = toml::from_str(
        "disallowed-methods = [{ path = 'oer_portable_mmio::assume_owned', reason = 'ownership' }]",
    )
    .expect("common config");
    let complete: toml::Value = toml::from_str(
        "disallowed-methods = ['oer_portable_mmio::assume_owned', 'slice::split_at']",
    )
    .expect("complete config");
    assert!(missing_lints(&complete, &common, "disallowed-methods").is_empty());
    let empty: toml::Value = toml::from_str("").expect("empty config");
    assert_eq!(
        missing_lints(&empty, &common, "disallowed-methods"),
        BTreeSet::from(["oer_portable_mmio::assume_owned".into()])
    );
}

/// Neither low-level HAL transactions nor chip facts may pull in the engine,
/// including through target-specific, optional or indirect normal dependencies.
#[test]
fn hal_and_knowledge_do_not_reach_the_ieee802154_engine_or_driver() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let graph = packages(&root)
        .values()
        .map(|manifest| {
            (
                manifest["package"]["name"]
                    .as_str()
                    .expect("name")
                    .to_owned(),
                normal_dependencies(manifest, &workspace["workspace"]),
            )
        })
        .collect();
    for owner in ["oer-chips-esp32s31-hal", "oer-chips-esp32s31-knowledge"] {
        let violations = forbidden_paths(&graph, owner, ieee802154_execution);
        assert!(violations.is_empty(), "{owner}: {violations:?}");
    }
}

fn ieee802154_execution(name: &str) -> bool {
    matches!(
        name,
        "oer-chips-family-radio-ieee802154-engine" | "oer-chips-esp32s31-radio-ieee802154-backend"
    )
}

#[test]
fn the_ieee802154_boundary_rejects_the_indirect_knowledge_path() {
    let graph = BTreeMap::from([
        ("hal".into(), BTreeSet::from(["knowledge".into()])),
        (
            "knowledge".into(),
            BTreeSet::from([
                "hal".into(),
                "oer-chips-family-radio-ieee802154-engine".into(),
            ]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "hal", ieee802154_execution),
        ["hal -> knowledge -> oer-chips-family-radio-ieee802154-engine"],
    );
    assert!(forbidden_paths(&graph, "unrelated", ieee802154_execution).is_empty());
}

/// The shared RF owner consumes client contracts, never the chip adapters of
/// the protocols, including optional, target-specific and transitive normal
/// dependencies.
#[test]
fn radio_systems_do_not_reach_protocol_drivers() {
    let graph = workspace_dependency_graph();
    let roles = workspace_roles();
    let services = roles
        .iter()
        .filter(|(_, role)| **role == Role::Service)
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !services.is_empty(),
        "the radio system has the service role"
    );
    for owner in services {
        let violations = forbidden_paths(&graph, owner, |name| {
            roles.get(name) == Some(&Role::ChipAdapter)
        });
        assert!(violations.is_empty(), "{owner}: {violations:?}");
    }
}

#[test]
fn radio_system_boundary_rejects_optional_target_and_transitive_driver_paths() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
bluetooth = { package = "bluetooth-driver", version = "1", optional = true }
client = "1"
[target.'cfg(target_arch = "riscv32")'.dependencies]
wifi = { workspace = true }
[dev-dependencies]
test-driver = "1"
[build-dependencies]
build-driver = "1"
"#,
    )
    .expect("manifest");
    let workspace: toml::Value =
        toml::from_str("[dependencies]\nwifi = { package = 'wifi-driver', version = '1' }")
            .expect("workspace");
    let graph = BTreeMap::from([
        ("system".into(), normal_dependencies(&manifest, &workspace)),
        (
            "client".into(),
            BTreeSet::from(["hal".into(), "phy".into()]),
        ),
        (
            "phy".into(),
            BTreeSet::from(["client".into(), "ieee802154-driver".into()]),
        ),
        ("allowed".into(), BTreeSet::from(["hal".into()])),
    ]);
    let roles = BTreeMap::from([
        ("bluetooth-driver", Role::ChipAdapter),
        ("wifi-driver", Role::ChipAdapter),
        ("ieee802154-driver", Role::ChipAdapter),
        ("client", Role::ServiceClient),
        ("phy", Role::HardwareDriver),
        ("hal", Role::Hal),
    ]);
    let driver = |name: &str| roles.get(name) == Some(&Role::ChipAdapter);
    assert_eq!(
        forbidden_paths(&graph, "system", driver),
        [
            "system -> bluetooth-driver",
            "system -> client -> phy -> ieee802154-driver",
            "system -> wifi-driver",
        ]
    );
    assert!(forbidden_paths(&graph, "allowed", driver).is_empty());
}

/// Protocol roles follow the leaf directory, including nested protocols such
/// as bluetooth/le. Shared radio, network and HIL vocabulary and the bridges
/// between formats are infrastructure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtocolRole {
    Wire,
    Cipher,
    /// A contract below the port and its backends, under `contracts/`: the
    /// owner of what a protocol installs and a backend programs.
    Contract,
    Port,
    Core,
    /// A backend conformance suite under `testing/`: cases and observation
    /// checks over the port, which backends and models run.
    Conformance,
    /// A testing package under `testing/` that joins a core with a port:
    /// the installation machine over a real port. It may take the
    /// installation core; the backends and the model take it as a
    /// dev-dependency only.
    CoreTesting,
    Model,
    Runtime,
}

fn protocol_role(path: &str) -> Option<ProtocolRole> {
    if ROLES
        .iter()
        .any(|(listed, role)| *listed == path && *role == Role::HilBinding)
    {
        return None;
    }
    let protocol = path.strip_prefix("protocols/")?;
    if ["radio", "network", "hil", "bridges"].contains(&protocol.split('/').next()?) {
        return None;
    }
    let leaf = protocol.rsplit('/').next()?;
    match (protocol.rsplit('/').nth(1), leaf) {
        // The port is the contract its backends implement; it keeps its role.
        (Some("contracts"), "port") => return Some(ProtocolRole::Port),
        (Some("contracts"), _) => return Some(ProtocolRole::Contract),
        (Some("testing"), "conformance") => return Some(ProtocolRole::Conformance),
        (Some("testing"), "installation") => return Some(ProtocolRole::CoreTesting),
        _ => {}
    }
    Some(match leaf {
        // A HIL binding is no protocol layer.
        "hil" => return None,
        "wire" => ProtocolRole::Wire,
        "cipher" => ProtocolRole::Cipher,
        "port" => ProtocolRole::Port,
        "model" => ProtocolRole::Model,
        "runtime" => ProtocolRole::Runtime,
        _ => ProtocolRole::Core,
    })
}

/// The layer of a workspace package (`docs/architecture.md`, Layers). Chip
/// and HIL packages take theirs from [`ROLES`], never from a path suffix; the
/// other packages follow their root and, in `protocols/`, their leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    /// Values, tables and pure functions about the hardware, without access
    /// to it.
    Facts,
    /// The generated register model and its one claim.
    Pac,
    /// Owners, partitions, typestate and single hardware atoms over the PAC.
    Hal,
    /// The operations of a block or subsystem: sequences, waits, errors and
    /// recovery, over a register port or HAL atoms. Family or chip.
    HardwareDriver,
    /// A chip's implementation of a protocol port or a family engine contract
    /// over its HAL: the protocol's backend on this chip.
    ChipAdapter,
    /// Family policy that sits above the backend with the protocol cores.
    CoreSidePolicy,
    /// The client contract of a chip service.
    ServiceClient,
    /// The one owner of a shared resource for its clients.
    Service,
    /// Family platform contracts that the HAL implements or names: the
    /// platform port, the interrupt table and the staged layout.
    PlatformContract,
    /// Startup order, staging, entry, executor and the board's platform data.
    Platform,
    /// The chip's side of an image: it may assemble runtimes and backends.
    Composition,
    /// A product facade over a protocol runtime and its chip backend.
    Facade,
    /// A family's application entry: for the chip its feature selects, it
    /// places drivers, services, facades and their tasks for an image.
    Sdk,
    /// An instrumentation event vocabulary: values, no owner.
    ObservationContract,
    /// The HIL framing, envelope and message skeleton.
    HilProtocol,
    /// The device-side HIL library: transport, services, lifecycle.
    HilAgent,
    /// The HIL dictionary beside one facade.
    HilBinding,
    /// The HIL agent on a family's application entry: the console, boot
    /// evidence and request hand-over of a test image built on the SDK.
    HilSdkAgent,
    /// A HIL test image.
    HilImage,
    Protocol(ProtocolRole),
    /// `protocols/radio/*` and `protocols/network/*`: vocabulary shared by the
    /// protocols.
    SharedVocabulary,
    /// `protocols/bridges/*`: the one place two protocols' codecs meet.
    Bridge,
    Portable,
    /// `portable/embassy/*`, the adapters of protocol boundaries.
    EmbassyAdapter,
    /// `portable/mmio` and `portable/riscv`: volatile register access and
    /// architectural instructions.
    HardwareAccess,
    Image,
    Tool,
}

/// The role of every chip and HIL package. A package under `chips/`,
/// `protocols/hil/` or `hil/` (but `hil/probes/`), or a `hil` leaf, is listed here.
const ROLES: &[(&str, Role)] = &[
    ("chips/family/facts", Role::Facts),
    ("chips/family/drivers/efuse", Role::Facts),
    ("chips/family/radio/ieee802154/values", Role::Facts),
    ("chips/esp32s31/knowledge", Role::Facts),
    ("chips/esp32s31/pac", Role::Pac),
    ("chips/esp32s31/hal", Role::Hal),
    ("chips/family/drivers/regi2c", Role::HardwareDriver),
    ("chips/family/boot", Role::HardwareDriver),
    ("chips/family/radio/coex", Role::HardwareDriver),
    ("chips/family/radio/ieee802154/engine", Role::HardwareDriver),
    ("chips/family/radio/modem-clock", Role::HardwareDriver),
    ("chips/family/radio/power", Role::HardwareDriver),
    ("chips/family/radio/phy", Role::HardwareDriver),
    ("chips/family/radio/bluetooth/ports", Role::HardwareDriver),
    ("chips/family/drivers/usb-serial-jtag", Role::HardwareDriver),
    ("chips/family/drivers/watchdog", Role::HardwareDriver),
    ("chips/family/drivers/system-timer", Role::HardwareDriver),
    ("chips/family/drivers/pmu", Role::HardwareDriver),
    ("chips/esp32s31/radio/phy", Role::HardwareDriver),
    ("chips/esp32s31/drivers/aes", Role::HardwareDriver),
    ("chips/esp32s31/radio/bluetooth/backend", Role::ChipAdapter),
    ("chips/esp32s31/radio/ieee80211/backend", Role::ChipAdapter),
    ("chips/esp32s31/radio/ieee802154/backend", Role::ChipAdapter),
    // docs/repository-layout.md: "Wi-Fi upper-MAC and the family policy
    // select each attempt's rate and protection" places this family policy
    // above the backend with the cores.
    ("chips/family/radio/ieee80211/policy", Role::CoreSidePolicy),
    ("chips/esp32s31/radio/client", Role::ServiceClient),
    ("chips/esp32s31/radio/system", Role::Service),
    (
        "chips/family/platform/interrupt-table",
        Role::PlatformContract,
    ),
    ("chips/family/platform/port", Role::PlatformContract),
    ("chips/family/platform/layout", Role::PlatformContract),
    ("chips/family/platform/executor", Role::Platform),
    ("chips/family/platform/bootstrap", Role::Platform),
    ("chips/family/platform/runtime", Role::Platform),
    ("chips/esp32s31/platform/board", Role::Platform),
    ("chips/esp32s31/platform/bootstrap", Role::Image),
    ("chips/esp32s31/platform/layout", Role::Platform),
    ("chips/esp32s31/platform/runtime", Role::Composition),
    ("chips/esp32s31/radio/ieee80211/station", Role::Facade),
    ("oer", Role::Sdk),
    ("chips/family/app", Role::Sdk),
    ("chips/esp32s31/app", Role::Sdk),
    ("protocols/hil/protocol", Role::HilProtocol),
    ("protocols/hil/agent", Role::HilAgent),
    ("hil/agent", Role::HilSdkAgent),
    ("hil/images/aes", Role::HilImage),
    ("hil/images/wifi-station", Role::HilImage),
    ("hil/images/wifi-access-point", Role::HilImage),
    ("hil/services/aes", Role::HilBinding),
    ("hil/services/fault", Role::HilBinding),
    ("hil/services/load", Role::HilBinding),
    ("hil/services/sdk-station", Role::HilBinding),
    ("hil/services/wifi-station", Role::HilBinding),
    ("hil/services/wifi-access-point", Role::HilBinding),
    ("hil/scenarios/aes", Role::Tool),
    ("hil/scenarios/sdk-station", Role::Tool),
    ("hil/scenarios/wifi-station", Role::Tool),
    ("hil/scenarios/wifi-load", Role::Tool),
    ("hil/scenarios/wifi-access-point", Role::Tool),
];

/// Roles fixed before their package exists, with what creates it. When the
/// package joins the workspace its entry moves to [`ROLES`].
const AWAITED_ROLES: &[(&str, Role, &str)] = &[];

/// Whether `path` is a package that [`ROLES`] must name.
fn listed_role_required(path: &str) -> bool {
    path.starts_with("chips/")
        || path.starts_with("protocols/hil/")
        || (path.starts_with("hil/") && !path.starts_with("hil/probes/"))
        || path.rsplit('/').next() == Some("hil")
}

/// Whether `path` is an image package: an example, a HIL image or probe, or
/// a chip's bootstrap.
fn image_path(path: &str) -> bool {
    ["examples/", "hil/images/", "hil/probes/"]
        .iter()
        .any(|root| path.starts_with(root))
        || (path.starts_with("chips/")
            && !path.starts_with("chips/family/")
            && path.ends_with("/bootstrap"))
}

fn role(path: &str) -> Option<Role> {
    if let Some(role) = ROLES
        .iter()
        .copied()
        .chain(AWAITED_ROLES.iter().map(|(path, role, _)| (*path, *role)))
        .find_map(|(listed, role)| (listed == path).then_some(role))
    {
        return Some(role);
    }
    if listed_role_required(path) {
        return None;
    }
    let mut parts = path.split('/');
    Some(match (parts.next(), parts.next()) {
        (Some("protocols"), Some("radio" | "network")) => Role::SharedVocabulary,
        (Some("protocols"), Some("bridges")) => Role::Bridge,
        (Some("protocols"), _) => Role::Protocol(protocol_role(path)?),
        (Some("portable"), Some("mmio" | "riscv")) => Role::HardwareAccess,
        (Some("portable"), Some("embassy")) => Role::EmbassyAdapter,
        (Some("portable"), _) => Role::Portable,
        (Some("examples"), _) | (Some("hil"), Some("probes")) => Role::Image,
        (Some("tools"), _) => Role::Tool,
        _ => return None,
    })
}

/// Whether a package of role `from` may take a direct normal dependency on
/// the workspace package at `to_path` of role `to`. Roles outside `chips/`
/// and HIL keep their own rules and are not limited here.
fn role_allows(from: Role, to: Role, to_path: &str) -> bool {
    use ProtocolRole::{Cipher, Contract, Core, Port, Runtime, Wire};
    let family_driver = to == Role::HardwareDriver && to_path.starts_with("chips/family/");
    // What a driver, an adapter or a service stands on.
    let below_a_driver = matches!(
        to,
        Role::Hal
            | Role::HardwareDriver
            | Role::PlatformContract
            | Role::Facts
            | Role::ObservationContract
            | Role::SharedVocabulary
            | Role::Portable
            | Role::Protocol(Wire | Port | Cipher | Contract)
    );
    // Everything a composition may assemble: runtimes and backends together.
    let assembled = matches!(
        to,
        Role::Hal
            | Role::HardwareDriver
            | Role::ChipAdapter
            | Role::CoreSidePolicy
            | Role::ServiceClient
            | Role::Service
            | Role::PlatformContract
            | Role::Platform
            | Role::Facts
            | Role::ObservationContract
            | Role::Protocol(_)
            | Role::SharedVocabulary
            | Role::Portable
            | Role::EmbassyAdapter
            | Role::HardwareAccess
    );
    match from {
        Role::Facts => matches!(
            to,
            Role::Facts | Role::SharedVocabulary | Role::Portable | Role::Protocol(Wire)
        ),
        Role::ObservationContract => matches!(
            to,
            Role::ObservationContract
                | Role::Facts
                | Role::SharedVocabulary
                | Role::Portable
                | Role::Protocol(Wire | Port)
        ),
        Role::Pac => matches!(to, Role::Facts | Role::Portable | Role::HardwareAccess),
        Role::PlatformContract => matches!(
            to,
            Role::PlatformContract | Role::Facts | Role::Portable | Role::HardwareAccess
        ),
        // The HAL implements the register ports of the family drivers; it
        // runs no chip driver and no protocol.
        Role::Hal => {
            family_driver
                || matches!(
                    to,
                    Role::Pac
                        | Role::PlatformContract
                        | Role::Facts
                        | Role::Portable
                        | Role::HardwareAccess
                )
        }
        Role::HardwareDriver | Role::ServiceClient => below_a_driver,
        Role::ChipAdapter => {
            below_a_driver || matches!(to, Role::ChipAdapter | Role::ServiceClient)
        }
        Role::Service => below_a_driver || to == Role::ServiceClient,
        Role::CoreSidePolicy => matches!(
            to,
            Role::Protocol(Wire | Port | Cipher | Contract | Core)
                | Role::Facts
                | Role::ObservationContract
                | Role::SharedVocabulary
                | Role::Portable
        ),
        // The platform reaches the chip only through its contracts.
        Role::Platform => {
            family_driver
                || matches!(
                    to,
                    Role::Platform
                        | Role::PlatformContract
                        | Role::Facts
                        | Role::SharedVocabulary
                        | Role::Portable
                        | Role::HardwareAccess
                )
        }
        Role::Composition => assembled,
        Role::Facade => assembled || to == Role::Composition,
        // The entry (`oer`) selects a chip's application crate, which builds
        // on the family's.
        Role::Sdk => assembled || matches!(to, Role::Composition | Role::Facade | Role::Sdk),
        Role::HilProtocol => to == Role::Portable,
        Role::HilAgent => matches!(to, Role::HilProtocol | Role::Portable),
        // It serves the HIL console over the SDK's console and evidence; the
        // composition writes notes before the console opens, the platform
        // places the console and names the image, and `oer-portable-riscv` raises the
        // fault a test requests.
        Role::HilSdkAgent => {
            matches!(
                to,
                Role::Sdk
                    | Role::HilAgent
                    | Role::HilProtocol
                    | Role::Platform
                    | Role::Composition
                    | Role::Portable
            ) || (to == Role::HardwareAccess && to_path == "portable/riscv")
        }
        // A bridge stands only on the codecs it joins.
        Role::Bridge => matches!(
            to,
            Role::Protocol(Wire) | Role::SharedVocabulary | Role::Portable
        ),
        // A binding is a dictionary without a clock: no time crate.
        Role::HilBinding => {
            matches!(
                to,
                Role::HilProtocol
                    | Role::HilAgent
                    | Role::Facade
                    | Role::ObservationContract
                    | Role::SharedVocabulary
                    | Role::Protocol(Wire | Port | Runtime)
            ) || (to == Role::Portable
                && !["portable/time", "portable/time-virtual"].contains(&to_path))
        }
        // A HIL image is an application of the entry: besides it, only the
        // HIL agent on the entry, the HIL protocol and agent and its
        // services' dictionaries.
        Role::HilImage => matches!(
            to,
            Role::Sdk
                | Role::HilSdkAgent
                | Role::HilBinding
                | Role::HilAgent
                | Role::HilProtocol
                | Role::Portable
                | Role::EmbassyAdapter
        ),
        Role::Protocol(_)
        | Role::SharedVocabulary
        | Role::Portable
        | Role::EmbassyAdapter
        | Role::HardwareAccess
        | Role::Image
        | Role::Tool => true,
    }
}

/// Whether a package of `role` keeps the backend rule: no normal path to a
/// protocol core or runtime. Only a composition and a facade assemble them.
fn keeps_the_backend_rule(role: Role) -> bool {
    matches!(
        role,
        Role::Facts
            | Role::Pac
            | Role::Hal
            | Role::HardwareDriver
            | Role::ChipAdapter
            | Role::ServiceClient
            | Role::Service
            | Role::PlatformContract
            | Role::Platform
            | Role::ObservationContract
            | Role::Protocol(ProtocolRole::Conformance | ProtocolRole::Model)
    )
}

fn core_or_runtime(role: Role) -> bool {
    matches!(
        role,
        Role::Protocol(ProtocolRole::Core | ProtocolRole::Runtime) | Role::CoreSidePolicy
    )
}

/// Each workspace package's name and role.
fn workspace_roles() -> BTreeMap<String, Role> {
    workspace_package_paths()
        .into_iter()
        .filter_map(|(name, path)| Some((name, role(&path)?)))
        .collect()
}

fn backend_core_paths(
    graph: &BTreeMap<String, BTreeSet<String>>,
    roles: &BTreeMap<String, Role>,
    start: &str,
) -> Vec<String> {
    forbidden_paths(graph, start, |name| {
        roles.get(name).copied().is_some_and(core_or_runtime)
    })
}

#[test]
fn backends_and_models_do_not_reach_protocol_cores_or_runtimes() {
    let graph = workspace_dependency_graph();
    let roles = workspace_roles();
    let violations = roles
        .iter()
        .filter(|(_, role)| keeps_the_backend_rule(**role))
        .flat_map(|(name, _)| backend_core_paths(&graph, &roles, name))
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "backend reaches core/runtime:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_chip_and_hil_package_has_a_listed_role() {
    let paths = workspace_package_paths()
        .into_values()
        .collect::<BTreeSet<_>>();
    let unlisted = paths
        .iter()
        .filter(|path| role(path).is_none())
        .cloned()
        .collect::<Vec<_>>();
    assert!(unlisted.is_empty(), "packages without a role: {unlisted:?}");
    let stale = ROLES
        .iter()
        .map(|(path, _)| *path)
        .filter(|path| !paths.contains(*path))
        .collect::<Vec<_>>();
    assert!(stale.is_empty(), "roles of no package: {stale:?}");
    let arrived = AWAITED_ROLES
        .iter()
        .map(|(path, _, _)| *path)
        .filter(|path| paths.contains(*path))
        .collect::<Vec<_>>();
    assert!(arrived.is_empty(), "move into ROLES: {arrived:?}");
    let listed = ROLES
        .iter()
        .map(|(path, _)| *path)
        .chain(AWAITED_ROLES.iter().map(|(path, _, _)| *path))
        .collect::<Vec<_>>();
    assert_eq!(
        listed.len(),
        listed.iter().collect::<BTreeSet<_>>().len(),
        "a package is listed twice"
    );
}

#[test]
fn protocol_roles_and_the_explicit_chip_roles_follow_the_layout() {
    for prefix in [
        "protocols/ieee802154",
        "protocols/bluetooth/le",
        "protocols/example",
    ] {
        for (directory, protocol) in [
            ("wire", ProtocolRole::Wire),
            ("cipher", ProtocolRole::Cipher),
            ("port", ProtocolRole::Port),
            ("model", ProtocolRole::Model),
            ("runtime", ProtocolRole::Runtime),
            ("mac", ProtocolRole::Core),
            ("controller", ProtocolRole::Core),
            ("contracts/security", ProtocolRole::Contract),
            ("contracts/keys", ProtocolRole::Contract),
            ("contracts/port", ProtocolRole::Port),
            ("testing/conformance", ProtocolRole::Conformance),
            ("testing/installation", ProtocolRole::CoreTesting),
            ("testing/model", ProtocolRole::Model),
            ("core/upper-mac", ProtocolRole::Core),
            ("core/security/cipher", ProtocolRole::Cipher),
        ] {
            let path = format!("{prefix}/{directory}");
            assert_eq!(protocol_role(&path), Some(protocol));
            assert_eq!(role(&path), Some(Role::Protocol(protocol)));
        }
        assert_eq!(role(&format!("{prefix}/hil")), None);
    }
    assert_eq!(
        protocol_role("protocols/bluetooth/hci"),
        Some(ProtocolRole::Core)
    );
    for path in [
        "protocols/radio/port",
        "protocols/hil/protocol",
        "protocols/network/port",
        "protocols/network/wire",
        "protocols/bridges/ieee80211-ethernet",
        "portable/wire",
        "chips/esp32s31/hal",
    ] {
        assert_eq!(protocol_role(path), None, "{path}");
    }
    for (path, expected) in [
        ("protocols/radio/port", Some(Role::SharedVocabulary)),
        ("protocols/network/port", Some(Role::SharedVocabulary)),
        ("protocols/network/wire", Some(Role::SharedVocabulary)),
        ("protocols/bridges/ieee80211-ethernet", Some(Role::Bridge)),
        ("protocols/hil/protocol", Some(Role::HilProtocol)),
        ("protocols/hil/agent", Some(Role::HilAgent)),
        ("protocols/hil/new", None),
        ("portable/wire", Some(Role::Portable)),
        ("portable/memory", Some(Role::Portable)),
        ("portable/mmio", Some(Role::HardwareAccess)),
        ("portable/riscv", Some(Role::HardwareAccess)),
        ("portable/embassy/net", Some(Role::EmbassyAdapter)),
        (
            "chips/family/radio/ieee80211/policy",
            Some(Role::CoreSidePolicy),
        ),
        (
            "chips/esp32s31/radio/ieee80211/backend",
            Some(Role::ChipAdapter),
        ),
        ("chips/esp32s31/radio/system", Some(Role::Service)),
        ("chips/esp32s31/platform/runtime", Some(Role::Composition)),
        ("chips/esp32s31/radio/ieee80211/station", Some(Role::Facade)),
        // A new chip package has no role until it is listed, whatever its
        // directory says.
        ("chips/esp32s31/new/driver", None),
        ("hil/images/aes", Some(Role::HilImage)),
        ("hil/new", None),
        ("hil/probes/esp32s31-boot-smoke", Some(Role::Image)),
        ("tools/hil/cli", Some(Role::Tool)),
    ] {
        assert_eq!(role(path), expected, "{path}");
    }
}

#[test]
fn backend_rule_rejects_transitive_cross_protocol_and_model_paths() {
    let roles = BTreeMap::from([
        ("wire".into(), Role::Protocol(ProtocolRole::Wire)),
        ("port".into(), Role::Protocol(ProtocolRole::Port)),
        ("cipher".into(), Role::Protocol(ProtocolRole::Cipher)),
        ("core".into(), Role::Protocol(ProtocolRole::Core)),
        ("other-core".into(), Role::Protocol(ProtocolRole::Core)),
        ("family-policy".into(), Role::CoreSidePolicy),
        ("runtime".into(), Role::Protocol(ProtocolRole::Runtime)),
    ]);
    let graph = BTreeMap::from([
        ("backend".into(), BTreeSet::from(["hal".into()])),
        ("hal".into(), BTreeSet::from(["coex".into()])),
        (
            "coex".into(),
            BTreeSet::from(["hal".into(), "other-core".into()]),
        ),
        ("model".into(), BTreeSet::from(["helper".into()])),
        ("helper".into(), BTreeSet::from(["runtime".into()])),
        (
            "backend-policy".into(),
            BTreeSet::from(["family-policy".into()]),
        ),
        (
            "allowed".into(),
            BTreeSet::from(["port".into(), "wire".into(), "cipher".into()]),
        ),
    ]);
    assert_eq!(
        backend_core_paths(&graph, &roles, "backend"),
        ["backend -> hal -> coex -> other-core"]
    );
    assert_eq!(
        backend_core_paths(&graph, &roles, "model"),
        ["model -> helper -> runtime"]
    );
    assert!(backend_core_paths(&graph, &roles, "allowed").is_empty());
    assert_eq!(
        backend_core_paths(&graph, &roles, "backend-policy"),
        ["backend-policy -> family-policy"]
    );
}

#[test]
fn backend_rule_checks_optional_target_aliases_and_excludes_dev_and_build_edges() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
optional = { package = "core", version = "1", optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
alias = { workspace = true }
[dev-dependencies]
model = "1"
[build-dependencies]
build-core = "1"
"#,
    )
    .expect("manifest");
    let workspace: toml::Value =
        toml::from_str("[dependencies]\nalias = { package = 'runtime', version = '1' }")
            .expect("workspace");
    let graph = BTreeMap::from([("backend".into(), normal_dependencies(&manifest, &workspace))]);
    let roles = BTreeMap::from([
        ("core".into(), Role::Protocol(ProtocolRole::Core)),
        ("runtime".into(), Role::Protocol(ProtocolRole::Runtime)),
        ("build-core".into(), Role::Protocol(ProtocolRole::Core)),
        ("model".into(), Role::Protocol(ProtocolRole::Model)),
    ]);
    assert_eq!(
        backend_core_paths(&graph, &roles, "backend"),
        ["backend -> core", "backend -> runtime"]
    );
}

#[test]
fn bluetooth_backend_core_edges_have_no_exemption() {
    let graph = BTreeMap::from([(
        "oer-chips-esp32s31-radio-bluetooth-backend".into(),
        BTreeSet::from([
            "oer-protocols-bluetooth-le-ll".into(),
            "oer-protocols-bluetooth-hci".into(),
            "oer-protocols-bluetooth-le-port".into(),
            "oer-protocols-bluetooth-le-wire".into(),
        ]),
    )]);
    let roles = BTreeMap::from([
        (
            "oer-protocols-bluetooth-le-ll".into(),
            Role::Protocol(ProtocolRole::Core),
        ),
        (
            "oer-protocols-bluetooth-hci".into(),
            Role::Protocol(ProtocolRole::Core),
        ),
        (
            "oer-protocols-bluetooth-le-port".into(),
            Role::Protocol(ProtocolRole::Port),
        ),
        (
            "oer-protocols-bluetooth-le-wire".into(),
            Role::Protocol(ProtocolRole::Wire),
        ),
    ]);
    assert_eq!(
        backend_core_paths(&graph, &roles, "oer-chips-esp32s31-radio-bluetooth-backend"),
        [
            "oer-chips-esp32s31-radio-bluetooth-backend -> oer-protocols-bluetooth-hci",
            "oer-chips-esp32s31-radio-bluetooth-backend -> oer-protocols-bluetooth-le-ll",
        ]
    );
}

/// The IEEE 802.11 cores above the lower-MAC port.
const IEEE80211_CORES: [&str; 7] = [
    "oer-protocols-ieee80211-core-upper-mac",
    "oer-protocols-ieee80211-core-security-rsn",
    "oer-protocols-ieee80211-core-roles-sta",
    "oer-protocols-ieee80211-core-roles-ap",
    "oer-protocols-ieee80211-core-arbiter",
    "oer-protocols-ieee80211-core-extensions-esp-now",
    "oer-protocols-ieee80211-runtime",
];

fn ieee80211_core(name: &str) -> bool {
    IEEE80211_CORES.contains(&name)
}

/// The lower-MAC port holds only the peripheral boundary, so no normal
/// dependency of it, optional, target-specific or transitive, reaches a core.
#[test]
fn the_ieee80211_port_reaches_no_core() {
    let graph = workspace_dependency_graph();
    for name in IEEE80211_CORES
        .iter()
        .chain(&["oer-protocols-ieee80211-contracts-port"])
    {
        assert!(graph.contains_key(*name), "{name} is a workspace package");
    }
    let violations = forbidden_paths(
        &graph,
        "oer-protocols-ieee80211-contracts-port",
        ieee80211_core,
    );
    assert!(violations.is_empty(), "port reaches a core: {violations:?}");
}

#[test]
fn the_ieee80211_port_rule_rejects_direct_and_transitive_core_paths() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
wire = { package = "oer-protocols-ieee80211-wire", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
policy = { package = "oer-protocols-ieee80211-core-roles-sta", version = "1", optional = true }
[dev-dependencies]
model = { package = "oer-protocols-ieee80211-core-upper-mac", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let graph = BTreeMap::from([
        (
            "oer-protocols-ieee80211-contracts-port".into(),
            normal_dependencies(&manifest, &workspace),
        ),
        (
            "oer-protocols-ieee80211-wire".into(),
            BTreeSet::from(["helper".into()]),
        ),
        (
            "helper".into(),
            BTreeSet::from(["oer-protocols-ieee80211-runtime".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(
            &graph,
            "oer-protocols-ieee80211-contracts-port",
            ieee80211_core
        ),
        [
            "oer-protocols-ieee80211-contracts-port -> oer-protocols-ieee80211-core-roles-sta",
            "oer-protocols-ieee80211-contracts-port -> oer-protocols-ieee80211-wire -> helper -> oer-protocols-ieee80211-runtime",
        ]
    );
    let clean = BTreeMap::from([(
        "oer-protocols-ieee80211-contracts-port".into(),
        BTreeSet::from([
            "oer-protocols-ieee80211-wire".into(),
            "oer-protocols-radio-port".into(),
        ]),
    )]);
    assert!(
        forbidden_paths(
            &clean,
            "oer-protocols-ieee80211-contracts-port",
            ieee80211_core
        )
        .is_empty()
    );
}

/// What the IEEE 802.11 wire formats may not reach: time, which belongs to
/// the cores and runtimes, and every IEEE 802.11 crate above the formats.
fn above_ieee80211_wire(name: &str) -> bool {
    name == "oer-portable-time"
        || (name.starts_with("oer-protocols-ieee80211-") && name != "oer-protocols-ieee80211-wire")
}

fn workspace_dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    packages(&root)
        .values()
        .map(|manifest| {
            (
                manifest["package"]["name"]
                    .as_str()
                    .expect("name")
                    .to_owned(),
                normal_dependencies(manifest, &workspace["workspace"]),
            )
        })
        .collect()
}

/// The wire crate holds only formats and standard constants: no normal
/// dependency of it, optional, target-specific or transitive, reaches time
/// or an IEEE 802.11 crate above it.
#[test]
fn the_ieee80211_wire_reaches_neither_time_nor_a_crate_above_it() {
    let graph = workspace_dependency_graph();
    for name in [
        "oer-protocols-ieee80211-wire",
        "oer-protocols-ieee80211-contracts-port",
        "oer-portable-time",
    ] {
        assert!(graph.contains_key(name), "{name} is a workspace package");
    }
    let violations = forbidden_paths(&graph, "oer-protocols-ieee80211-wire", above_ieee80211_wire);
    assert!(
        violations.is_empty(),
        "wire reaches time or a crate above it: {violations:?}"
    );
}

#[test]
fn the_ieee80211_wire_rule_rejects_direct_and_transitive_time_and_upper_paths() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
format = { package = "oer-portable-wire", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
clock = { package = "oer-portable-time", version = "1", optional = true }
[dev-dependencies]
model = { package = "oer-protocols-ieee80211-testing-model", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let graph = BTreeMap::from([
        (
            "oer-protocols-ieee80211-wire".into(),
            normal_dependencies(&manifest, &workspace),
        ),
        (
            "oer-portable-wire".into(),
            BTreeSet::from(["helper".into()]),
        ),
        (
            "helper".into(),
            BTreeSet::from(["oer-protocols-ieee80211-contracts-port".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "oer-protocols-ieee80211-wire", above_ieee80211_wire),
        [
            "oer-protocols-ieee80211-wire -> oer-portable-time",
            "oer-protocols-ieee80211-wire -> oer-portable-wire -> helper -> oer-protocols-ieee80211-contracts-port",
        ]
    );
    let clean = BTreeMap::from([(
        "oer-protocols-ieee80211-wire".into(),
        BTreeSet::from([
            "oer-portable-wire".into(),
            "oer-protocols-radio-regulatory".into(),
        ]),
    )]);
    assert!(
        forbidden_paths(&clean, "oer-protocols-ieee80211-wire", above_ieee80211_wire).is_empty()
    );
}

/// Each workspace package's name and its directory below the root.
fn workspace_package_paths() -> BTreeMap<String, String> {
    packages(&root())
        .into_iter()
        .filter_map(|(path, manifest)| {
            let name = manifest.get("package")?.get("name")?.as_str()?;
            Some((name.to_owned(), path))
        })
        .collect()
}

/// The workspace packages among `dependencies` outside `allowed`, judged by
/// name or by directory.
fn disallowed_workspace_dependencies(
    dependencies: &BTreeSet<String>,
    paths: &BTreeMap<String, String>,
    allowed: impl Fn(&str, &str) -> bool,
) -> Vec<String> {
    dependencies
        .iter()
        .filter_map(|name| {
            let path = paths.get(name)?;
            (!allowed(name, path)).then(|| format!("{name} ({path})"))
        })
        .collect()
}

fn portable_or_radio(path: &str) -> bool {
    path.starts_with("portable/") || path.starts_with("protocols/radio/")
}

/// The layer table of `docs/repository-layout.md`: the frame cipher takes
/// only the wire formats, the security contract and portable support; the
/// port takes the wire formats, the security contract, portable support and
/// the radio vocabulary; the conformance suite takes the port, the formats
/// and portable support; the reference model takes the port, its
/// conformance suite, the formats, the contract and the cipher beside them.
fn ieee80211_layer_allows(package: &str, name: &str, path: &str) -> bool {
    const CONTRACT: &str = "oer-protocols-ieee80211-contracts-security";
    match package {
        "oer-protocols-ieee80211-core-security-cipher" => {
            ["oer-protocols-ieee80211-wire", CONTRACT].contains(&name)
                || path.starts_with("portable/")
        }
        "oer-protocols-ieee80211-contracts-port" => {
            ["oer-protocols-ieee80211-wire", CONTRACT].contains(&name) || portable_or_radio(path)
        }
        "oer-protocols-ieee80211-testing-conformance" => {
            [
                "oer-protocols-ieee80211-contracts-port",
                "oer-protocols-ieee80211-wire",
            ]
            .contains(&name)
                || path.starts_with("portable/")
        }
        "oer-protocols-ieee80211-testing-model" => {
            [
                "oer-protocols-ieee80211-contracts-port",
                "oer-protocols-ieee80211-testing-conformance",
                "oer-protocols-ieee80211-wire",
                "oer-protocols-ieee80211-core-security-cipher",
                CONTRACT,
            ]
            .contains(&name)
                || portable_or_radio(path)
        }
        _ => true,
    }
}

#[test]
fn ieee80211_cipher_port_and_model_take_only_the_layers_below_them() {
    let graph = workspace_dependency_graph();
    let paths = workspace_package_paths();
    for package in [
        "oer-protocols-ieee80211-core-security-cipher",
        "oer-protocols-ieee80211-contracts-port",
        "oer-protocols-ieee80211-testing-conformance",
        "oer-protocols-ieee80211-testing-model",
    ] {
        let dependencies = graph.get(package).expect("workspace package");
        let violations = disallowed_workspace_dependencies(dependencies, &paths, |name, path| {
            ieee80211_layer_allows(package, name, path)
        });
        assert!(
            violations.is_empty(),
            "{package} depends on a layer above or beside it: {violations:?}"
        );
    }
}

#[test]
fn ieee80211_layer_table_rejects_cores_chips_and_a_cipher_on_the_port() {
    let paths = BTreeMap::from([
        (
            "oer-protocols-ieee80211-wire".into(),
            "protocols/ieee80211/wire".into(),
        ),
        (
            "oer-protocols-ieee80211-contracts-port".into(),
            "protocols/ieee80211/contracts/port".into(),
        ),
        (
            "oer-protocols-ieee80211-core-security-cipher".into(),
            "protocols/ieee80211/core/security/cipher".into(),
        ),
        (
            "oer-protocols-ieee80211-core-roles-sta".into(),
            "protocols/ieee80211/core/roles/sta".into(),
        ),
        (
            "oer-protocols-radio-port".into(),
            "protocols/radio/port".into(),
        ),
        ("oer-portable-wire".into(), "portable/wire".into()),
        ("oer-chips-esp32s31-hal".into(), "chips/esp32s31/hal".into()),
    ]);
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
format = { package = "oer-protocols-ieee80211-wire", version = "1" }
port = { package = "oer-protocols-ieee80211-contracts-port", version = "1" }
support = { package = "oer-portable-wire", version = "1" }
aes = "0.8"
[target.'cfg(target_arch = "riscv32")'.dependencies]
hal = { package = "oer-chips-esp32s31-hal", version = "1", optional = true }
[dev-dependencies]
station = { package = "oer-protocols-ieee80211-core-roles-sta", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let dependencies = normal_dependencies(&manifest, &workspace);
    assert_eq!(
        disallowed_workspace_dependencies(&dependencies, &paths, |name, path| {
            ieee80211_layer_allows("oer-protocols-ieee80211-core-security-cipher", name, path)
        }),
        [
            "oer-chips-esp32s31-hal (chips/esp32s31/hal)",
            "oer-protocols-ieee80211-contracts-port (protocols/ieee80211/contracts/port)",
        ]
    );
    let model = BTreeSet::from([
        "oer-protocols-ieee80211-contracts-port".into(),
        "oer-protocols-ieee80211-core-security-cipher".into(),
        "oer-protocols-radio-port".into(),
        "oer-protocols-ieee80211-core-roles-sta".into(),
    ]);
    assert_eq!(
        disallowed_workspace_dependencies(&model, &paths, |name, path| {
            ieee80211_layer_allows("oer-protocols-ieee80211-testing-model", name, path)
        }),
        ["oer-protocols-ieee80211-core-roles-sta (protocols/ieee80211/core/roles/sta)"]
    );
}

/// What the Wi-Fi runtime reaches by no normal path: chip crates (their
/// HALs, drivers and executor binding among them), an executor and Embassy
/// time. It binds no executor and takes time from `oer-portable-time`.
fn below_the_ieee80211_runtime(paths: &BTreeMap<String, String>, name: &str) -> bool {
    paths
        .get(name)
        .is_some_and(|path| path.starts_with("chips/"))
        || ["embassy-time", "embassy-executor"].contains(&name)
}

/// The air formats and frame protection the runtime never names: the cores
/// parse, build and protect frames and hand it finished actions.
const BESIDE_THE_IEEE80211_RUNTIME: [&str; 2] = [
    "oer-protocols-ieee80211-wire",
    "oer-protocols-ieee80211-core-security-cipher",
];

fn runtime_direct_violations(dependencies: &BTreeSet<String>) -> Vec<String> {
    dependencies
        .iter()
        .filter(|name| BESIDE_THE_IEEE80211_RUNTIME.contains(&name.as_str()))
        .cloned()
        .collect()
}

#[test]
fn the_ieee80211_runtime_takes_neither_formats_cipher_chips_nor_an_executor() {
    let graph = workspace_dependency_graph();
    let paths = workspace_package_paths();
    let direct = graph
        .get("oer-protocols-ieee80211-runtime")
        .expect("the runtime is a workspace package");
    assert!(
        runtime_direct_violations(direct).is_empty(),
        "the runtime depends on formats or the cipher: {:?}",
        runtime_direct_violations(direct)
    );
    let violations = forbidden_paths(&graph, "oer-protocols-ieee80211-runtime", |name| {
        below_the_ieee80211_runtime(&paths, name)
    });
    assert!(
        violations.is_empty(),
        "the runtime reaches a chip, an executor or Embassy time: {violations:?}"
    );
}

#[test]
fn the_ieee80211_runtime_rule_rejects_formats_and_transitive_chip_and_time_paths() {
    let paths = BTreeMap::from([(
        "oer-chips-family-platform-executor".into(),
        "chips/family/platform/executor".into(),
    )]);
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
format = { package = "oer-protocols-ieee80211-wire", version = "1" }
station = { package = "oer-protocols-ieee80211-core-roles-sta", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
clock = { package = "embassy-time", version = "0.5", optional = true }
[dev-dependencies]
protect = { package = "oer-protocols-ieee80211-core-security-cipher", version = "1" }
policy = { package = "oer-chips-family-radio-ieee80211-policy", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let direct = normal_dependencies(&manifest, &workspace);
    assert_eq!(
        runtime_direct_violations(&direct),
        ["oer-protocols-ieee80211-wire"]
    );
    let graph = BTreeMap::from([
        ("oer-protocols-ieee80211-runtime".into(), direct),
        (
            "oer-protocols-ieee80211-core-roles-sta".into(),
            BTreeSet::from(["helper".into()]),
        ),
        (
            "helper".into(),
            BTreeSet::from(["oer-chips-family-platform-executor".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "oer-protocols-ieee80211-runtime", |name| {
            below_the_ieee80211_runtime(&paths, name)
        }),
        [
            "oer-protocols-ieee80211-runtime -> embassy-time",
            "oer-protocols-ieee80211-runtime -> oer-protocols-ieee80211-core-roles-sta -> helper -> oer-chips-family-platform-executor",
        ]
    );
}

/// Whether the protocol package at `path` must never wait: every package
/// under `protocols/` but a runtime, a port, a model, an agent, a package
/// under `testing/` and the radio port. They take inputs and return
/// actions as values; the set is computed, so a new core is covered the day
/// it appears.
fn must_not_wait(path: &str) -> bool {
    let Some(below) = path.strip_prefix("protocols/") else {
        return false;
    };
    let leaf = below.rsplit('/').next().unwrap_or(below);
    !["runtime", "port", "model", "agent"].contains(&leaf)
        && !below.split('/').any(|part| part == "testing")
        && path != "protocols/radio/port"
}

/// The production files of a package that must not wait which still do,
/// with their exact marker counts and the change that removes each. The
/// list only shrinks: a lower count lowers its entry, and a file without
/// markers leaves it. No new file belongs here.
const WAITING_DEVIATIONS: &[(&str, usize, &str)] = &[
    (
        "protocols/network/core/src/owner.rs",
        2,
        "the network owner in the network runtime (phase C): `register_port` takes a `Context`",
    ),
    (
        "protocols/hil/protocol/src/io.rs",
        1,
        "the async `write_frame` out of the wire protocol, to the HIL agent's transport; it has no caller in the workspace",
    ),
];

/// The identifiers of waiting: an `async` item and the task vocabulary.
const WAITING: [&str; 5] = ["async", "Future", "Poll", "Waker", "Context"];

/// Whether an outer attribute's tokens configure its item for tests only:
/// `cfg(test)` or a `cfg(all(test, ...))`.
fn test_only(attribute: &TokenStream) -> bool {
    let mut tokens = attribute.clone().into_iter();
    let (Some(TokenTree::Ident(name)), Some(TokenTree::Group(arguments))) =
        (tokens.next(), tokens.next())
    else {
        return false;
    };
    if name != "cfg" {
        return false;
    }
    let mut predicate = arguments.stream().into_iter();
    match (predicate.next(), predicate.next()) {
        (Some(TokenTree::Ident(test)), None) => test == "test",
        (Some(TokenTree::Ident(all)), Some(TokenTree::Group(terms))) if all == "all" => terms
            .stream()
            .into_iter()
            .any(|term| matches!(term, TokenTree::Ident(term) if term == "test")),
        _ => false,
    }
}

/// The `#[path = "..."]` of an outer attribute.
fn path_attribute(attribute: &TokenStream) -> Option<String> {
    let mut tokens = attribute.clone().into_iter();
    match (tokens.next(), tokens.next(), tokens.next()) {
        (
            Some(TokenTree::Ident(name)),
            Some(TokenTree::Punct(equals)),
            Some(TokenTree::Literal(path)),
        ) if name == "path" && equals.as_char() == '=' => {
            Some(path.to_string().trim_matches('"').to_owned())
        }
        _ => None,
    }
}

/// Whether an identifier marks waiting or sequencing for a scan. It is
/// asked of each identifier, and of `Type::name` where the identifier
/// follows a path's `::` after another one.
type Marker = dyn Fn(&str) -> bool;

/// What a walk of production code shows its visitor: a token at its index
/// in its group, the attribute of a production item, or a module file it
/// could not read.
enum Seen<'a> {
    Token(&'a [TokenTree], usize),
    Attribute(&'a TokenStream),
    Unreadable,
}

/// A walk's visitor: what it was shown, with the file it is in.
type Visit<'v> = dyn FnMut(&Path, Seen<'_>) + 'v;

/// Walk the production tokens of one module body: items configured for
/// tests are skipped; out-of-line production modules are followed from
/// `directory`. A `#[path]` at the top of a file is relative to the file's
/// own directory, inside an inline module to that module's directory. Each
/// token is shown to `visit` before its group is walked, and each attribute
/// of a production item before the item.
/// Where a walked module body is: a file's top, whose `#[path]` is relative
/// to the file's own directory, or an inline module.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nesting {
    FileTop,
    Inline,
}

fn walk(tokens: TokenStream, directory: &Path, file: &Path, nesting: Nesting, visit: &mut Visit) {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    let mut index = 0;
    let mut attributes = Vec::new();
    while let Some(token) = tokens.get(index) {
        index += 1;
        match token {
            TokenTree::Punct(hash) if hash.as_char() == '#' => {
                if let Some(TokenTree::Group(attribute)) = tokens.get(index)
                    && attribute.delimiter() == Delimiter::Bracket
                {
                    attributes.push(attribute.stream());
                    index += 1;
                    continue;
                }
            }
            // A visibility keeps the item's attributes: `#[cfg(test)] pub mod`.
            TokenTree::Ident(word) if word == "pub" => {
                if let Some(TokenTree::Group(scope)) = tokens.get(index)
                    && scope.delimiter() == Delimiter::Parenthesis
                {
                    index += 1;
                }
                continue;
            }
            TokenTree::Ident(word) if word == "mod" => {
                let test = attributes.iter().any(test_only);
                let path = attributes.iter().find_map(path_attribute);
                attributes.clear();
                let (Some(TokenTree::Ident(name)), Some(body)) =
                    (tokens.get(index), tokens.get(index + 1))
                else {
                    continue;
                };
                index += 2;
                if test {
                    continue;
                }
                match body {
                    TokenTree::Group(inline) if inline.delimiter() == Delimiter::Brace => {
                        walk(
                            inline.stream(),
                            &directory.join(name.to_string()),
                            file,
                            Nesting::Inline,
                            visit,
                        );
                    }
                    _ => {
                        let child = match path {
                            Some(path) if nesting == Nesting::FileTop => {
                                file.parent().unwrap_or(directory).join(path)
                            }
                            Some(path) => directory.join(path),
                            None if directory.join(format!("{name}.rs")).is_file() => {
                                directory.join(format!("{name}.rs"))
                            }
                            None => directory.join(name.to_string()).join("mod.rs"),
                        };
                        walk_file(&child, visit);
                    }
                }
                continue;
            }
            _ => {}
        }
        if attributes.iter().any(test_only) {
            // Skip the test-only item: through its first brace body or `;`.
            let mut current = Some(token);
            while let Some(token) = current {
                match token {
                    TokenTree::Group(body) if body.delimiter() == Delimiter::Brace => break,
                    TokenTree::Punct(end) if end.as_char() == ';' => break,
                    _ => {}
                }
                current = tokens.get(index);
                index += 1;
            }
            attributes.clear();
            continue;
        }
        for attribute in &attributes {
            visit(file, Seen::Attribute(attribute));
        }
        attributes.clear();
        visit(file, Seen::Token(&tokens, index - 1));
        // Item bodies (`impl`, `fn`, blocks) skip their test-only items too.
        if let TokenTree::Group(group) = token {
            walk(group.stream(), directory, file, Nesting::Inline, visit);
        }
    }
}

/// Walk a module file: `lib.rs` and `mod.rs` own their directory; another
/// file `name.rs` owns `name/`.
fn walk_file(file: &Path, visit: &mut Visit) {
    let tokens = std::fs::read_to_string(file)
        .ok()
        .and_then(|source| source.parse::<TokenStream>().ok());
    let (Some(tokens), Some(stem), Some(parent)) = (tokens, file.file_stem(), file.parent()) else {
        // A module the scan cannot read is reported, never skipped.
        visit(file, Seen::Unreadable);
        return;
    };
    let stem = stem.to_string_lossy();
    let directory = if ["lib", "mod", "main"].contains(&stem.as_ref()) {
        parent.to_path_buf()
    } else {
        parent.join(stem.as_ref())
    };
    walk(tokens, &directory, file, Nesting::FileTop, visit);
}

/// Walk a crate root: it owns its directory, whatever its name (a binary's
/// `src/hardware.rs` declares its modules beside it).
fn walk_crate_root(file: &Path, visit: &mut Visit) {
    let tokens = std::fs::read_to_string(file)
        .ok()
        .and_then(|source| source.parse::<TokenStream>().ok());
    let (Some(tokens), Some(parent)) = (tokens, file.parent()) else {
        visit(file, Seen::Unreadable);
        return;
    };
    walk(tokens, parent, file, Nesting::FileTop, visit);
}

/// The identifiers `marker` accepts in a walk, each with its file; a module
/// the walk could not read is reported, never skipped.
fn markers<'m>(marker: &'m Marker, found: &'m mut Vec<String>) -> impl FnMut(&Path, Seen<'_>) + 'm {
    move |file, seen| match seen {
        Seen::Unreadable => found.push(format!("{}: unreadable module", file.display())),
        Seen::Attribute(_) => {}
        Seen::Token(tokens, at) => {
            let Some(token) = tokens.get(at) else {
                return;
            };
            // The path's last two segments, when `word` follows `Type::`.
            let qualified = match (
                at.checked_sub(3).and_then(|at| tokens.get(at)),
                at.checked_sub(2).and_then(|at| tokens.get(at)),
                at.checked_sub(1).and_then(|at| tokens.get(at)),
                token,
            ) {
                (
                    Some(TokenTree::Ident(owner)),
                    Some(TokenTree::Punct(first)),
                    Some(TokenTree::Punct(second)),
                    TokenTree::Ident(word),
                ) if first.as_char() == ':' && second.as_char() == ':' => {
                    Some(format!("{owner}::{word}"))
                }
                _ => None,
            };
            match token {
                TokenTree::Ident(word) if marker(&word.to_string()) => {
                    found.push(format!("{}: {word}", file.display()));
                }
                TokenTree::Ident(_) if qualified.as_deref().is_some_and(marker) => {
                    let qualified = qualified.unwrap_or_default();
                    found.push(format!("{}: {qualified}", file.display()));
                }
                _ => {}
            }
        }
    }
}

/// The identifiers `marker` accepts in the production module `file` and the
/// modules it reaches.
fn production_file_markers(file: &Path, marker: &Marker, found: &mut Vec<String>) {
    walk_file(file, &mut markers(marker, found));
}

/// The identifiers `marker` accepts from a crate root.
fn crate_root_markers(file: &Path, marker: &Marker, found: &mut Vec<String>) {
    walk_crate_root(file, &mut markers(marker, found));
}

fn waiting(word: &str) -> bool {
    WAITING.contains(&word)
}

#[test]
fn protocol_cores_never_wait_outside_their_tests() {
    let root = root();
    let mut found = BTreeMap::new();
    let mut cores = 0;
    for (path, manifest) in packages(&root)
        .into_iter()
        .filter(|(path, _)| must_not_wait(path))
    {
        cores += 1;
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            found.extend(marker_counts(&root, &crate_root, &waiting));
        }
    }
    // The rule takes the cores the tree has today, and more as they come.
    assert!(cores >= 22, "only {cores} packages must not wait");
    let known = WAITING_DEVIATIONS
        .iter()
        .map(|(file, count, _)| (*file, *count))
        .collect::<Vec<_>>();
    let differences = count_differences(&found, &known);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn every_protocol_package_but_runtimes_ports_models_and_tests_must_not_wait() {
    for core in [
        "protocols/ieee80211/core/upper-mac",
        "protocols/ieee80211/wire",
        "protocols/network/core",
        "protocols/bridges/ieee80211-ethernet",
        "protocols/radio/coex",
        "protocols/hil/protocol",
        "protocols/ieee80211/contracts/security",
        // A core that does not exist yet is taken the day it appears.
        "protocols/zigbee/nwk",
    ] {
        assert!(must_not_wait(core), "{core}");
    }
    for other in [
        "protocols/ieee80211/runtime",
        "protocols/network/port",
        "protocols/bluetooth/le/model",
        "protocols/hil/agent",
        "protocols/ieee80211/testing/conformance",
        "protocols/network/testing/model",
        "protocols/radio/port",
        "portable/time",
        "chips/esp32s31/hal",
    ] {
        assert!(!must_not_wait(other), "{other}");
    }
}

#[test]
fn the_waiting_scan_skips_tests_and_follows_production_modules() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("src");
    std::fs::create_dir_all(source.join("machine")).expect("module directory");
    std::fs::write(
        source.join("lib.rs"),
        r#"
pub mod machine;
#[cfg(test)]
mod tests;
#[cfg(all(test, not(target_arch = "riscv32")))]
fn fixture(cx: &mut core::task::Context<'_>) {}
#[cfg(test)]
use core::task::Waker;
// A Future in a comment, and "async" in a string, are not code.
pub const NAME: &str = "async";
"#,
    )
    .expect("root");
    std::fs::write(
        source.join("machine.rs"),
        "pub struct StaTxContext;\nmod inner { pub async fn wait() {} }\n#[path = \"beside.rs\"]\npub(crate) mod beside;\n#[cfg(test)]\npub mod checks { async fn f() {} }\nimpl StaTxContext {\n    #[cfg(test)]\n    pub(crate) fn fixture() { loop {} }\n    pub fn live(&self) -> Poll { todo!() }\n}\n",
    )
    .expect("module");
    std::fs::write(source.join("beside.rs"), "pub fn poll_beside() -> Poll {}")
        .expect("module beside its parent");
    std::fs::write(
        source.join("tests.rs"),
        "fn poll() -> core::task::Poll<()> { core::task::Poll::Pending }",
    )
    .expect("tests");
    let mut found = Vec::new();
    production_file_markers(&source.join("lib.rs"), &waiting, &mut found);
    assert_eq!(
        found,
        [
            format!("{}: async", source.join("machine.rs").display()),
            format!("{}: Poll", source.join("beside.rs").display()),
            format!("{}: Poll", source.join("machine.rs").display()),
        ]
    );
}

/// A workspace package as the structure rules see it: its directory below the
/// root, its name, whether it builds a program, and the packages of its
/// normal dependencies (optional and target-specific ones included).
struct Member {
    path: String,
    name: String,
    executable: bool,
    dependencies: BTreeSet<String>,
    build_dependencies: BTreeSet<String>,
}

/// Whether the package has a binary target: `src/main.rs`, `src/bin/*` or a
/// `[[bin]]` entry.
fn has_binary(directory: &Path, manifest: &toml::Value) -> bool {
    let autobins = manifest
        .get("package")
        .and_then(|package| package.get("autobins"))
        .and_then(toml::Value::as_bool)
        != Some(false);
    let automatic = autobins
        && (directory.join("src/main.rs").is_file()
            || std::fs::read_dir(directory.join("src/bin"))
                .is_ok_and(|mut entries| entries.next().is_some()));
    automatic
        || manifest
            .get("bin")
            .and_then(toml::Value::as_array)
            .is_some_and(|bins| !bins.is_empty())
}

fn workspace_members() -> Vec<Member> {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"))
        .get("workspace")
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));
    packages(&root)
        .into_iter()
        .map(|(path, manifest)| Member {
            executable: has_binary(&root.join(&path), &manifest),
            name: manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            dependencies: normal_dependencies(&manifest, &workspace),
            build_dependencies: scoped_dependencies(&manifest, &workspace, "build-dependencies"),
            path,
        })
        .collect()
}

/// The members whose package name is not the one their path gives.
fn name_violations(members: &[Member]) -> Vec<String> {
    members
        .iter()
        .filter_map(|member| {
            let expected = expected_name(&member.path);
            (member.name != expected).then(|| {
                format!(
                    "{}: named {}, expected {expected}",
                    member.path, member.name
                )
            })
        })
        .collect()
}

#[test]
fn every_package_is_named_by_its_path() {
    let members = workspace_members();
    assert!(members.len() > 100, "the workspace members are read");
    let violations = name_violations(&members);
    assert!(
        violations.is_empty(),
        "package names off the path rule:\n{}",
        violations.join("\n")
    );
}

/// What a test member builds: an executable or a library.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Built {
    Executable,
    Library,
}

fn member(path: &str, name: &str, built: Built, dependencies: &[&str]) -> Member {
    Member {
        path: path.to_owned(),
        name: name.to_owned(),
        executable: built == Built::Executable,
        dependencies: dependencies.iter().map(|name| (*name).to_owned()).collect(),
        build_dependencies: BTreeSet::new(),
    }
}

/// Direct normal uses of external async crates. Transitive dependencies of
/// the network stack and bt-hci are outside this rule; dev dependencies are
/// outside `Member::dependencies`.
fn external_crate_allowed(path: &str, role: Role, dependency: &str) -> bool {
    match dependency {
        "embassy-futures" => !matches!(
            role,
            Role::Facts
                | Role::Pac
                | Role::Hal
                | Role::CoreSidePolicy
                | Role::Protocol(ProtocolRole::Wire | ProtocolRole::Cipher | ProtocolRole::Core)
        ),
        "embassy-sync" => false,
        "embassy-time" => {
            path == "portable/embassy/time"
                // The application entry re-exports the executors' clock.
                || path == "chips/esp32s31/app"
        }
        "embassy-time-driver" => path == "chips/family/platform/executor",
        "trouble-host" => path == "portable/embassy/trouble" || image_path(path),
        "bt-hci" => {
            path == "protocols/bluetooth/hci"
                || path == "portable/embassy/trouble"
                || path.starts_with("tools/")
        }
        _ => true,
    }
}

fn external_crate_violations(members: &[Member]) -> Result<BTreeSet<Edge>, String> {
    let mut found = BTreeSet::new();
    for member in members {
        let role = role(&member.path).ok_or_else(|| format!("no role: {}", member.path))?;
        found.extend(
            member
                .dependencies
                .iter()
                .filter(|dependency| !external_crate_allowed(&member.path, role, dependency))
                .map(|dependency| (member.path.clone(), dependency.clone())),
        );
    }
    Ok(found)
}

/// Remaining direct embassy-sync edges and the change that removes them.
const EXTERNAL_ASYNC_CRATES: &[Deviation] = &[(
    "protocols/hil/agent",
    "embassy-sync",
    "console on owned queues and waiter slots (#848)",
)];

#[test]
fn external_embassy_crates_follow_the_role_table() {
    let differences = deviation_differences(
        &external_crate_violations(&workspace_members()).expect("workspace roles"),
        EXTERNAL_ASYNC_CRATES,
    );
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn external_crates_reject_aliased_optional_target_and_runtime_stack_edges() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
cooperate = { workspace = true, optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
signal = { package = "embassy-sync", version = "1", optional = true }
host = { package = "trouble-host", version = "1", optional = true }
clock = { package = "embassy-time", version = "1" }
abi = { package = "embassy-time-driver", version = "1" }
codec = { package = "bt-hci", version = "1" }
[dev-dependencies]
embassy-sync = "1"
[build-dependencies]
embassy-sync = "1"
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str(
        "[dependencies]\ncooperate = { package = 'embassy-futures', version = '1' }",
    )
    .expect("workspace");
    let dependencies = normal_dependencies(&manifest, &workspace);
    let path = "protocols/ieee80211/runtime";
    let runtime = Member {
        dependencies: dependencies.clone(),
        ..member(path, "oer-protocols-ieee80211-runtime", Built::Library, &[])
    };
    assert_eq!(
        external_crate_violations(&[runtime]).expect("runtime role"),
        [
            "bt-hci",
            "embassy-sync",
            "embassy-time",
            "embassy-time-driver",
            "trouble-host",
        ]
        .map(|dependency| (path.to_owned(), dependency.to_owned()))
        .into()
    );
    let core = Member {
        dependencies,
        ..member(
            "protocols/ieee80211/core/roles/sta",
            "oer-protocols-ieee80211-core-roles-sta",
            Built::Library,
            &[],
        )
    };
    assert!(
        external_crate_violations(&[core])
            .expect("core role")
            .contains(&(
                "protocols/ieee80211/core/roles/sta".into(),
                "embassy-futures".into(),
            ))
    );
    let dev_only: toml::Value =
        toml::from_str("[dev-dependencies]\nembassy-sync = '1'").expect("dev manifest");
    assert!(normal_dependencies(&dev_only, &workspace).is_empty());
    assert!(!external_crate_allowed(
        path,
        role(path).unwrap(),
        "embassy-sync"
    ));
}

/// What an IEEE 802.11 protocol package never takes as a direct normal
/// dependency: trace records, transport serialization, HIL and the network
/// codec. Its owners report to the sink their caller lends a call
/// (`oer-portable-events`); a facade or an image chooses what an event becomes and how
/// it leaves the device. Network fields reach it only through the bridge
/// (`protocols/bridges/ieee80211-ethernet`).
fn ieee80211_foreign_dependency(dependency: &str) -> bool {
    matches!(
        dependency,
        "oer-portable-trace"
            | "serde"
            | "postcard"
            | "postcard-schema"
            | "oer-protocols-network-wire"
    ) || dependency.starts_with("oer-protocols-hil-")
}

fn ieee80211_foreign_edges(members: &[Member]) -> BTreeSet<Edge> {
    members
        .iter()
        .filter(|member| member.path.starts_with("protocols/ieee80211/"))
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .filter(|dependency| ieee80211_foreign_dependency(dependency))
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect()
}

/// The codecs whose every normal and build dependency is `oer-portable-wire`.
const STRICT_CODECS: [&str; 2] = ["protocols/ieee80211/wire", "protocols/network/wire"];

/// A strict codec's dependencies other than `oer-portable-wire`, normal or build,
/// optional or target-specific.
fn strict_codec_edges(members: &[Member]) -> BTreeSet<Edge> {
    members
        .iter()
        .filter(|member| STRICT_CODECS.contains(&member.path.as_str()))
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .chain(&member.build_dependencies)
                .filter(|dependency| *dependency != "oer-portable-wire")
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect()
}

#[test]
fn the_ieee80211_and_network_codecs_depend_on_oer_wire_alone() {
    let members = workspace_members();
    for path in STRICT_CODECS {
        assert!(members.iter().any(|member| member.path == path), "{path}");
    }
    let edges = strict_codec_edges(&members);
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn a_strict_codec_refuses_optional_target_and_build_dependencies() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
oer-portable-wire = { path = "../../../portable/wire" }
regulatory = { package = "oer-protocols-radio-regulatory", path = "../../radio/regulatory", optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
zeroize = "1"
[build-dependencies]
cc = "1"
[dev-dependencies]
tempfile = "3"
"#,
    )
    .expect("manifest");
    let workspace = toml::Value::Table(toml::Table::new());
    let codec = Member {
        dependencies: normal_dependencies(&manifest, &workspace),
        build_dependencies: scoped_dependencies(&manifest, &workspace, "build-dependencies"),
        ..member(
            "protocols/ieee80211/wire",
            "oer-protocols-ieee80211-wire",
            Built::Library,
            &[],
        )
    };
    let other = member(
        "protocols/ieee80211/core/roles/sta",
        "oer-protocols-ieee80211-core-roles-sta",
        Built::Library,
        &["zeroize"],
    );
    assert_eq!(
        strict_codec_edges(&[codec, other]),
        ["cc", "oer-protocols-radio-regulatory", "zeroize"]
            .map(|dependency| ("protocols/ieee80211/wire".to_owned(), dependency.to_owned()))
            .into()
    );
}

/// The workspace edges by which a protocol contract reaches another layer
/// than portable support.
fn contract_edges(members: &[Member], roles: &BTreeMap<String, Role>) -> BTreeSet<Edge> {
    members
        .iter()
        .filter(|member| role(&member.path) == Some(Role::Protocol(ProtocolRole::Contract)))
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .filter(|dependency| {
                    roles
                        .get(*dependency)
                        .is_some_and(|role| *role != Role::Portable)
                })
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect()
}

#[test]
fn a_protocol_contract_depends_on_no_protocol_package() {
    let members = workspace_members();
    assert!(
        members
            .iter()
            .any(|member| member.path == "protocols/ieee80211/contracts/security")
    );
    let edges = contract_edges(&members, &workspace_roles());
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn a_contract_edge_to_a_wire_or_core_is_found_and_portable_support_is_not() {
    let contract = member(
        "protocols/ieee80211/contracts/security",
        "oer-protocols-ieee80211-contracts-security",
        Built::Library,
        &[
            "oer-portable-wire",
            "oer-protocols-ieee80211-wire",
            "oer-protocols-ieee80211-core-roles-sta",
            "zeroize",
        ],
    );
    let roles = BTreeMap::from([
        ("oer-portable-wire".to_owned(), Role::Portable),
        (
            "oer-protocols-ieee80211-wire".to_owned(),
            Role::Protocol(ProtocolRole::Wire),
        ),
        (
            "oer-protocols-ieee80211-core-roles-sta".to_owned(),
            Role::Protocol(ProtocolRole::Core),
        ),
    ]);
    assert_eq!(
        contract_edges(&[contract], &roles),
        [
            "oer-protocols-ieee80211-core-roles-sta",
            "oer-protocols-ieee80211-wire"
        ]
        .map(|dependency| (
            "protocols/ieee80211/contracts/security".to_owned(),
            dependency.to_owned()
        ))
        .into()
    );
}

#[test]
fn ieee80211_protocol_packages_take_no_trace_serialization_hil_or_network_codec() {
    let edges = ieee80211_foreign_edges(&workspace_members());
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn ieee80211_foreign_dependencies_include_aliases_and_spare_other_families() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
events = { package = "oer-portable-events", path = "../../../portable/events" }
records = { package = "oer-portable-trace", path = "../../../portable/trace" }
fields = { package = "oer-protocols-network-wire", path = "../../network/wire" }
bridge = { package = "oer-protocols-bridges-ieee80211-ethernet", path = "../../bridges/ieee80211-ethernet" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
schema = { package = "postcard-schema", version = "0.2", optional = true }
agent = { package = "oer-protocols-hil-agent", path = "../../hil/agent" }
[dev-dependencies]
serde = "1"
"#,
    )
    .expect("manifest");
    let workspace = toml::Value::Table(toml::Table::new());
    let dependencies = normal_dependencies(&manifest, &workspace);
    let runtime = Member {
        dependencies: dependencies.clone(),
        ..member(
            "protocols/ieee80211/runtime",
            "oer-protocols-ieee80211-runtime",
            Built::Library,
            &[],
        )
    };
    let facade = Member {
        dependencies,
        ..member(
            "chips/esp32s31/radio/ieee80211/station",
            "oer-chips-esp32s31-radio-ieee80211-station",
            Built::Library,
            &[],
        )
    };
    assert_eq!(
        ieee80211_foreign_edges(&[runtime, facade]),
        [
            "oer-protocols-hil-agent",
            "oer-protocols-network-wire",
            "oer-portable-trace",
            "postcard-schema"
        ]
        .map(|dependency| (
            "protocols/ieee80211/runtime".to_owned(),
            dependency.to_owned()
        ))
        .into()
    );
}

#[test]
fn external_crate_homes_and_futures_exclusions_are_explicit() {
    for (dependency, allowed) in [
        ("embassy-time", vec!["portable/embassy/time"]),
        (
            "embassy-time-driver",
            vec!["chips/family/platform/executor"],
        ),
        (
            "trouble-host",
            vec!["portable/embassy/trouble", "hil/probes/esp32s31-radio-root"],
        ),
        (
            "bt-hci",
            vec![
                "protocols/bluetooth/hci",
                "portable/embassy/trouble",
                "tools/lab/fixture-helpers",
            ],
        ),
    ] {
        for path in allowed {
            assert!(
                external_crate_allowed(path, role(path).unwrap(), dependency),
                "{path} -> {dependency}"
            );
        }
        for path in [
            "portable/embassy/other",
            "protocols/bluetooth/le/runtime",
            "chips/esp32s31/hal",
        ] {
            assert!(
                !external_crate_allowed(path, role(path).unwrap(), dependency),
                "{path} -> {dependency}"
            );
        }
    }
    for (path, allowed) in [
        ("protocols/bluetooth/le/wire", false),
        ("protocols/ieee80211/core/security/cipher", false),
        ("protocols/bluetooth/le/controller", false),
        ("chips/family/radio/ieee80211/policy", false),
        ("chips/family/facts", false),
        ("chips/esp32s31/pac", false),
        ("chips/esp32s31/hal", false),
        ("protocols/bluetooth/le/runtime", true),
        ("protocols/bluetooth/le/port", true),
        ("protocols/bluetooth/le/model", true),
        ("portable/cooperate", true),
        ("portable/embassy/net", true),
        ("chips/esp32s31/radio/bluetooth/backend", true),
        ("hil/probes/esp32s31-radio-root", true),
        ("tools/lab/fixture-helpers", true),
    ] {
        assert_eq!(
            external_crate_allowed(path, role(path).unwrap(), "embassy-futures"),
            allowed,
            "{path}"
        );
        assert!(
            !external_crate_allowed(path, role(path).unwrap(), "embassy-sync"),
            "{path}"
        );
    }
}

/// Token paths, including a grouped `use bt_hci::{controller::...}`.
/// Comments and strings do not count as uses of the async Controller API.
/// Where a token group stands: inside a `bt_hci::{…}` path, or elsewhere.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BtHci {
    Inside,
    Elsewhere,
}

fn bt_hci_controller(tokens: TokenStream, at: BtHci) -> bool {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    (at == BtHci::Inside
        && tokens.iter().any(|token| matches!(token, TokenTree::Ident(name) if name == "controller")))
        || tokens.iter().any(|token| {
            matches!(token, TokenTree::Group(group) if bt_hci_controller(group.stream(), BtHci::Elsewhere))
        })
        || tokens.windows(4).any(|path| match path {
            [TokenTree::Ident(name), TokenTree::Punct(first), TokenTree::Punct(second), next]
                if name == "bt_hci" && first.as_char() == ':' && second.as_char() == ':' => {
                match next {
                    TokenTree::Ident(name) => name == "controller",
                    TokenTree::Group(group) => bt_hci_controller(group.stream(), BtHci::Inside),
                    _ => false,
                }
            }
            _ => false,
        })
}

fn bt_hci_controller_sources(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .path();
        if path.is_dir() {
            bt_hci_controller_sources(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let tokens = source
                .parse::<TokenStream>()
                .map_err(|error| format!("{} does not tokenize: {error}", path.display()))?;
            if bt_hci_controller(tokens, BtHci::Elsewhere) {
                found.push(path);
            }
        }
    }
    Ok(())
}

#[test]
fn protocols_never_use_the_bt_hci_async_controller() {
    let mut found = Vec::new();
    bt_hci_controller_sources(&root().join("protocols"), &mut found).expect("protocol sources");
    assert!(found.is_empty(), "async bt-hci Controller uses: {found:?}");
}

#[test]
fn bt_hci_controller_scan_checks_paths_and_grouped_imports_not_comments_or_strings() {
    for source in [
        "type Host = bt_hci::controller::ExternalController;",
        "use ::bt_hci /* comment */ :: controller::{Controller, ExternalController};",
        "use bt_hci::{cmd::Cmd, controller::{Controller as Host}};",
        "mod nested { use bt_hci::controller; }",
    ] {
        assert!(
            bt_hci_controller(source.parse().unwrap(), BtHci::Elsewhere),
            "{source}"
        );
    }
    for source in [
        "// bt_hci::controller::Controller\nuse bt_hci::cmd::Cmd;",
        "const DOC: &str = \"bt_hci::controller::Controller\";",
        "use bt_hci::{cmd::{Cmd, Opcode}, param::Status};",
        "mod controller {} use crate::controller::Command;",
    ] {
        assert!(
            !bt_hci_controller(source.parse().unwrap(), BtHci::Elsewhere),
            "{source}"
        );
    }
}

#[test]
fn the_name_rule_tells_libraries_from_programs_by_their_binary_targets() {
    let members = [
        member(
            "protocols/ieee80211/wire",
            "oer-protocols-ieee80211-wire",
            Built::Library,
            &[],
        ),
        member(
            "protocols/ieee80211/core/roles/sta",
            "oer-sta",
            Built::Library,
            &[],
        ),
        member(
            "hil/probes/esp32s31-boot-smoke",
            "oer-hil-probes-esp32s31-boot-smoke",
            Built::Executable,
            &[],
        ),
        member(
            "examples/example",
            "oer-apps-example",
            Built::Executable,
            &[],
        ),
        member(
            "tools/image/build",
            "oer-tools-image-build",
            Built::Library,
            &[],
        ),
        member(
            "tools/image/cli",
            "oer-tools-image-cli",
            Built::Executable,
            &[],
        ),
        member(
            "tools/image/bundle",
            "oer-tool-image-bundle",
            Built::Library,
            &[],
        ),
        member("tools/pins", "oer-pins", Built::Executable, &[]),
    ];
    assert_eq!(
        name_violations(&members),
        [
            "protocols/ieee80211/core/roles/sta: named oer-sta, expected oer-protocols-ieee80211-core-roles-sta",
            "examples/example: named oer-apps-example, expected oer-examples-example",
            "tools/image/bundle: named oer-tool-image-bundle, expected oer-tools-image-bundle",
            "tools/pins: named oer-pins, expected oer-tools-pins",
        ]
    );
}

#[test]
fn a_binary_target_is_found_by_auto_discovery_and_by_manifest_entries() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let package = |name: &str, files: &[&str], extra: &str| {
        let path = directory.path().join(name);
        for file in files {
            std::fs::create_dir_all(path.join(file).parent().expect("parent")).expect("directory");
            std::fs::write(path.join(file), "").expect("file");
        }
        let manifest: toml::Value =
            toml::from_str(&format!("[package]\nname = \"{name}\"\n{extra}")).expect("manifest");
        has_binary(&path, &manifest)
    };
    assert!(!package("lib", &["src/lib.rs"], ""));
    assert!(package("main", &["src/lib.rs", "src/main.rs"], ""));
    assert!(package("bin-dir", &["src/lib.rs", "src/bin/tool.rs"], ""));
    assert!(!package(
        "manual",
        &["src/main.rs", "src/bin/tool.rs"],
        "autobins = false"
    ));
    assert!(package(
        "listed",
        &["src/lib.rs"],
        "autobins = false\n[[bin]]\nname = \"x\"\npath = \"src/x.rs\""
    ));
}

/// An edge a structure rule forbids, from the dependent's path to the
/// dependency's path.
type Edge = (String, String);

/// The packages whose normal dependencies include a package that `forbids`
/// them, as path edges.
fn forbidden_edges(
    members: &[Member],
    forbids: impl Fn(&Member, &Member) -> bool,
) -> BTreeSet<Edge> {
    let by_name = members
        .iter()
        .map(|member| (member.name.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    members
        .iter()
        .flat_map(|from| {
            from.dependencies
                .iter()
                .filter_map(|name| by_name.get(name.as_str()))
                .filter(|to| forbids(from, to))
                .map(|to| (from.path.clone(), to.path.clone()))
        })
        .collect()
}

/// An edge the tree still has against a rule, with what removes it. The
/// list of a rule only shrinks; `docs/repository-layout.md` states each entry.
type Deviation = (&'static str, &'static str, &'static str);

/// Compares the edges a rule finds with its known deviations: a new edge and
/// an entry whose edge is gone both fail.
fn deviation_differences(found: &BTreeSet<Edge>, known: &[Deviation]) -> Vec<String> {
    let known_edges = known
        .iter()
        .map(|(from, to, _)| ((*from).to_owned(), (*to).to_owned()))
        .collect::<BTreeSet<_>>();
    let new = found
        .difference(&known_edges)
        .map(|(from, to)| format!("new: {from} -> {to}"));
    let stale = known_edges
        .difference(found)
        .map(|(from, to)| format!("gone, drop the entry: {from} -> {to}"));
    new.chain(stale).collect()
}

/// The members with their build dependencies counted as edges too: a build
/// script that names a package carries its knowledge as well.
fn with_build_edges(members: Vec<Member>) -> Vec<Member> {
    members
        .into_iter()
        .map(|mut member| {
            let build = std::mem::take(&mut member.build_dependencies);
            member.dependencies.extend(build);
            member
        })
        .collect()
}

fn image_or_program(_: &Member, to: &Member) -> bool {
    image_path(&to.path) || (to.path.starts_with("tools/") && to.executable)
}

fn family_reaches_a_chip(from: &Member, to: &Member) -> bool {
    from.path.starts_with("chips/family/")
        && to.path.starts_with("chips/")
        && !to.path.starts_with("chips/family/")
}

fn portable_reaches_a_protocol_or_a_chip(from: &Member, to: &Member) -> bool {
    let embassy = from.path.starts_with("portable/embassy/");
    from.path.starts_with("portable/")
        && ((!embassy && to.path.starts_with("protocols/")) || to.path.starts_with("chips/"))
}

/// Nothing depends on an image or on a program of `tools/` (a package with a
/// binary target) through a normal dependency.
const NO_EDGE_ON_AN_IMAGE_OR_PROGRAM: &[Deviation] = &[];

/// `chips/family` depends on no chip, through a normal or a build
/// dependency: it is the family below them.
const FAMILY_BELOW_THE_CHIPS: &[Deviation] = &[];

/// `portable` depends on no protocol and no chip; `portable/embassy` alone
/// adapts protocol boundaries.
const PORTABLE_BELOW_THE_PROTOCOLS: &[Deviation] = &[];

#[test]
fn no_normal_edge_reaches_an_image_or_a_program() {
    let found = forbidden_edges(&workspace_members(), image_or_program);
    let differences = deviation_differences(&found, NO_EDGE_ON_AN_IMAGE_OR_PROGRAM);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn the_espressif_family_depends_on_no_chip() {
    let found = forbidden_edges(
        &with_build_edges(workspace_members()),
        family_reaches_a_chip,
    );
    let differences = deviation_differences(&found, FAMILY_BELOW_THE_CHIPS);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn portable_depends_on_no_protocol_and_no_chip_except_the_embassy_adapters() {
    let found = forbidden_edges(&workspace_members(), portable_reaches_a_protocol_or_a_chip);
    let differences = deviation_differences(&found, PORTABLE_BELOW_THE_PROTOCOLS);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn the_edge_rules_see_optional_target_and_aliased_dependencies_and_judge_by_path() {
    let manifest = |dependencies: &str| -> BTreeSet<String> {
        let manifest: toml::Value = toml::from_str(dependencies).expect("manifest");
        let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
        normal_dependencies(&manifest, &workspace)
    };
    let members = [
        member(
            "chips/family/drivers/regi2c",
            "oer-chips-family-drivers-regi2c",
            Built::Library,
            &[],
        ),
        Member {
            dependencies: manifest(
                r#"
[dependencies]
hal = { package = "oer-chips-esp32s31-hal", version = "1", optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
app = { package = "oer-hil-probes-esp32s31-radio-root", version = "1" }
[dev-dependencies]
tool = { package = "oer-tool-pins", version = "1" }
"#,
            ),
            ..member(
                "chips/family/platform/executor",
                "oer-chips-family-platform-executor",
                Built::Library,
                &[],
            )
        },
        member(
            "chips/esp32s31/hal",
            "oer-chips-esp32s31-hal",
            Built::Library,
            &[],
        ),
        member(
            "hil/probes/esp32s31-radio-root",
            "oer-hil-probes-esp32s31-radio-root",
            Built::Executable,
            &[],
        ),
        member("tools/pins", "oer-tool-pins", Built::Executable, &[]),
        member(
            "tools/image/build",
            "oer-tools-image-build",
            Built::Library,
            &[],
        ),
        member(
            "tools/image/checks",
            "oer-tools-image-checks",
            Built::Library,
            &["oer-tool-pins", "oer-tools-image-build"],
        ),
        member(
            "portable/time",
            "oer-portable-time",
            Built::Library,
            &["oer-protocols-ieee80211-wire", "oer-chips-esp32s31-hal"],
        ),
        member(
            "protocols/ieee80211/wire",
            "oer-protocols-ieee80211-wire",
            Built::Library,
            &[],
        ),
    ];
    let edge = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    assert_eq!(
        forbidden_edges(&members, image_or_program),
        BTreeSet::from([
            edge(
                "chips/family/platform/executor",
                "hil/probes/esp32s31-radio-root"
            ),
            edge("tools/image/checks", "tools/pins"),
        ])
    );
    assert_eq!(
        forbidden_edges(&members, family_reaches_a_chip),
        BTreeSet::from([edge("chips/family/platform/executor", "chips/esp32s31/hal")])
    );
    assert_eq!(
        forbidden_edges(&members, portable_reaches_a_protocol_or_a_chip),
        BTreeSet::from([
            edge("portable/time", "protocols/ieee80211/wire"),
            edge("portable/time", "chips/esp32s31/hal"),
        ])
    );
}

#[test]
fn the_family_rule_sees_build_dependencies_and_ignores_dev_dependencies() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
port = { package = "oer-chips-family-platform-port", version = "1" }
[build-dependencies]
facts = { package = "oer-chips-family-facts", version = "1" }
[target.'cfg(unix)'.build-dependencies]
layout = { package = "oer-chips-esp32s31-platform-layout", version = "1", optional = true }
[dev-dependencies]
hal = { package = "oer-chips-esp32s31-hal", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let members = vec![
        Member {
            dependencies: normal_dependencies(&manifest, &workspace),
            build_dependencies: scoped_dependencies(&manifest, &workspace, "build-dependencies"),
            ..member(
                "chips/family/platform/runtime",
                "oer-chips-family-platform-runtime",
                Built::Library,
                &[],
            )
        },
        member(
            "chips/family/platform/port",
            "oer-chips-family-platform-port",
            Built::Library,
            &[],
        ),
        member(
            "chips/family/facts",
            "oer-chips-family-facts",
            Built::Library,
            &[],
        ),
        member(
            "chips/esp32s31/platform/layout",
            "oer-chips-esp32s31-platform-layout",
            Built::Library,
            &[],
        ),
        member(
            "chips/esp32s31/hal",
            "oer-chips-esp32s31-hal",
            Built::Library,
            &[],
        ),
    ];
    assert!(forbidden_edges(&members, family_reaches_a_chip).is_empty());
    assert_eq!(
        forbidden_edges(&with_build_edges(members), family_reaches_a_chip),
        BTreeSet::from([(
            "chips/family/platform/runtime".to_owned(),
            "chips/esp32s31/platform/layout".to_owned()
        )])
    );
}

#[test]
fn a_deviation_list_fails_on_a_new_edge_and_on_an_edge_that_is_gone() {
    let edge = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    let known: &[Deviation] = &[("a", "b", "a decision"), ("c", "d", "a decision")];
    assert!(
        deviation_differences(&BTreeSet::from([edge("a", "b"), edge("c", "d")]), known).is_empty()
    );
    assert_eq!(
        deviation_differences(&BTreeSet::from([edge("a", "b"), edge("e", "f")]), known),
        ["new: e -> f", "gone, drop the entry: c -> d"]
    );
}

/// A chip or HIL package's direct normal dependency that its role does not
/// allow ([`role_allows`]). A package without a role fails
/// `every_chip_and_hil_package_has_a_listed_role` instead.
fn outside_its_role(from: &Member, to: &Member) -> bool {
    match (role(&from.path), role(&to.path)) {
        (Some(from_role), Some(to_role)) => !role_allows(from_role, to_role, &to.path),
        _ => false,
    }
}

/// Direct normal edges against [`role_allows`], with what removes each.
const OUTSIDE_THE_ROLES: &[Deviation] = &[
    (
        "chips/esp32s31/hal",
        "protocols/radio/coex",
        "radio/coex merges into the HAL (#848)",
    ),
    (
        "chips/esp32s31/radio/ieee802154/backend",
        "portable/mmio",
        "IEEE 802.15.4 lifecycle out of the HAL (#848)",
    ),
    (
        "chips/esp32s31/pac",
        "chips/family/drivers/regi2c",
        "analog-I2C model types out of the family driver (#848)",
    ),
];

#[test]
fn every_chip_and_hil_package_depends_only_on_what_its_role_allows() {
    let found = forbidden_edges(&workspace_members(), outside_its_role);
    let differences = deviation_differences(&found, OUTSIDE_THE_ROLES);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// A role's allowed and forbidden direct dependencies: `(dependency role,
/// dependency path, allowed)`.
type RoleCases = &'static [(Role, &'static str, bool)];

fn assert_role_cases(from: Role, cases: RoleCases) {
    for (to, path, allowed) in cases {
        assert_eq!(
            role_allows(from, *to, path),
            *allowed,
            "{from:?} -> {to:?} ({path})"
        );
    }
}

const PROTOCOL_CORE: Role = Role::Protocol(ProtocolRole::Core);
const PROTOCOL_RUNTIME: Role = Role::Protocol(ProtocolRole::Runtime);
const PROTOCOL_PORT: Role = Role::Protocol(ProtocolRole::Port);
const PROTOCOL_WIRE: Role = Role::Protocol(ProtocolRole::Wire);

#[test]
fn facts_pac_and_platform_contracts_stand_on_values_alone() {
    assert_role_cases(
        Role::Facts,
        &[
            (Role::Facts, "chips/family/facts", true),
            (Role::SharedVocabulary, "protocols/radio/regulatory", true),
            (Role::Portable, "portable/memory", true),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::HardwareDriver, "chips/family/drivers/regi2c", false),
            (PROTOCOL_PORT, "protocols/ieee802154/port", false),
        ],
    );
    assert_role_cases(
        Role::Pac,
        &[
            (Role::HardwareAccess, "portable/mmio", true),
            (Role::Facts, "chips/esp32s31/knowledge", true),
            (Role::HardwareDriver, "chips/family/drivers/regi2c", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::PlatformContract,
        &[
            (Role::HardwareAccess, "portable/riscv", true),
            (Role::PlatformContract, "chips/family/platform/layout", true),
            (Role::Platform, "chips/family/platform/runtime", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
}

#[test]
fn the_hal_implements_family_ports_and_runs_no_chip_driver_or_protocol() {
    assert_role_cases(
        Role::Hal,
        &[
            (Role::Pac, "chips/esp32s31/pac", true),
            (Role::PlatformContract, "chips/family/platform/port", true),
            (Role::HardwareDriver, "chips/family/boot", true),
            (Role::HardwareDriver, "chips/esp32s31/radio/phy", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
            (Role::Service, "chips/esp32s31/radio/system", false),
            (Role::Platform, "chips/family/platform/runtime", false),
            (Role::SharedVocabulary, "protocols/radio/coex", false),
            (PROTOCOL_PORT, "protocols/ieee80211/contracts/port", false),
        ],
    );
}

#[test]
fn drivers_and_adapters_stand_on_the_hal_and_the_port_never_above_it() {
    let below = [
        (Role::Hal, "chips/esp32s31/hal", true),
        (Role::HardwareDriver, "chips/family/drivers/regi2c", true),
        (
            Role::PlatformContract,
            "chips/family/platform/interrupt-table",
            true,
        ),
        (
            Role::ObservationContract,
            "protocols/radio/instrumentation",
            true,
        ),
        (PROTOCOL_PORT, "protocols/ieee80211/contracts/port", true),
        (PROTOCOL_WIRE, "protocols/ieee80211/wire", true),
        (PROTOCOL_CORE, "protocols/ieee80211/core/roles/sta", false),
        (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
        (
            Role::CoreSidePolicy,
            "chips/family/radio/ieee80211/policy",
            false,
        ),
        (Role::Pac, "chips/esp32s31/pac", false),
        (Role::HardwareAccess, "portable/mmio", false),
        (Role::Platform, "chips/family/platform/runtime", false),
        (Role::Service, "chips/esp32s31/radio/system", false),
        (Role::Composition, "chips/esp32s31/platform/runtime", false),
        (
            Role::Facade,
            "chips/esp32s31/radio/ieee80211/station",
            false,
        ),
        (Role::HilAgent, "protocols/hil/agent", false),
    ];
    for from in [
        Role::HardwareDriver,
        Role::ChipAdapter,
        Role::ServiceClient,
        Role::Service,
    ] {
        for (to, path, allowed) in below {
            assert_eq!(role_allows(from, to, path), allowed, "{from:?} -> {path}");
        }
    }
    for (from, allowed) in [
        (Role::HardwareDriver, false),
        (Role::ChipAdapter, true),
        (Role::ServiceClient, false),
        (Role::Service, true),
    ] {
        assert_eq!(
            role_allows(from, Role::ServiceClient, "chips/esp32s31/radio/client"),
            allowed,
            "{from:?}"
        );
    }
    assert!(role_allows(
        Role::ChipAdapter,
        Role::ChipAdapter,
        "chips/esp32s31/radio/ieee80211/backend"
    ));
    assert!(!role_allows(
        Role::Service,
        Role::ChipAdapter,
        "chips/esp32s31/radio/ieee80211/backend"
    ));
}

#[test]
fn core_side_policy_stays_with_the_cores_and_the_platform_reaches_no_chip_hal() {
    assert_role_cases(
        Role::CoreSidePolicy,
        &[
            (PROTOCOL_CORE, "protocols/ieee80211/core/upper-mac", true),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::Platform,
        &[
            (Role::PlatformContract, "chips/family/platform/port", true),
            (Role::HardwareDriver, "chips/family/boot", true),
            (Role::HardwareDriver, "chips/esp32s31/radio/phy", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
            (PROTOCOL_PORT, "protocols/ieee80211/contracts/port", false),
        ],
    );
}

#[test]
fn only_a_composition_and_a_facade_assemble_runtimes_and_backends() {
    for from in [Role::Composition, Role::Facade] {
        assert_role_cases(
            from,
            &[
                (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", true),
                (PROTOCOL_CORE, "protocols/ieee80211/core/roles/sta", true),
                (
                    Role::ChipAdapter,
                    "chips/esp32s31/radio/ieee80211/backend",
                    true,
                ),
                (Role::Service, "chips/esp32s31/radio/system", true),
                (Role::Platform, "chips/family/platform/executor", true),
                (Role::Pac, "chips/esp32s31/pac", false),
                (Role::HilAgent, "protocols/hil/agent", false),
                (Role::HilImage, "hil/images/aes", false),
                (Role::Image, "hil/probes/esp32s31-radio-root", false),
            ],
        );
        assert!(!keeps_the_backend_rule(from), "{from:?}");
    }
    assert!(role_allows(
        Role::Facade,
        Role::Composition,
        "chips/esp32s31/platform/runtime"
    ));
    assert!(!role_allows(
        Role::Composition,
        Role::Facade,
        "chips/esp32s31/radio/ieee80211/station"
    ));
    for keeps in [
        Role::Facts,
        Role::Pac,
        Role::Hal,
        Role::HardwareDriver,
        Role::ChipAdapter,
        Role::ServiceClient,
        Role::Service,
        Role::PlatformContract,
        Role::Platform,
        Role::ObservationContract,
        Role::Protocol(ProtocolRole::Conformance),
        Role::Protocol(ProtocolRole::Model),
    ] {
        assert!(keeps_the_backend_rule(keeps), "{keeps:?}");
    }
    let roles = BTreeMap::from([
        ("adapter".into(), Role::ChipAdapter),
        ("driver".into(), Role::HardwareDriver),
        ("facade".into(), Role::Facade),
        ("runtime".into(), PROTOCOL_RUNTIME),
    ]);
    let graph = BTreeMap::from([
        ("adapter".into(), BTreeSet::from(["driver".into()])),
        ("driver".into(), BTreeSet::from(["runtime".into()])),
        (
            "facade".into(),
            BTreeSet::from(["adapter".into(), "runtime".into()]),
        ),
    ]);
    let violations = roles
        .iter()
        .filter(|(_, role)| keeps_the_backend_rule(**role))
        .flat_map(|(name, _)| backend_core_paths(&graph, &roles, name))
        .collect::<Vec<_>>();
    assert_eq!(
        violations,
        ["adapter -> driver -> runtime", "driver -> runtime"]
    );
}

#[test]
fn an_observation_contract_names_values_and_no_owner() {
    assert_role_cases(
        Role::ObservationContract,
        &[
            (Role::Portable, "portable/trace", true),
            (PROTOCOL_WIRE, "protocols/ieee80211/wire", true),
            (PROTOCOL_CORE, "protocols/ieee80211/core/roles/sta", false),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
            (Role::Platform, "chips/family/platform/runtime", false),
        ],
    );
}

#[test]
fn hil_roles_keep_radio_out_of_the_framework_and_images_behind_the_facade() {
    assert_role_cases(
        Role::HilProtocol,
        &[
            (Role::Portable, "portable/memory", true),
            (PROTOCOL_PORT, "protocols/ieee80211/contracts/port", false),
            (Role::EmbassyAdapter, "portable/embassy/net", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::HilAgent,
        &[
            (Role::HilProtocol, "protocols/hil/protocol", true),
            (Role::Portable, "portable/diagnostics", true),
            (PROTOCOL_WIRE, "protocols/ieee80211/wire", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
            (Role::HardwareAccess, "portable/mmio", false),
        ],
    );
    assert_role_cases(
        Role::HilSdkAgent,
        &[
            (Role::Sdk, "oer", true),
            (Role::HilAgent, "protocols/hil/agent", true),
            (Role::Composition, "chips/esp32s31/platform/runtime", true),
            (Role::Platform, "chips/family/platform/runtime", true),
            (Role::HardwareAccess, "portable/riscv", true),
            (Role::HardwareAccess, "portable/mmio", false),
            (
                Role::Facade,
                "chips/esp32s31/radio/ieee80211/station",
                false,
            ),
            (Role::HilBinding, "hil/services/sdk-station", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::HilBinding,
        &[
            (Role::HilAgent, "protocols/hil/agent", true),
            (Role::Facade, "chips/esp32s31/radio/ieee80211/station", true),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", true),
            (Role::Portable, "portable/memory", true),
            (Role::Portable, "portable/time", false),
            (Role::EmbassyAdapter, "portable/embassy/time", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
        ],
    );
    assert_role_cases(
        Role::HilImage,
        &[
            (Role::Sdk, "oer", true),
            (Role::HilBinding, "protocols/ieee80211/hil", true),
            (Role::HilSdkAgent, "hil/agent", true),
            (Role::EmbassyAdapter, "portable/embassy/net", true),
            (Role::Composition, "chips/esp32s31/platform/runtime", false),
            (
                Role::Facade,
                "chips/esp32s31/radio/ieee80211/station",
                false,
            ),
            (Role::Platform, "chips/family/platform/executor", false),
            (
                Role::ObservationContract,
                "protocols/radio/instrumentation",
                false,
            ),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::Pac, "chips/esp32s31/pac", false),
            (Role::HardwareDriver, "chips/esp32s31/radio/phy", false),
            (
                Role::ChipAdapter,
                "chips/esp32s31/radio/ieee80211/backend",
                false,
            ),
            (Role::Service, "chips/esp32s31/radio/system", false),
            (PROTOCOL_CORE, "protocols/ieee80211/core/roles/sta", false),
            (PROTOCOL_PORT, "protocols/ieee80211/contracts/port", false),
            (PROTOCOL_WIRE, "protocols/ieee80211/wire", false),
            (Role::Image, "hil/probes/esp32s31-radio-root", false),
        ],
    );
}

/// What an image built on the application entry may take besides it. The
/// entry places the compositions, facades, platform and interrupt table,
/// so an example and a HIL test image of one function build it the same
/// way; a HIL image adds only the HIL agent on the entry, the HIL protocol
/// and agent, and its services' dictionaries.
fn beside_the_entry(from: Role, to: Role) -> bool {
    match to {
        Role::Sdk | Role::Portable | Role::EmbassyAdapter => true,
        Role::HilSdkAgent | Role::HilAgent | Role::HilProtocol | Role::HilBinding => {
            from == Role::HilImage
        }
        _ => false,
    }
}

/// An image's direct normal dependency beside the application entry that
/// [`beside_the_entry`] does not allow.
fn outside_the_entry(members: &[Member]) -> BTreeSet<Edge> {
    let sdk = members
        .iter()
        .filter(|member| role(&member.path) == Some(Role::Sdk))
        .map(|member| member.name.clone())
        .collect::<BTreeSet<_>>();
    let on_the_entry = |member: &Member| {
        matches!(role(&member.path), Some(Role::Image | Role::HilImage))
            && !member.dependencies.is_disjoint(&sdk)
    };
    forbidden_edges(members, |from, to| {
        on_the_entry(from)
            && match (role(&from.path), role(&to.path)) {
                (Some(from_role), Some(to_role)) => !beside_the_entry(from_role, to_role),
                _ => true,
            }
    })
}

#[test]
fn an_image_on_the_application_entry_takes_nothing_the_entry_places() {
    let members = workspace_members();
    assert_eq!(outside_the_entry(&members), BTreeSet::new());
    for (from, to, allowed) in [
        (Role::HilImage, Role::HilSdkAgent, true),
        (Role::HilImage, Role::HilBinding, true),
        (Role::HilImage, Role::EmbassyAdapter, true),
        (Role::Image, Role::Sdk, true),
        (Role::Image, Role::HilSdkAgent, false),
        (Role::HilImage, Role::Composition, false),
        (Role::HilImage, Role::Facade, false),
        (Role::HilImage, Role::Platform, false),
        (Role::HilImage, Role::PlatformContract, false),
        (Role::Image, Role::Facade, false),
    ] {
        assert_eq!(beside_the_entry(from, to), allowed, "{from:?} -> {to:?}");
    }
}

#[test]
fn an_image_beside_the_application_entry_is_judged_and_one_without_it_is_not() {
    let members = [
        member(
            "hil/images/wifi-station",
            "image",
            Built::Executable,
            &["oer", "oer-chips-esp32s31-platform-runtime"],
        ),
        member(
            "hil/images/aes",
            "hand-built",
            Built::Executable,
            &["oer-chips-esp32s31-platform-runtime"],
        ),
        member("oer", "oer", Built::Library, &[]),
        member(
            "chips/esp32s31/platform/runtime",
            "oer-chips-esp32s31-platform-runtime",
            Built::Library,
            &[],
        ),
    ];
    assert_eq!(
        outside_the_entry(&members),
        BTreeSet::from([(
            "hil/images/wifi-station".to_owned(),
            "chips/esp32s31/platform/runtime".to_owned(),
        )])
    );
}

/// The HIL images that do not depend on the application entry.
fn hil_images_off_the_entry(members: &[Member]) -> BTreeSet<String> {
    let sdk = members
        .iter()
        .filter(|member| role(&member.path) == Some(Role::Sdk))
        .map(|member| member.name.clone())
        .collect::<BTreeSet<_>>();
    members
        .iter()
        .filter(|member| {
            role(&member.path) == Some(Role::HilImage) && member.dependencies.is_disjoint(&sdk)
        })
        .map(|member| member.path.clone())
        .collect()
}

#[test]
fn every_hil_image_is_an_application_of_the_entry() {
    assert_eq!(
        hil_images_off_the_entry(&workspace_members()),
        BTreeSet::new()
    );
}

#[test]
fn a_hil_image_without_the_entry_is_named_and_another_image_is_not() {
    let members = [
        member(
            "hil/images/wifi-station",
            "station",
            Built::Executable,
            &["oer"],
        ),
        member(
            "hil/images/aes",
            "hand-built",
            Built::Executable,
            &["oer-chips-esp32s31-platform-runtime"],
        ),
        member(
            "hil/probes/esp32s31-radio-root",
            "radio-root",
            Built::Executable,
            &["oer-chips-esp32s31-platform-runtime"],
        ),
        member("oer", "oer", Built::Library, &[]),
        member(
            "chips/esp32s31/platform/runtime",
            "oer-chips-esp32s31-platform-runtime",
            Built::Library,
            &[],
        ),
    ];
    assert_eq!(
        hil_images_off_the_entry(&members),
        BTreeSet::from(["hil/images/aes".to_owned()])
    );
}

#[test]
fn the_role_rule_judges_listed_packages_by_their_optional_target_and_aliased_edges() {
    let manifest = |dependencies: &str| -> BTreeSet<String> {
        let manifest: toml::Value = toml::from_str(dependencies).expect("manifest");
        let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
        normal_dependencies(&manifest, &workspace)
    };
    let members = [
        Member {
            dependencies: manifest(
                r#"
[dependencies]
pac = { package = "oer-chips-esp32s31-pac", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
phy = { package = "oer-chips-esp32s31-radio-phy", version = "1", optional = true }
[dev-dependencies]
adapter = { package = "oer-chips-esp32s31-radio-ieee80211-backend", version = "1" }
"#,
            ),
            ..member(
                "chips/esp32s31/hal",
                "oer-chips-esp32s31-hal",
                Built::Library,
                &[],
            )
        },
        member(
            "chips/esp32s31/pac",
            "oer-chips-esp32s31-pac",
            Built::Library,
            &[],
        ),
        member(
            "chips/esp32s31/radio/phy",
            "oer-chips-esp32s31-radio-phy",
            Built::Library,
            &[],
        ),
        member(
            "chips/esp32s31/radio/ieee80211/backend",
            "oer-chips-esp32s31-radio-ieee80211-backend",
            Built::Library,
            &[
                "oer-protocols-ieee80211-runtime",
                "oer-chips-esp32s31-radio-phy",
            ],
        ),
        member(
            "protocols/ieee80211/runtime",
            "oer-protocols-ieee80211-runtime",
            Built::Library,
            &[],
        ),
        member(
            "chips/esp32s31/radio/ieee80211/station",
            "oer-chips-esp32s31-radio-ieee80211-station",
            Built::Library,
            &[
                "oer-protocols-ieee80211-runtime",
                "oer-chips-esp32s31-radio-ieee80211-backend",
            ],
        ),
        member(
            "hil/images/aes",
            "oer-firmware-hil-esp32s31",
            Built::Executable,
            &[
                "oer-chips-esp32s31-radio-ieee80211-station",
                "oer-chips-esp32s31-hal",
            ],
        ),
        // A package without a listed role is the completeness rule's failure,
        // not an allowed edge.
        member(
            "chips/esp32s31/unlisted",
            "oer-esp32s31-unlisted",
            Built::Library,
            &["oer-protocols-ieee80211-runtime"],
        ),
    ];
    let edge = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    assert_eq!(
        forbidden_edges(&members, outside_its_role),
        BTreeSet::from([
            edge("chips/esp32s31/hal", "chips/esp32s31/radio/phy"),
            edge(
                "chips/esp32s31/radio/ieee80211/backend",
                "protocols/ieee80211/runtime"
            ),
            edge("hil/images/aes", "chips/esp32s31/hal"),
            edge("hil/images/aes", "chips/esp32s31/radio/ieee80211/station"),
        ])
    );
}

/// A source file outside the images that includes an image source through
/// `#[path]`: the including file and the included file, below the root.
type Inclusion = (String, String);

/// Each `#[path = "..."]` in the tokens of `file` that resolves into
/// an image below `root` ([`image_path`]), at any nesting depth.
fn firmware_paths(tokens: TokenStream, root: &Path, file: &Path, found: &mut BTreeSet<Inclusion>) {
    let mut previous_hash = false;
    for token in tokens {
        let hash = matches!(&token, TokenTree::Punct(hash) if hash.as_char() == '#');
        let after_hash = core::mem::replace(&mut previous_hash, hash);
        let TokenTree::Group(group) = token else {
            continue;
        };
        let attribute = after_hash && group.delimiter() == Delimiter::Bracket;
        if let Some(path) = attribute.then(|| path_attribute(&group.stream())).flatten() {
            let mut resolved = PathBuf::new();
            for component in file.parent().unwrap_or(root).join(&path).components() {
                match component {
                    std::path::Component::ParentDir => {
                        resolved.pop();
                    }
                    std::path::Component::CurDir => {}
                    other => resolved.push(other),
                }
            }
            if let (Ok(including), Ok(included)) =
                (file.strip_prefix(root), resolved.strip_prefix(root))
            {
                let (including, included) = (
                    including.display().to_string(),
                    included.display().to_string(),
                );
                if image_path(&included) && !image_path(&including) {
                    found.insert((including, included));
                }
            }
        } else {
            firmware_paths(group.stream(), root, file, found);
        }
    }
}

/// The inclusions of image sources in the Rust files below `directory`; an
/// unreadable entry or a file that does not tokenize is an error.
fn firmware_inclusions(
    root: &Path,
    directory: &Path,
    found: &mut BTreeSet<Inclusion>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if path.is_dir() {
            if ![".git", "target"].contains(&name.as_deref().unwrap_or_default()) {
                firmware_inclusions(root, &path, found)?;
            }
            continue;
        }
        if path.extension().is_none_or(|extension| extension != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if !source.contains("path") {
            continue;
        }
        let tokens = source
            .parse::<TokenStream>()
            .map_err(|error| format!("{} does not tokenize: {error}", path.display()))?;
        firmware_paths(tokens, root, &path, found);
    }
    Ok(())
}

/// Image sources that a package outside the images includes through
/// `#[path]`, with what removes each.
const FIRMWARE_SOURCES_INCLUDED: &[Deviation] = &[];

#[test]
fn no_package_outside_firmware_includes_an_image_source() {
    let root = root();
    let mut found = BTreeSet::new();
    for directory in ["protocols", "portable", "chips", "tools", "oer"] {
        firmware_inclusions(&root, &root.join(directory), &mut found)
            .expect("the sources are readable");
    }
    let differences = deviation_differences(&found, FIRMWARE_SOURCES_INCLUDED);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn the_inclusion_scan_resolves_nested_path_attributes_into_firmware() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    let tests = root.join("tools/example/tests");
    std::fs::create_dir_all(&tests).expect("test directory");
    std::fs::write(
        tests.join("cases.rs"),
        r##"
#[path = "../../../hil/images/image/src/cache.rs"]
mod cache;
mod nested {
    #[path = "../../../hil/probes/esp32s31-boot-smoke/src/crc.rs"]
    mod crc;
}
#[path = "../src/helper.rs"]
mod helper;
// #[path = "../../../hil/probes/esp32s31-boot-smoke/src/comment.rs"]
const TEXT: &str = "#[path = \"../../../hil/probes/esp32s31-boot-smoke/src/string.rs\"]";
"##,
    )
    .expect("source");
    let mut found = BTreeSet::new();
    firmware_inclusions(root, &root.join("tools"), &mut found).expect("the sources are readable");
    let inclusion = |included: &str| {
        (
            "tools/example/tests/cases.rs".to_owned(),
            included.to_owned(),
        )
    };
    assert_eq!(
        found,
        BTreeSet::from([
            inclusion("hil/probes/esp32s31-boot-smoke/src/crc.rs"),
            inclusion("hil/images/image/src/cache.rs"),
        ])
    );
}

/// Whether an identifier in HAL production code marks waiting or
/// sequencing: an asynchronous item, task polling, an unconditional `loop`,
/// a spin, a deadline, a boot wait or a time instant. `while` is not a
/// marker: const functions iterate with it.
fn hal_sequence_marker(word: &str) -> bool {
    [
        "async",
        "await",
        "Poll",
        "Waker",
        "poll_fn",
        "loop",
        "spin_loop",
        "Instant",
        "BootWait",
    ]
    .contains(&word)
        || word.to_ascii_lowercase().contains("deadline")
}

/// The HAL production files that still hold driver code, with their exact
/// marker counts. The list only shrinks: a lower count lowers its entry, and
/// a file without markers leaves it. No new file belongs here.
const HAL_SEQUENCES: &[(&str, usize)] = &[
    ("chips/esp32s31/hal/src/radio/bluetooth.rs", 10),
    ("chips/esp32s31/hal/src/radio/bluetooth/engine/lease.rs", 4),
    ("chips/esp32s31/hal/src/radio/bluetooth/engine/live.rs", 27),
    ("chips/esp32s31/hal/src/radio/bluetooth/engine/owner.rs", 6),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/hardware.rs",
        13,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/hardware/live.rs",
        17,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/interrupt.rs",
        45,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/timing.rs",
        1,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/modem_timer.rs",
        10,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/scheduler_execution_lock.rs",
        11,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/scheduler_execution_modify.rs",
        26,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/scheduler_stop.rs",
        12,
    ),
    ("chips/esp32s31/hal/src/system/boot.rs", 16),
    ("chips/esp32s31/hal/src/radio/ieee80211/init.rs", 5),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/init/cold/handshake.rs",
        8,
    ),
    ("chips/esp32s31/hal/src/peripherals/entropy.rs", 2),
];

/// The change that moves the driver code out of each HAL module, by path
/// prefix below `chips/esp32s31/hal/src/`.
const HAL_SEQUENCE_REMOVAL: &[(&str, &str)] = &[
    (
        "radio/bluetooth",
        "Bluetooth controller driver out of the HAL (#848)",
    ),
    (
        "system/boot",
        "boot block drivers and the cold-start sequence (#848)",
    ),
    (
        "radio/ieee80211",
        "Wi-Fi hardware engine out of the HAL (#844, #848)",
    ),
    (
        "peripherals/entropy.rs",
        "the entropy read gap in a driver (#848)",
    ),
    ("phy", "PHY analog bus and delays in the PHY driver (#848)"),
];

/// HAL files whose markers a review found to be atoms, not waits or
/// sequences, with the exact count and what each marker is. A changed count
/// fails like a sequence; a new file never joins without that review.
const HAL_MARKERS_IN_ATOMS: &[(&str, usize, &str)] = &[
    (
        "chips/esp32s31/hal/src/system/cores.rs",
        4,
        "a terminal halt after the fatal path requested reset",
    ),
    (
        "chips/esp32s31/hal/src/peripherals/watchdog.rs",
        9,
        "the family watchdog driver's deadline types over the HAL's register port",
    ),
    (
        "chips/esp32s31/hal/src/radio/phy/prelude.rs",
        2,
        "the SDM deadline counter register, sampled",
    ),
    (
        "chips/esp32s31/hal/src/radio/phy/registers.rs",
        1,
        "the SDM deadline counter register, sampled",
    ),
    (
        "chips/esp32s31/hal/src/radio/phy/sequence.rs",
        1,
        "the SDM deadline counter register, sampled",
    ),
];

/// The calls that make a proof of a port's installation state.
const PROOF_ISSUES: [&str; 4] = [
    "Stopped::issue",
    "Starting::issue",
    "Retired::issue",
    "StopRight::issue",
];

fn issues_a_proof(word: &str) -> bool {
    PROOF_ISSUES.contains(&word)
}

/// Whether production code of the package at `path`, in `file`, may make a
/// proof: a port's implementation (the S31 backend's port) and the testing
/// packages (the reference model, the conformance suite's forged proof).
fn may_issue_proofs(file: &str) -> bool {
    file.starts_with("chips/esp32s31/radio/ieee80211/backend/src/port")
        || file.starts_with("protocols/ieee80211/testing/")
}

/// Only a port issues a proof of its state: outside its implementation and
/// the tests, no production code makes one, so none skips the transition
/// the proof stands for. The list of exceptions is empty.
#[test]
fn only_a_port_and_its_tests_issue_its_proofs() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let mut found = BTreeMap::new();
    let mut issuers = 0;
    // The proofs are the port contract's: only it and its dependents can
    // name them.
    let contract = "oer-protocols-ieee80211-contracts-port";
    for (path, manifest) in packages(&root).into_iter().filter(|(_, manifest)| {
        manifest["package"]["name"].as_str() == Some(contract)
            || normal_dependencies(manifest, &workspace["workspace"]).contains(contract)
    }) {
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            for (file, count) in marker_counts(&root, &crate_root, &issues_a_proof) {
                if may_issue_proofs(&file) {
                    issuers += count;
                } else {
                    found.insert(file, count);
                }
            }
        }
    }
    // The scan sees the ports' own issues: it reads the paths it names.
    assert!(issuers > 0, "no port issues a proof");
    assert!(found.is_empty(), "proofs made outside a port: {found:?}");
}

#[test]
fn the_proof_scan_reads_qualified_calls_and_skips_tests() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("lib.rs");
    std::fs::write(
        &source,
        "fn forge() -> Retired<'static> { Retired::issue(StateSerial::FIRST) }
         fn other() { slot.issue(3); Stopped::is_of(&p, s); }
         #[cfg(test)]
mod tests { fn t() { let _ = Stopped::issue(FIRST); } }
",
    )
    .expect("source");
    let mut found = Vec::new();
    production_file_markers(&source, &issues_a_proof, &mut found);
    assert_eq!(found, [format!("{}: Retired::issue", source.display())]);
}

/// Marker counts of the production files reachable from `crate_root`, by
/// path below `root`.
fn marker_counts(root: &Path, crate_root: &Path, marker: &Marker) -> BTreeMap<String, usize> {
    let mut found = Vec::new();
    crate_root_markers(crate_root, marker, &mut found);
    let unreadable = found
        .iter()
        .filter(|entry| entry.ends_with(": unreadable module"))
        .collect::<Vec<_>>();
    assert!(unreadable.is_empty(), "{unreadable:?}");
    let mut counts = BTreeMap::new();
    for entry in found {
        let file = entry
            .rsplit_once(": ")
            .map_or(entry.as_str(), |(file, _)| file);
        let file = Path::new(file)
            .strip_prefix(root)
            .map_or_else(|_| file.to_owned(), |file| file.display().to_string());
        *counts.entry(file).or_insert(0) += 1;
    }
    counts
}

/// Compares marker counts with a known list: a new file, a changed count and
/// an entry whose file has no markers all fail.
fn count_differences(found: &BTreeMap<String, usize>, known: &[(&str, usize)]) -> Vec<String> {
    let known = known
        .iter()
        .map(|(file, count)| ((*file).to_owned(), *count))
        .collect::<BTreeMap<_, _>>();
    let mut differences = Vec::new();
    for (file, count) in found {
        match known.get(file) {
            None => differences.push(format!("new: {file} ({count})")),
            Some(known) if known < count => {
                differences.push(format!("grew: {file} ({known} -> {count})"));
            }
            Some(known) if known > count => {
                differences.push(format!("lower the entry: {file} ({known} -> {count})"));
            }
            Some(_) => {}
        }
    }
    for file in known.keys().filter(|file| !found.contains_key(*file)) {
        differences.push(format!("gone, drop the entry: {file}"));
    }
    differences
}

#[test]
fn hal_production_code_gains_no_waits_or_sequences() {
    let root = root();
    let package = "chips/esp32s31/hal";
    let manifest = packages(&root)
        .remove(package)
        .expect("the HAL is a workspace member");
    let mut found = BTreeMap::new();
    for crate_root in crate_roots(&root.join(package), &manifest) {
        found.extend(marker_counts(&root, &crate_root, &hal_sequence_marker));
    }
    let known = HAL_SEQUENCES
        .iter()
        .copied()
        .chain(
            HAL_MARKERS_IN_ATOMS
                .iter()
                .map(|(file, count, _)| (*file, *count)),
        )
        .collect::<Vec<_>>();
    let files = known.iter().map(|(file, _)| *file).collect::<BTreeSet<_>>();
    assert_eq!(files.len(), known.len(), "a HAL file is listed twice");
    let differences = count_differences(&found, &known);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    let unassigned = HAL_SEQUENCES
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| {
            !HAL_SEQUENCE_REMOVAL.iter().any(|(prefix, _)| {
                file.strip_prefix("chips/esp32s31/hal/src/")
                    .is_some_and(|below| below.starts_with(prefix))
            })
        })
        .collect::<Vec<_>>();
    assert!(unassigned.is_empty(), "no removing series: {unassigned:?}");
}

/// The removal of the Wi-Fi DMA's machines from the HAL.
const WIFI_DMA: &str = "Wi-Fi DMA ownership out of the HAL, with the Wi-Fi quarantines";
/// The removal of the PHY's calibration machines and temperature resource.
const PHY_DRIVER: &str = "to the PHY driver, with the PHY's poison";

/// HAL production code that decides (state machines, error policy,
/// quarantine) beside its owners and atoms, by path below
/// `chips/esp32s31/hal/src/`: a file, or a directory (`…/`) whose production
/// files count together, each with its lines and the change that removes
/// it. The list only shrinks; under [`HAL_DECISION_FILE_ROOTS`] every
/// production file has an entry of its own, so no new file joins there.
const HAL_DECISIONS: &[(&str, usize, &str)] = &[
    ("radio/ieee80211/dma/descriptor.rs", 362, WIFI_DMA),
    ("radio/ieee80211/dma/ring_index.rs", 72, WIFI_DMA),
    ("radio/ieee80211/dma/rx_dma.rs", 527, WIFI_DMA),
    ("radio/ieee80211/dma/rx_dma/model.rs", 580, WIFI_DMA),
    ("radio/ieee80211/dma/rx_ring.rs", 3096, WIFI_DMA),
    ("radio/ieee80211/dma/rx_storage.rs", 1198, WIFI_DMA),
    ("radio/ieee80211/dma/rx_storage/buffers.rs", 511, WIFI_DMA),
    ("radio/ieee80211/dma/rx_storage/host_dma.rs", 203, WIFI_DMA),
    ("radio/ieee80211/dma/space.rs", 95, WIFI_DMA),
    ("radio/ieee80211/dma/tx_ampdu_storage.rs", 1375, WIFI_DMA),
    ("radio/ieee80211/dma/tx_storage.rs", 565, WIFI_DMA),
    (
        "radio/bluetooth/engine/",
        15970,
        "Bluetooth controller driver out of the HAL (#848)",
    ),
    ("radio/phy/i2c.rs", 685, PHY_DRIVER),
    ("radio/phy/restore.rs", 368, PHY_DRIVER),
    ("radio/phy/temperature.rs", 152, PHY_DRIVER),
    ("radio/phy/temperature/", 78, PHY_DRIVER),
    ("radio/domain/transaction.rs", 542, PHY_DRIVER),
    ("radio/domain/transaction/", 321, PHY_DRIVER),
];

/// Directories below the HAL's sources whose every production file is an
/// entry of [`HAL_DECISIONS`] by itself.
const HAL_DECISION_FILE_ROOTS: &[&str] = &["radio/ieee80211/dma/"];

/// The production files reachable from `crate_root`, by path below `root`:
/// every module the production tree follows, its test modules skipped.
fn production_files(root: &Path, crate_root: &Path) -> BTreeSet<String> {
    marker_counts(root, crate_root, &|_: &str| true)
        .into_keys()
        .collect()
}

/// Lines of each entry of `known`, from the production `files` (paths below
/// `prefix`) and the file system below `root`: a file's own lines, a
/// directory's production files summed; and each production file below a
/// file root that has no entry of its own, by itself.
fn decision_lines(
    root: &Path,
    prefix: &str,
    files: &BTreeSet<String>,
    known: &[(&str, usize, &str)],
    file_roots: &[&str],
) -> BTreeMap<String, usize> {
    let lines = |file: &str| {
        std::fs::read_to_string(root.join(file)).map_or(0, |source| source.lines().count())
    };
    let mut found = BTreeMap::new();
    for (entry, _, _) in known {
        let total = if entry.ends_with('/') {
            files
                .iter()
                .filter(|file| {
                    file.strip_prefix(prefix)
                        .is_some_and(|below| below.starts_with(entry))
                })
                .map(|file| lines(file))
                .sum()
        } else if files.contains(&format!("{prefix}{entry}")) {
            lines(&format!("{prefix}{entry}"))
        } else {
            0
        };
        if total != 0 {
            found.insert((*entry).to_owned(), total);
        }
    }
    for file in files {
        let Some(below) = file.strip_prefix(prefix) else {
            continue;
        };
        if file_roots.iter().any(|root| below.starts_with(root)) && !found.contains_key(below) {
            found.insert(below.to_owned(), lines(file));
        }
    }
    found
}

#[test]
fn hal_code_that_decides_is_listed_and_only_shrinks() {
    let root = root();
    let package = "chips/esp32s31/hal";
    let manifest = packages(&root)
        .remove(package)
        .expect("the HAL is a workspace member");
    let mut files = BTreeSet::new();
    for crate_root in crate_roots(&root.join(package), &manifest) {
        files.extend(production_files(&root, &crate_root));
    }
    let found = decision_lines(
        &root,
        "chips/esp32s31/hal/src/",
        &files,
        HAL_DECISIONS,
        HAL_DECISION_FILE_ROOTS,
    );
    let known = HAL_DECISIONS
        .iter()
        .map(|(entry, lines, _)| (*entry, *lines))
        .collect::<Vec<_>>();
    let differences = count_differences(&found, &known);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn the_decision_list_takes_directories_whole_and_refuses_a_new_file_under_a_file_root() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    for (file, lines) in [
        ("src/dma/ring.rs", 3),
        ("src/dma/new.rs", 2),
        ("src/engine/a.rs", 4),
        ("src/engine/b/c.rs", 5),
        ("src/other.rs", 7),
    ] {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("directory");
        std::fs::write(path, "x\n".repeat(lines)).expect("file");
    }
    let files = [
        "src/dma/ring.rs",
        "src/dma/new.rs",
        "src/engine/a.rs",
        "src/engine/b/c.rs",
        "src/other.rs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    let known: &[(&str, usize, &str)] = &[("dma/ring.rs", 3, "x"), ("engine/", 9, "y")];
    let found = decision_lines(root, "src/", &files, known, &["dma/"]);
    let known = known
        .iter()
        .map(|(entry, lines, _)| (*entry, *lines))
        .collect::<Vec<_>>();
    // A directory counts its files together; a new file under a file root
    // is its own new entry; a file outside both is not taken.
    assert_eq!(count_differences(&found, &known), ["new: dma/new.rs (2)"]);
}

/// Whether a package's production sources are library code an image may
/// reach, for the `dyn` scan: every package under `portable/`,
/// `protocols/`, `chips/`, `hil/images`, `hil/services` and `hil/agent`, and
/// the entry `oer`.
fn reaches_images(path: &str) -> bool {
    [
        "portable/",
        "protocols/",
        "chips/",
        "hil/images/",
        "hil/services/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || ["oer", "hil/agent"].contains(&path)
}

fn trait_object(word: &str) -> bool {
    word == "dyn"
}

/// The production files of those packages that still hold a trait object,
/// with their exact `dyn` counts and why each stays or what removes it. The
/// list only shrinks.
const TRAIT_OBJECTS: &[(&str, usize, &str)] = &[
    // Product code: each goes with a generic parameter or an enum (T8b).
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_storage.rs",
        2,
        "product: `&mut dyn FnMut(&[u8])` of `read_chain`, removed in T8b",
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_ampdu_storage.rs",
        1,
        "product: `&mut dyn FnMut(&[u8])` of `read_chain`, removed in T8b",
    ),
    (
        "chips/esp32s31/platform/runtime/src/sampler.rs",
        1,
        "product: `&mut dyn FnMut()` of the masked section, removed in T8b",
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs",
        1,
        "product: `&mut dyn FindOldest`, removed in T8b",
    ),
    (
        "protocols/ieee802154/port/src/owner.rs",
        1,
        "product: `&dyn Release`, removed in T8b",
    ),
    // Never in an image: each stays.
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_dma/model.rs",
        2,
        "stays: the receive walker's host model, behind `cfg(any(test, feature = \"test-support\"))`",
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs",
        2,
        "stays: the hardware model, behind `test` or host `test-support`",
    ),
    (
        "chips/esp32s31/platform/layout/src/lib.rs",
        3,
        "stays: the layout crate's host half, behind the `build` feature its build scripts take",
    ),
    (
        "chips/family/platform/layout/src/build.rs",
        5,
        "stays: the layout crate's host half, behind the `build` feature its build scripts take",
    ),
    (
        "chips/family/facts/src/layers.rs",
        5,
        "stays: the facts' layer loader, a build-dependency of image crates and a host tool's",
    ),
    (
        "chips/family/facts/src/lib.rs",
        2,
        "stays: the facts' layer loader, a build-dependency of image crates and a host tool's",
    ),
    (
        "protocols/ieee80211/testing/model/src/air_time.rs",
        2,
        "stays: the IEEE 802.11 reference model, a testing package",
    ),
    (
        "protocols/ieee80211/testing/model/src/data_port.rs",
        1,
        "stays: the IEEE 802.11 reference model, a testing package",
    ),
    (
        "protocols/ieee80211/testing/model/src/lib.rs",
        3,
        "stays: the IEEE 802.11 reference model, a testing package",
    ),
];

#[test]
fn the_trait_object_scan_takes_library_packages_and_skips_tests_and_tools() {
    for library in [
        "portable/time",
        "protocols/network/core",
        "chips/esp32s31/hal",
        "oer",
        "hil/agent",
        "hil/images/wifi-station",
        "hil/services/load",
    ] {
        assert!(reaches_images(library), "{library}");
    }
    for other in [
        "tools/image/build",
        "hil/scenarios/wifi-load",
        "hil/probes/esp32s31-mcycle",
        "examples/wifi-station",
    ] {
        assert!(!reaches_images(other), "{other}");
    }
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("lib.rs");
    std::fs::write(
        &source,
        "pub fn run(f: &mut dyn FnMut()) {}\n#[cfg(test)]\nmod tests { fn t(_: &dyn Fn()) {} }\n// a dyn in a comment\n",
    )
    .expect("source");
    let mut found = Vec::new();
    production_file_markers(&source, &trait_object, &mut found);
    assert_eq!(found, [format!("{}: dyn", source.display())]);
}

#[test]
fn library_code_holds_no_trait_object() {
    let root = root();
    let mut found = BTreeMap::new();
    for (path, manifest) in packages(&root)
        .into_iter()
        .filter(|(path, _)| reaches_images(path))
    {
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            found.extend(marker_counts(&root, &crate_root, &trait_object));
        }
    }
    let known = TRAIT_OBJECTS
        .iter()
        .map(|(file, count, _)| (*file, *count))
        .collect::<Vec<_>>();
    let differences = count_differences(&found, &known);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// `Trait::method` of each method of a trait in `tokens` whose return type
/// is `bool`; items configured for tests are skipped.
fn bool_trait_methods(tokens: TokenStream, found: &mut BTreeSet<String>) {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    let mut index = 0;
    let mut test = false;
    while let Some(token) = tokens.get(index) {
        index += 1;
        match token {
            TokenTree::Punct(hash) if hash.as_char() == '#' => {
                if let Some(TokenTree::Group(attribute)) = tokens.get(index)
                    && attribute.delimiter() == Delimiter::Bracket
                {
                    test |= test_only(&attribute.stream());
                    index += 1;
                }
                continue;
            }
            TokenTree::Ident(word) if word == "trait" && !test => {
                let Some(TokenTree::Ident(name)) = tokens.get(index) else {
                    continue;
                };
                // The trait's body: its first brace group.
                let body = tokens.iter().skip(index).find_map(|token| match token {
                    TokenTree::Group(body) if body.delimiter() == Delimiter::Brace => Some(body),
                    _ => None,
                });
                if let Some(body) = body {
                    bool_methods(&name.to_string(), body.stream(), found);
                }
            }
            TokenTree::Group(group) if !test => bool_trait_methods(group.stream(), found),
            _ => {}
        }
        if !matches!(token, TokenTree::Ident(word) if word == "pub") {
            test = false;
        }
    }
}

/// The methods of trait `name`'s `body` that return `bool`.
fn bool_methods(name: &str, body: TokenStream, found: &mut BTreeSet<String>) {
    let tokens = body.into_iter().collect::<Vec<_>>();
    let mut test = false;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Group(attribute)
                if attribute.delimiter() == Delimiter::Bracket
                    && matches!(index.checked_sub(1).and_then(|at| tokens.get(at)), Some(TokenTree::Punct(hash)) if hash.as_char() == '#') =>
            {
                test |= test_only(&attribute.stream());
            }
            TokenTree::Ident(word) if word == "fn" => {
                let method = tokens.get(index + 1);
                // `-> bool` ends the signature: before `;`, `{` or `where`.
                let mut at = index + 2;
                let mut returns_bool = false;
                while let Some(token) = tokens.get(at) {
                    match token {
                        TokenTree::Punct(end) if end.as_char() == ';' => break,
                        TokenTree::Group(body) if body.delimiter() == Delimiter::Brace => break,
                        TokenTree::Punct(dash)
                            if dash.as_char() == '-'
                                && matches!(tokens.get(at + 1), Some(TokenTree::Punct(arrow)) if arrow.as_char() == '>') =>
                        {
                            returns_bool = matches!(tokens.get(at + 2), Some(TokenTree::Ident(ty)) if ty == "bool")
                                && !matches!(tokens.get(at + 3), Some(TokenTree::Punct(more)) if more.as_char() == ':' || more.as_char() == '<');
                            break;
                        }
                        _ => {}
                    }
                    at += 1;
                }
                if let (Some(TokenTree::Ident(method)), true, false) = (method, returns_bool, test)
                {
                    found.insert(format!("{name}::{method}"));
                }
                test = false;
            }
            TokenTree::Punct(end) if end.as_char() == ';' => test = false,
            _ => {}
        }
    }
}

/// The methods of a trait in production code of `protocols/` and `chips/`
/// that answer a bare `bool`, each a predicate of one fact (which stays) or
/// a decision with the package that removes it. The list only shrinks.
/// A register or hardware port's sampled bit: a predicate of one fact.
const SAMPLED_BIT: &str = "predicate: a sampled bit of the hardware";
/// A conformance harness's observation, in a testing package.
const HARNESS: &str = "predicate: a test harness observation (testing package)";
const P1: &str = "decision: P1, the installation in the port contract";
const P2: &str = "decision: P2, the installation machine and the composition";
const P3: &str = "decision: P3, the engine";
const PHASE_C: &str = "decision: phase C, the network owner in the network runtime";
const RADIO_SYSTEM: &str = "decision: no package yet (the radio system and Bluetooth)";

const BOOL_TRAIT_METHODS: &[(&str, &str)] = &[
    ("AccessPointHarness::holds_beacon", HARNESS),
    ("AccessPointHarness::idle", HARNESS),
    ("AccessPointHarness::install_station_key", HARNESS),
    ("AccessPointHarness::take_station_configured", HARNESS),
    (
        "AirWorld::land",
        "predicate: a frame landed (reference model, testing package)",
    ),
    ("Alarm::alarm_at", SAMPLED_BIT),
    ("Alarm::start", SAMPLED_BIT),
    ("AnalogI2cPowerAccess::analog_i2c_is_powered", SAMPLED_BIT),
    (
        "AnalogI2cPowerAccess::analog_i2c_reset_is_released",
        SAMPLED_BIT,
    ),
    ("BankControl::fault_acknowledged", SAMPLED_BIT),
    ("BluetoothBackend::admits", RADIO_SYSTEM),
    (
        "BluetoothBackend::is_stopped",
        "predicate: the backend stopped",
    ),
    (
        "BluetoothSchedulerCancellationControl::observe_status_0208",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerCancellationControl::observe_status_0324",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerInsertionExecutionObservationControl::observe_execution_lock_ready",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerInsertionExecutionObservationControl::observe_execution_modify_ready",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerInsertionExecutionObservationControl::observe_execution_modify_rejected",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerSoftwareListRemovalControl::read_command_0_status_26",
        SAMPLED_BIT,
    ),
    (
        "BluetoothSchedulerSoftwareListRemovalControl::read_command_1_status_18",
        SAMPLED_BIT,
    ),
    ("CacheSyncRegisters::done", SAMPLED_BIT),
    (
        "Capability::supported",
        "predicate: a capability is supported",
    ),
    ("CcmpKeyHardware::ccmp_entry_is_valid", SAMPLED_BIT),
    ("ClientBackend::admits", RADIO_SYSTEM),
    ("ClockUpdate::update_pending", SAMPLED_BIT),
    ("Control::btbb_held", SAMPLED_BIT),
    ("Control::commands_ready", SAMPLED_BIT),
    ("Control::engines_idle", SAMPLED_BIT),
    ("Control::is_busy", SAMPLED_BIT),
    ("Control::ready", SAMPLED_BIT),
    ("Control::rejected", SAMPLED_BIT),
    ("Control::reset_released", SAMPLED_BIT),
    ("Control::time_pending", SAMPLED_BIT),
    ("Control::timer_separated", SAMPLED_BIT),
    (
        "ControllerTimeRegisters::latch_request_pending",
        SAMPLED_BIT,
    ),
    (
        "Correlation::is_backend_reserved",
        "predicate: the correlation is the backend's",
    ),
    ("Count::on", "predicate: the switchable diagnostics are on"),
    ("CounterLatch::valid", SAMPLED_BIT),
    ("DataPort::lend_room", P3),
    ("DataPortHarness::publish_held_arena", HARNESS),
    ("DataPortHarness::take_configured", HARNESS),
    ("Device::cut_held_by_units", P3),
    ("Device::event_waiting", P3),
    ("Device::is_running", P2),
    ("Device::returns_quiet", P3),
    (
        "Diagnostics::on",
        "predicate: the switchable diagnostics are on",
    ),
    ("DriverEnv::halted", P2),
    ("DriverEnv::reject_group_replacement", P3),
    ("DriverEnv::stopping", P2),
    ("EarlyText::text", "predicate: the bytes are whole text"),
    ("EndpointRegisters::has_data", SAMPLED_BIT),
    ("EndpointRegisters::has_room", SAMPLED_BIT),
    ("ExecutorPlatform::debugger_connected", SAMPLED_BIT),
    ("ExecutorPlatform::is_pending", SAMPLED_BIT),
    ("FetchSuspendRegisters::autoload_done", SAMPLED_BIT),
    ("FetchSuspendRegisters::autoload_enabled", SAMPLED_BIT),
    ("FetchSuspendRegisters::freeze_done", SAMPLED_BIT),
    ("FetchSuspendRegisters::lock_done", SAMPLED_BIT),
    ("FetchSuspendRegisters::preload_done", SAMPLED_BIT),
    ("FetchSuspendRegisters::sync_done", SAMPLED_BIT),
    ("FlashCommandRegisters::busy", SAMPLED_BIT),
    ("FlashCommandRegisters::mode_bits_from_status", SAMPLED_BIT),
    ("FramePort::is_fault", PHASE_C),
    (
        "FramePort::is_final",
        "predicate: whether the port ended the frame",
    ),
    ("FrameSource::register", PHASE_C),
    ("Hooks::observing", "predicate: the trace observes"),
    (
        "Hooks::recycled_probe_pending",
        "predicate: a probe is pending (trace hooks)",
    ),
    ("Ieee802154LowLevel::cca_busy", SAMPLED_BIT),
    ("Ieee802154LowLevel::etm_channel_enabled", SAMPLED_BIT),
    ("Ieee802154LowLevel::is_current_rx_frame", SAMPLED_BIT),
    ("Ieee802154LowLevel::pending_mode", SAMPLED_BIT),
    ("Ieee802154LowLevel::rx_auto_ack", SAMPLED_BIT),
    ("Ieee802154LowLevel::tx_auto_ack", SAMPLED_BIT),
    ("Ieee802154LowLevel::tx_enhanced_ack", SAMPLED_BIT),
    ("Io::ready", SAMPLED_BIT),
    ("Local::again", PHASE_C),
    ("Local::deliver", PHASE_C),
    ("Local::offers", PHASE_C),
    ("Local::register", PHASE_C),
    (
        "MacColdHandshakeHardware::sample_cold_handshake_ready",
        SAMPLED_BIT,
    ),
    (
        "MacRuntimeLowRateHardware::phy_low_rate_enabled",
        SAMPLED_BIT,
    ),
    (
        "Mask::take_interrupts",
        "predicate: interrupts were enabled before the mask",
    ),
    ("Memory::retirement_ready", SAMPLED_BIT),
    ("Memory::scheduler_stopped", SAMPLED_BIT),
    (
        "ModemClockDependency::refcounted",
        "predicate: a fact of the modem clock's dependency (chip knowledge)",
    ),
    (
        "ModemClockDependency::retained_while_wifi_initialized",
        "predicate: a fact of the modem clock's dependency (chip knowledge)",
    ),
    (
        "ModemLpTimerHandlerRegisterTransaction::sample_state_0024_low_byte_nonzero",
        SAMPLED_BIT,
    ),
    (
        "ModemLpTimerHandlerRegisterTransaction::sample_state_002c_low_byte_nonzero",
        SAMPLED_BIT,
    ),
    (
        "ModemLpTimerInterruptTransaction::control_0058_bit_2_is_set",
        SAMPLED_BIT,
    ),
    (
        "ModemLpTimerLowPowerInitTransaction::control_2_is_set",
        SAMPLED_BIT,
    ),
    (
        "ModemLpTimerRuntimeTransaction::rollover_low_byte_nonzero",
        SAMPLED_BIT,
    ),
    ("Observation::rx_walker_enabled", SAMPLED_BIT),
    ("OwnedTransmit::notice_waiting", PHASE_C),
    ("ParallelAnalogBus::is_busy", SAMPLED_BIT),
    ("PbusAccess::is_busy", SAMPLED_BIT),
    ("PbusContext::wifi_baseband_enabled", SAMPLED_BIT),
    ("PhyI2cParallelAccess::is_busy", SAMPLED_BIT),
    (
        "PhyShortDelay::settle_micros",
        "decision: whether a short settle was taken; no package yet (the PHY driver, with the PHY's poison)",
    ),
    ("PllRegisters::calibration_done", SAMPLED_BIT),
    (
        "PowerBaselinePort::bus_matches",
        "predicate: a register matches the cold baseline",
    ),
    (
        "PowerBaselinePort::source_matches",
        "predicate: a register matches the cold baseline",
    ),
    (
        "PowerBaselinePort::syscon_matches",
        "predicate: a register matches the cold baseline",
    ),
    ("Produce::current", PHASE_C),
    ("Produce::register", PHASE_C),
    ("PsramCommandRegisters::busy", SAMPLED_BIT),
    ("RadioController::can_prepare", RADIO_SYSTEM),
    (
        "RadioController::is_retired",
        "predicate: the controller retired",
    ),
    ("Registers::mode1_reset", SAMPLED_BIT),
    ("Registers::mode1_software", SAMPLED_BIT),
    ("Registers::rtc_wake_pending", SAMPLED_BIT),
    ("RuntimeRegisters::held", SAMPLED_BIT),
    ("RuntimeRegisters::is_stalled", SAMPLED_BIT),
    ("RxDma::reload_pending", SAMPLED_BIT),
    ("RxDma::walker_enabled", SAMPLED_BIT),
    ("SamplePort::is_busy", SAMPLED_BIT),
    ("SamplerTimer::acknowledge", SAMPLED_BIT),
    ("SamplerTimer::alarm_at", SAMPLED_BIT),
    ("SamplerTimer::arm_after", SAMPLED_BIT),
    ("SchedulerRoleStorage::admits", RADIO_SYSTEM),
    ("SchedulerTaskPort::commands_ready", SAMPLED_BIT),
    ("SchedulerTaskPort::execution_modify_ready", SAMPLED_BIT),
    ("SchedulerTaskPort::execution_modify_rejected", SAMPLED_BIT),
    ("SecondaryFetchRegisters::autoload_done", SAMPLED_BIT),
    ("SecondaryFetchRegisters::autoload_enabled", SAMPLED_BIT),
    ("SecondaryFetchRegisters::freeze_done", SAMPLED_BIT),
    ("SecondaryFetchRegisters::preload_done", SAMPLED_BIT),
    (
        "SequenceSpace::precedes",
        "predicate: sequence-number order (IEEE 802.11 modulo arithmetic)",
    ),
    ("SharedPhyAccess::analog_i2c_is_powered", SAMPLED_BIT),
    ("SharedPhyAccess::analog_i2c_reset_is_released", SAMPLED_BIT),
    ("SharedPhyAccess::tx_dc_measurement_is_ready", SAMPLED_BIT),
    ("TaskControl::head_present", SAMPLED_BIT),
    ("TxArena::claim_arenas", P3),
    ("TxArena::event_waiting", P3),
    (
        "TxHardware::begin_tx_timeout_abort",
        "decision: P3, the transmit slot the hardware never ends",
    ),
];

#[test]
fn the_bool_scan_takes_trait_methods_and_leaves_tests_free_functions_and_other_types() {
    let source = r#"
pub trait Port {
    fn ready(&self) -> bool;
    fn taken(&mut self) -> Result<bool, ()>;
    fn proof(&self) -> Option<Proof>;
    #[cfg(test)]
    fn fixture(&self) -> bool;
    fn with_default(&self) -> bool { true }
    fn generic<T>(&self, value: T) -> bool where T: Copy;
}
pub fn free() -> bool { false }
impl Port for Thing { fn ready(&self) -> bool { true } }
#[cfg(test)]
trait Fake { fn fake(&self) -> bool; }
mod inner { pub(crate) trait Nested { fn nested(&self) -> bool; } }
"#;
    let mut found = BTreeSet::new();
    bool_trait_methods(source.parse().expect("tokens"), &mut found);
    assert_eq!(
        found.into_iter().collect::<Vec<_>>(),
        [
            "Nested::nested",
            "Port::generic",
            "Port::ready",
            "Port::with_default"
        ]
    );
}

#[test]
fn contracts_answer_no_bare_yes_or_no() {
    let root = root();
    let mut found = BTreeSet::new();
    for (path, manifest) in packages(&root)
        .into_iter()
        .filter(|(path, _)| path.starts_with("protocols/") || path.starts_with("chips/"))
    {
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            for file in production_files(&root, &crate_root) {
                let tokens = std::fs::read_to_string(root.join(&file))
                    .ok()
                    .and_then(|source| source.parse::<TokenStream>().ok());
                match tokens {
                    Some(tokens) => bool_trait_methods(tokens, &mut found),
                    None => panic!("unreadable production file {file}"),
                }
            }
        }
    }
    let known = BOOL_TRAIT_METHODS
        .iter()
        .map(|(method, _)| (*method).to_owned())
        .collect::<BTreeSet<_>>();
    let new = found.difference(&known).collect::<Vec<_>>();
    let gone = known.difference(&found).collect::<Vec<_>>();
    assert!(
        new.is_empty() && gone.is_empty(),
        "new: {new:?}\ngone, drop the entry: {gone:?}"
    );
}

#[test]
fn hal_markers_name_waits_and_leave_settles_and_const_iteration() {
    for word in [
        "async",
        "await",
        "Poll",
        "poll_fn",
        "loop",
        "spin_loop",
        "Instant",
        "BootWait",
        "BootDeadline",
        "deadline",
        "DeadlineLease",
    ] {
        assert!(hal_sequence_marker(word), "{word}");
    }
    for word in [
        "while", "for", "settle", "Settle", "write", "modify", "Timer",
    ] {
        assert!(!hal_sequence_marker(word), "{word}");
    }
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("src");
    std::fs::create_dir_all(source.join("boot")).expect("module directory");
    std::fs::write(
        source.join("lib.rs"),
        "pub mod boot;\npub mod atoms;\n#[cfg(test)]\nmod tests;\n",
    )
    .expect("root");
    std::fs::write(
        source.join("boot.rs"),
        "pub mod wait;\npub fn select(deadline: &BootDeadline) { loop {} }\n",
    )
    .expect("boot");
    std::fs::write(
        source.join("boot/wait.rs"),
        "pub async fn ready() { poll().await }\n",
    )
    .expect("wait");
    std::fs::write(
        source.join("atoms.rs"),
        "pub const fn mask(v: &[u8]) { let mut i = 0; while i < v.len() { i += 1; } }\n",
    )
    .expect("atoms");
    std::fs::write(source.join("tests.rs"), "async fn fixture() {}").expect("tests");
    assert_eq!(
        marker_counts(
            directory.path(),
            &source.join("lib.rs"),
            &hal_sequence_marker
        ),
        BTreeMap::from([("src/boot.rs".into(), 3), ("src/boot/wait.rs".into(), 2)])
    );
}

#[test]
fn the_hal_ratchet_fails_on_a_new_file_a_changed_count_and_a_gone_entry() {
    let found = BTreeMap::from([
        ("same.rs".to_owned(), 2),
        ("grown.rs".to_owned(), 3),
        ("lowered.rs".to_owned(), 1),
        ("new.rs".to_owned(), 1),
    ]);
    let known = [
        ("same.rs", 2),
        ("grown.rs", 2),
        ("lowered.rs", 4),
        ("gone.rs", 1),
    ];
    assert_eq!(
        count_differences(&found, &known),
        [
            "grew: grown.rs (2 -> 3)",
            "lower the entry: lowered.rs (4 -> 1)",
            "new: new.rs (1)",
            "gone, drop the entry: gone.rs",
        ]
    );
    assert!(
        count_differences(
            &found,
            &[
                ("same.rs", 2),
                ("grown.rs", 3),
                ("lowered.rs", 1),
                ("new.rs", 1)
            ]
        )
        .is_empty()
    );
}

/// Whether `role` is a testing package: a backend conformance suite or a
/// reference model.
fn testing_role(role: Role) -> bool {
    matches!(
        role,
        Role::Protocol(ProtocolRole::Conformance | ProtocolRole::Model | ProtocolRole::CoreTesting)
    )
}

/// The normal or build edges by which a package other than a testing
/// package reaches one; dev dependencies are outside `Member`.
fn testing_edges(members: &[Member], roles: &BTreeMap<String, Role>) -> BTreeSet<Edge> {
    members
        .iter()
        .filter(|member| !role(&member.path).is_some_and(testing_role))
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .chain(&member.build_dependencies)
                .filter(|dependency| roles.get(*dependency).copied().is_some_and(testing_role))
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect()
}

#[test]
fn no_product_package_depends_on_a_conformance_suite_or_a_model() {
    let members = workspace_members();
    assert!(
        members
            .iter()
            .any(|member| member.path == "protocols/ieee80211/testing/conformance")
    );
    let edges = testing_edges(&members, &workspace_roles());
    assert!(edges.is_empty(), "{edges:?}");
}

/// The installation core over a real port joins a core with a port: no
/// package takes it as a normal or build dependency, a model or a
/// conformance suite included; the backends and the model run it as a
/// dev-dependency.
#[test]
fn only_dev_dependencies_take_a_core_testing_package() {
    let members = workspace_members();
    let roles = workspace_roles();
    assert!(
        roles
            .values()
            .any(|role| *role == Role::Protocol(ProtocolRole::CoreTesting))
    );
    let edges: BTreeSet<Edge> = members
        .iter()
        .flat_map(|member| {
            member
                .dependencies
                .iter()
                .chain(&member.build_dependencies)
                .filter(|dependency| {
                    roles.get(*dependency) == Some(&Role::Protocol(ProtocolRole::CoreTesting))
                })
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect();
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn a_product_edge_to_a_testing_package_is_found_and_one_between_them_is_not() {
    let model = member(
        "protocols/ieee80211/testing/model",
        "oer-protocols-ieee80211-testing-model",
        Built::Library,
        &["oer-protocols-ieee80211-testing-conformance"],
    );
    let mut runtime = member(
        "protocols/ieee80211/runtime",
        "oer-protocols-ieee80211-runtime",
        Built::Library,
        &["oer-protocols-ieee80211-contracts-port"],
    );
    runtime
        .build_dependencies
        .insert("oer-protocols-ieee80211-testing-model".to_owned());
    let core = member(
        "protocols/ieee80211/core/upper-mac",
        "oer-protocols-ieee80211-core-upper-mac",
        Built::Library,
        &["oer-protocols-ieee80211-testing-conformance"],
    );
    let roles = BTreeMap::from([
        (
            "oer-protocols-ieee80211-testing-conformance".to_owned(),
            Role::Protocol(ProtocolRole::Conformance),
        ),
        (
            "oer-protocols-ieee80211-testing-model".to_owned(),
            Role::Protocol(ProtocolRole::Model),
        ),
        (
            "oer-protocols-ieee80211-contracts-port".to_owned(),
            Role::Protocol(ProtocolRole::Port),
        ),
    ]);
    assert_eq!(
        testing_edges(&[model, runtime, core], &roles),
        [
            (
                "protocols/ieee80211/core/upper-mac",
                "oer-protocols-ieee80211-testing-conformance"
            ),
            (
                "protocols/ieee80211/runtime",
                "oer-protocols-ieee80211-testing-model"
            ),
        ]
        .map(|(path, dependency)| (path.to_owned(), dependency.to_owned()))
        .into()
    );
}

/// Every network package and the workspace packages it may take, normal or
/// build: the codec stands on `oer-portable-wire` alone and the contract on the codec.
const NETWORK_LAYERS: [(&str, &[&str]); 5] = [
    ("protocols/network/wire", &["oer-portable-wire"]),
    ("protocols/network/port", &["oer-protocols-network-wire"]),
    (
        "protocols/network/core",
        &[
            "oer-portable-events",
            "oer-portable-time",
            "oer-portable-wire",
            "oer-protocols-network-port",
            "oer-protocols-network-wire",
        ],
    ),
    (
        "protocols/network/runtime",
        &[
            "oer-portable-events",
            "oer-portable-memory",
            "oer-portable-time",
            "oer-protocols-network-core",
            "oer-protocols-network-port",
            "oer-protocols-network-wire",
        ],
    ),
    (
        "protocols/network/testing/model",
        &[
            "oer-portable-time",
            "oer-protocols-ieee80211-core-upper-mac",
            "oer-protocols-ieee80211-runtime",
            "oer-protocols-network-port",
            "oer-protocols-network-wire",
        ],
    ),
];

/// The network packages missing from [`NETWORK_LAYERS`], and the workspace
/// edges a listed one takes beyond its entry.
fn network_layer_violations(
    members: &[Member],
    workspace: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeSet<Edge>) {
    let network = members
        .iter()
        .filter(|member| member.path.starts_with("protocols/network/"));
    let unlisted = network
        .clone()
        .filter(|member| !NETWORK_LAYERS.iter().any(|(path, _)| *path == member.path))
        .map(|member| member.path.clone())
        .collect();
    let edges = network
        .filter_map(|member| {
            let (_, allowed) = NETWORK_LAYERS
                .iter()
                .find(|(path, _)| *path == member.path)?;
            Some((member, *allowed))
        })
        .flat_map(|(member, allowed)| {
            member
                .dependencies
                .iter()
                .chain(&member.build_dependencies)
                .filter(|dependency| {
                    workspace.contains(*dependency) && !allowed.contains(&dependency.as_str())
                })
                .map(|dependency| (member.path.clone(), dependency.clone()))
        })
        .collect();
    (unlisted, edges)
}

#[test]
fn every_network_package_has_an_explicit_layer() {
    let members = workspace_members();
    let workspace = members.iter().map(|member| member.name.clone()).collect();
    let (unlisted, edges) = network_layer_violations(&members, &workspace);
    assert!(
        unlisted.is_empty(),
        "unlisted network packages: {unlisted:?}"
    );
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn an_unlisted_network_package_and_a_contract_on_another_protocol_are_found() {
    let port = member(
        "protocols/network/port",
        "oer-protocols-network-port",
        Built::Library,
        &[
            "oer-protocols-network-wire",
            "oer-protocols-ieee80211-wire",
            "heapless",
        ],
    );
    let codec = member(
        "protocols/network/wire",
        "oer-protocols-network-wire",
        Built::Library,
        &["oer-portable-wire"],
    );
    let stack = member(
        "protocols/network/stack",
        "oer-network-stack",
        Built::Library,
        &[],
    );
    let workspace = [
        "oer-protocols-network-wire",
        "oer-protocols-network-port",
        "oer-protocols-ieee80211-wire",
        "oer-portable-wire",
    ]
    .map(str::to_owned)
    .into();
    let (unlisted, edges) = network_layer_violations(&[port, codec, stack], &workspace);
    assert_eq!(unlisted, ["protocols/network/stack".to_owned()].into());
    assert_eq!(
        edges,
        [(
            "protocols/network/port".to_owned(),
            "oer-protocols-ieee80211-wire".to_owned()
        )]
        .into()
    );
}

// ---- T8c: the rules without a check get a list that only shrinks ----------

/// A finder of one scan: what the token at `at` of `tokens` is, when the
/// scan takes it.
type Finder = dyn Fn(&[TokenTree], usize) -> Option<String>;
/// A finder of attributes.
type AttributeFinder = dyn Fn(&TokenStream) -> Option<String>;
/// A package selector.
type Select = dyn Fn(&str) -> bool;
/// One scan: its list's name, its packages, its finders.
type Scan<'s> = (&'s str, &'s Select, &'s Finder, &'s AttributeFinder);

/// Every hit of `find` and `attribute` in the production code of the
/// packages `select` takes, each with its file below the root.
fn scan_hits(
    select: &dyn Fn(&str) -> bool,
    find: &Finder,
    attribute: &AttributeFinder,
) -> Vec<(String, String)> {
    let root = root();
    let mut hits = Vec::new();
    let mut unreadable = Vec::new();
    for (path, manifest) in packages(&root) {
        if !select(&path) {
            continue;
        }
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            walk_crate_root(&crate_root, &mut |file: &Path, seen: Seen<'_>| {
                let file = file.strip_prefix(&root).map_or_else(
                    |_| file.display().to_string(),
                    |file| file.display().to_string(),
                );
                if file.ends_with("generated.rs") {
                    return;
                }
                match seen {
                    Seen::Unreadable => unreadable.push(file),
                    Seen::Attribute(tokens) => {
                        if let Some(what) = attribute(tokens) {
                            hits.push((file, what));
                        }
                    }
                    Seen::Token(tokens, at) => {
                        if let Some(what) = find(tokens, at) {
                            hits.push((file, what));
                        }
                    }
                }
            });
        }
    }
    assert!(unreadable.is_empty(), "{unreadable:?}");
    hits
}

fn ident_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    match tokens.get(at) {
        Some(TokenTree::Ident(word)) => Some(word.to_string()),
        _ => None,
    }
}

fn punct_at(tokens: &[TokenTree], at: usize, character: char) -> bool {
    matches!(tokens.get(at), Some(TokenTree::Punct(punct)) if punct.as_char() == character)
}

fn group_at(tokens: &[TokenTree], at: usize, delimiter: Delimiter) -> Option<&proc_macro2::Group> {
    match tokens.get(at) {
        Some(TokenTree::Group(group)) if group.delimiter() == delimiter => Some(group),
        _ => None,
    }
}

/// `Owner::word` ending at `at`.
fn path_owner(tokens: &[TokenTree], at: usize) -> Option<String> {
    let owner = ident_at(tokens, at.checked_sub(3)?)?;
    (punct_at(tokens, at - 2, ':') && punct_at(tokens, at - 1, ':')).then_some(owner)
}

/// The packages of the scans of production code: `protocols/`, `chips/`,
/// `portable/` and the entry, but testing packages and models.
fn product_code(path: &str) -> bool {
    (["protocols/", "chips/", "portable/"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || path == "oer")
        && !path.split('/').any(|part| part == "testing")
        && !path.ends_with("/model")
}

/// An enum at `at` (`enum Name … { … }`): its name and its variants' names.
fn enum_at(tokens: &[TokenTree], at: usize) -> Option<(String, Vec<String>)> {
    if ident_at(tokens, at)? != "enum" {
        return None;
    }
    let name = ident_at(tokens, at + 1)?;
    let body = tokens
        .iter()
        .skip(at + 2)
        .take_while(|token| !matches!(token, TokenTree::Punct(end) if end.as_char() == ';'))
        .find_map(|token| match token {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => Some(group),
            _ => None,
        })?;
    let body = body.stream().into_iter().collect::<Vec<_>>();
    let mut variants = Vec::new();
    let mut start = true;
    let mut index = 0;
    while let Some(token) = body.get(index) {
        match token {
            TokenTree::Punct(hash) if hash.as_char() == '#' => index += 1,
            TokenTree::Punct(comma) if comma.as_char() == ',' => start = true,
            TokenTree::Ident(word) if start => {
                variants.push(word.to_string());
                start = false;
            }
            _ => start = false,
        }
        index += 1;
    }
    Some((name, variants))
}

/// S1: the packages of the claims, the entry and the facade.
fn claims_scope(path: &str) -> bool {
    [
        "chips/esp32s31/app",
        "chips/esp32s31/radio/ieee80211/station",
    ]
    .contains(&path)
}

/// S1: a place claimed at run time: `try_take`, `try_init`, `try_uninit`
/// called, or an option's `take()` answered by a refusal (`ok_or`,
/// `ok_or_else`, or a `let … else`).
fn claim_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let word = ident_at(tokens, at)?;
    let called = group_at(tokens, at + 1, Delimiter::Parenthesis).is_some();
    if ["try_take", "try_init", "try_uninit"].contains(&word.as_str()) && called {
        return Some(word);
    }
    if word == "take"
        && punct_at(tokens, at.checked_sub(1)?, '.')
        && group_at(tokens, at + 1, Delimiter::Parenthesis)
            .is_some_and(|args| args.stream().is_empty())
    {
        let refused = (punct_at(tokens, at + 2, '.')
            && ident_at(tokens, at + 3)
                .is_some_and(|next| next == "ok_or" || next == "ok_or_else"))
            || ident_at(tokens, at + 2).is_some_and(|next| next == "else");
        return refused.then(|| "take refused".to_owned());
    }
    None
}

/// S2: the runtime's violation enums.
const VIOLATIONS: [&str; 3] = ["ActorViolation", "LifecycleViolation", "ApContract"];

/// S2: a site that makes or matches a violation: `Enum::Variant`.
fn violation_site_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let owner = path_owner(tokens, at)?;
    VIOLATIONS
        .contains(&owner.as_str())
        .then(|| format!("{owner}::{}", ident_at(tokens, at).unwrap_or_default()))
}

/// S3: a name of an error type.
fn error_name(name: &str) -> bool {
    [
        "Error",
        "Refused",
        "Refusal",
        "Failure",
        "Fault",
        "Unavailable",
    ]
    .iter()
    .any(|end| name.ends_with(end))
}

/// S3: a variant that names the caller's state.
fn caller_state(variant: &str) -> bool {
    variant.starts_with("Already")
        || [
            "Stopped",
            "Started",
            "Running",
            "Connected",
            "Installed",
            "Registered",
        ]
        .iter()
        .any(|state| variant.strip_prefix("Not") == Some(state))
        || ["Running", "Stopped", "Closed", "Open", "Active"].contains(&variant)
}

/// S3: an error enum with variants that name the caller's state.
fn error_state_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let (name, variants) = enum_at(tokens, at)?;
    let states = variants
        .iter()
        .filter(|variant| caller_state(variant))
        .cloned()
        .collect::<Vec<_>>();
    (error_name(&name) && !states.is_empty()).then(|| format!("{name}: {}", states.join(" ")))
}

/// S4: a name of a status.
fn status_name(name: &str) -> bool {
    [
        "Step",
        "Phase",
        "State",
        "Progress",
        "Poll",
        "Collected",
        "Selection",
    ]
    .iter()
    .any(|end| name.ends_with(end))
}

/// S4: a variant that is a failure.
fn failure_variant(variant: &str) -> bool {
    [
        "Error",
        "Failed",
        "Failure",
        "Fault",
        "Refused",
        "Invalid",
        "Timeout",
        "TimedOut",
        "Rejected",
        "Violation",
        "Lost",
        "Corrupt",
        "Unsupported",
        "Denied",
        "Aborted",
        "Overflow",
        "Unconfirmed",
        "Mismatch",
        "Broken",
        "Missing",
        "Unavailable",
    ]
    .iter()
    .any(|word| variant.contains(word))
}

/// S4: a status enum with a failure among its variants.
fn status_failure_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let (name, variants) = enum_at(tokens, at)?;
    let failures = variants
        .iter()
        .filter(|variant| failure_variant(variant))
        .cloned()
        .collect::<Vec<_>>();
    (status_name(&name) && !error_name(&name) && !failures.is_empty())
        .then(|| format!("{name}: {}", failures.join(" ")))
}

/// S5: a result dropped or replaced: a statement `.ok();`, an
/// `unwrap_or_default()`, an `unwrap_or_else(|_| …)`.
fn dropped_result_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let word = ident_at(tokens, at)?;
    let args = group_at(tokens, at + 1, Delimiter::Parenthesis)?;
    let empty = args.stream().is_empty();
    match word.as_str() {
        "ok" if empty
            && punct_at(tokens, at.checked_sub(1)?, '.')
            && punct_at(tokens, at + 2, ';') =>
        {
            Some(".ok();".to_owned())
        }
        "unwrap_or_default" if empty => Some(word),
        "unwrap_or_else" => {
            let mut inner = args.stream().into_iter();
            let closure = matches!(
                (inner.next(), inner.next(), inner.next()),
                (Some(TokenTree::Punct(a)), Some(TokenTree::Ident(underscore)), Some(TokenTree::Punct(b)))
                    if a.as_char() == '|' && underscore == "_" && b.as_char() == '|'
            );
            closure.then(|| "unwrap_or_else(|_|".to_owned())
        }
        _ => None,
    }
}

/// S6: memory kept out of its owner, and a quarantine by name.
fn kept_out_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let word = ident_at(tokens, at)?;
    let owner = path_owner(tokens, at);
    let found = word == "ManuallyDrop"
        || (word == "forget" && owner.as_deref() == Some("mem"))
        || (word == "leak" && owner.as_deref() == Some("Box"))
        || ["Poison", "Quarantin", "ResetRequired", "Unfinished"]
            .iter()
            .any(|prefix| word.starts_with(prefix));
    found.then_some(word)
}

/// S7: a compatibility layer: a renamed re-export, `pub use … as …`.
fn compatibility_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    if ident_at(tokens, at)? != "use" {
        return None;
    }
    let public = ident_at(tokens, at.checked_sub(1)?).is_some_and(|word| word == "pub")
        || (group_at(tokens, at - 1, Delimiter::Parenthesis).is_some()
            && at >= 2
            && ident_at(tokens, at - 2).is_some_and(|word| word == "pub"));
    if !public {
        return None;
    }
    let renamed = tokens
        .iter()
        .skip(at + 1)
        .take_while(|token| !matches!(token, TokenTree::Punct(end) if end.as_char() == ';'))
        .any(|token| {
            fn has_as(token: &TokenTree) -> bool {
                match token {
                    TokenTree::Ident(word) => word == "as",
                    TokenTree::Group(group) => group.stream().into_iter().any(|t| has_as(&t)),
                    _ => false,
                }
            }
            has_as(token)
        });
    renamed.then(|| "pub use … as".to_owned())
}

/// S7: a compatibility attribute: `serde(default)`, `serde(alias)`,
/// `deprecated`.
fn compatibility_attribute(attribute: &TokenStream) -> Option<String> {
    let mut tokens = attribute.clone().into_iter();
    match (tokens.next(), tokens.next()) {
        (Some(TokenTree::Ident(word)), _) if word == "deprecated" => Some("deprecated".to_owned()),
        (Some(TokenTree::Ident(word)), Some(TokenTree::Group(args))) if word == "serde" => {
            args.stream().into_iter().find_map(|token| match token {
                TokenTree::Ident(word) if word == "default" || word == "alias" => {
                    Some(format!("serde({word})"))
                }
                _ => None,
            })
        }
        _ => None,
    }
}

/// S8a: the packages that reach registers: the HAL, the radio crates and
/// the platform, the PAC aside.
fn register_code(path: &str) -> bool {
    [
        "chips/esp32s31/hal",
        "chips/esp32s31/radio/",
        "chips/esp32s31/platform/",
        "chips/family/radio/",
        "chips/family/platform/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// S8a: a call of the PAC's raw conversions, `from_bits` and `.bits()`. The
/// accessors of a register or field the model names `WORD_…_OPAQUE` or
/// `BITS_…_OPAQUE` are typed, and the model lists its opaque names itself.
fn raw_register_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let word = ident_at(tokens, at)?;
    group_at(tokens, at + 1, Delimiter::Parenthesis)?;
    let method = at
        .checked_sub(1)
        .is_some_and(|before| punct_at(tokens, before, '.'));
    // The raw conversions alone: an accessor of a register or field the
    // model names `WORD_…_OPAQUE` or `BITS_…_OPAQUE` is a typed one.
    let raw = word == "from_bits" || (word == "bits" && method);
    raw.then_some(word)
}

/// S8b: the protocol packages, the wire crates among them: the one place a
/// frame's bytes are reached by hand is `oer-portable-wire`, outside them.
fn protocol_code(path: &str) -> bool {
    path.starts_with("protocols/")
        && !path.split('/').any(|part| part == "testing")
        && !path.ends_with("/model")
}

/// S8b: a byte conversion (`from_le_bytes`, `from_be_bytes`, `to_le_bytes`,
/// `to_be_bytes`), or an index or a range of a slice of bytes by name (a
/// name ending in `bytes`, or `frame`, `body`, `payload`, `buffer`, `buf`,
/// `data`, `memory`).
fn raw_bytes_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let word = ident_at(tokens, at)?;
    if [
        "from_le_bytes",
        "from_be_bytes",
        "to_le_bytes",
        "to_be_bytes",
    ]
    .contains(&word.as_str())
        && group_at(tokens, at + 1, Delimiter::Parenthesis).is_some()
    {
        return Some(word);
    }
    let bytes = word.ends_with("bytes")
        || [
            "frame", "body", "payload", "buffer", "buf", "data", "memory",
        ]
        .contains(&word.as_str());
    (bytes && group_at(tokens, at + 1, Delimiter::Bracket).is_some()).then(|| "index".to_owned())
}

/// Hits by file.
fn by_file(hits: &[(String, String)]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (file, _) in hits {
        *counts.entry(file.clone()).or_insert(0) += 1;
    }
    counts
}

/// What removes a list's entry, by where it is.
const BY_P2: &str = "P2, the installation machine and the composition";
const BY_P3: &str = "P3, the engine";
const BY_PHY: &str = "the PHY, after the Wi-Fi path";
const BY_RADIO_SYSTEM: &str = "no package yet: the radio system and Bluetooth";
const BY_PHASE_C: &str = "phase C, the network owner in the network runtime";
const BY_NONE: &str = "no package yet";

/// The package that removes a list's entry in `file`, by its place: the
/// first assignment, which a package that removes one corrects.
fn remover(file: &str) -> &'static str {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| file.starts_with(prefix));
    if starts(&[
        "chips/esp32s31/hal/src/radio/phy",
        "chips/esp32s31/hal/src/radio/domain",
        "chips/esp32s31/radio/phy/",
        "chips/family/radio/phy",
    ]) {
        BY_PHY
    } else if file.contains("bluetooth")
        || file.contains("coex")
        || starts(&[
            "chips/esp32s31/hal/src/radio/modem",
            "chips/esp32s31/hal/src/radio.rs",
            "chips/esp32s31/radio/system/",
            "chips/esp32s31/radio/client/",
            "chips/family/radio/modem-clock/",
            "chips/family/radio/power/",
            "protocols/radio/",
        ])
    {
        BY_RADIO_SYSTEM
    } else if starts(&[
        "protocols/ieee80211/runtime/",
        "chips/esp32s31/radio/ieee80211/station/",
        "chips/esp32s31/app/",
        "oer/",
    ]) {
        BY_P2
    } else if starts(&[
        "chips/esp32s31/radio/ieee80211/",
        "chips/family/radio/ieee80211/",
        "protocols/ieee80211/",
        "chips/esp32s31/hal/src/radio/ieee80211/",
    ]) {
        BY_P3
    } else if starts(&["protocols/network/"]) {
        BY_PHASE_C
    } else {
        BY_NONE
    }
}

fn remover_name(file: &str) -> &'static str {
    match remover(file) {
        BY_P2 => "BY_P2",
        BY_P3 => "BY_P3",
        BY_PHY => "BY_PHY",
        BY_RADIO_SYSTEM => "BY_RADIO_SYSTEM",
        BY_PHASE_C => "BY_PHASE_C",
        _ => "BY_NONE",
    }
}

/// S1: places claimed at run time in the entry and the facade (`try_take`,
/// `try_init`, `try_uninit`, an option's `take()` answered by a refusal), by
/// file with their count. The list only shrinks.
const CLAIMS: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/app/src/lib.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/observed.rs", 3, BY_P2),
    ("chips/esp32s31/app/src/wifi.rs", 7, BY_P2),
    ("chips/esp32s31/app/src/wifi/access_point.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/wifi/network.rs", 1, BY_P2),
    (
        "chips/esp32s31/radio/ieee80211/station/src/access_point.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/chip.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/dma.rs",
        9,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/lib.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/radio.rs",
        5,
        BY_P2,
    ),
];
/// S3: error types that name the caller's state (an enum named `…Error`,
/// `…Refused`, `…Refusal`, `…Failure`, `…Fault` or `…Unavailable` with a
/// variant `Already…`, `Not…` of a state, `Running`, `Stopped`, `Closed`,
/// `Open` or `Active`), by file with their count of such variants. The list
/// only shrinks.
const ERROR_STATES: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/app/src/lib.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/wifi/station.rs", 1, BY_P2),
    ("chips/esp32s31/hal/src/power/radio.rs", 1, BY_NONE),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/controller_time.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/live.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/owner.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/executor.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/interrupt.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("chips/esp32s31/hal/src/radio/coex.rs", 2, BY_RADIO_SYSTEM),
    ("chips/esp32s31/hal/src/radio/domain.rs", 3, BY_PHY),
    ("chips/esp32s31/hal/src/radio/phy/i2c.rs", 2, BY_PHY),
    ("chips/esp32s31/hal/src/radio/phy/temperature.rs", 1, BY_PHY),
    ("chips/esp32s31/hal/src/system/cores.rs", 2, BY_NONE),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/interrupt/gate.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/frontier/state.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/access_point.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/phy/src/analog/crystal_duty.rs",
        5,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/dcode.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/analog/frequency.rs",
        4,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/i2c.rs", 7, BY_PHY),
    ("chips/esp32s31/radio/phy/src/analog/pbus.rs", 3, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/analog/pbus/memory.rs",
        2,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/rfpll.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/analog/temperature.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/baseband.rs",
        2,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/bluetooth.rs",
        2,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/cold.rs",
        3,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/estimator.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/registration.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/channel.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/concurrent.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/lifecycle.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/dc_offset.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/iq.rs", 7, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/saturation.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/signal_power.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tracking/calibration.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tracking/i2c.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tracking/parameters.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tracking/power.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tracking/rfpll.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tracking/rfpll/search.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tx/calibration.rs", 5, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tx/dc_offset.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tx/dc_power_detector.rs",
        2,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tx/iq.rs", 6, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tx/power.rs", 2, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tx/power_detector.rs",
        2,
        BY_PHY,
    ),
    ("chips/family/drivers/regi2c/src/lib.rs", 1, BY_NONE),
    ("chips/family/radio/power/src/domain.rs", 2, BY_RADIO_SYSTEM),
    (
        "chips/family/radio/power/src/membership.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("chips/family/radio/power/src/modem.rs", 1, BY_RADIO_SYSTEM),
    ("portable/memory/src/waking_spsc/closing.rs", 1, BY_NONE),
    ("portable/time/src/connected_tasks.rs", 1, BY_NONE),
    ("portable/time/src/deadline.rs", 1, BY_NONE),
    ("protocols/ieee80211/core/arbiter/src/lib.rs", 1, BY_P3),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/security.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/commands.rs",
        1,
        BY_P3,
    ),
    ("protocols/ieee802154/port/src/work.rs", 1, BY_NONE),
    ("protocols/network/runtime/src/local.rs", 1, BY_PHASE_C),
    ("protocols/network/runtime/src/returns.rs", 1, BY_PHASE_C),
    ("protocols/network/runtime/src/socket.rs", 1, BY_PHASE_C),
];
/// S4: statuses that hold a failure and are no outcome (an enum named
/// `…Step`, `…Phase`, `…State`, `…Progress`, `…Poll`, `…Collected` or
/// `…Selection` with a failure among its variants), by file with their
/// count of failure variants. The list only shrinks.
const STATUS_FAILURES: &[(&str, usize, &str)] = &[
    (
        "chips/esp32s31/radio/bluetooth/backend/src/controller_time.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/radio.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/lifecycle/bank.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/phy/src/analog/crystal_duty.rs",
        5,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/dcode.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/analog/frequency.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/rfpll.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/analog/temperature.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/baseband.rs",
        2,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/bluetooth.rs",
        3,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/estimator.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/calibration/registration.rs",
        2,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/channel.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/executor/step.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/dc_offset.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/iq.rs", 6, BY_PHY),
    ("chips/esp32s31/radio/phy/src/rx/signal_power.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tracking/calibration.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/phy/src/tracking/observation/health.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tx/calibration.rs", 5, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tx/dc_offset.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tx/dc_power_detector.rs",
        2,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/tx/iq.rs", 5, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tx/power.rs", 2, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tx/power_detector.rs",
        1,
        BY_PHY,
    ),
    ("chips/family/app/src/observe/connect.rs", 1, BY_NONE),
    ("chips/family/drivers/regi2c/src/lib.rs", 2, BY_NONE),
    (
        "chips/family/platform/executor/src/observation/timer.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/family/radio/ieee80211/policy/src/rate_control.rs",
        1,
        BY_P3,
    ),
    ("portable/memory/src/rx_external_handoff.rs", 1, BY_NONE),
    (
        "protocols/bluetooth/le/ll/src/security.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/ieee80211/contracts/port/src/access_point.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/contracts/port/src/data_port.rs",
        1,
        BY_P3,
    ),
    ("protocols/ieee80211/core/roles/sta/src/ftm.rs", 1, BY_P3),
    (
        "protocols/ieee80211/core/roles/sta/src/join/association.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/sa_query.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/state/authenticator.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/state/supplicant.rs",
        1,
        BY_P3,
    ),
    ("protocols/radio/port/src/slot.rs", 1, BY_RADIO_SYSTEM),
];
/// S5: results dropped or replaced (a statement `.ok();`, an
/// `unwrap_or_default()`, an `unwrap_or_else(|_| …)`), by file with their
/// count. The list only shrinks; each site is read by the inventory.
const DROPPED_RESULTS: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/app/src/wifi.rs", 1, BY_P2),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/dtm/codec.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/executor/live.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_dma/model.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/platform/runtime/src/exception.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/port/lifecycle.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters/stall.rs",
        3,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/receive.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/access_point.rs",
        2,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/chip.rs",
        3,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/station.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/supply.rs",
        1,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/phy/src/tx/dc_power_detector.rs",
        1,
        BY_PHY,
    ),
    ("chips/family/facts/src/generate.rs", 1, BY_NONE),
    ("chips/family/facts/src/layers.rs", 1, BY_NONE),
    ("chips/family/platform/bootstrap/src/startup.rs", 3, BY_NONE),
    ("chips/family/platform/executor/src/executor.rs", 2, BY_NONE),
    ("chips/family/platform/runtime/src/panic.rs", 2, BY_NONE),
    ("portable/memory/src/pinned_tx.rs", 2, BY_NONE),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/protocol.rs",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/monitor.rs",
        1,
        BY_P3,
    ),
    ("protocols/ieee802154/port/src/link_metrics.rs", 1, BY_NONE),
    ("protocols/network/core/src/block.rs", 1, BY_PHASE_C),
    ("protocols/network/core/src/dhcp.rs", 1, BY_PHASE_C),
    ("protocols/network/runtime/src/service.rs", 1, BY_PHASE_C),
];
/// S6: memory kept out of its owner and quarantines by name
/// (`ManuallyDrop`, `mem::forget`, `Box::leak`, a name `Poison…`,
/// `Quarantin…`, `ResetRequired…`, `Unfinished…`), mentions by file. The
/// list only shrinks.
const KEPT_OUT: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/app/src/wifi/station.rs", 1, BY_P2),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/access.rs",
        7,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/finished.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/live.rs",
        13,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/owner.rs",
        6,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler/interrupt.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("chips/esp32s31/hal/src/radio/domain.rs", 4, BY_PHY),
    (
        "chips/esp32s31/hal/src/radio/domain/transaction.rs",
        9,
        BY_PHY,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_ring.rs",
        13,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_storage.rs",
        7,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_storage/buffers.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_ampdu_storage.rs",
        11,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_storage.rs",
        12,
        BY_P3,
    ),
    ("chips/esp32s31/hal/src/radio/phy/temperature.rs", 3, BY_PHY),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/port.rs",
        4,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/port/lifecycle.rs",
        13,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/port/mailbox.rs",
        8,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/port/owned.rs",
        6,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/roles.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/client/src/bluetooth.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/client/src/lib.rs",
        14,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/client/src/poison.rs",
        8,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/client/src/poison/diagnostic_path.rs",
        6,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac.rs",
        8,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs",
        4,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/keys.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/lifecycle.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/irq/epoch.rs",
        7,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/rx.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/rx/prefix.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx.rs",
        4,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu/lifecycle.rs",
        5,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu/owner.rs",
        18,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/test_support.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/ordinary_tx.rs",
        7,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/runtime.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/control.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/receive.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/access_point.rs",
        9,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/lib.rs",
        4,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/radio.rs",
        6,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/station.rs",
        8,
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee802154/backend/src/engine.rs",
        8,
        BY_NONE,
    ),
    ("chips/esp32s31/radio/phy/src/concurrent.rs", 22, BY_PHY),
    ("chips/esp32s31/radio/phy/src/diagnostic_path.rs", 1, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/target_port/concurrent.rs",
        9,
        BY_PHY,
    ),
    (
        "chips/esp32s31/radio/system/src/actor_core.rs",
        13,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/system/src/bluetooth.rs",
        5,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/system/src/esp32s31.rs",
        44,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/system/src/esp32s31/bluetooth.rs",
        31,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/system/src/lib.rs",
        35,
        BY_RADIO_SYSTEM,
    ),
    ("chips/family/platform/executor/src/fault.rs", 4, BY_NONE),
    ("chips/family/platform/executor/src/task.rs", 6, BY_NONE),
    (
        "chips/family/platform/interrupt-table/src/binding.rs",
        12,
        BY_NONE,
    ),
    (
        "chips/family/platform/interrupt-table/src/lib.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/family/platform/interrupt-table/src/router.rs",
        5,
        BY_NONE,
    ),
    (
        "chips/family/platform/interrupt-table/src/signal.rs",
        7,
        BY_NONE,
    ),
    (
        "chips/family/platform/runtime/src/interrupts.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/family/platform/runtime/src/retained_fault.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/family/radio/modem-clock/src/execution.rs",
        4,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/family/radio/modem-clock/src/lib.rs",
        12,
        BY_RADIO_SYSTEM,
    ),
    ("chips/family/radio/power/src/modem.rs", 8, BY_RADIO_SYSTEM),
    ("portable/memory/src/indexed_custody.rs", 7, BY_NONE),
    ("portable/memory/src/init_in_place.rs", 1, BY_NONE),
    ("portable/memory/src/lib.rs", 1, BY_NONE),
    ("portable/memory/src/placed.rs", 2, BY_NONE),
    ("portable/memory/src/request_slot.rs", 4, BY_NONE),
    ("portable/time/src/connected_tasks.rs", 3, BY_NONE),
    ("portable/waker/src/lib.rs", 4, BY_NONE),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs",
        9,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/port/src/conformance.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("protocols/bluetooth/le/port/src/lib.rs", 1, BY_RADIO_SYSTEM),
    (
        "protocols/bluetooth/le/runtime/src/host.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("protocols/ieee80211/contracts/port/src/arena.rs", 1, BY_P3),
    ("protocols/ieee80211/contracts/port/src/lib.rs", 1, BY_P3),
    ("protocols/ieee802154/port/src/config.rs", 1, BY_NONE),
    ("protocols/ieee802154/port/src/lib.rs", 1, BY_NONE),
    ("protocols/ieee802154/port/src/owner.rs", 6, BY_NONE),
    (
        "protocols/radio/port/src/conformance/registration.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("protocols/radio/port/src/lib.rs", 3, BY_RADIO_SYSTEM),
    ("protocols/radio/port/src/lifecycle.rs", 24, BY_RADIO_SYSTEM),
    ("protocols/radio/port/src/slot.rs", 21, BY_RADIO_SYSTEM),
];
/// S7: compatibility layers (`#[serde(default)]`, `#[serde(alias)]`,
/// `#[deprecated]`, a renamed re-export `pub use … as …`), by file with
/// their count. The list only shrinks.
const COMPATIBILITY: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/app/src/console.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/cpu.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/lib.rs", 2, BY_P2),
    ("chips/esp32s31/app/src/wifi/network.rs", 1, BY_P2),
    ("chips/esp32s31/app/src/wifi/station.rs", 1, BY_P2),
    ("chips/esp32s31/hal/src/peripherals/aes.rs", 1, BY_NONE),
    (
        "chips/esp32s31/hal/src/radio/domain/transaction.rs",
        1,
        BY_PHY,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_storage.rs",
        1,
        BY_P3,
    ),
    ("chips/esp32s31/hal/src/radio/phy/i2c.rs", 1, BY_PHY),
    (
        "chips/esp32s31/hal/src/system/boot/clock/lp_fast.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/observation.rs",
        1,
        BY_P2,
    ),
    ("chips/esp32s31/radio/phy/src/lib.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/target_port.rs", 1, BY_PHY),
    ("chips/esp32s31/radio/phy/src/tracking.rs", 2, BY_PHY),
    (
        "chips/esp32s31/radio/phy/src/tracking/rfpll/search.rs",
        1,
        BY_PHY,
    ),
    ("chips/family/facts/src/lib.rs", 19, BY_NONE),
    ("chips/family/facts/src/model.rs", 27, BY_NONE),
    (
        "chips/family/platform/executor/src/installations.rs",
        1,
        BY_NONE,
    ),
    ("chips/family/platform/executor/src/lib.rs", 1, BY_NONE),
    ("chips/family/platform/runtime/src/lib.rs", 4, BY_NONE),
    ("oer/src/print.rs", 1, BY_P2),
    ("portable/trace-recorder/src/lib.rs", 1, BY_NONE),
    ("protocols/hil/agent/src/lib.rs", 1, BY_NONE),
    ("protocols/hil/protocol/src/base/boot.rs", 3, BY_NONE),
    ("protocols/hil/protocol/src/lib.rs", 2, BY_NONE),
    ("protocols/ieee80211/core/roles/ap/src/engine.rs", 1, BY_P3),
    ("protocols/network/core/src/lib.rs", 1, BY_PHASE_C),
];
/// S2: the variants of the runtime's violation enums, each with the count of
/// the production sites that make or match it, and what removes it by what
/// it guards. The list only shrinks; a variant without an entry fails.
const VIOLATION_VARIANTS: &[(&str, usize, &str)] = &[];

/// S8a: the PAC's raw conversions outside the PAC, in
/// the HAL, the radio crates and the platform, by file with their count:
/// a value reaches a register by hand only through these. An entry that is
/// a policy `whole-word` image or a hardware-consumed layout says so. The
/// list only shrinks.
const RAW_REGISTERS: &[(&str, usize, &str)] = &[
    ("chips/esp32s31/hal/src/peripherals/aes.rs", 4, BY_NONE),
    ("chips/esp32s31/hal/src/peripherals/aes_dma.rs", 6, BY_NONE),
    (
        "chips/esp32s31/hal/src/peripherals/watchdog/registers.rs",
        3,
        BY_NONE,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/controller_time.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/advertising_image.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/ble_phy_engine.rs",
        9,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/connectable_advertising.rs",
        14,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/dtm/codec.rs",
        15,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/dtm_image.rs",
        7,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/dtm_rx_result.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/le_phy_packet.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/le_rx_packet.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/le_tx_packet.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/legacy_advertising.rs",
        11,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/legacy_scanning.rs",
        14,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/peripheral_connection.rs",
        15,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/rx_memory_list.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/rx_packet_bytes.rs",
        6,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/scheduler_item.rs",
        10,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/engine/sram_link.rs",
        4,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/observation.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/controller/time.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/interrupt.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/modem_timer.rs",
        12,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/phy.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/scheduler.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/scheduler/runtime.rs",
        5,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/bluetooth/registers/scheduler/stop.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("chips/esp32s31/hal/src/radio/coex.rs", 1, BY_RADIO_SYSTEM),
    (
        "chips/esp32s31/hal/src/radio/coex/external.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/coex/ports.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/descriptor.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_dma.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/rx_ring.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_ampdu_storage.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/dma/tx_storage.rs",
        4,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/block_ack.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/channel.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/coex/runtime.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/crypto.rs",
        7,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/he/beamforming.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/he/init.rs",
        6,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/he/init_suffix.rs",
        5,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/he/ofdma.rs",
        5,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/he/peer.rs",
        5,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/interrupt.rs",
        3,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/tx.rs",
        74,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/tx/power_init.rs",
        20,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/tx/queue.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/txrx_init.rs",
        12,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee80211/registers/values.rs",
        23,
        BY_P3,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee802154/mac/codes.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee802154/mac/etm.rs",
        4,
        BY_NONE,
    ),
    (
        "chips/esp32s31/hal/src/radio/ieee802154/mac/task.rs",
        1,
        BY_NONE,
    ),
    (
        "chips/esp32s31/hal/src/radio/modem/platform.rs",
        8,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/modem/syscon.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/modem/values.rs",
        22,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/hal/src/radio/phy/baseband/ops.rs",
        2,
        BY_PHY,
    ),
    (
        "chips/esp32s31/hal/src/radio/phy/frequency/ops.rs",
        1,
        BY_PHY,
    ),
    ("chips/esp32s31/hal/src/radio/phy/i2c/master.rs", 1, BY_PHY),
    ("chips/esp32s31/hal/src/system/boot.rs", 6, BY_NONE),
    (
        "chips/esp32s31/hal/src/system/boot/clock/lp_fast.rs",
        1,
        BY_NONE,
    ),
    ("chips/esp32s31/hal/src/system/boot/psram.rs", 2, BY_NONE),
    (
        "chips/esp32s31/hal/src/system/memory/dma_address.rs",
        6,
        BY_NONE,
    ),
    (
        "chips/esp32s31/hal/src/system/platform/interrupt_controller.rs",
        4,
        BY_NONE,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/controller_time.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/bluetooth/backend/src/modem_lp_timer_queue.rs",
        8,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs",
        3,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/rx/prefix.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/metadata.rs",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs",
        3,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/receive.rs",
        1,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/phy/src/analog/frequency.rs",
        25,
        BY_PHY,
    ),
    ("chips/esp32s31/radio/phy/src/analog/rfpll.rs", 3, BY_PHY),
    (
        "chips/esp32s31/radio/system/src/observation.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "chips/family/platform/interrupt-table/src/router.rs",
        1,
        "the family's encoding of a line's expectation: `Control::bits`, a portable value's byte",
    ),
];

/// S8b: byte conversions and indexing of bytes by name in the protocol
/// packages, the `wire` crates among them, by file with their count: an
/// air format reaches its bytes by hand only through these, and only
/// `oer-portable-wire` may. It misses an index
/// of a slice of bytes named otherwise. The list only shrinks.
const RAW_BYTES: &[(&str, usize, &str)] = &[
    (
        "protocols/bluetooth/hci/src/controller/bootstrap/reset.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/bootstrap/response.rs",
        7,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/accept_list.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/acl.rs",
        13,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/advertising.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/data_length.rs",
        14,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/dtm.rs",
        10,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/peripheral.rs",
        35,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/scanning.rs",
        11,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/random.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/response.rs",
        3,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/transport/packet.rs",
        2,
        BY_RADIO_SYSTEM,
    ),
    ("protocols/bluetooth/hci/src/wire.rs", 1, BY_RADIO_SYSTEM),
    (
        "protocols/bluetooth/le/cipher/src/lib.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/bootstrap.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs",
        4,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs",
        11,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/port/src/request.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/wire/src/connection.rs",
        14,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/wire/src/control.rs",
        10,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/wire/src/data_length.rs",
        8,
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/wire/src/encryption.rs",
        1,
        BY_RADIO_SYSTEM,
    ),
    ("protocols/hil/agent/src/postmortem.rs", 3, BY_NONE),
    ("protocols/hil/protocol/src/framing.rs", 12, BY_NONE),
    ("protocols/hil/protocol/src/key.rs", 1, BY_NONE),
    (
        "protocols/ieee80211/contracts/port/src/monitor.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/protocol.rs",
        6,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/security.rs",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/association.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/ethernet.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/management.rs",
        3,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/receive.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/sa_query.rs",
        3,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/security.rs",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/crypto.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/crypto/pbkdf2.rs",
        1,
        BY_P3,
    ),
    ("protocols/ieee80211/core/security/rsn/src/kdf.rs", 2, BY_P3),
    ("protocols/ieee80211/core/security/rsn/src/sae.rs", 7, BY_P3),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/banks.rs",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/fragmentation/reassembly.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/receive.rs",
        4,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/ap.rs", 38, BY_P3),
    ("protocols/ieee80211/wire/src/ap/probe.rs", 2, BY_P3),
    ("protocols/ieee80211/wire/src/ap/profile.rs", 3, BY_P3),
    ("protocols/ieee80211/wire/src/beacon.rs", 5, BY_P3),
    ("protocols/ieee80211/wire/src/bip.rs", 5, BY_P3),
    ("protocols/ieee80211/wire/src/block_ack/frame.rs", 23, BY_P3),
    ("protocols/ieee80211/wire/src/ccmp.rs", 16, BY_P3),
    ("protocols/ieee80211/wire/src/ccmp/protected.rs", 4, BY_P3),
    ("protocols/ieee80211/wire/src/data.rs", 8, BY_P3),
    ("protocols/ieee80211/wire/src/eapol.rs", 11, BY_P3),
    ("protocols/ieee80211/wire/src/encoding.rs", 2, BY_P3),
    (
        "protocols/ieee80211/wire/src/extensions/espressif/esp_now.rs",
        55,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/extensions/espressif/esp_now/v2.rs",
        16,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/fragmentation.rs", 3, BY_P3),
    (
        "protocols/ieee80211/wire/src/fragmentation/parsing.rs",
        10,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/frame.rs", 17, BY_P3),
    ("protocols/ieee80211/wire/src/ftm.rs", 19, BY_P3),
    ("protocols/ieee80211/wire/src/he.rs", 11, BY_P3),
    ("protocols/ieee80211/wire/src/ht.rs", 6, BY_P3),
    ("protocols/ieee80211/wire/src/management.rs", 4, BY_P3),
    (
        "protocols/ieee80211/wire/src/monitor_injection.rs",
        3,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/ndpa.rs", 19, BY_P3),
    ("protocols/ieee80211/wire/src/sae.rs", 3, BY_P3),
    ("protocols/ieee80211/wire/src/scan.rs", 5, BY_P3),
    ("protocols/ieee80211/wire/src/security/rsn.rs", 8, BY_P3),
    ("protocols/ieee80211/wire/src/sequence.rs", 3, BY_P3),
    ("protocols/ieee80211/wire/src/station.rs", 6, BY_P3),
    (
        "protocols/ieee80211/wire/src/station/association.rs",
        6,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/station/data.rs", 2, BY_P3),
    (
        "protocols/ieee80211/wire/src/station/data/amsdu.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/station/data/template.rs",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/station/management.rs",
        10,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/station/security.rs", 3, BY_P3),
    ("protocols/ieee80211/wire/src/station_beacon.rs", 4, BY_P3),
    (
        "protocols/ieee80211/wire/src/station_power_save.rs",
        2,
        BY_P3,
    ),
    ("protocols/ieee80211/wire/src/trigger.rs", 10, BY_P3),
    ("protocols/ieee80211/wire/src/tsf.rs", 2, BY_P3),
    ("protocols/ieee80211/wire/src/twt.rs", 16, BY_P3),
    ("protocols/ieee802154/port/src/link_metrics.rs", 1, BY_NONE),
    ("protocols/ieee802154/wire/src/csl.rs", 5, BY_NONE),
    ("protocols/ieee802154/wire/src/enhanced_ack.rs", 4, BY_NONE),
    ("protocols/ieee802154/wire/src/frame.rs", 3, BY_NONE),
    ("protocols/ieee802154/wire/src/link_metrics.rs", 4, BY_NONE),
    ("protocols/ieee802154/wire/src/security.rs", 1, BY_NONE),
    ("protocols/ieee802154/wire/src/time_sync.rs", 1, BY_NONE),
    ("protocols/network/core/src/dhcp.rs", 3, BY_PHASE_C),
    ("protocols/network/core/src/interface.rs", 4, BY_PHASE_C),
    ("protocols/network/wire/src/arp.rs", 3, BY_PHASE_C),
    ("protocols/network/wire/src/checksum.rs", 3, BY_PHASE_C),
    ("protocols/network/wire/src/dhcp.rs", 3, BY_PHASE_C),
    ("protocols/network/wire/src/ethernet.rs", 2, BY_PHASE_C),
    ("protocols/network/wire/src/icmp.rs", 6, BY_PHASE_C),
    ("protocols/network/wire/src/ip.rs", 5, BY_PHASE_C),
    ("protocols/network/wire/src/transport.rs", 13, BY_PHASE_C),
    ("protocols/network/wire/src/udp_frame.rs", 5, BY_PHASE_C),
];

/// The hits of a scan against its list: a new file, a grown count and an
/// entry whose file has no hits all fail.
fn scan_differences(hits: &[(String, String)], list: &[(&str, usize, &str)]) -> Vec<String> {
    let known = list
        .iter()
        .map(|(file, count, _)| (*file, *count))
        .collect::<Vec<_>>();
    count_differences(&by_file(hits), &known)
}

fn assert_scan(hits: &[(String, String)], list: &[(&str, usize, &str)]) {
    let differences = scan_differences(hits, list);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

const NO_ATTRIBUTE: &AttributeFinder = &|_| None;

#[test]
fn places_claimed_at_run_time_are_listed_and_only_shrink() {
    assert_scan(&scan_hits(&claims_scope, &claim_at, NO_ATTRIBUTE), CLAIMS);
}

#[test]
fn violation_variants_and_their_sites_are_listed_and_only_shrink() {
    let mut sites = BTreeMap::<String, usize>::new();
    for (_, site) in scan_hits(&product_code, &violation_site_at, NO_ATTRIBUTE) {
        *sites.entry(site).or_insert(0) += 1;
    }
    // Every variant has an entry, with no site too.
    for (_, definition) in scan_hits(
        &|path| path == "protocols/ieee80211/runtime",
        &|tokens, at| {
            let (name, variants) = enum_at(tokens, at)?;
            VIOLATIONS.contains(&name.as_str()).then(|| {
                variants
                    .iter()
                    .map(|variant| format!("{name}::{variant}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        },
        NO_ATTRIBUTE,
    ) {
        for variant in definition.split(' ') {
            sites.entry(variant.to_owned()).or_insert(0);
        }
    }
    let known = VIOLATION_VARIANTS
        .iter()
        .map(|(variant, count, _)| ((*variant).to_owned(), *count))
        .collect::<BTreeMap<_, _>>();
    let new = sites
        .keys()
        .filter(|variant| !known.contains_key(*variant))
        .collect::<Vec<_>>();
    let grown = sites
        .iter()
        .filter(|(variant, count)| known.get(*variant).is_some_and(|known| *count > known))
        .collect::<Vec<_>>();
    let lower = sites
        .iter()
        .filter(|(variant, count)| known.get(*variant).is_some_and(|known| *count < known))
        .collect::<Vec<_>>();
    let gone = known
        .keys()
        .filter(|variant| !sites.contains_key(*variant))
        .collect::<Vec<_>>();
    assert!(
        new.is_empty() && grown.is_empty() && lower.is_empty() && gone.is_empty(),
        "new {new:?}, grown {grown:?}, lower the entry {lower:?}, gone {gone:?}"
    );
}

#[test]
fn error_types_that_name_the_callers_state_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&product_code, &error_state_at, NO_ATTRIBUTE),
        ERROR_STATES,
    );
}

#[test]
fn statuses_that_hold_a_failure_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&product_code, &status_failure_at, NO_ATTRIBUTE),
        STATUS_FAILURES,
    );
}

#[test]
fn results_dropped_or_replaced_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&product_code, &dropped_result_at, NO_ATTRIBUTE),
        DROPPED_RESULTS,
    );
}

#[test]
fn memory_kept_out_and_quarantines_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&product_code, &kept_out_at, NO_ATTRIBUTE),
        KEPT_OUT,
    );
}

#[test]
fn raw_register_calls_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&register_code, &raw_register_at, NO_ATTRIBUTE),
        RAW_REGISTERS,
    );
}

#[test]
fn raw_byte_access_in_protocols_is_listed_and_only_shrinks() {
    assert_scan(
        &scan_hits(&protocol_code, &raw_bytes_at, NO_ATTRIBUTE),
        RAW_BYTES,
    );
}

#[test]
fn compatibility_layers_are_listed_and_only_shrink() {
    assert_scan(
        &scan_hits(&product_code, &compatibility_at, &compatibility_attribute),
        COMPATIBILITY,
    );
}

/// Every entry of the lists names what removes it, and the scan's
/// differences name each kind.
#[test]
fn the_rule_lists_name_their_removers() {
    for list in [
        CLAIMS,
        VIOLATION_VARIANTS,
        ERROR_STATES,
        STATUS_FAILURES,
        DROPPED_RESULTS,
        KEPT_OUT,
        COMPATIBILITY,
        RAW_REGISTERS,
        RAW_BYTES,
        HIDDEN_FLAGS,
        HIDDEN_BOOL_PARAMETERS,
    ] {
        for (entry, _, remover) in list {
            assert!(!remover.is_empty(), "{entry}");
        }
    }
    for (entry, remover) in OPTION_SLOTS {
        assert!(!remover.is_empty(), "{entry}");
    }
    let hits = [
        ("a.rs".to_owned(), "x".to_owned()),
        ("b.rs".to_owned(), "y".to_owned()),
    ];
    assert_eq!(
        scan_differences(&hits, &[("a.rs", 2, BY_NONE), ("c.rs", 1, BY_NONE)]),
        [
            "lower the entry: a.rs (2 -> 1)",
            "new: b.rs (1)",
            "gone, drop the entry: c.rs"
        ]
    );
}

/// The finders take their shapes in production code and skip what tests
/// hold.
#[test]
fn the_rule_scans_find_their_shapes_and_skip_tests() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("lib.rs");
    std::fs::write(
        &source,
        r#"
pub enum StartError { AlreadyRunning, NotStarted, Device }
pub enum PollStep { Done, Failed }
pub enum Quiet { Running }
pub use inner::Thing as Other;
#[serde(default)]
pub struct Config { #[deprecated] old: u8 }
fn f(cell: &mut Option<u8>, r: Result<u8, ()>) -> Result<u8, ()> {
    let _claimed = place.try_take();
    let value = cell.take().ok_or(())?;
    r.ok();
    let _ = r.unwrap_or_default();
    let _ = r.unwrap_or_else(|_| 0);
    let _kept = core::mem::ManuallyDrop::new(value);
    let _gone = Box::leak(Box::new(1));
    let _word = Field::from_bits(value).bits();
    let _wide = u16::from_le_bytes([payload[0], value]);
    Ok(value)
}
#[cfg(test)]
mod tests {
    pub enum TestError { AlreadyRunning }
    fn t(r: Result<u8, ()>) { r.ok(); }
}
"#,
    )
    .expect("source");
    let mut found = BTreeMap::<&str, Vec<String>>::new();
    let finders: [(&str, &Finder); 8] = [
        ("claims", &claim_at),
        ("errors", &error_state_at),
        ("statuses", &status_failure_at),
        ("dropped", &dropped_result_at),
        ("kept", &kept_out_at),
        ("compatibility", &compatibility_at),
        ("registers", &raw_register_at),
        ("bytes", &raw_bytes_at),
    ];
    walk_file(&source, &mut |_: &Path, seen: Seen<'_>| match seen {
        Seen::Token(tokens, at) => {
            for (name, find) in &finders {
                if let Some(hit) = find(tokens, at) {
                    found.entry(name).or_default().push(hit);
                }
            }
        }
        Seen::Attribute(attribute) => {
            if let Some(hit) = compatibility_attribute(attribute) {
                found.entry("compatibility").or_default().push(hit);
            }
        }
        Seen::Unreadable => panic!("readable"),
    });
    assert_eq!(found["claims"], ["try_take", "take refused"]);
    assert_eq!(found["errors"], ["StartError: AlreadyRunning NotStarted"]);
    assert_eq!(found["statuses"], ["PollStep: Failed"]);
    assert_eq!(
        found["dropped"],
        [".ok();", "unwrap_or_default", "unwrap_or_else(|_|"]
    );
    assert_eq!(found["kept"], ["ManuallyDrop", "leak"]);
    assert_eq!(
        found["compatibility"],
        ["pub use … as", "serde(default)", "deprecated"]
    );
    assert_eq!(found["registers"], ["from_bits", "bits"]);
    assert_eq!(found["bytes"], ["from_le_bytes", "index"]);
}

#[test]
#[ignore = "prints the T8c lists from the scans' hits"]
fn t8c_print() {
    let none: &AttributeFinder = &|_| None;
    let scans: [Scan<'_>; 9] = [
        ("CLAIMS", &claims_scope, &claim_at, none),
        ("VIOLATION_SITES", &product_code, &violation_site_at, none),
        ("ERROR_STATES", &product_code, &error_state_at, none),
        ("STATUS_FAILURES", &product_code, &status_failure_at, none),
        ("DROPPED_RESULTS", &product_code, &dropped_result_at, none),
        ("KEPT_OUT", &product_code, &kept_out_at, none),
        (
            "COMPATIBILITY",
            &product_code,
            &compatibility_at,
            &compatibility_attribute,
        ),
        ("RAW_REGISTERS", &register_code, &raw_register_at, none),
        ("RAW_BYTES", &protocol_code, &raw_bytes_at, none),
    ];
    for (name, select, find, attribute) in scans {
        let hits = scan_hits(select, find, attribute);
        std::println!("const {name}: &[(&str, usize, &str)] = &[");
        for (file, count) in by_file(&hits) {
            std::println!("    (\"{file}\", {count}, {}),", remover_name(&file));
        }
        std::println!("];");
    }
}

// ---- K0: a state is a sum, and one function calls a port's transition -----

/// K0's packages: the protocols, the Wi-Fi radio crates, the applications'
/// entries and `oer`, but testing packages and models.
fn state_code(path: &str) -> bool {
    let wifi = path.starts_with("chips/") && path.contains("/radio/ieee80211");
    let app = path.starts_with("chips/") && path.ends_with("/app");
    (path.starts_with("protocols/") || wifi || app || path == "oer")
        && !path.split('/').any(|part| part == "testing")
        && !path.ends_with("/model")
}

/// The parts of `tokens` between top-level commas, the angle brackets of a
/// type counted (an arrow's `>` is not one).
fn comma_parts(tokens: &[TokenTree]) -> Vec<&[TokenTree]> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, token) in tokens.iter().enumerate() {
        if let TokenTree::Punct(punct) = token {
            let arrow = index
                .checked_sub(1)
                .is_some_and(|before| punct_at(tokens, before, '-'));
            match punct.as_char() {
                '<' => depth += 1,
                '>' if !arrow => depth -= 1,
                ',' if depth == 0 => {
                    parts.extend(tokens.get(start..index));
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if start < tokens.len() {
        parts.extend(tokens.get(start..));
    }
    parts
}

/// The attributes and the visibility at the start of a field, a variant or
/// a parameter, passed over: the index after them.
fn past_attributes(part: &[TokenTree]) -> usize {
    let mut at = 0;
    loop {
        match part.get(at) {
            Some(TokenTree::Punct(hash)) if hash.as_char() == '#' => at += 2,
            Some(TokenTree::Ident(word)) if word == "pub" || word == "mut" => {
                at += 1;
                if group_at(part, at, Delimiter::Parenthesis).is_some() {
                    at += 1;
                }
            }
            _ => return at,
        }
    }
}

/// A named field or a parameter: its name and its type's tokens.
fn named_part(part: &[TokenTree]) -> Option<(String, &[TokenTree])> {
    let at = past_attributes(part);
    let name = ident_at(part, at)?;
    (punct_at(part, at + 1, ':') && !punct_at(part, at + 2, ':'))
        .then(|| (name, part.get(at + 2..).unwrap_or(&[])))
}

/// Whether a type's tokens are `bool`, the one spelling the lints count.
fn is_bool(ty: &[TokenTree]) -> bool {
    matches!(ty, [TokenTree::Ident(word)] if word == "bool")
}

/// Whether a type's tokens mention `bool` anywhere but as a function's
/// result (after `->`): `bool`, `Option<bool>`, `Cell<bool>`, `[bool; N]`,
/// a tuple with one. Groups are searched.
fn mentions_bool(ty: &[TokenTree]) -> bool {
    let mut depth = 0_i32;
    let mut skip_from: Option<i32> = None;
    for (index, token) in ty.iter().enumerate() {
        match token {
            TokenTree::Punct(punct) => {
                let arrow = index
                    .checked_sub(1)
                    .is_some_and(|before| punct_at(ty, before, '-'));
                match punct.as_char() {
                    '>' if arrow => {
                        skip_from.get_or_insert(depth);
                    }
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if skip_from.is_some_and(|from| depth < from) {
                            skip_from = None;
                        }
                    }
                    ',' if skip_from == Some(depth) => skip_from = None,
                    _ => {}
                }
            }
            _ if skip_from.is_some() => {}
            TokenTree::Ident(word) if word == "bool" => return true,
            TokenTree::Group(group)
                if mentions_bool(&group.stream().into_iter().collect::<Vec<_>>()) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether a type's tokens are an `Option<…>`.
fn is_option(ty: &[TokenTree]) -> bool {
    let words = ty
        .iter()
        .take_while(|token| !matches!(token, TokenTree::Punct(punct) if punct.as_char() == '<'))
        .filter_map(|token| match token {
            TokenTree::Ident(word) => Some(word.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    words.last().is_some_and(|word| word == "Option")
        && ty
            .iter()
            .any(|token| matches!(token, TokenTree::Punct(punct) if punct.as_char() == '<'))
}

/// The item named after the keyword `keyword` at `at`, and its first group
/// after its name and generics, before a `;` or a body of another kind.
fn item_group<'t>(
    tokens: &'t [TokenTree],
    at: usize,
    keyword: &str,
) -> Option<(String, &'t proc_macro2::Group)> {
    if ident_at(tokens, at)? != keyword {
        return None;
    }
    let name = ident_at(tokens, at + 1)?;
    let mut index = at + 2;
    while let Some(token) = tokens.get(index) {
        match token {
            TokenTree::Group(group)
                if matches!(group.delimiter(), Delimiter::Brace | Delimiter::Parenthesis) =>
            {
                return Some((name, group));
            }
            TokenTree::Punct(punct) if punct.as_char() == ';' => return None,
            _ => index += 1,
        }
    }
    None
}

/// The fields of a body (a struct's or a variant's), named or not: each
/// with its name (`0`, `1`, … for a tuple's) and its type.
fn body_fields(body: &proc_macro2::Group) -> Vec<(String, Vec<TokenTree>)> {
    let tokens = body.stream().into_iter().collect::<Vec<_>>();
    comma_parts(&tokens)
        .into_iter()
        .enumerate()
        .filter_map(|(index, part)| match body.delimiter() {
            Delimiter::Brace => named_part(part).map(|(name, ty)| (name, ty.to_vec())),
            _ => Some((
                index.to_string(),
                part.get(past_attributes(part)..)?.to_vec(),
            )),
        })
        .collect()
}

/// A struct's name and its fields.
type Fields = (String, Vec<(String, Vec<TokenTree>)>);

/// The fields of the struct at `at`, named or a tuple's.
fn struct_fields(tokens: &[TokenTree], at: usize) -> Option<Fields> {
    let (name, body) = item_group(tokens, at, "struct")?;
    Some((name, body_fields(body)))
}

/// The number of a body's flags (fields whose type mentions `bool`), when
/// no lint sees them: two or more, and at most one a plain `bool`
/// (`struct_excessive_bools` counts plain `bool` fields of a struct, and
/// sees no enum's variant).
/// The body whose flags are counted: a struct's, whose plain `bool` fields
/// the lint counts, or an enum variant's, which it does not see.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Body {
    Struct,
    Variant,
}

fn unseen_flags(fields: &[(String, Vec<TokenTree>)], body: Body) -> Option<usize> {
    let flags = fields.iter().filter(|(_, ty)| mentions_bool(ty)).count();
    let plain = fields.iter().filter(|(_, ty)| is_bool(ty)).count();
    (flags >= 2 && !(body == Body::Struct && plain >= 2)).then_some(flags)
}

/// `STATE_FLAGS`: what `struct_excessive_bools` does not see of a state kept
/// as flags: a struct's or an enum variant's fields that mention `bool`,
/// two or more, as `Name N` or `Enum::Variant N`.
fn state_flags_at(tokens: &[TokenTree], at: usize) -> Vec<String> {
    if let Some((name, fields)) = struct_fields(tokens, at) {
        return unseen_flags(&fields, Body::Struct)
            .map(|flags| std::format!("{name} {flags}"))
            .into_iter()
            .collect();
    }
    let Some((name, body)) = item_group(tokens, at, "enum") else {
        return Vec::new();
    };
    let variants = body.stream().into_iter().collect::<Vec<_>>();
    comma_parts(&variants)
        .into_iter()
        .filter_map(|variant| {
            let at = past_attributes(variant);
            let variant_name = ident_at(variant, at)?;
            let fields = match variant.get(at + 1) {
                Some(TokenTree::Group(group)) => body_fields(group),
                _ => return None,
            };
            unseen_flags(&fields, Body::Variant)
                .map(|flags| std::format!("{name}::{variant_name} {flags}"))
        })
        .collect()
}

/// `BOOL_PARAMETERS`: what `fn_params_excessive_bools` does not see of a
/// parameter that selects behaviour: one whose type mentions `bool` but is
/// not a plain `bool`, as `name N`.
fn bool_parameter_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let (name, parameters) = item_group(tokens, at, "fn")?;
    if parameters.delimiter() != Delimiter::Parenthesis {
        return None;
    }
    let parameters = parameters.stream().into_iter().collect::<Vec<_>>();
    let unseen = comma_parts(&parameters)
        .into_iter()
        .filter_map(named_part)
        .filter(|(_, ty)| mentions_bool(ty) && !is_bool(ty))
        .count();
    (unseen > 0).then(|| std::format!("{name} {unseen}"))
}

/// `bool_parameter_at`, as the counted scans read it.
fn bool_parameters_at(tokens: &[TokenTree], at: usize) -> Vec<String> {
    bool_parameter_at(tokens, at).into_iter().collect()
}

/// What a slot's use at `at` does to the field it names: `.field.take()`,
/// `.replace(…)`, `.insert(…)`, `.get_or_insert…`; any assignment
/// `.field = …`; and `&mut ….field`, what `mem::take`, `mem::replace` and an
/// alias go through.
fn slot_use_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    if punct_at(tokens, at, '&') && ident_at(tokens, at + 1).is_some_and(|word| word == "mut") {
        // The place borrowed: its last field.
        let mut index = at + 2;
        let mut field = None;
        while ident_at(tokens, index).is_some() {
            if punct_at(tokens, index + 1, '.') {
                index += 2;
                field = ident_at(tokens, index).or(field);
            } else {
                break;
            }
        }
        return field;
    }
    if !punct_at(tokens, at, '.') {
        return None;
    }
    let field = ident_at(tokens, at + 1)?;
    if punct_at(tokens, at + 2, '.') {
        let method = ident_at(tokens, at + 3)?;
        let fills = ["take", "replace", "insert"].contains(&method.as_str())
            || method.starts_with("get_or_insert");
        return (fills && group_at(tokens, at + 4, Delimiter::Parenthesis).is_some())
            .then_some(field);
    }
    let assigns = matches!(
        tokens.get(at + 2),
        Some(TokenTree::Punct(equals)) if equals.as_char() == '='
            && equals.spacing() == proc_macro2::Spacing::Alone
    );
    assigns.then_some(field)
}

/// The production code of the packages `select` takes, by crate: what
/// `find` reports of each token, with its file below the root.
fn crate_hits(select: &Select, find: &Finder) -> Vec<Vec<(String, String)>> {
    let root = root();
    let mut crates = Vec::new();
    for (path, manifest) in packages(&root) {
        if !select(&path) {
            continue;
        }
        for crate_root in crate_roots(&root.join(&path), &manifest) {
            let mut hits = Vec::new();
            walk_crate_root(&crate_root, &mut |file: &Path, seen: Seen<'_>| {
                let file = file.strip_prefix(&root).map_or_else(
                    |_| file.display().to_string(),
                    |file| file.display().to_string(),
                );
                if let Seen::Token(tokens, at) = seen
                    && let Some(what) = find(tokens, at)
                {
                    hits.push((file, what));
                }
            });
            crates.push(hits);
        }
    }
    crates
}

/// `OPTION_SLOTS`: an `Option` field its own crate fills, takes, assigns or
/// borrows `&mut`, as `file::Struct.field`.
fn option_slots(select: &Select) -> BTreeSet<String> {
    let fields: &Finder = &|tokens, at| {
        let (name, fields) = struct_fields(tokens, at)?;
        let options = fields
            .iter()
            .filter(|(_, ty)| is_option(ty))
            .map(|(field, _)| field.as_str())
            .collect::<Vec<_>>();
        (!options.is_empty()).then(|| std::format!("{name} {}", options.join(" ")))
    };
    let uses: &Finder = &|tokens, at| slot_use_at(tokens, at).map(|field| std::format!(".{field}"));
    let both: &Finder = &|tokens, at| fields(tokens, at).or_else(|| uses(tokens, at));
    let mut slots = BTreeSet::new();
    for hits in crate_hits(select, both) {
        let used = hits
            .iter()
            .filter_map(|(_, what)| what.strip_prefix('.'))
            .collect::<BTreeSet<_>>();
        for (file, what) in &hits {
            let mut words = what.split(' ');
            let Some(name) = words.next().filter(|name| !name.starts_with('.')) else {
                continue;
            };
            for field in words.filter(|field| used.contains(field)) {
                slots.insert(std::format!("{file}::{name}.{field}"));
            }
        }
    }
    slots
}

/// The hits of a finder that reports `name N`, as `file::name` repeated `N`
/// times: a list of paths with their counts, where a path gaining one
/// fails as a file gaining a hit does.
fn counted_paths(
    select: &Select,
    find: fn(&[TokenTree], usize) -> Vec<String>,
) -> Vec<(String, String)> {
    let finder: &Finder = &move |tokens, at| {
        let found = find(tokens, at);
        (!found.is_empty()).then(|| found.join("\n"))
    };
    let mut hits = Vec::new();
    for (file, found) in scan_hits(select, finder, NO_ATTRIBUTE) {
        for entry in found.lines() {
            let Some((name, count)) = entry.rsplit_once(' ') else {
                continue;
            };
            for _ in 0..count.parse::<usize>().unwrap_or(1) {
                hits.push((std::format!("{file}::{name}"), String::new()));
            }
        }
    }
    hits
}

/// The paths of a scan against its list: a new one and an entry with no
/// hit both fail.
fn assert_paths(found: &BTreeSet<String>, list: &[(&str, &str)]) {
    let listed = list.iter().map(|(path, _)| *path).collect::<BTreeSet<_>>();
    let new = found
        .iter()
        .filter(|path| !listed.contains(path.as_str()))
        .map(|path| std::format!("new: {path}"));
    let gone = listed
        .iter()
        .filter(|path| !found.contains(**path))
        .map(|path| std::format!("gone, remove its entry: {path}"));
    let differences = new.chain(gone).collect::<Vec<_>>();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// The two lints of the rule, spelled so that this file names neither.
fn bool_lints() -> [String; 2] {
    [
        ["struct", "_excessive_bools"].concat(),
        ["fn_params", "_excessive_bools"].concat(),
    ]
}

/// The lints an attribute must not name: the two, and the one group of
/// Clippy that holds them (`clippy-driver rustc -W help`, its lint groups:
/// `clippy::pedantic` alone).
fn bool_lint_names() -> [String; 3] {
    let [structs, parameters] = bool_lints();
    [
        std::format!("clippy::{structs}"),
        std::format!("clippy::{parameters}"),
        ["clippy::", "pedantic"].concat(),
    ]
}

/// Whether `source` holds an `allow`, `expect` or `warn` of one of the two
/// lints or of their group, an attribute's or a `cfg_attr`'s, of an item or
/// of a crate: each switches the denied lint off or lowers it.
fn silences_a_bool_lint(source: &str) -> bool {
    let names = bool_lint_names();
    ["allow(", "expect(", "warn("].iter().any(|attribute| {
        source.match_indices(attribute).any(|(at, _)| {
            let inside = source
                .get(at..)
                .and_then(|rest| rest.split(')').next())
                .unwrap_or("");
            names.iter().any(|name| inside.contains(name.as_str()))
        })
    })
}

/// The files that build a diagnostic record from a register's whole word:
/// the one door of `record_word`, the method a `record-words` entry of the
/// register model gives a read value. A word decides nothing, so a new
/// caller fails here; the developer's tools are aside.
const RECORD_WORD_CALLERS: &[&str] = &[
    "chips/esp32s31/hal/src/radio/bluetooth.rs",
    "chips/esp32s31/hal/src/radio/bluetooth/observation.rs",
    "chips/esp32s31/hal/src/radio/bluetooth/registers/modem_timer.rs",
    "chips/esp32s31/hal/src/system/platform/interrupt_controller.rs",
    "hil/probes/esp32s31-facts/src/image/analog_pmu.rs",
    "hil/probes/esp32s31-facts/src/image/ble.rs",
    "hil/probes/esp32s31-facts/src/image/ble/reset.rs",
    "hil/probes/esp32s31-facts/src/image/ble_masked.rs",
    "hil/probes/esp32s31-facts/src/image/ble_wave2/lp.rs",
    "hil/probes/esp32s31-facts/src/image/lp_ack_race.rs",
    "hil/probes/esp32s31-facts/src/image/lp_lanes.rs",
    "hil/probes/esp32s31-facts/src/image/lp_lanes/rest.rs",
    "hil/probes/esp32s31-facts/src/image/lp_softreset.rs",
    "hil/probes/esp32s31-facts/src/image/pb1.rs",
];

/// A call of `.record_word()`.
fn record_word_at(tokens: &[TokenTree], at: usize) -> Option<String> {
    let called = ident_at(tokens, at)? == "record_word"
        && group_at(tokens, at + 1, Delimiter::Parenthesis).is_some()
        && at
            .checked_sub(1)
            .is_some_and(|before| punct_at(tokens, before, '.'));
    called.then(|| "record_word".to_owned())
}

#[test]
fn a_registers_record_word_is_taken_only_where_a_record_is_built() {
    // Every package's code, tests among it; the developer's tools aside.
    let callers = scan_hits(
        &|path: &str| !path.starts_with("tools/"),
        &record_word_at,
        NO_ATTRIBUTE,
    )
    .into_iter()
    .map(|(file, _)| file)
    .collect::<BTreeSet<_>>();
    let listed = RECORD_WORD_CALLERS
        .iter()
        .map(|file| (*file).to_owned())
        .collect::<BTreeSet<_>>();
    let new = callers.difference(&listed).collect::<Vec<_>>();
    let gone = listed.difference(&callers).collect::<Vec<_>>();
    assert!(
        new.is_empty() && gone.is_empty(),
        "new callers {new:?}; listed without a call {gone:?}"
    );
}

/// Every Rust file under `directory`, `target` and `.git` aside.
fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if ![".git", "target"].contains(&entry.file_name().to_string_lossy().as_ref()) {
                rust_files(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The packages outside the two lints: the developer's machine
/// (`tools/`), but the layer tests, which are the check and keep the rule.
fn outside_the_bool_lints(path: &str) -> bool {
    path.starts_with("tools/") && path != "tools/test/layers"
}

/// The packages at `max-fn-params-bools = 1`: those of the layer tests'
/// roles `Pac`, `Hal` and `HardwareDriver`, where one `bool` parameter is
/// a bit's value.
fn bit_parameters(path: &str) -> bool {
    matches!(
        role(path),
        Some(Role::Pac | Role::Hal | Role::HardwareDriver)
    )
}

/// The Clippy configuration a package reads, as Clippy finds it: the
/// nearest `clippy.toml` from the package's directory up to the root.
fn clippy_config_of(path: &str) -> (PathBuf, toml::Value) {
    let root = root();
    let mut directory = root.join(path);
    let file = loop {
        let candidate = directory.join("clippy.toml");
        if candidate.is_file() || directory == root {
            break candidate;
        }
        match directory.parent() {
            Some(parent) => directory = parent.to_path_buf(),
            None => break root.join("clippy.toml"),
        }
    };
    let config = read_toml(&file);
    (file, config)
}

#[test]
fn no_source_allows_or_expects_a_bool_lint() {
    let root = root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let silenced = files
        .into_iter()
        .filter(|file| {
            std::fs::read_to_string(file).is_ok_and(|source| silences_a_bool_lint(&source))
        })
        .map(|file| {
            file.strip_prefix(&root)
                .unwrap_or(&file)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(
        silenced.is_empty(),
        "the bool lints are silenced in:\n{}",
        silenced.join("\n")
    );
}

#[test]
fn a_bool_lint_silenced_by_any_spelling_is_found() {
    let [structs, parameters] = bool_lints();
    for source in [
        std::format!("#[allow(clippy::{structs})]"),
        std::format!("#[expect(clippy::{parameters}, reason = \"x\")]"),
        std::format!("#![allow(clippy::{structs})]"),
        std::format!("#[cfg_attr(test, allow(clippy::{parameters}))]"),
        std::format!("#[expect(\n    clippy::{structs},\n    reason = \"x\"\n)]"),
        std::format!("#[warn(clippy::{structs})]"),
        ["#[allow(clippy::", "pedantic)]"].concat(),
        ["#![warn(clippy::", "pedantic)]"].concat(),
    ] {
        assert!(silences_a_bool_lint(&source), "{source}");
    }
    assert!(!silences_a_bool_lint(
        "#[allow(clippy::large_enum_variant)]"
    ));
}

#[test]
fn every_clippy_config_holds_the_rules_thresholds() {
    let root = root();
    let common = read_toml(&root.join("clippy.toml"));
    let mut problems = Vec::new();
    for (path, _) in packages(&root) {
        let (file, config) = clippy_config_of(&path);
        let (struct_bools, fn_bools) = if outside_the_bool_lints(&path) {
            (255, 255)
        } else {
            (1, i64::from(bit_parameters(&path)))
        };
        // The file the package reads keeps every common ban, wherever it
        // stands: one between the root and the package checks for each
        // package below it.
        let missing = missing_lints(&config, &common, "disallowed-methods");
        if !missing.is_empty() {
            problems.push(std::format!(
                "{path}: {} lost common bans: {missing:?}",
                file.display()
            ));
        }
        if config
            .get("max-struct-bools")
            .and_then(toml::Value::as_integer)
            != Some(struct_bools)
        {
            problems.push(std::format!(
                "{path}: max-struct-bools is not {struct_bools} in {}",
                file.display()
            ));
        }
        if config
            .get("max-fn-params-bools")
            .and_then(toml::Value::as_integer)
            != Some(fn_bools)
        {
            problems.push(std::format!(
                "{path}: max-fn-params-bools is not {fn_bools} in {}",
                file.display()
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_package_inherits_the_workspace_lints_and_no_more() {
    let wrong = packages(&root())
        .into_iter()
        .filter(|(_, manifest)| {
            let lints = manifest.get("lints").and_then(toml::Value::as_table);
            lints.is_none_or(|lints| {
                lints.len() != 1
                    || lints.get("workspace").and_then(toml::Value::as_bool) != Some(true)
            })
        })
        .map(|(path, _)| path)
        .collect::<Vec<_>>();
    assert!(
        wrong.is_empty(),
        "not `[lints] workspace = true` alone:\n{}",
        wrong.join("\n")
    );
}

/// Whether `file` is a Clippy configuration the checks would not read: a
/// `.clippy.toml`, which Clippy reads before a `clippy.toml` beside it.
fn clippy_config_hidden(file: &Path) -> bool {
    file.file_name().and_then(std::ffi::OsStr::to_str) == Some(".clippy.toml")
}

/// Whether a Cargo configuration names one of the two lints, a Clippy lint
/// or group after `-A` or `-W`, or `cap-lints`.
fn cargo_config_lowers_a_lint(source: &str) -> bool {
    let [structs, parameters] = bool_lints();
    source.contains(&structs)
        || source.contains(&parameters)
        || source.contains("cap-lints")
        || ["-A", "-W"].iter().any(|flag| {
            source.match_indices(flag).any(|(at, _)| {
                source.get(at + flag.len()..).is_some_and(|rest| {
                    rest.trim_start_matches([' ', '"', '\'', ','])
                        .starts_with("clippy::")
                })
            })
        })
}

/// Every file under `directory`, `target` and `.git` aside.
fn every_file(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !["target", ".git"].contains(&entry.file_name().to_string_lossy().as_ref()) {
                every_file(&path, found);
            }
        } else {
            found.push(path);
        }
    }
}

#[test]
fn no_clippy_configuration_hides_and_no_cargo_configuration_lowers_a_lint() {
    let root = root();
    let mut files = Vec::new();
    every_file(&root, &mut files);
    let relative = |file: &PathBuf| {
        file.strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string()
    };
    let hidden = files
        .iter()
        .filter(|file| clippy_config_hidden(file))
        .map(relative)
        .collect::<Vec<_>>();
    assert!(
        hidden.is_empty(),
        "a `.clippy.toml`:\n{}",
        hidden.join("\n")
    );
    let lowering = files
        .iter()
        .filter(|file| {
            file.strip_prefix(&root)
                .is_ok_and(|file| file.starts_with(".cargo"))
        })
        .filter(|file| {
            std::fs::read_to_string(file).is_ok_and(|source| cargo_config_lowers_a_lint(&source))
        })
        .map(relative)
        .collect::<Vec<_>>();
    assert!(
        lowering.is_empty(),
        "a Cargo configuration lowers a lint:\n{}",
        lowering.join("\n")
    );
}

#[test]
fn the_workspace_denies_both_bool_lints() {
    let manifest = read_toml(&root().join("Cargo.toml"));
    let table = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(|lints| lints.get("clippy"));
    for lint in bool_lints() {
        let level = table
            .and_then(|table| table.get(lint.as_str()))
            .and_then(|level| {
                level
                    .as_str()
                    .or_else(|| level.get("level").and_then(toml::Value::as_str))
            });
        assert!(
            matches!(level, Some("deny" | "forbid")),
            "clippy::{lint} is {level:?} in the workspace's table"
        );
    }
}

#[test]
fn a_hidden_or_lowering_configuration_is_found() {
    assert!(clippy_config_hidden(Path::new("/r/a/b/.clippy.toml")));
    assert!(clippy_config_hidden(Path::new("/r/.clippy.toml")));
    assert!(!clippy_config_hidden(Path::new("/r/a/clippy.toml")));
    let [structs, _] = bool_lints();
    for source in [
        std::format!("rustflags = [\"-A\", \"clippy::{structs}\"]"),
        "rustflags = [\"-W\", \"clippy::pedantic\"]".to_owned(),
        "rustflags = [\"-Wclippy::all\"]".to_owned(),
        "rustflags = [\"--cap-lints\", \"warn\"]".to_owned(),
    ] {
        assert!(cargo_config_lowers_a_lint(&source), "{source}");
    }
    assert!(!cargo_config_lowers_a_lint(
        "rustflags = [\"-C\", \"force-frame-pointers=yes\"]"
    ));
}

#[test]
fn hidden_flags_are_listed_and_only_shrink() {
    assert_scan(&counted_paths(&state_code, state_flags_at), HIDDEN_FLAGS);
}

#[test]
fn hidden_bool_parameters_are_listed_and_only_shrink() {
    assert_scan(
        &counted_paths(&state_code, bool_parameters_at),
        HIDDEN_BOOL_PARAMETERS,
    );
}

#[test]
fn option_slots_are_listed_and_only_shrink() {
    assert_paths(&option_slots(&state_code), OPTION_SLOTS);
}

/// The K0 finders on a source of each shape, its tests skipped.
#[test]
fn the_state_scans_find_their_shapes_and_skip_tests() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("lib.rs");
    std::fs::write(
        &source,
        r#"
pub struct Plain { a: bool, b: bool }
pub struct Hidden { running: Option<bool>, stopping: core::cell::Cell<bool>, one: bool }
pub struct Tuple(bool, [bool; 2]);
pub enum Phase { Idle, Busy { started: bool, stopped: Option<bool> }, Done(bool, (u8, bool)) }
pub struct Calls { f: fn(u8) -> bool, g: Option<fn() -> bool> }
pub struct Slots { taken: Option<u8>, set: Option<u8>, borrowed: Option<u8>, read: Option<u8> }
fn f(this: &mut Slots, on: Option<bool>, plain: bool, cb: fn(u8) -> bool, v: Option<u8>) {
    let _ = this.taken.take();
    this.set = v;
    let _ = core::mem::take(&mut this.borrowed);
    let _ = this.read.as_ref();
}
#[cfg(test)]
mod tests {
    struct T { a: Option<bool>, b: Option<bool> }
    fn t(x: Option<bool>) {}
}
"#,
    )
    .expect("source");
    let mut flags = Vec::new();
    let mut parameters = Vec::new();
    let mut uses = Vec::new();
    walk_file(&source, &mut |_: &Path, seen: Seen<'_>| {
        if let Seen::Token(tokens, at) = seen {
            flags.extend(state_flags_at(tokens, at));
            parameters.extend(bool_parameter_at(tokens, at));
            uses.extend(slot_use_at(tokens, at));
        }
    });
    // `Plain` is the lint's; a function's result is no flag.
    assert_eq!(
        flags,
        ["Hidden 3", "Tuple 2", "Phase::Busy 2", "Phase::Done 2"]
    );
    assert_eq!(parameters, ["f 1"]);
    assert_eq!(uses, ["taken", "set", "borrowed"]);
}

#[test]
#[ignore = "prints the K0 lists from the scans' hits"]
fn k0_print() {
    let counted = |name: &str, hits: Vec<(String, String)>| {
        std::println!("const {name}: &[(&str, usize, &str)] = &[");
        for (path, count) in by_file(&hits) {
            std::println!("    ({path:?}, {count}, {}),", remover_name(&path));
        }
        std::println!("];");
    };
    counted("HIDDEN_FLAGS", counted_paths(&state_code, state_flags_at));
    counted(
        "HIDDEN_BOOL_PARAMETERS",
        counted_paths(&state_code, bool_parameters_at),
    );
    std::println!("const OPTION_SLOTS: &[(&str, &str)] = &[");
    for path in option_slots(&state_code) {
        std::println!("    ({path:?}, {}),", remover_name(&path));
    }
    std::println!("];");
}

/// K0, `HIDDEN_FLAGS`: what `struct_excessive_bools` does not see of a state
/// kept as flags (an enum variant's fields; a struct's fields whose type
/// mentions `bool` without being it), by `file::Struct` or
/// `file::Enum::Variant` with the number of flags. It misses a flag behind
/// a type alias (`type Flag = bool`). The list only shrinks.
const HIDDEN_FLAGS: &[(&str, usize, &str)] = &[
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu.rs::HtAmpduTxStorage",
        2,
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/observation.rs::RateClass::Ht",
        2,
        BY_P2,
    ),
    ("chips/family/app/src/observe/link.rs::Rate::Ht", 2, BY_NONE),
    (
        "protocols/ieee80211/contracts/port/src/contract.rs::MacRxMetadata",
        3,
        BY_P3,
    ),
    (
        "protocols/ieee80211/contracts/port/src/rx.rs::RxMeta",
        3,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::ApSaeFault::InvalidSession",
        3,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/banks.rs::RxBlockAckEvent::Contained",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/ap.rs::ApManagementRequest::Association",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/block_ack/frame.rs::BlockAckAction::AddbaRequest",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/block_ack/frame.rs::BlockAckAction::AddbaResponse",
        2,
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/he.rs::HeMuSigBNonMimoUser::Scheduled",
        3,
        BY_P3,
    ),
];

/// K0, `HIDDEN_BOOL_PARAMETERS`: what `fn_params_excessive_bools` does not
/// see of a parameter that selects behaviour (a type that mentions `bool`
/// without being it), by `file::function` with their number. It misses a
/// `bool` behind a type alias, and a closure's parameter. The list only
/// shrinks.
const HIDDEN_BOOL_PARAMETERS: &[(&str, usize, &str)] = &[
    (
        "chips/esp32s31/radio/ieee80211/station/src/observation.rs::of",
        1,
        BY_P2,
    ),
    (
        "chips/family/radio/ieee80211/policy/src/classification.rs::apply_wmm_admission",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::probe",
        1,
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::commit_step_with",
        1,
        BY_P3,
    ),
];

/// K0, `OPTION_SLOTS`: an `Option` field its own crate takes, replaces,
/// inserts into, assigns or borrows `&mut`, by `file::Struct.field`: a slot
/// filled and taken. It names a field by its name in the crate, so a field
/// of another struct of that name counts it; it misses a slot filled
/// through a method of the field's own type that takes `&mut self` (an
/// `Option` wrapped in a newtype). The list only shrinks.
const OPTION_SLOTS: &[(&str, &str)] = &[
    (
        "chips/esp32s31/app/src/lib.rs::Boot.access_point_device",
        BY_P2,
    ),
    ("chips/esp32s31/app/src/lib.rs::Boot.app_alarm", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.boot_id", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.device", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.entropy", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.observe", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.radio", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.timer", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.timers", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Boot.urgent", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::GroupBoot.observe", BY_P2),
    (
        "chips/esp32s31/app/src/lib.rs::GroupPeripherals.access_point",
        BY_P2,
    ),
    (
        "chips/esp32s31/app/src/lib.rs::GroupPeripherals.device",
        BY_P2,
    ),
    ("chips/esp32s31/app/src/lib.rs::Handed.access_point", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Handed.device", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Pending.access_point", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Pending.datapath", BY_P2),
    ("chips/esp32s31/app/src/lib.rs::Pending.station", BY_P2),
    ("chips/esp32s31/app/src/observed.rs::Sampler.ended", BY_P2),
    ("chips/esp32s31/app/src/wifi.rs::Device.network", BY_P2),
    (
        "chips/esp32s31/app/src/wifi/access_point.rs::AccessPoint.end",
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::Queue.block_ack",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::Queue.completion",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::Queue.format",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::Queue.interface",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::Queue.named",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::State.access_point_tsf",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::State.active_reads",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::State.he",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::State.he_bss",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/hardware_model.rs::State.tbtt",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac.rs::LowerMacCore.fault",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac.rs::LowerMacCore.tbtt",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/ampdu.rs::AmpduBudget.he",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs::Chain.admission",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs::Esp32s31TxArena.fault",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs::Esp32s31TxArena.program",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs::OnAir.retry",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/arena.rs::SingleRetry.air_rate",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/lifecycle/bank.rs::CoreOwner.bank",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/lower_mac/lifecycle/bank.rs::VacantView.bank",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/irq/consumer.rs::IrqPublication.receive",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/irq/model.rs::MacInterruptModel.fatal_assertion_raw",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/local_time.rs::MacTimeline.last",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/rate/low.rs::MacLowRateSession.hardware",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/rx/pool.rs::NetworkRxFrame.times",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx.rs::HeAmpduTxConfig.trigger_based",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx.rs::HeTriggerBasedTxConfig.runtime_handoff_window_micros",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx.rs::TxSlot.home",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu.rs::HtAmpduTxStorage.trigger_publication_snapshot",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu.rs::HtAmpduTxStorage.trigger_reservation",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu/owner.rs::RetainedDmaAmpduTx.dma",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx/ampdu/owner.rs::RetainedDmaAmpduTx.storage",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/mac/tx_cost.rs::TxCost.ppdu_micros",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/ordinary_tx.rs::OrdinaryTxOwner.active",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/ordinary_tx.rs::OrdinaryTxOwner.last_outcome",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/ordinary_tx.rs::OrdinaryTxParked.last_outcome",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/ordinary_tx.rs::OrdinaryTxReport.completion",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/buffer.rs::Buffer.admission",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/buffer.rs::Buffer.loan",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/buffer.rs::Execution.beacon",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/buffer.rs::NetworkBuffer.admission",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/buffer.rs::NetworkBuffer.loan",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.active",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.actor",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.continuation",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.failed_retune",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.power",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::RadioDevice.retune",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/device.rs::StallOwner.after",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.arena",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.barrier",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.closing",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.configuring",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.observations",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Driver.retained_group",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Pending.air",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Pending.beacon",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Pending.try_work",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/driver.rs::Sink.tbtt",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.access_point_holding",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.access_point_since",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.deadline",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.fault",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.group_mutator",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.group_since",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.holding",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.receipt",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::Esp32s31Port.waker",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::PortSettings.color",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/engines.rs::PortSettings.he",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/keys.rs::Removing.place",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs::CallPlace.item",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs::CallPlace.vacancy",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs::Store.access_point_configure",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs::Store.gate",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/port/unconsumed.rs::Store.station_configure",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/runtime.rs::WifiRetunePending.immediate",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/runtime.rs::WifiRetunePending.pending",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters.rs::WifiRxExhaustion.last_after_doorbell",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters.rs::WifiRxExhaustion.next_after_doorbell",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters/stall.rs::RxStallSnapshot.hardware",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/counters/stall.rs::RxStallSnapshot.mac",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/receive.rs::Receiver.full_since",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/receive.rs::RecycledProbe.wait",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/session.rs::ReceiveSession.full_since",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/session.rs::ReceiveSession.port_close_started",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/session.rs::ReceiveSession.port_mask_started",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/session.rs::ReceiveSession.port_settle",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/backend/src/rx/session.rs::ReceiveSession.port_stop_deadline",
        BY_P3,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/chip.rs::Config.target",
        BY_P2,
    ),
    (
        "chips/esp32s31/radio/ieee80211/station/src/radio.rs::Platform.power",
        BY_P2,
    ),
    (
        "chips/family/app/src/observe/connect.rs::Connect.ended",
        BY_NONE,
    ),
    (
        "chips/family/app/src/observe/connect.rs::Visit.ended",
        BY_NONE,
    ),
    (
        "chips/family/app/src/observe/network.rs::TurnTally.publish",
        BY_NONE,
    ),
    (
        "chips/family/app/src/observe/profile.rs::ProfileTally.began",
        BY_NONE,
    ),
    (
        "chips/family/app/src/observe/profile.rs::ProfileTally.ended",
        BY_NONE,
    ),
    (
        "chips/family/app/src/observe/profile.rs::ProfileTally.gap",
        BY_NONE,
    ),
    (
        "chips/family/radio/ieee80211/policy/src/rate_control.rs::StaRateControlAssociation.ampdu_runtime",
        BY_P3,
    ),
    (
        "chips/family/radio/ieee80211/policy/src/rate_selection.rs::EspressifRateSelection.control",
        BY_P3,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/acl.rs::LeHostAclPacket.in_flight_end",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/dtm.rs::LeDtmCommandCompleteEvent.packet_count",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/controller/le/scanning.rs::LeLegacyScanningConfiguration.parameters",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/hci/src/h4.rs::CommandReceiver.expected",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/bootstrap.rs::LeControllerBootstrap.host_buffers",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/bootstrap.rs::LeControllerBootstrap.requested_random_address",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Pending.admitted",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Pending.config",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Service.lease",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Service.pending",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Service.response",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/controller/src/dtm.rs::Service.retained",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs::LePeripheralConnection.pending_channel_map",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs::LePeripheralConnection.pending_connection_update",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs::LePeripheralConnectionEventCompleted.connection_timing_transition",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs::LePeripheralConnectionEventPrepared.connection_timing_transition",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection.rs::LePeripheralConnectionRecurringEventProvisional.connection_timing_transition",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/connection/acknowledgement.rs::Acknowledgement.in_flight",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.data_length_request",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.local_version",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.peer_features",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.peer_version",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.remote_features_result",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.remote_version_result",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/bluetooth/le/ll/src/control.rs::LePeripheralControl.termination",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/hil/agent/src/capture/collector.rs::State.failure",
        BY_NONE,
    ),
    (
        "protocols/hil/agent/src/capture/collector.rs::State.identity",
        BY_NONE,
    ),
    (
        "protocols/hil/protocol/src/base/image_keys.rs::ImageKeysCheck.previous",
        BY_NONE,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/configuration.rs::WifiConfig.access_point",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/configuration.rs::WifiConfig.monitor",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/configuration.rs::WifiPlan.access_point",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/configuration.rs::WifiPlan.monitor",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/coordinator.rs::ChannelCoordinator.last_follow",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::AbsenceTerminal.then_leave",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Arbiter.absence",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Arbiter.cycle",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Arbiter.next_id",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Arbiter.primary",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Lease.access_point",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.absence_end",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.absence_started",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.announce",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.leave",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.moved",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.primary_end",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.primary_started",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::Outbox.start",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::StationPlaceReturns.absence",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::StationPlaceReturns.lease",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/arbiter/src/lib.rs::StoppedArbiter.next_id",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/protocol.rs::EspNowPeerSlot.config",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/security.rs::EspNowEncryptedPeerSlot.state",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/extensions/esp-now/src/extensions/espressif/esp_now/security.rs::EspNowPmkOwner.key",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/beacon.rs::ApBeacon.channel_switch",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/beacon.rs::ApBeacon.next_publication",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/engine.rs::ApEngine.probe_after",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/engine.rs::ApEngine.tsf_epoch",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApAssociationCapabilities.ht",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeer.authenticator",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeer.ht",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeer.pmk",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeer.pmkid",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeer.wpa2_retry_alarm",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/ap/src/service.rs::ApPeerStatus.ht",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs::ArenaBook.published",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs::ArenaBook.right",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs::DataLink.retry",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs::DataPlane.link",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/data/plane.rs::DataPlane.rate",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/events.rs::AggregateSettled.rate",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/ftm.rs::FtmPartialResult.pending",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/ftm.rs::SessionData.last_measurement",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/ftm.rs::SessionData.pending",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link.rs::LinkHandover.data",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link.rs::LinkReturn.tids",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link.rs::RxLink.replay",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link.rs::TxNumbers.tids",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link_monitor.rs::StaBeaconMonitor.deadline",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/link_monitor.rs::StaBeaconMonitor.last_observation",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Connected.background",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Connected.block_ack",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Connected.confirmation",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Connected.report",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Connected.rsn",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::ConnectedRsn.fault",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::ConnectedRsn.management",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::ConnectedRsn.replay",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Handshake.management",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Handshake.supplicant",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Keys.group_permission",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Keys.stray",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::NewConnection.confirmation",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::NewHandshake.management",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::NewHandshake.supplicant",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::RetiredStation.configuration",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::RetiredStation.notice",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::RetiredStation.profile",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Scan.access_point",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::StaConfig.tx_block_ack",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::StaConfigurationContext.epoch",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.absence",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.absence_place",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.configuration",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.epoch",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.place",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::Station.profile",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::StationRestart.notice",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.absence",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.absence_place",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.configuration",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.epoch",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.place",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine.rs::TransitionContext.profile",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/group_keys.rs::GroupKeys.current",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/management.rs::ManagementProtection.group",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/management.rs::ManagementProtection.request",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/management.rs::ManagementProtection.response",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/tx_block_ack.rs::StaTxBlockAck.pending",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/machine/tx_block_ack.rs::StaTxBlockAck.timing",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/modem_sleep.rs::PhaseTiming.deadline",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/request.rs::StationScanChannelOrderIter.preferred",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/sa_query.rs::ActiveSaQuery.next_retry",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/roles/sta/src/sa_query.rs::StationSaQuery.active",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator.rs::RsnAuthenticator.association_security_binding",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator.rs::RsnAuthenticator.pending_ptk",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::Session.accepted_scalar",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::Session.keys",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::Session.own",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::Session.peer_commit",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/authenticator/sae.rs::Session.stored_confirm",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/retry.rs::RsnRetry.pending",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/sae.rs::SaeComebackTokens.key_drawn",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/state/authenticator.rs::RsnApState.latest_built",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/supplicant.rs::RsnConnectedSupplicant.completed_group_message1",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/supplicant.rs::RsnStaSupplicant.completed_message3",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/security/rsn/src/supplicant.rs::RsnStaSupplicant.ptk",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/aggregate.rs::AmpduRun.previous",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/banks.rs::Bank.active",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/banks.rs::Bank.free",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/banks.rs::Header.association",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/originator.rs::Agreement.alarm",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/originator.rs::Agreement.due",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/recipient.rs::RxBlockAckNegotiation.inactivity",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/recipient.rs::RxBlockAckSession.gap",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/recipient.rs::RxBlockAckSession.inactivity",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/block_ack/reorder.rs::RxReorderRelease.rejected",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/fragmentation/reassembly.rs::ReassemblySlot.identity",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/monitor.rs::MonitorFilter.any_address",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/monitor.rs::MonitorFilter.minimum_rssi_dbm",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/protection.rs::ProtectionPolicy.rts_length_threshold",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/tx.rs::PublicationReport.ack_snr_db",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/tx.rs::TxExchange.ack_snr_db",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/tx/ampdu_status.rs::MacAmpduTxStatus.ack_snr_db",
        BY_P3,
    ),
    (
        "protocols/ieee80211/core/upper-mac/src/tx/ampdu_status.rs::MacIndividualRetries.final_rate",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/ap.rs::ApAssociationSecurityObservation.rsn_ie",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/ap.rs::ApAssociationSecurityObservation.rsnxe",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::AddressFields.address3",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::AddressFields.address4",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::AddressFields.sequence_control",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::AddressFields.transmitter",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::DecodedHeader.address4",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::DecodedHeader.qos",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::MacHeader.address4",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/frame.rs::MacHeader.qos",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/sae.rs::SaeReceivedCommit.rejected_groups",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/sae.rs::SaeReceivedCommit.token",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/scan.rs::ScanRecord.ds_channel",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/scan.rs::ScanRecord.ht_capability_ie",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/scan.rs::ScanRecord.ht_operation_ie",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/security.rs::RsnAssociation.pmkid",
        BY_P3,
    ),
    (
        "protocols/ieee80211/wire/src/station_beacon.rs::StaBeaconProtection.he_txop_rts_threshold",
        BY_P3,
    ),
    (
        "protocols/network/core/src/dhcp.rs::Client.lease",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/dhcp.rs::Client.link",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/dhcp.rs::Client.owed",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/dhcp.rs::Client.sent",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Configuration.dhcp",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Configuration.ipv4",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Configuration.lease",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Kept.outcome",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Kept.tag",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Socket.port",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/machine.rs::Socket.prepared",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/owner.rs::Transmitted.ended",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/payload.rs::Payload.left",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/places.rs::Line.head",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/places.rs::Line.tail",
        BY_PHASE_C,
    ),
    (
        "protocols/network/core/src/places.rs::Place.next",
        BY_PHASE_C,
    ),
    ("protocols/network/core/src/places.rs::Place.on", BY_PHASE_C),
    (
        "protocols/network/core/src/places.rs::Place.previous",
        BY_PHASE_C,
    ),
    ("protocols/network/port/src/slot.rs::Slot.held", BY_PHASE_C),
    (
        "protocols/network/runtime/src/local.rs::Producer.at",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/local.rs::Producer.ends",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/local.rs::Producer.hold",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/local.rs::Producer.socket",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/service.rs::Bound.closing",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/service.rs::Bound.lent",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/service.rs::Bound.open",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/socket.rs::Inbound.stopped",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/socket.rs::LateSocket.loans",
        BY_PHASE_C,
    ),
    (
        "protocols/network/runtime/src/socket.rs::Socket.inbound",
        BY_PHASE_C,
    ),
    (
        "protocols/network/wire/src/checksum.rs::Checksum.odd",
        BY_PHASE_C,
    ),
    (
        "protocols/radio/port/src/pool/fifo.rs::Place.ticket",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/radio/port/src/slot.rs::SlotRegistration.invocation",
        BY_RADIO_SYSTEM,
    ),
    (
        "protocols/radio/port/src/slot.rs::SlotRegistration.owner",
        BY_RADIO_SYSTEM,
    ),
];
