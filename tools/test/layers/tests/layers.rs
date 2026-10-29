//! Structural rules read from package manifests and sources: names, the
//! unsafe boundary, layer roles and their dependencies, protocol core/runtime
//! isolation from backends, and the HAL's atoms (`README.md`).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

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
        ("tools/image/bundle", false, "oer-image-bundle"),
        ("tools/vendor/scenario", false, "oer-vendor-scenario"),
        (
            "tools/vendor/scenario-cli",
            true,
            "oer-tool-vendor-scenario-cli",
        ),
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
/// HAL, the platform and executor bindings, the volatile
/// register-access core, the cores of memory the hardware shares, the host's operating-system bindings, the C-driver
/// stand, and the bootstrap and probe images. Every other package forbids
/// unsafe code.
const BELOW_THE_BOUNDARY: &[&str] = &[
    "chips/esp32s31/pac",
    "chips/esp32s31/hal",
    "chips/esp32s31/board",
    "chips/espressif/staged-bootstrap",
    "chips/espressif/staged-layout",
    "chips/espressif/staged-runtime",
    "chips/espressif/executor",
    "portable/memory",
    "portable/mmio",
    "portable/riscv",
    "chips/espressif/interrupt-table",
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
        "tools/os/serial",
        "firmware/probes/esp32s31-radio",
    ] {
        assert!(below_the_boundary(package), "{package}");
    }
    for package in [
        "chips/esp32s31/bluetooth/driver",
        "chips/esp32s31/phy",
        "portable/trace",
        "portable/embassy/net",
        "portable/embassy/time",
        "portable/interrupt-table",
        "tools/os/durable",
        "tools/hil/link",
        "firmware/apps/w1",
        "firmware/hil/esp32s31",
        "tools/lab/fixture-install",
    ] {
        assert!(!below_the_boundary(package), "{package}");
    }
}

// The metadata-derived Espressif value/algorithm crates in docs/architecture.md.
const ESPRESSIF_SANS_IO: &[&str] = &[
    "chips/esp32s31/phy-probe",
    "chips/esp32s31/instrumentation-hil",
    "chips/esp32s31/instrumentation",
    "chips/espressif/analog",
    "chips/espressif/boot",
    "chips/espressif/coex",
    "chips/espressif/efuse",
    "chips/espressif/facts",
    "chips/espressif/ieee80211-policy",
    "chips/espressif/ieee802154-engine",
    "chips/espressif/ieee802154-mac-values",
    "chips/espressif/modem-clock",
];

fn sans_io(package: &str) -> bool {
    package.starts_with("protocols/")
        || (package.starts_with("portable/")
            && !["portable/mmio", "portable/riscv", "portable/memory"].contains(&package))
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
        forbidden_paths(&graph, "oer-espressif-ieee802154-engine", |name| {
            forbidden_dependency(name) || name == "oer-espressif-staged-runtime"
        })
        .is_empty()
    );
    let mut pending = vec![root.join("chips/espressif/ieee802154-engine/src")];
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
oer-mmio = "1"
[build-dependencies]
oer-riscv = "1"
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
        "disallowed-methods = [{ path = 'oer_mmio::assume_owned', reason = 'ownership' }]",
    )
    .expect("common config");
    let complete: toml::Value =
        toml::from_str("disallowed-methods = ['oer_mmio::assume_owned', 'slice::split_at']")
            .expect("complete config");
    assert!(missing_lints(&complete, &common, "disallowed-methods").is_empty());
    let empty: toml::Value = toml::from_str("").expect("empty config");
    assert_eq!(
        missing_lints(&empty, &common, "disallowed-methods"),
        BTreeSet::from(["oer_mmio::assume_owned".into()])
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
    for owner in ["oer-esp32s31-hal", "oer-esp32s31-knowledge"] {
        let violations = forbidden_paths(&graph, owner, ieee802154_execution);
        assert!(violations.is_empty(), "{owner}: {violations:?}");
    }
}

fn ieee802154_execution(name: &str) -> bool {
    matches!(
        name,
        "oer-espressif-ieee802154-engine" | "oer-esp32s31-ieee802154-driver"
    )
}

