//! The dependency rules of `docs/repository-layout.md`, read from the path
//! dependencies of every package manifest, and the rule that a package is
//! named after its directory. A package's root is its top folder; manifests
//! carry no layer metadata.

use std::path::{Component, Path, PathBuf};

/// The roots that hold packages, in the layout's order.
const ROOTS: [&str; 5] = ["portable", "chips", "host", "firmware", "tools"];

/// The packages outside `firmware/` that build only for a chip: a host
/// `cargo test` does not build them.
const CHIP_ONLY: [&str; 6] = [
    "chips/esp32c5/board",
    "chips/esp32s31/board",
    "chips/espressif/staged-bootstrap",
    "chips/espressif/staged-runtime",
    "portable/hil-agent",
    "chips/esp32c5/layout",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Portable,
    Chips,
    Host,
    Firmware,
    Tools,
}

fn root_of(path: &str) -> Option<Root> {
    Some(match path.split('/').next()? {
        "portable" => Root::Portable,
        "chips" => Root::Chips,
        "host" => Root::Host,
        "firmware" => Root::Firmware,
        "tools" => Root::Tools,
        _ => return None,
    })
}

/// The domains of `host/`, lowest first: a host package depends only on
/// its own domain and those before it.
const HOST_DOMAINS: [&str; 11] = [
    "os",
    "test",
    "analysis",
    "chip",
    "pins",
    "registers",
    "probe",
    "image",
    "lab",
    "hil",
    "vendor",
];

/// Why the host package `from` may not depend on the host package `to`, or
/// `None` when the domain order allows it or either is outside `host/`.
fn domain_refusal(from: &str, to: &str) -> Option<String> {
    let domain = |path: &str| -> Option<Option<usize>> {
        let mut parts = path.split('/');
        (parts.next()? == "host").then(|| {
            let name = parts.next().unwrap_or_default();
            HOST_DOMAINS.iter().position(|domain| *domain == name)
        })
    };
    match (domain(from)?, domain(to)?) {
        (Some(source), Some(target)) if target <= source => None,
        (Some(_), Some(_)) => Some(format!(
            "{from} → {to}: a host domain depends only on the domains before it ({})",
            HOST_DOMAINS.join(" → ")
        )),
        _ => Some(format!(
            "{from} → {to}: a host package outside the domains of HOST_DOMAINS"
        )),
    }
}

/// How a package reaches a dependency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    /// A normal dependency of a library or program.
    Normal,
    /// Code that runs on the host while the package builds or is tested: a
    /// build or dev dependency, or a proc macro.
    HostTime,
}

