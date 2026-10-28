//! Structural rules read from package manifests: names, the unsafe boundary
//! and protocol core/runtime isolation from backends.

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
    let targets = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values());
    core::iter::once(manifest)
        .chain(targets)
        .filter_map(|scope| scope.get("dependencies").and_then(toml::Value::as_table))
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

/// The shared RF owner consumes client contracts, never protocol drivers,
/// including optional, target-specific and transitive normal dependencies.
#[test]
fn radio_systems_do_not_reach_protocol_drivers() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let packages = packages(&root);
    let drivers: BTreeSet<_> = packages
        .iter()
        .filter(|(path, _)| path.starts_with("chips/") && path.ends_with("/driver"))
        .map(|(_, manifest)| manifest["package"]["name"].as_str().expect("name"))
        .collect();
    let graph = packages
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
    for (_, manifest) in packages
        .iter()
        .filter(|(path, _)| path.starts_with("chips/") && path.ends_with("/radio/system"))
    {
        let owner = manifest["package"]["name"].as_str().expect("name");
        let violations = forbidden_paths(&graph, owner, |name| drivers.contains(name));
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
    let driver = |name: &str| name.ends_with("-driver");
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
    let protocol = path.strip_prefix("protocols/")?;
    if ["radio", "network", "hil"].contains(&protocol.split('/').next()?) {
        return None;
    }
    Some(match protocol.rsplit('/').next()? {
        "wire" => ProtocolRole::Wire,
        "cipher" => ProtocolRole::Cipher,
        "port" => ProtocolRole::Port,
        "model" => ProtocolRole::Model,
        "runtime" => ProtocolRole::Runtime,
        _ => ProtocolRole::Core,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChipRole {
    Backend,
    CoreSidePolicy,
}

fn chip_role(path: &str) -> Option<ChipRole> {
    if !path.starts_with("chips/") {
        return None;
    }
    // docs/repository-layout.md#a-protocol-wire-port-cores-backends:
    // "Wi-Fi upper-MAC and the family policy select each attempt's rate and
    // protection" places this family policy above the backend with the cores.
    Some(if path == "chips/espressif/ieee80211-policy" {
        ChipRole::CoreSidePolicy
    } else {
        ChipRole::Backend
    })
}

fn backend_core_paths(
    graph: &BTreeMap<String, BTreeSet<String>>,
    roles: &BTreeMap<String, ProtocolRole>,
    start: &str,
) -> Vec<String> {
    forbidden_paths(graph, start, |name| {
        matches!(
            roles.get(name),
            Some(ProtocolRole::Core | ProtocolRole::Runtime)
        )
    })
}

#[test]
fn backends_and_models_do_not_reach_protocol_cores_or_runtimes() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let packages = packages(&root);
    let roles = packages
        .iter()
        .filter_map(|(path, manifest)| {
            protocol_role(path)
                .or_else(|| {
                    (chip_role(path) == Some(ChipRole::CoreSidePolicy))
                        .then_some(ProtocolRole::Core)
                })
                .map(|role| {
                    (
                        manifest["package"]["name"]
                            .as_str()
                            .expect("name")
                            .to_owned(),
                        role,
                    )
                })
        })
        .collect();
    let graph = packages
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
    let violations = packages
        .iter()
        .filter(|(path, _)| {
            chip_role(path) == Some(ChipRole::Backend)
                || protocol_role(path) == Some(ProtocolRole::Model)
        })
        .flat_map(|(_, manifest)| {
            backend_core_paths(
                &graph,
                &roles,
                manifest["package"]["name"].as_str().expect("name"),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "backend reaches core/runtime:\n{}",
        violations.join("\n")
    );
}

#[test]
fn protocol_roles_and_core_side_family_policy_follow_the_layout() {
    for prefix in [
        "protocols/ieee802154",
        "protocols/bluetooth/le",
        "protocols/example",
    ] {
        for (directory, role) in [
            ("wire", ProtocolRole::Wire),
            ("cipher", ProtocolRole::Cipher),
            ("port", ProtocolRole::Port),
            ("model", ProtocolRole::Model),
            ("runtime", ProtocolRole::Runtime),
            ("mac", ProtocolRole::Core),
            ("controller", ProtocolRole::Core),
        ] {
            assert_eq!(protocol_role(&format!("{prefix}/{directory}")), Some(role));
        }
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
    assert_eq!(
        chip_role("chips/espressif/ieee80211-policy"),
        Some(ChipRole::CoreSidePolicy)
    );
    assert_eq!(
        chip_role("chips/esp32s31/ieee80211/driver"),
        Some(ChipRole::Backend)
    );
    assert_eq!(chip_role("protocols/ieee802154/model"), None);
}

#[test]
fn backend_rule_rejects_transitive_cross_protocol_and_model_paths() {
    let roles = BTreeMap::from([
        ("wire".into(), ProtocolRole::Wire),
        ("port".into(), ProtocolRole::Port),
        ("cipher".into(), ProtocolRole::Cipher),
        ("core".into(), ProtocolRole::Core),
        ("other-core".into(), ProtocolRole::Core),
        ("family-policy".into(), ProtocolRole::Core),
        ("runtime".into(), ProtocolRole::Runtime),
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
        ("core".into(), ProtocolRole::Core),
        ("runtime".into(), ProtocolRole::Runtime),
        ("build-core".into(), ProtocolRole::Core),
        ("model".into(), ProtocolRole::Model),
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
        ("oer-bluetooth-le-ll".into(), ProtocolRole::Core),
        ("oer-bluetooth-hci".into(), ProtocolRole::Core),
        ("oer-bluetooth-le-port".into(), ProtocolRole::Port),
        ("oer-bluetooth-le-wire".into(), ProtocolRole::Wire),
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
    "oer-ieee80211-softmac",
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
    "protocols/ieee80211/softmac",
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

/// Production tokens of one module body: items configured for tests are
/// skipped; out-of-line production modules are followed from `directory`.
fn production_waiting(tokens: TokenStream, directory: &Path, file: &Path, found: &mut Vec<String>) {
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
                        production_waiting(
                            inline.stream(),
                            &directory.join(name.to_string()),
                            file,
                            found,
                        );
                    }
                    _ => {
                        let child = match path {
                            Some(path) => directory.join(path),
                            None if directory.join(format!("{name}.rs")).is_file() => {
                                directory.join(format!("{name}.rs"))
                            }
                            None => directory.join(name.to_string()).join("mod.rs"),
                        };
                        production_file_waiting(&child, found);
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
        waiting_tokens(token, file, found);
    }
}

fn waiting_tokens(token: &TokenTree, file: &Path, found: &mut Vec<String>) {
    match token {
        TokenTree::Ident(word) if WAITING.contains(&word.to_string().as_str()) => {
            found.push(format!("{}: {word}", file.display()));
        }
        TokenTree::Group(group) => {
            for token in group.stream() {
                waiting_tokens(&token, file, found);
            }
        }
        _ => {}
    }
}

/// A module file: `lib.rs` and `mod.rs` own their directory; another file
/// `name.rs` owns `name/`.
fn production_file_waiting(file: &Path, found: &mut Vec<String>) {
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
    production_waiting(tokens, &directory, file, found);
}

#[test]
fn ieee80211_cores_never_wait_outside_their_tests() {
    let root = root();
    let packages = packages(&root);
    let mut found = Vec::new();
    for core in IEEE80211_SANS_IO_CORES {
        let manifest = packages.get(core).expect("the core is a workspace member");
        for crate_root in crate_roots(&root.join(core), manifest) {
            production_file_waiting(&crate_root, &mut found);
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
        "pub struct StaTxContext;\nmod inner { pub async fn wait() {} }\n",
    )
    .expect("module");
    std::fs::write(
        source.join("tests.rs"),
        "fn poll() -> core::task::Poll<()> { core::task::Poll::Pending }",
    )
    .expect("tests");
    let mut found = Vec::new();
    production_file_waiting(&source.join("lib.rs"), &mut found);
    assert_eq!(
        found,
        [format!("{}: async", source.join("machine.rs").display())]
    );
}