#[test]
fn the_ieee802154_boundary_rejects_the_indirect_knowledge_path() {
    let graph = BTreeMap::from([
        ("hal".into(), BTreeSet::from(["knowledge".into()])),
        (
            "knowledge".into(),
            BTreeSet::from(["hal".into(), "oer-espressif-ieee802154-engine".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "hal", ieee802154_execution),
        ["hal -> knowledge -> oer-espressif-ieee802154-engine"],
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
/// as bluetooth/le. Shared radio, network and HIL vocabulary is infrastructure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtocolRole {
    Wire,
    Cipher,
    Port,
    Core,
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
    if ["radio", "network", "hil"].contains(&protocol.split('/').next()?) {
        return None;
    }
    Some(match protocol.rsplit('/').next()? {
        // An event vocabulary and a HIL binding are no protocol layer.
        "instrumentation" | "hil" => return None,
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
    /// An instrumentation event vocabulary: values, no owner.
    ObservationContract,
    /// The HIL framing, envelope and message skeleton.
    HilProtocol,
    /// The device-side HIL library: transport, services, lifecycle.
    HilAgent,
    /// The HIL dictionary beside one facade.
    HilBinding,
    /// A HIL test image.
    HilImage,
    Protocol(ProtocolRole),
    /// `protocols/radio/*` and `protocols/network`: vocabulary shared by the
    /// protocols.
    SharedVocabulary,
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
/// `protocols/hil/` or `firmware/hil/`, or a `hil` leaf, is listed here.
const ROLES: &[(&str, Role)] = &[
    ("chips/espressif/facts", Role::Facts),
    ("chips/espressif/efuse", Role::Facts),
    ("chips/espressif/ieee802154-mac-values", Role::Facts),
    ("chips/esp32s31/knowledge", Role::Facts),
    ("chips/esp32s31/boot", Role::Composition),
    ("chips/esp32s31/pac", Role::Pac),
    ("chips/esp32s31/hal", Role::Hal),
    ("chips/espressif/analog", Role::HardwareDriver),
    ("chips/espressif/boot", Role::HardwareDriver),
    ("chips/espressif/coex", Role::HardwareDriver),
    ("chips/espressif/ieee802154-engine", Role::HardwareDriver),
    ("chips/espressif/modem-clock", Role::HardwareDriver),
    ("chips/espressif/usb-serial-jtag", Role::HardwareDriver),
    ("chips/espressif/watchdog", Role::HardwareDriver),
    ("chips/esp32s31/phy", Role::HardwareDriver),
    ("chips/esp32s31/bluetooth/driver", Role::ChipAdapter),
    ("chips/esp32s31/ieee80211/driver", Role::ChipAdapter),
    ("chips/esp32s31/ieee80211/mac", Role::ChipAdapter),
    ("chips/esp32s31/ieee802154/driver", Role::ChipAdapter),
    // docs/repository-layout.md: "Wi-Fi upper-MAC and the family policy
    // select each attempt's rate and protection" places this family policy
    // above the backend with the cores.
    ("chips/espressif/ieee80211-policy", Role::CoreSidePolicy),
    ("chips/esp32s31/radio/client", Role::ServiceClient),
    ("chips/esp32s31/radio/system", Role::Service),
    ("chips/espressif/interrupt-table", Role::PlatformContract),
    ("chips/espressif/platform-port", Role::PlatformContract),
    ("chips/espressif/staged-layout", Role::PlatformContract),
    ("chips/espressif/executor", Role::Platform),
    ("chips/espressif/staged-bootstrap", Role::Platform),
    ("chips/espressif/staged-runtime", Role::Platform),
    ("chips/esp32s31/board", Role::Platform),
    ("chips/esp32s31/layout", Role::Platform),
    ("chips/esp32s31/compose", Role::Composition),
    ("chips/esp32s31/wifi", Role::Facade),
    ("protocols/hil/protocol", Role::HilProtocol),
    ("protocols/hil/agent", Role::HilAgent),
    ("protocols/ieee80211/control", Role::HilBinding),
    ("protocols/network-control", Role::HilBinding),
    ("protocols/bluetooth/control", Role::HilBinding),
    ("protocols/ieee80211/conformance-control", Role::HilBinding),
    ("chips/esp32s31/phy-probe", Role::HilBinding),
    ("chips/esp32s31/instrumentation-hil", Role::HilBinding),
    ("chips/esp32s31/instrumentation", Role::ObservationContract),
    ("firmware/hil/esp32s31", Role::HilImage),
    ("firmware/hil/esp32s31-lower-mac", Role::HilImage),
    ("firmware/hil/esp32s31-phy", Role::HilImage),
];

/// Roles fixed before their package exists, with what creates it. When the
/// package joins the workspace its entry moves to [`ROLES`].
const AWAITED_ROLES: &[(&str, Role, &str)] = &[];

/// Whether `path` is a package that [`ROLES`] must name.
fn listed_role_required(path: &str) -> bool {
    path.starts_with("chips/")
        || path.starts_with("protocols/hil/")
        || path.starts_with("firmware/hil/")
        || path.rsplit('/').next() == Some("hil")
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
        (Some("protocols"), _) if path.ends_with("/instrumentation") => Role::ObservationContract,
        (Some("protocols"), Some("radio" | "network")) => Role::SharedVocabulary,
        (Some("protocols"), _) => Role::Protocol(protocol_role(path)?),
        (Some("portable"), Some("mmio" | "riscv")) => Role::HardwareAccess,
        (Some("portable"), Some("embassy")) => Role::EmbassyAdapter,
        (Some("portable"), _) => Role::Portable,
        (Some("firmware"), _) => Role::Image,
        (Some("tools"), _) => Role::Tool,
        _ => return None,
    })
}

/// Whether a package of role `from` may take a direct normal dependency on
/// the workspace package at `to_path` of role `to`. Roles outside `chips/`
/// and HIL keep their own rules and are not limited here.
fn role_allows(from: Role, to: Role, to_path: &str) -> bool {
    use ProtocolRole::{Cipher, Core, Port, Runtime, Wire};
    let family_driver = to == Role::HardwareDriver && to_path.starts_with("chips/espressif/");
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
            | Role::Protocol(Wire | Port | Cipher)
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
            Role::Protocol(Wire | Port | Cipher | Core)
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
        Role::HilProtocol => to == Role::Portable,
        Role::HilAgent => matches!(to, Role::HilProtocol | Role::Portable),
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
        // An image composes: it declares its interrupt table, entry, tasks
        // and observation collectors and never bypasses the facade.
        Role::HilImage => matches!(
            to,
            Role::Composition
                | Role::Facade
                | Role::ObservationContract
                | Role::HilBinding
                | Role::HilAgent
                | Role::HilProtocol
                | Role::Platform
                | Role::PlatformContract
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
            | Role::Protocol(ProtocolRole::Model)
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
        ] {
            let path = format!("{prefix}/{directory}");
            assert_eq!(protocol_role(&path), Some(protocol));
            assert_eq!(role(&path), Some(Role::Protocol(protocol)));
        }
        assert_eq!(
            role(&format!("{prefix}/instrumentation")),
            Some(Role::ObservationContract)
        );
        assert_eq!(role(&format!("{prefix}/hil")), None);
    }
    assert_eq!(
        protocol_role("protocols/bluetooth/hci"),
        Some(ProtocolRole::Core)
    );
    for path in [
        "protocols/radio/port",
        "protocols/hil/protocol",
        "protocols/network",
        "portable/wire",
        "chips/esp32s31/hal",
    ] {
        assert_eq!(protocol_role(path), None, "{path}");
    }
    for (path, expected) in [
        ("protocols/radio/port", Some(Role::SharedVocabulary)),
        ("protocols/network", Some(Role::SharedVocabulary)),
        ("protocols/hil/protocol", Some(Role::HilProtocol)),
        ("protocols/hil/agent", Some(Role::HilAgent)),
        ("protocols/hil/new", None),
        ("portable/wire", Some(Role::Portable)),
        ("portable/memory", Some(Role::Portable)),
        ("portable/mmio", Some(Role::HardwareAccess)),
        ("portable/riscv", Some(Role::HardwareAccess)),
        ("portable/embassy/net", Some(Role::EmbassyAdapter)),
        (
            "chips/espressif/ieee80211-policy",
            Some(Role::CoreSidePolicy),
        ),
        ("chips/esp32s31/ieee80211/driver", Some(Role::ChipAdapter)),
        ("chips/esp32s31/radio/system", Some(Role::Service)),
        ("chips/esp32s31/compose", Some(Role::Composition)),
        ("chips/esp32s31/wifi", Some(Role::Facade)),
        // A new chip package has no role until it is listed, whatever its
        // directory says.
        ("chips/esp32s31/new/driver", None),
        ("firmware/hil/esp32s31", Some(Role::HilImage)),
        ("firmware/hil/new", None),
        ("firmware/apps/w1", Some(Role::Image)),
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
        "oer-esp32s31-bluetooth-driver".into(),
        BTreeSet::from([
            "oer-bluetooth-le-ll".into(),
            "oer-bluetooth-hci".into(),
            "oer-bluetooth-le-port".into(),
            "oer-bluetooth-le-wire".into(),
        ]),
    )]);
    let roles = BTreeMap::from([
        (
            "oer-bluetooth-le-ll".into(),
            Role::Protocol(ProtocolRole::Core),
        ),
        (
            "oer-bluetooth-hci".into(),
            Role::Protocol(ProtocolRole::Core),
        ),
        (
            "oer-bluetooth-le-port".into(),
            Role::Protocol(ProtocolRole::Port),
        ),
        (
            "oer-bluetooth-le-wire".into(),
            Role::Protocol(ProtocolRole::Wire),
        ),
    ]);
    assert_eq!(
        backend_core_paths(&graph, &roles, "oer-esp32s31-bluetooth-driver"),
        [
            "oer-esp32s31-bluetooth-driver -> oer-bluetooth-hci",
            "oer-esp32s31-bluetooth-driver -> oer-bluetooth-le-ll",
        ]
    );
}

/// The IEEE 802.11 cores above the lower-MAC port.
const IEEE80211_CORES: [&str; 7] = [
    "oer-ieee80211-upper-mac",
    "oer-ieee80211-rsn",
    "oer-ieee80211-sta",
    "oer-ieee80211-ap",
    "oer-ieee80211-arbiter",
    "oer-ieee80211-esp-now",
    "oer-ieee80211-runtime",
];

fn ieee80211_core(name: &str) -> bool {
    IEEE80211_CORES.contains(&name)
}

/// The lower-MAC port holds only the peripheral boundary, so no normal
/// dependency of it, optional, target-specific or transitive, reaches a core.
#[test]
fn the_ieee80211_port_reaches_no_core() {
    let graph = workspace_dependency_graph();
    for name in IEEE80211_CORES.iter().chain(&["oer-ieee80211-port"]) {
        assert!(graph.contains_key(*name), "{name} is a workspace package");
    }
    let violations = forbidden_paths(&graph, "oer-ieee80211-port", ieee80211_core);
    assert!(violations.is_empty(), "port reaches a core: {violations:?}");
}

#[test]
fn the_ieee80211_port_rule_rejects_direct_and_transitive_core_paths() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
wire = { package = "oer-ieee80211-wire", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
policy = { package = "oer-ieee80211-sta", version = "1", optional = true }
[dev-dependencies]
model = { package = "oer-ieee80211-upper-mac", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let graph = BTreeMap::from([
        (
            "oer-ieee80211-port".into(),
            normal_dependencies(&manifest, &workspace),
        ),
        (
            "oer-ieee80211-wire".into(),
            BTreeSet::from(["helper".into()]),
        ),
        (
            "helper".into(),
            BTreeSet::from(["oer-ieee80211-runtime".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "oer-ieee80211-port", ieee80211_core),
        [
            "oer-ieee80211-port -> oer-ieee80211-sta",
            "oer-ieee80211-port -> oer-ieee80211-wire -> helper -> oer-ieee80211-runtime",
        ]
    );
    let clean = BTreeMap::from([(
        "oer-ieee80211-port".into(),
        BTreeSet::from(["oer-ieee80211-wire".into(), "oer-radio-port".into()]),
    )]);
    assert!(forbidden_paths(&clean, "oer-ieee80211-port", ieee80211_core).is_empty());
}

/// What the IEEE 802.11 wire formats may not reach: time, which belongs to
/// the cores and runtimes, and every IEEE 802.11 crate above the formats.
fn above_ieee80211_wire(name: &str) -> bool {
    name == "oer-time" || (name.starts_with("oer-ieee80211-") && name != "oer-ieee80211-wire")
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
    for name in ["oer-ieee80211-wire", "oer-ieee80211-port", "oer-time"] {
        assert!(graph.contains_key(name), "{name} is a workspace package");
    }
    let violations = forbidden_paths(&graph, "oer-ieee80211-wire", above_ieee80211_wire);
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
format = { package = "oer-wire", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
clock = { package = "oer-time", version = "1", optional = true }
[dev-dependencies]
model = { package = "oer-ieee80211-model", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let graph = BTreeMap::from([
        (
            "oer-ieee80211-wire".into(),
            normal_dependencies(&manifest, &workspace),
        ),
        ("oer-wire".into(), BTreeSet::from(["helper".into()])),
        (
            "helper".into(),
            BTreeSet::from(["oer-ieee80211-port".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "oer-ieee80211-wire", above_ieee80211_wire),
        [
            "oer-ieee80211-wire -> oer-time",
            "oer-ieee80211-wire -> oer-wire -> helper -> oer-ieee80211-port",
        ]
    );
    let clean = BTreeMap::from([(
        "oer-ieee80211-wire".into(),
        BTreeSet::from(["oer-wire".into(), "oer-radio-regulatory".into()]),
    )]);
    assert!(forbidden_paths(&clean, "oer-ieee80211-wire", above_ieee80211_wire).is_empty());
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
/// only the wire formats and portable support; the port takes the wire
/// formats, portable support and the radio vocabulary; the reference model
/// takes the port, the formats and the cipher beside them.
fn ieee80211_layer_allows(package: &str, name: &str, path: &str) -> bool {
    match package {
        "oer-ieee80211-cipher" => name == "oer-ieee80211-wire" || path.starts_with("portable/"),
        "oer-ieee80211-port" => name == "oer-ieee80211-wire" || portable_or_radio(path),
        "oer-ieee80211-model" => {
            [
                "oer-ieee80211-port",
                "oer-ieee80211-wire",
                "oer-ieee80211-cipher",
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
        "oer-ieee80211-cipher",
        "oer-ieee80211-port",
        "oer-ieee80211-model",
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
            "oer-ieee80211-wire".into(),
            "protocols/ieee80211/wire".into(),
        ),
        (
            "oer-ieee80211-port".into(),
            "protocols/ieee80211/port".into(),
        ),
        (
            "oer-ieee80211-cipher".into(),
            "protocols/ieee80211/cipher".into(),
        ),
        ("oer-ieee80211-sta".into(), "protocols/ieee80211/sta".into()),
        ("oer-radio-port".into(), "protocols/radio/port".into()),
        ("oer-wire".into(), "portable/wire".into()),
        ("oer-esp32s31-hal".into(), "chips/esp32s31/hal".into()),
    ]);
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
format = { package = "oer-ieee80211-wire", version = "1" }
port = { package = "oer-ieee80211-port", version = "1" }
support = { package = "oer-wire", version = "1" }
aes = "0.8"
[target.'cfg(target_arch = "riscv32")'.dependencies]
hal = { package = "oer-esp32s31-hal", version = "1", optional = true }
[dev-dependencies]
station = { package = "oer-ieee80211-sta", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let dependencies = normal_dependencies(&manifest, &workspace);
    assert_eq!(
        disallowed_workspace_dependencies(&dependencies, &paths, |name, path| {
            ieee80211_layer_allows("oer-ieee80211-cipher", name, path)
        }),
        [
            "oer-esp32s31-hal (chips/esp32s31/hal)",
            "oer-ieee80211-port (protocols/ieee80211/port)",
        ]
    );
    let model = BTreeSet::from([
        "oer-ieee80211-port".into(),
        "oer-ieee80211-cipher".into(),
        "oer-radio-port".into(),
        "oer-ieee80211-sta".into(),
    ]);
    assert_eq!(
        disallowed_workspace_dependencies(&model, &paths, |name, path| {
            ieee80211_layer_allows("oer-ieee80211-model", name, path)
        }),
        ["oer-ieee80211-sta (protocols/ieee80211/sta)"]
    );
}

/// What the Wi-Fi runtime reaches by no normal path: chip crates (their
/// HALs, drivers and executor binding among them), an executor and Embassy
/// time. It binds no executor and takes time from `oer-time`.
fn below_the_ieee80211_runtime(paths: &BTreeMap<String, String>, name: &str) -> bool {
    paths
        .get(name)
        .is_some_and(|path| path.starts_with("chips/"))
        || ["embassy-time", "embassy-executor"].contains(&name)
}

/// The air formats and frame protection the runtime never names: the cores
/// parse, build and protect frames and hand it finished actions.
const BESIDE_THE_IEEE80211_RUNTIME: [&str; 2] = ["oer-ieee80211-wire", "oer-ieee80211-cipher"];

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
        .get("oer-ieee80211-runtime")
        .expect("the runtime is a workspace package");
    assert!(
        runtime_direct_violations(direct).is_empty(),
        "the runtime depends on formats or the cipher: {:?}",
        runtime_direct_violations(direct)
    );
    let violations = forbidden_paths(&graph, "oer-ieee80211-runtime", |name| {
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
        "oer-espressif-executor".into(),
        "chips/espressif/executor".into(),
    )]);
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
format = { package = "oer-ieee80211-wire", version = "1" }
station = { package = "oer-ieee80211-sta", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
clock = { package = "embassy-time", version = "0.5", optional = true }
[dev-dependencies]
protect = { package = "oer-ieee80211-cipher", version = "1" }
policy = { package = "oer-espressif-ieee80211-policy", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let direct = normal_dependencies(&manifest, &workspace);
    assert_eq!(runtime_direct_violations(&direct), ["oer-ieee80211-wire"]);
    let graph = BTreeMap::from([
        ("oer-ieee80211-runtime".into(), direct),
        (
            "oer-ieee80211-sta".into(),
            BTreeSet::from(["helper".into()]),
        ),
        (
            "helper".into(),
            BTreeSet::from(["oer-espressif-executor".into()]),
        ),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "oer-ieee80211-runtime", |name| {
            below_the_ieee80211_runtime(&paths, name)
        }),
        [
            "oer-ieee80211-runtime -> embassy-time",
            "oer-ieee80211-runtime -> oer-ieee80211-sta -> helper -> oer-espressif-executor",
        ]
    );
}

/// The sans-IO IEEE 802.11 cores: they take inputs and return actions as
/// values and never wait.
const IEEE80211_SANS_IO_CORES: [&str; 6] = [
    "protocols/ieee80211/upper-mac",
    "protocols/ieee80211/rsn",
    "protocols/ieee80211/sta",
    "protocols/ieee80211/ap",
    "protocols/ieee80211/arbiter",
    "protocols/ieee80211/esp-now",
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

/// Whether an identifier marks waiting or sequencing for a scan.
type Marker = dyn Fn(&str) -> bool;

/// Production tokens of one module body: items configured for tests are
/// skipped; out-of-line production modules are followed from `directory`.
/// A `#[path]` at the top of a file is relative to the file's own directory,
/// inside an inline module to that module's directory. Each identifier that
/// `marker` accepts is reported with its file.
fn production_markers(
    tokens: TokenStream,
    directory: &Path,
    file: &Path,
    top_level: bool,
    marker: &Marker,
    found: &mut Vec<String>,
) {
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
                        production_markers(
                            inline.stream(),
                            &directory.join(name.to_string()),
                            file,
                            false,
                            marker,
                            found,
                        );
                    }
                    _ => {
                        let child = match path {
                            Some(path) if top_level => {
                                file.parent().unwrap_or(directory).join(path)
                            }
                            Some(path) => directory.join(path),
                            None if directory.join(format!("{name}.rs")).is_file() => {
                                directory.join(format!("{name}.rs"))
                            }
                            None => directory.join(name.to_string()).join("mod.rs"),
                        };
                        production_file_markers(&child, marker, found);
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
        attributes.clear();
        match token {
            TokenTree::Ident(word) if marker(&word.to_string()) => {
                found.push(format!("{}: {word}", file.display()));
            }
            // Item bodies (`impl`, `fn`, blocks) skip their test-only items too.
            TokenTree::Group(group) => {
                production_markers(group.stream(), directory, file, false, marker, found);
            }
            _ => {}
        }
    }
}

/// A module file: `lib.rs` and `mod.rs` own their directory; another file
/// `name.rs` owns `name/`.
fn production_file_markers(file: &Path, marker: &Marker, found: &mut Vec<String>) {
    let tokens = std::fs::read_to_string(file)
        .ok()
        .and_then(|source| source.parse::<TokenStream>().ok());
    let (Some(tokens), Some(stem), Some(parent)) = (tokens, file.file_stem(), file.parent()) else {
        // A module the scan cannot read is reported, never skipped.
        found.push(format!("{}: unreadable module", file.display()));
        return;
    };
    let stem = stem.to_string_lossy();
    let directory = if ["lib", "mod", "main"].contains(&stem.as_ref()) {
        parent.to_path_buf()
    } else {
        parent.join(stem.as_ref())
    };
    production_markers(tokens, &directory, file, true, marker, found);
}

fn waiting(word: &str) -> bool {
    WAITING.contains(&word)
}

#[test]
fn ieee80211_cores_never_wait_outside_their_tests() {
    let root = root();
    let packages = packages(&root);
    let mut found = Vec::new();
    for core in IEEE80211_SANS_IO_CORES {
        let manifest = packages.get(core).expect("the core is a workspace member");
        for crate_root in crate_roots(&root.join(core), manifest) {
            production_file_markers(&crate_root, &waiting, &mut found);
        }
    }
    assert!(found.is_empty(), "a core waits: {found:?}");
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
            let expected = expected_name(&member.path, member.executable);
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

fn member(path: &str, name: &str, executable: bool, dependencies: &[&str]) -> Member {
    Member {
        path: path.to_owned(),
        name: name.to_owned(),
        executable,
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
        "embassy-time" => path == "portable/embassy/time" || path == "firmware/hil/esp32s31",
        "embassy-time-driver" => path == "chips/espressif/executor",
        "embassy-net" | "xarxa" | "xarxa-driver" => {
            path == "portable/embassy/net" || path.starts_with("firmware/")
        }
        "trouble-host" => path == "portable/embassy/trouble" || path.starts_with("firmware/"),
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
const EXTERNAL_ASYNC_CRATES: &[Deviation] = &[
    (
        "protocols/hil/agent",
        "embassy-sync",
        "console on owned queues and waiter slots (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "embassy-sync",
        "HIL image on SPSC and OnceValue (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "embassy-sync",
        "HIL image on SPSC and OnceValue (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "embassy-sync",
        "HIL image on SPSC and OnceValue (#848)",
    ),
];

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
stack = { package = "embassy-net", version = "1" }
cooperate = { workspace = true, optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
signal = { package = "embassy-sync", version = "1", optional = true }
host = { package = "trouble-host", version = "1", optional = true }
clock = { package = "embassy-time", version = "1" }
driver = { package = "xarxa-driver", version = "1" }
ip = { package = "xarxa", version = "1" }
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
        ..member(path, "oer-ieee80211-runtime", false, &[])
    };
    assert_eq!(
        external_crate_violations(&[runtime]).expect("runtime role"),
        [
            "bt-hci",
            "embassy-net",
            "embassy-sync",
            "embassy-time",
            "embassy-time-driver",
            "trouble-host",
            "xarxa",
            "xarxa-driver",
        ]
        .map(|dependency| (path.to_owned(), dependency.to_owned()))
        .into()
    );
    let core = Member {
        dependencies,
        ..member("protocols/ieee80211/sta", "oer-ieee80211-sta", false, &[])
    };
    assert!(
        external_crate_violations(&[core])
            .expect("core role")
            .contains(&("protocols/ieee80211/sta".into(), "embassy-futures".into(),))
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

#[test]
fn external_crate_homes_and_futures_exclusions_are_explicit() {
    for (dependency, allowed) in [
        (
            "embassy-time",
            vec!["portable/embassy/time", "firmware/hil/esp32s31"],
        ),
        ("embassy-time-driver", vec!["chips/espressif/executor"]),
        (
            "embassy-net",
            vec!["portable/embassy/net", "firmware/apps/w3"],
        ),
        (
            "xarxa",
            vec!["portable/embassy/net", "firmware/hil/esp32s31"],
        ),
        (
            "xarxa-driver",
            vec!["portable/embassy/net", "firmware/apps/w3"],
        ),
        (
            "trouble-host",
            vec!["portable/embassy/trouble", "firmware/apps/b0"],
        ),
        (
            "bt-hci",
            vec![
                "protocols/bluetooth/hci",
                "portable/embassy/trouble",
                "tools/hil/fixture",
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
        ("protocols/ieee80211/cipher", false),
        ("protocols/bluetooth/le/controller", false),
        ("chips/espressif/ieee80211-policy", false),
        ("chips/espressif/facts", false),
        ("chips/esp32s31/pac", false),
        ("chips/esp32s31/hal", false),
        ("protocols/bluetooth/le/runtime", true),
        ("protocols/bluetooth/le/port", true),
        ("protocols/bluetooth/le/model", true),
        ("portable/cooperate", true),
        ("portable/embassy/net", true),
        ("chips/esp32s31/bluetooth/driver", true),
        ("firmware/apps/b0", true),
        ("tools/hil/fixture", true),
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
fn bt_hci_controller(tokens: TokenStream, in_bt_hci: bool) -> bool {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    (in_bt_hci
        && tokens.iter().any(|token| matches!(token, TokenTree::Ident(name) if name == "controller")))
        || tokens.iter().any(|token| {
            matches!(token, TokenTree::Group(group) if bt_hci_controller(group.stream(), false))
        })
        || tokens.windows(4).any(|path| match path {
            [TokenTree::Ident(name), TokenTree::Punct(first), TokenTree::Punct(second), next]
                if name == "bt_hci" && first.as_char() == ':' && second.as_char() == ':' => {
                match next {
                    TokenTree::Ident(name) => name == "controller",
                    TokenTree::Group(group) => bt_hci_controller(group.stream(), true),
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
            if bt_hci_controller(tokens, false) {
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
            bt_hci_controller(source.parse().unwrap(), false),
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
            !bt_hci_controller(source.parse().unwrap(), false),
            "{source}"
        );
    }
}

#[test]
fn the_name_rule_tells_libraries_from_programs_by_their_binary_targets() {
    let members = [
        member("protocols/ieee80211/wire", "oer-ieee80211-wire", false, &[]),
        member("protocols/ieee80211/sta", "oer-sta", false, &[]),
        member("firmware/apps/w1", "oer-firmware-apps-w1", true, &[]),
        member("firmware/apps/w2", "oer-apps-w2", true, &[]),
        member("tools/image/build", "oer-image-build", false, &[]),
        member("tools/image/cli", "oer-tool-image-cli", true, &[]),
        member("tools/image/bundle", "oer-tool-image-bundle", false, &[]),
        member("tools/pins", "oer-pins", true, &[]),
    ];
    assert_eq!(
        name_violations(&members),
        [
            "protocols/ieee80211/sta: named oer-sta, expected oer-ieee80211-sta",
            "firmware/apps/w2: named oer-apps-w2, expected oer-firmware-apps-w2",
            "tools/image/bundle: named oer-tool-image-bundle, expected oer-image-bundle",
            "tools/pins: named oer-pins, expected oer-tool-pins",
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
    to.path.starts_with("firmware/") || (to.path.starts_with("tools/") && to.executable)
}

fn family_reaches_a_chip(from: &Member, to: &Member) -> bool {
    from.path.starts_with("chips/espressif/")
        && to.path.starts_with("chips/")
        && !to.path.starts_with("chips/espressif/")
}

fn portable_reaches_a_protocol_or_a_chip(from: &Member, to: &Member) -> bool {
    let embassy = from.path.starts_with("portable/embassy/");
    from.path.starts_with("portable/")
        && ((!embassy && to.path.starts_with("protocols/")) || to.path.starts_with("chips/"))
}

/// Nothing depends on an image or on a program of `tools/` (a package with a
/// binary target) through a normal dependency.
const NO_EDGE_ON_AN_IMAGE_OR_PROGRAM: &[Deviation] = &[(
    "firmware/hil/esp32s31",
    "firmware/apps/w3",
    "shared network observation/ICMP helpers move to the IP adapter (N-d)",
)];

/// `chips/espressif` depends on no chip, through a normal or a build
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
        member("chips/espressif/analog", "oer-espressif-analog", false, &[]),
        Member {
            dependencies: manifest(
                r#"
[dependencies]
hal = { package = "oer-esp32s31-hal", version = "1", optional = true }
[target.'cfg(target_arch = "riscv32")'.dependencies]
app = { package = "oer-firmware-apps-w3", version = "1" }
[dev-dependencies]
tool = { package = "oer-tool-pins", version = "1" }
"#,
            ),
            ..member(
                "chips/espressif/executor",
                "oer-espressif-executor",
                false,
                &[],
            )
        },
        member("chips/esp32s31/hal", "oer-esp32s31-hal", false, &[]),
        member("firmware/apps/w3", "oer-firmware-apps-w3", true, &[]),
        member("tools/pins", "oer-tool-pins", true, &[]),
        member("tools/image/build", "oer-image-build", false, &[]),
        member(
            "tools/image/checks",
            "oer-image-checks",
            false,
            &["oer-tool-pins", "oer-image-build"],
        ),
        member(
            "portable/time",
            "oer-time",
            false,
            &["oer-ieee80211-wire", "oer-esp32s31-hal"],
        ),
        member(
            "portable/embassy/net",
            "oer-embassy-net",
            false,
            &["oer-ieee80211-wire", "oer-esp32s31-hal"],
        ),
        member("protocols/ieee80211/wire", "oer-ieee80211-wire", false, &[]),
    ];
    let edge = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    assert_eq!(
        forbidden_edges(&members, image_or_program),
        BTreeSet::from([
            edge("chips/espressif/executor", "firmware/apps/w3"),
            edge("tools/image/checks", "tools/pins"),
        ])
    );
    assert_eq!(
        forbidden_edges(&members, family_reaches_a_chip),
        BTreeSet::from([edge("chips/espressif/executor", "chips/esp32s31/hal")])
    );
    assert_eq!(
        forbidden_edges(&members, portable_reaches_a_protocol_or_a_chip),
        BTreeSet::from([
            edge("portable/time", "protocols/ieee80211/wire"),
            edge("portable/time", "chips/esp32s31/hal"),
            edge("portable/embassy/net", "chips/esp32s31/hal"),
        ])
    );
}

#[test]
fn the_family_rule_sees_build_dependencies_and_ignores_dev_dependencies() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
port = { package = "oer-espressif-platform-port", version = "1" }
[build-dependencies]
facts = { package = "oer-espressif-facts", version = "1" }
[target.'cfg(unix)'.build-dependencies]
layout = { package = "oer-esp32s31-layout", version = "1", optional = true }
[dev-dependencies]
hal = { package = "oer-esp32s31-hal", version = "1" }
"#,
    )
    .expect("manifest");
    let workspace: toml::Value = toml::from_str("[dependencies]").expect("workspace");
    let members = vec![
        Member {
            dependencies: normal_dependencies(&manifest, &workspace),
            build_dependencies: scoped_dependencies(&manifest, &workspace, "build-dependencies"),
            ..member(
                "chips/espressif/staged-runtime",
                "oer-espressif-staged-runtime",
                false,
                &[],
            )
        },
        member(
            "chips/espressif/platform-port",
            "oer-espressif-platform-port",
            false,
            &[],
        ),
        member("chips/espressif/facts", "oer-espressif-facts", false, &[]),
        member("chips/esp32s31/layout", "oer-esp32s31-layout", false, &[]),
        member("chips/esp32s31/hal", "oer-esp32s31-hal", false, &[]),
    ];
    assert!(forbidden_edges(&members, family_reaches_a_chip).is_empty());
    assert_eq!(
        forbidden_edges(&with_build_edges(members), family_reaches_a_chip),
        BTreeSet::from([(
            "chips/espressif/staged-runtime".to_owned(),
            "chips/esp32s31/layout".to_owned()
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
        "chips/esp32s31/ieee80211/driver",
        "chips/espressif/staged-runtime",
        "driver trace through the observer contract (#848)",
    ),
    (
        "chips/esp32s31/ieee802154/driver",
        "portable/mmio",
        "IEEE 802.15.4 lifecycle out of the HAL (#848)",
    ),
    (
        "chips/esp32s31/pac",
        "chips/espressif/analog",
        "analog-I2C model types out of the family driver (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "chips/esp32s31/hal",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "chips/esp32s31/ieee80211/driver",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "chips/esp32s31/phy",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "chips/esp32s31/radio/client",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "chips/espressif/usb-serial-jtag",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "firmware/apps/w3",
        "shared network observation/ICMP helpers move to the IP adapter (N-d)",
    ),
    (
        "firmware/hil/esp32s31",
        "portable/riscv",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/arbiter",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/cipher",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/port",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/rsn",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/runtime",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/sta",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/upper-mac",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/ieee80211/wire",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31",
        "protocols/radio/port",
        "the Wi-Fi station image on the facade and compose (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/hal",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/ieee80211/driver",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/ieee80211/mac",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/phy",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/radio/client",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/esp32s31/radio/system",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/espressif/coex",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/espressif/ieee80211-policy",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "chips/espressif/usb-serial-jtag",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/ieee80211/port",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/ieee80211/runtime",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/ieee80211/upper-mac",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/ieee80211/wire",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/radio/port",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-lower-mac",
        "protocols/radio/regulatory",
        "the lower-MAC conformance image (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/hal",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/ieee80211/driver",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/ieee80211/mac",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/ieee802154/driver",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/knowledge",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/esp32s31/phy",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/espressif/ieee802154-engine",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/espressif/ieee802154-mac-values",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "chips/espressif/usb-serial-jtag",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "portable/riscv",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "protocols/ieee80211/wire",
        "the PHY capture probe and instrumentation (#848)",
    ),
    (
        "firmware/hil/esp32s31-phy",
        "protocols/radio/regulatory",
        "the PHY capture probe and instrumentation (#848)",
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
            (Role::Facts, "chips/espressif/facts", true),
            (Role::SharedVocabulary, "protocols/radio/regulatory", true),
            (Role::Portable, "portable/memory", true),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::HardwareDriver, "chips/espressif/analog", false),
            (PROTOCOL_PORT, "protocols/ieee802154/port", false),
        ],
    );
    assert_role_cases(
        Role::Pac,
        &[
            (Role::HardwareAccess, "portable/mmio", true),
            (Role::Facts, "chips/esp32s31/knowledge", true),
            (Role::HardwareDriver, "chips/espressif/analog", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::PlatformContract,
        &[
            (Role::HardwareAccess, "portable/riscv", true),
            (
                Role::PlatformContract,
                "chips/espressif/staged-layout",
                true,
            ),
            (Role::Platform, "chips/espressif/staged-runtime", false),
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
            (
                Role::PlatformContract,
                "chips/espressif/platform-port",
                true,
            ),
            (Role::HardwareDriver, "chips/espressif/boot", true),
            (Role::HardwareDriver, "chips/esp32s31/phy", false),
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
            (Role::Service, "chips/esp32s31/radio/system", false),
            (Role::Platform, "chips/espressif/staged-runtime", false),
            (Role::SharedVocabulary, "protocols/radio/coex", false),
            (PROTOCOL_PORT, "protocols/ieee80211/port", false),
        ],
    );
}

#[test]
fn drivers_and_adapters_stand_on_the_hal_and_the_port_never_above_it() {
    let below = [
        (Role::Hal, "chips/esp32s31/hal", true),
        (Role::HardwareDriver, "chips/espressif/analog", true),
        (
            Role::PlatformContract,
            "chips/espressif/interrupt-table",
            true,
        ),
        (
            Role::ObservationContract,
            "protocols/ieee80211/instrumentation",
            true,
        ),
        (PROTOCOL_PORT, "protocols/ieee80211/port", true),
        (PROTOCOL_WIRE, "protocols/ieee80211/wire", true),
        (PROTOCOL_CORE, "protocols/ieee80211/sta", false),
        (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
        (
            Role::CoreSidePolicy,
            "chips/espressif/ieee80211-policy",
            false,
        ),
        (Role::Pac, "chips/esp32s31/pac", false),
        (Role::HardwareAccess, "portable/mmio", false),
        (Role::Platform, "chips/espressif/staged-runtime", false),
        (Role::Service, "chips/esp32s31/radio/system", false),
        (Role::Composition, "chips/esp32s31/compose", false),
        (Role::Facade, "chips/esp32s31/wifi", false),
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
        "chips/esp32s31/ieee80211/mac"
    ));
    assert!(!role_allows(
        Role::Service,
        Role::ChipAdapter,
        "chips/esp32s31/ieee80211/driver"
    ));
}

#[test]
fn core_side_policy_stays_with_the_cores_and_the_platform_reaches_no_chip_hal() {
    assert_role_cases(
        Role::CoreSidePolicy,
        &[
            (PROTOCOL_CORE, "protocols/ieee80211/upper-mac", true),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
            (Role::Hal, "chips/esp32s31/hal", false),
        ],
    );
    assert_role_cases(
        Role::Platform,
        &[
            (
                Role::PlatformContract,
                "chips/espressif/platform-port",
                true,
            ),
            (Role::HardwareDriver, "chips/espressif/boot", true),
            (Role::HardwareDriver, "chips/esp32s31/phy", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
            (PROTOCOL_PORT, "protocols/ieee80211/port", false),
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
                (PROTOCOL_CORE, "protocols/ieee80211/sta", true),
                (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", true),
                (Role::Service, "chips/esp32s31/radio/system", true),
                (Role::Platform, "chips/espressif/executor", true),
                (Role::Pac, "chips/esp32s31/pac", false),
                (Role::HilAgent, "protocols/hil/agent", false),
                (Role::HilImage, "firmware/hil/esp32s31", false),
                (Role::Image, "firmware/apps/w3", false),
            ],
        );
        assert!(!keeps_the_backend_rule(from), "{from:?}");
    }
    assert!(role_allows(
        Role::Facade,
        Role::Composition,
        "chips/esp32s31/compose"
    ));
    assert!(!role_allows(
        Role::Composition,
        Role::Facade,
        "chips/esp32s31/wifi"
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
            (PROTOCOL_CORE, "protocols/ieee80211/sta", false),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
            (Role::Platform, "chips/espressif/staged-runtime", false),
        ],
    );
}

#[test]
fn hil_roles_keep_radio_out_of_the_framework_and_images_behind_the_facade() {
    assert_role_cases(
        Role::HilProtocol,
        &[
            (Role::Portable, "portable/memory", true),
            (PROTOCOL_PORT, "protocols/ieee80211/port", false),
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
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
            (Role::HardwareAccess, "portable/mmio", false),
        ],
    );
    assert_role_cases(
        Role::HilBinding,
        &[
            (Role::HilAgent, "protocols/hil/agent", true),
            (Role::Facade, "chips/esp32s31/wifi", true),
            (PROTOCOL_RUNTIME, "protocols/ieee80211/runtime", true),
            (Role::Portable, "portable/memory", true),
            (Role::Portable, "portable/time", false),
            (Role::EmbassyAdapter, "portable/embassy/time", false),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
        ],
    );
    assert_role_cases(
        Role::HilImage,
        &[
            (Role::Composition, "chips/esp32s31/compose", true),
            (Role::Facade, "chips/esp32s31/wifi", true),
            (Role::HilBinding, "protocols/ieee80211/hil", true),
            (Role::Platform, "chips/espressif/executor", true),
            (Role::EmbassyAdapter, "portable/embassy/net", true),
            (
                Role::ObservationContract,
                "protocols/ieee80211/instrumentation",
                true,
            ),
            (Role::Hal, "chips/esp32s31/hal", false),
            (Role::Pac, "chips/esp32s31/pac", false),
            (Role::HardwareDriver, "chips/esp32s31/phy", false),
            (Role::ChipAdapter, "chips/esp32s31/ieee80211/driver", false),
            (Role::Service, "chips/esp32s31/radio/system", false),
            (PROTOCOL_CORE, "protocols/ieee80211/sta", false),
            (PROTOCOL_PORT, "protocols/ieee80211/port", false),
            (PROTOCOL_WIRE, "protocols/ieee80211/wire", false),
            (Role::Image, "firmware/apps/w3", false),
        ],
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
pac = { package = "oer-esp32s31-pac", version = "1" }
[target.'cfg(target_arch = "riscv32")'.dependencies]
phy = { package = "oer-esp32s31-phy", version = "1", optional = true }
[dev-dependencies]
adapter = { package = "oer-esp32s31-ieee80211-driver", version = "1" }
"#,
            ),
            ..member("chips/esp32s31/hal", "oer-esp32s31-hal", false, &[])
        },
        member("chips/esp32s31/pac", "oer-esp32s31-pac", false, &[]),
        member("chips/esp32s31/phy", "oer-esp32s31-phy", false, &[]),
        member(
            "chips/esp32s31/ieee80211/driver",
            "oer-esp32s31-ieee80211-driver",
            false,
            &["oer-ieee80211-runtime", "oer-esp32s31-phy"],
        ),
        member(
            "protocols/ieee80211/runtime",
            "oer-ieee80211-runtime",
            false,
            &[],
        ),
        member(
            "chips/esp32s31/wifi",
            "oer-esp32s31-wifi",
            false,
            &["oer-ieee80211-runtime", "oer-esp32s31-ieee80211-driver"],
        ),
        member(
            "firmware/hil/esp32s31",
            "oer-firmware-hil-esp32s31",
            true,
            &["oer-esp32s31-wifi", "oer-esp32s31-hal"],
        ),
        // A package without a listed role is the completeness rule's failure,
        // not an allowed edge.
        member(
            "chips/esp32s31/unlisted",
            "oer-esp32s31-unlisted",
            false,
            &["oer-ieee80211-runtime"],
        ),
    ];
    let edge = |from: &str, to: &str| (from.to_owned(), to.to_owned());
    assert_eq!(
        forbidden_edges(&members, outside_its_role),
        BTreeSet::from([
            edge("chips/esp32s31/hal", "chips/esp32s31/phy"),
            edge(
                "chips/esp32s31/ieee80211/driver",
                "protocols/ieee80211/runtime"
            ),
            edge("firmware/hil/esp32s31", "chips/esp32s31/hal"),
        ])
    );
}

/// The host HIL families, which link no HAL and no
/// PAC (`docs/repository-layout.md`, HIL).
fn host_family(path: &str) -> bool {
    path.starts_with("tools/hil/family-")
}

fn hal_or_pac(roles: &BTreeMap<String, Role>, name: &str) -> bool {
    matches!(roles.get(name), Some(Role::Hal | Role::Pac))
}

#[test]
fn host_families_link_no_hal_or_pac() {
    let graph = workspace_dependency_graph();
    let roles = workspace_roles();
    let families = workspace_package_paths()
        .into_iter()
        .filter(|(_, path)| host_family(path))
        .collect::<Vec<_>>();
    assert!(!families.is_empty(), "the host families are read");
    let violations = families
        .iter()
        .flat_map(|(name, _)| forbidden_paths(&graph, name, |name| hal_or_pac(&roles, name)))
        .collect::<Vec<_>>();
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn the_host_family_rule_finds_a_transitive_hal() {
    let roles = BTreeMap::from([
        ("oer-esp32s31-hal".into(), Role::Hal),
        ("oer-esp32s31-knowledge".into(), Role::Facts),
    ]);
    let graph = BTreeMap::from([
        (
            "family".into(),
            BTreeSet::from(["helper".into(), "oer-esp32s31-knowledge".into()]),
        ),
        ("helper".into(), BTreeSet::from(["oer-esp32s31-hal".into()])),
    ]);
    assert_eq!(
        forbidden_paths(&graph, "family", |name| hal_or_pac(&roles, name)),
        ["family -> helper -> oer-esp32s31-hal"]
    );
    assert!(host_family("tools/hil/family-wifi"));
    assert!(host_family("tools/hil/family-phy-esp32s31"));
    assert!(!host_family("tools/hil/runner"));
}

/// A source file outside `firmware/` that includes an image source through
/// `#[path]`: the including file and the included file, below the root.
type Inclusion = (String, String);

/// Each `#[path = "..."]` in the tokens of `file` that resolves into
/// `firmware/` below `root`, at any nesting depth.
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
            if let (Ok(including), Ok(included)) = (
                file.strip_prefix(root),
                resolved.strip_prefix(root.join("firmware")),
            ) {
                found.insert((
                    including.display().to_string(),
                    Path::new("firmware").join(included).display().to_string(),
                ));
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
        if !source.contains("firmware") {
            continue;
        }
        let tokens = source
            .parse::<TokenStream>()
            .map_err(|error| format!("{} does not tokenize: {error}", path.display()))?;
        firmware_paths(tokens, root, &path, found);
    }
    Ok(())
}

/// Image sources that a package outside `firmware/` includes through
/// `#[path]`, with what removes each.
const FIRMWARE_SOURCES_INCLUDED: &[Deviation] = &[(
    "tools/image/cli/src/main.rs",
    "firmware/apps/w1/src/own/crc32.rs",
    "the W1 image removal (#848)",
)];

#[test]
fn no_package_outside_firmware_includes_an_image_source() {
    let root = root();
    let mut found = BTreeSet::new();
    for directory in ["protocols", "portable", "chips", "tools"] {
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
#[path = "../../../firmware/hil/image/src/cache.rs"]
mod cache;
mod nested {
    #[path = "../../../firmware/apps/w1/src/crc.rs"]
    mod crc;
}
#[path = "../src/helper.rs"]
mod helper;
// #[path = "../../../firmware/apps/w1/src/comment.rs"]
const TEXT: &str = "#[path = \"../../../firmware/apps/w1/src/string.rs\"]";
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
            inclusion("firmware/apps/w1/src/crc.rs"),
            inclusion("firmware/hil/image/src/cache.rs"),
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
    ("chips/esp32s31/hal/src/bluetooth.rs", 10),
    ("chips/esp32s31/hal/src/bluetooth/engine/lease.rs", 4),
    ("chips/esp32s31/hal/src/bluetooth/engine/live.rs", 27),
    ("chips/esp32s31/hal/src/bluetooth/engine/owner.rs", 6),
    (
        "chips/esp32s31/hal/src/bluetooth/engine/scheduler/hardware.rs",
        13,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/engine/scheduler/hardware/live.rs",
        17,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/engine/scheduler/interrupt.rs",
        45,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/engine/scheduler/timing.rs",
        1,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/registers/modem_timer.rs",
        10,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/scheduler_execution_lock.rs",
        11,
    ),
    (
        "chips/esp32s31/hal/src/bluetooth/scheduler_execution_modify.rs",
        26,
    ),
    ("chips/esp32s31/hal/src/bluetooth/scheduler_stop.rs", 12),
    ("chips/esp32s31/hal/src/boot.rs", 16),
    ("chips/esp32s31/hal/src/ieee80211/init.rs", 5),
    ("chips/esp32s31/hal/src/ieee80211/init/cold/handshake.rs", 8),
    ("chips/esp32s31/hal/src/peripherals/entropy.rs", 2),
    ("chips/esp32s31/hal/src/phy/delay.rs", 1),
    ("chips/esp32s31/hal/src/phy/i2c.rs", 5),
];

/// The change that moves the driver code out of each HAL module, by path
/// prefix below `chips/esp32s31/hal/src/`.
const HAL_SEQUENCE_REMOVAL: &[(&str, &str)] = &[
    (
        "bluetooth",
        "Bluetooth controller driver out of the HAL (#848)",
    ),
    (
        "boot",
        "boot block drivers and the cold-start sequence (#848)",
    ),
    (
        "ieee80211",
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
        "chips/esp32s31/hal/src/cores.rs",
        4,
        "a terminal halt after the fatal path requested reset",
    ),
    (
        "chips/esp32s31/hal/src/peripherals/watchdog.rs",
        9,
        "the family watchdog driver's deadline types over the HAL's register port",
    ),
    (
        "chips/esp32s31/hal/src/phy/prelude.rs",
        2,
        "the SDM deadline counter register, sampled",
    ),
    (
        "chips/esp32s31/hal/src/phy/registers.rs",
        1,
        "the SDM deadline counter register, sampled",
    ),
    (
        "chips/esp32s31/hal/src/phy/sequence.rs",
        1,
        "the SDM deadline counter register, sampled",
    ),
];

/// Marker counts of the production files reachable from `crate_root`, by
/// path below `root`.
fn marker_counts(root: &Path, crate_root: &Path, marker: &Marker) -> BTreeMap<String, usize> {
    let mut found = Vec::new();
    production_file_markers(crate_root, marker, &mut found);
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

fn w3_bypasses_the_facade(from: &Member, to: &Member) -> bool {
    from.path == "firmware/apps/w3"
        && !matches!(
            role(&to.path),
            Some(
                Role::Facade
                    | Role::Composition
                    | Role::Platform
                    | Role::PlatformContract
                    | Role::Portable
                    | Role::EmbassyAdapter
                    | Role::SharedVocabulary
                    | Role::ObservationContract
            )
        )
}
// Typed interrupt/root ownership, the SAR handler, and the explicitly image-selected
// unwrap engine are the only remaining implementation edges. No port or runtime.
const W3_IMPLEMENTATION_EDGES: &[Deviation] = &[
    (
        "firmware/apps/w3",
        "chips/espressif/usb-serial-jtag",
        "application console endpoint; shared image setup moves to compose (#848)",
    ),
    (
        "firmware/apps/w3",
        "chips/esp32s31/hal",
        "image root and interrupt owners; common image setup moves to compose (#848)",
    ),
    (
        "firmware/apps/w3",
        "chips/esp32s31/phy",
        "SAR/PWDET interrupt binding moves to compose (#848)",
    ),
    (
        "firmware/apps/w3",
        "protocols/ieee80211/cipher",
        "key-unwrap engine remains selected by the image (#844)",
    ),
];
#[test]
fn w3_reaches_the_station_only_through_its_facade() {
    let found = forbidden_edges(&workspace_members(), w3_bypasses_the_facade);
    let differences = deviation_differences(&found, W3_IMPLEMENTATION_EDGES);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
#[test]
fn the_thin_image_rule_accepts_facades_and_rejects_backend_runtime_and_port_bypasses() {
    let image = member("firmware/apps/w3", "oer-firmware-apps-w3", true, &[]);
    for path in [
        "chips/esp32s31/wifi",
        "chips/esp32s31/compose",
        "portable/embassy/net",
        "protocols/network",
    ] {
        assert!(
            !w3_bypasses_the_facade(&image, &member(path, "fixture", false, &[])),
            "{path}"
        );
    }
    for path in [
        "chips/esp32s31/ieee80211/driver",
        "chips/esp32s31/ieee80211/mac",
        "protocols/ieee80211/runtime",
        "protocols/ieee80211/port",
        "protocols/ieee80211/sta",
    ] {
        assert!(
            w3_bypasses_the_facade(&image, &member(path, "fixture", false, &[])),
            "{path}"
        );
    }
    for backend in [Role::HardwareDriver, Role::ChipAdapter] {
        assert!(!role_allows(
            backend,
            PROTOCOL_RUNTIME,
            "protocols/ieee80211/runtime"
        ));
        assert!(!role_allows(backend, Role::Facade, "chips/esp32s31/wifi"));
    }
}