/// Why `from` may not depend on `to`, both repository-relative package
/// directories, or `None` when it may.
fn refusal(from: &str, to: &str, kind: Kind) -> Option<String> {
    use Root::*;
    let refused = |why: &str| Some(format!("{from} → {to}: {why}"));
    let (Some(source), Some(target)) = (root_of(from), root_of(to)) else {
        return refused("a package outside portable/, chips/, host/, firmware/ and tools/");
    };
    let allowed = match (source, target) {
        (_, Portable) => true,
        (Chips | Firmware, Chips) => true,
        (Host | Tools, Host) => true,
        // Build-time code and tests run on the host.
        (_, Host) => kind == Kind::HostTime,
        _ => false,
    };
    (!allowed).then(|| {
        format!(
            "{from} → {to}: portable → portable; chips → portable, chips; host → portable, host; \
             firmware → portable, chips; tools → portable, host"
        )
    })
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

/// Every package directory below `directory`, repository-relative.
fn packages(root: &Path, directory: &str, found: &mut Vec<String>) {
    let mut stack = vec![root.join(directory)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() && name != "target" && !name.to_string_lossy().starts_with('.') {
                stack.push(path);
            } else if name == "Cargo.toml" {
                let package = path.parent().unwrap().strip_prefix(root).unwrap();
                found.push(package.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
}

/// `path`, joined to `base` and with `.`/`..` resolved, repository-relative.
fn resolve(base: &str, path: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(base).join(path).components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    parts.join("/")
}

/// Whether the package at `package` is a proc macro.
fn is_proc_macro(root: &Path, package: &str) -> bool {
    let text = std::fs::read_to_string(root.join(package).join("Cargo.toml")).unwrap();
    let manifest: toml::Table = toml::from_str(&text).unwrap();
    manifest
        .get("lib")
        .and_then(|lib| lib.get("proc-macro"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false)
}

/// The path dependencies of the package at `package` (all kinds, targets
/// included), repository-relative, with how it reaches each.
fn path_dependencies(root: &Path, package: &str) -> Vec<(String, Kind)> {
    let text = std::fs::read_to_string(root.join(package).join("Cargo.toml")).unwrap();
    let manifest: toml::Table = toml::from_str(&text).unwrap();
    let mut tables = Vec::new();
    let mut collect = |table: &toml::Table| {
        for (kind, reach) in [
            ("dependencies", Kind::Normal),
            ("dev-dependencies", Kind::HostTime),
            ("build-dependencies", Kind::HostTime),
        ] {
            if let Some(toml::Value::Table(dependencies)) = table.get(kind) {
                tables.push((dependencies.clone(), reach));
            }
        }
    };
    collect(&manifest);
    if let Some(toml::Value::Table(targets)) = manifest.get("target") {
        for target in targets.values() {
            if let toml::Value::Table(target) = target {
                collect(target);
            }
        }
    }
    tables
        .iter()
        .flat_map(|(dependencies, reach)| {
            dependencies.values().filter_map(move |dependency| {
                let path = dependency.get("path")?.as_str()?;
                let path = resolve(package, path);
                let reach = if is_proc_macro(root, &path) {
                    Kind::HostTime
                } else {
                    *reach
                };
                Some((path, reach))
            })
        })
        .collect()
}

/// Every package of the roots, repository-relative.
fn all_packages(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for directory in ROOTS {
        packages(root, directory, &mut found);
    }
    found.sort();
    found
}

#[test]
fn every_dependency_goes_down_its_layers() {
    let root = root();
    let found = all_packages(&root);
    assert!(found.len() > 100, "{found:?}");
    let refusals = found
        .iter()
        .flat_map(|package| {
            path_dependencies(&root, package)
                .into_iter()
                .filter_map(|(dependency, kind)| {
                    refusal(package, &dependency, kind)
                        .or_else(|| domain_refusal(package, &dependency))
                })
        })
        .collect::<Vec<_>>();
    assert!(refusals.is_empty(), "{}", refusals.join("\n"));
}

#[test]
fn the_host_domains_go_down_their_order() {
    for (from, to) in [
        ("tools/registers/generate", "tools/vendor/provenance"),
        ("tools/os/process", "tools/analysis/elf"),
        ("tools/image/build", "tools/lab/board"),
        ("host/unknown/x", "tools/os/process"),
    ] {
        assert!(domain_refusal(from, to).is_some(), "{from} → {to}");
    }
    for (from, to) in [
        ("tools/vendor/provenance", "tools/registers/model"),
        ("tools/image/build", "tools/image/checks"),
        ("tools/hil/run", "tools/os/process"),
        ("tools/image/cli", "tools/lab/board"),
        ("tools/os/process", "portable/time"),
    ] {
        assert_eq!(domain_refusal(from, to), None, "{from} → {to}");
    }
}

/// The executable targets of `package`: `[[bin]]` sections, `src/main.rs`
/// and `src/bin/`.
fn executables(root: &Path, package: &str) -> Vec<String> {
    let directory = root.join(package);
    let manifest = std::fs::read_to_string(directory.join("Cargo.toml")).unwrap();
    let mut found = Vec::new();
    if manifest.lines().any(|line| line.trim() == "[[bin]]") {
        found.push("[[bin]]".to_owned());
    }
    for target in ["src/main.rs", "src/bin"] {
        if directory.join(target).exists() {
            found.push(target.to_owned());
        }
    }
    found
}

#[test]
fn executables_live_only_in_firmware_and_tools() {
    let root = root();
    let misplaced = all_packages(&root)
        .into_iter()
        .filter(|package| {
            matches!(
                root_of(package),
                Some(Root::Portable | Root::Chips | Root::Host)
            )
        })
        .filter_map(|package| {
            let found = executables(&root, &package);
            (!found.is_empty()).then(|| format!("{package}: {}", found.join(", ")))
        })
        .collect::<Vec<_>>();
    assert!(misplaced.is_empty(), "{}", misplaced.join("\n"));
    assert!(!executables(&root, "tools/image/cli").is_empty());
}

#[test]
fn no_package_sits_inside_another() {
    let root = root();
    let found = all_packages(&root);
    let nested = found
        .iter()
        .filter_map(|inner| {
            found
                .iter()
                .find(|outer| *outer != inner && inner.starts_with(&format!("{outer}/")))
                .map(|outer| format!("{inner} is inside {outer}"))
        })
        .collect::<Vec<_>>();
    assert!(nested.is_empty(), "{}", nested.join("\n"));
}

#[test]
fn the_rules_refuse_what_the_layout_forbids() {
    for (from, to, kind) in [
        ("portable/memory", "tools/os/process", Kind::Normal),
        (
            "protocols/ieee80211/mac",
            "chips/esp32s31/hal",
            Kind::Normal,
        ),
        ("chips/esp32s31/hal", "tools/os/process", Kind::Normal),
        (
            "tools/registers/generate",
            "chips/esp32s31/pac",
            Kind::Normal,
        ),
        ("tools/image/firmware", "firmware/apps/w1", Kind::Normal),
        ("tools/image/cli", "tools/registers/cli", Kind::Normal),
        ("tools/image/cli", "chips/esp32s31/hal", Kind::Normal),
        (
            "tools/vendor/ieee802154-esp32s31",
            "chips/esp32s31/hal",
            Kind::Normal,
        ),
        (
            "portable/esp32s31/phy-relation",
            "chips/esp32s31/phy",
            Kind::Normal,
        ),
        ("firmware/apps/w1", "tools/image/firmware", Kind::Normal),
        ("tools/image/cli", "outside/base/time", Kind::Normal),
    ] {
        assert!(refusal(from, to, kind).is_some(), "{from} → {to}");
    }
    for (from, to, kind) in [
        ("tools/image/cli", "tools/image/firmware", Kind::Normal),
        (
            "tools/image/firmware",
            "chips/esp32s31/layout",
            Kind::Normal,
        ),
        ("chips/esp32s31/hal", "portable/time", Kind::Normal),
        ("firmware/apps/w1", "chips/esp32s31/hal", Kind::Normal),
        (
            "tools/analysis/riscv-lift",
            "tools/analysis/elf",
            Kind::Normal,
        ),
        // Build-time code and tests run on the host.
        ("chips/esp32s31/hal", "tools/test/ui-cases", Kind::HostTime),
        (
            "firmware/probes/esp32s31-radio",
            "tools/probe/codegen",
            Kind::HostTime,
        ),
    ] {
        assert_eq!(refusal(from, to, kind), None, "{from} → {to}");
    }
}

/// The root manifest's `[workspace]` list `key`, repository-relative.
fn workspace_list(root: &Path, key: &str) -> Vec<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let manifest: toml::Table = toml::from_str(&text).unwrap();
    manifest["workspace"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn every_host_package_is_a_default_member_and_no_chip_only_one_is() {
    let root = root();
    let members = workspace_list(&root, "members");
    let defaults = workspace_list(&root, "default-members");
    // Everything but firmware/ builds on the host; firmware/ builds only for
    // the chip, so a host `cargo test` must not build it.
    let host = |member: &str| !member.starts_with("firmware/") && !CHIP_ONLY.contains(&member);
    let missing = members
        .iter()
        .filter(|member| host(member) && !defaults.contains(member))
        .map(|member| format!("{member} is a host package missing from default-members"))
        .chain(
            defaults
                .iter()
                .filter(|member| !host(member))
                .map(|member| format!("{member} builds only for the chip but is a default member")),
        )
        .chain(
            defaults
                .iter()
                .filter(|member| !members.contains(member))
                .map(|member| format!("{member} is a default member but no member")),
        )
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    // Every package directory of the roots is a member: the workspace
    // excludes nothing, so every package builds.
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    assert!(
        !manifest["workspace"]
            .as_table()
            .unwrap()
            .contains_key("exclude"),
        "the workspace excludes no package"
    );
    let found = all_packages(&root);
    let unlisted = found
        .iter()
        .filter(|package| !members.contains(package))
        .collect::<Vec<_>>();
    assert!(unlisted.is_empty(), "not workspace members: {unlisted:?}");
}

/// The package name the directory `path` must carry: a library is `oer-`
/// and its path below its root (`protocols/ieee80211/mac` is
/// `oer-ieee80211-mac`), an executable keeps its root (`firmware/apps/w1` is
/// `oer-firmware-apps-w1`, `tools/image/cli` is `oer-tool-image-cli`).
fn expected_name(path: &str) -> String {
    let parts = path.split('/').collect::<Vec<_>>();
    let parts = match parts.as_slice() {
        ["firmware", rest @ ..] => [&["oer", "firmware"][..], rest].concat(),
        ["tools", rest @ ..] => [&["oer", "tool"][..], rest].concat(),
        [_, rest @ ..] => [&["oer"][..], rest].concat(),
        [] => Vec::new(),
    };
    parts.join("-")
}

#[test]
fn every_package_is_named_after_its_directory() {
    let root = root();
    let found = all_packages(&root);
    let misnamed = found
        .iter()
        .filter_map(|package| {
            let text = std::fs::read_to_string(root.join(package).join("Cargo.toml")).unwrap();
            let manifest: toml::Table = toml::from_str(&text).unwrap();
            let expected = expected_name(package);
            let name = manifest.get("package")?.get("name")?.as_str()?;
            let renamed = manifest
                .get("lib")
                .and_then(|lib| lib.get("name"))
                .is_some();
            (name != expected || renamed).then(|| {
                format!("{package} is `{name}`, must be `{expected}` without a `[lib] name`")
            })
        })
        .collect::<Vec<_>>();
    assert!(misnamed.is_empty(), "{}", misnamed.join("\n"));
}

#[test]
fn the_name_follows_the_path() {
    for (path, name) in [
        ("portable/time", "oer-time"),
        (
            "chips/esp32s31/ieee80211/driver",
            "oer-esp32s31-ieee80211-driver",
        ),
        ("firmware/apps/w1", "oer-firmware-apps-w1"),
        ("tools/image/firmware", "oer-image-firmware"),
        ("tools/image/cli", "oer-tool-image-cli"),
    ] {
        assert_eq!(expected_name(path), name);
    }
}

/// The packages below the unsafe boundary (`docs/unsafe.md`), by directory
/// or, ending in `/`, by domain: the generated and the semantic PACs, the
/// HAL, the platform, executor and adapter bindings, the cores of memory the
/// hardware shares, the host's operating-system bindings, the C-driver
/// stand, and the bootstrap and probe images. Every other package forbids
/// unsafe code.
const BELOW_THE_BOUNDARY: &[&str] = &[
    "chips/esp32c5/pac-raw",
    "chips/esp32s31/pac-raw",
    "chips/espressif/ieee80211-pac-raw",
    "chips/esp32c5/pac",
    "chips/esp32s31/pac",
    "chips/esp32s31/hal",
    "chips/esp32s31/bluetooth/memory",
    "chips/platform/",
    "chips/exec/",
    "chips/adapters/",
    "portable/adapters/",
    "portable/memory",
    "portable/interrupt-table",
    "chips/espressif/staged-layout",
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

/// The crate roots of `package`: its library and every executable.
fn crate_roots(root: &Path, package: &str) -> Vec<PathBuf> {
    let directory = root.join(package);
    let manifest = std::fs::read_to_string(directory.join("Cargo.toml")).unwrap();
    let mut roots = Vec::new();
    for default in ["src/lib.rs", "src/main.rs"] {
        if directory.join(default).exists() {
            roots.push(directory.join(default));
        }
    }
    if let Ok(entries) = std::fs::read_dir(directory.join("src/bin")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                roots.push(path.join("main.rs"));
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                roots.push(path);
            }
        }
    }
    let mut in_bin = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
        } else if in_bin && let Some(path) = line.strip_prefix("path = ") {
            let path = directory.join(path.trim_matches('"'));
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

#[test]
fn every_crate_above_the_unsafe_boundary_forbids_unsafe_code() {
    let root = root();
    let missing = all_packages(&root)
        .into_iter()
        .filter(|package| !below_the_boundary(package))
        .flat_map(|package| crate_roots(&root, &package))
        .filter(|path| {
            !std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .any(|line| line.trim() == "#![forbid(unsafe_code)]")
        })
        .map(|path| path.strip_prefix(&root).unwrap().display().to_string())
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
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
