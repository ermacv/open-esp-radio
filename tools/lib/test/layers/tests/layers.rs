//! The dependency rules of `tools/README.md`, read from the path dependencies
//! of every `Cargo.toml` below `tools/` and `crates/`, and the rule that a
//! package is named after its directory. A package's layer is its folder;
//! manifests carry no layer metadata.

use std::path::{Component, Path, PathBuf};

/// The `service → service` dependencies allowed, each listed: a new edge is
/// added here on purpose, in the change that needs it.
const SERVICE_ORDER: [(&str, &str); 112] = [
    ("devices", "device-lock"),
    ("devices", "image-bundle"),
    ("firmware", "image-check-interrupts"),
    ("firmware", "image-check-stack"),
    ("firmware", "lab"),
    ("hil-analysis", "hil-run-bundle"),
    ("hil-analysis", "stats"),
    ("hil-executor", "hil-executor"),
    ("hil-executor", "stand-arbiter"),
    ("hil-experiment", "hil-analysis"),
    ("hil-experiment", "hil-image"),
    ("hil-experiment", "hil-observer"),
    ("hil-experiment", "hil-run-bundle"),
    ("hil-experiment", "hil-source-snapshot"),
    ("hil-experiment", "stand-arbiter"),
    ("hil-experiment", "stand-claims"),
    ("hil-experiment", "stand-owners"),
    ("hil-experiment", "stats"),
    ("hil-family-phy", "devices"),
    ("hil-family-phy", "hil-workload"),
    ("hil-family-phy", "phy-calibration-capture"),
    ("hil-family-phy-esp32s31", "hil-family-phy"),
    ("hil-family-phy-esp32s31", "phy-calibration-capture"),
    ("hil-family-phy-esp32s31", "phy-vendor-calibration-esp32s31"),
    ("hil-family-system", "devices"),
    ("hil-family-system", "hil-scenario"),
    ("hil-family-system", "hil-workload"),
    ("hil-fixture", "stand-fixture-install"),
    ("hil-flash", "device-lock"),
    ("hil-flash", "devices"),
    ("hil-flash", "image"),
    ("hil-flash", "image-bundle"),
    ("hil-flash", "stand-board"),
    ("hil-flash", "stand-journal"),
    ("hil-image", "hil-run-bundle"),
    ("hil-image", "hil-scenario"),
    ("hil-image", "hil-source-snapshot"),
    ("hil-image", "image"),
    ("hil-image", "image-bundle"),
    ("hil-image", "image-check-interrupts"),
    ("hil-image", "image-checks"),
    ("hil-image", "image-compare"),
    ("hil-lab", "device-lock"),
    ("hil-lab", "devices"),
    ("hil-lab", "hil-link"),
    ("hil-lab", "lab"),
    ("hil-lab", "stand-arbiter"),
    ("hil-lab", "stand-board"),
    ("hil-lab", "stand-claims"),
    ("hil-lab", "stand-fixture-install"),
    ("hil-lab", "stand-journal"),
    ("hil-lab", "stand-resource-lock"),
    ("hil-lab", "stand-ssh"),
    ("hil-link", "device-peer-line"),
    ("hil-net-traffic", "hil-fixture"),
    ("hil-net-traffic", "hil-link"),
    ("hil-run-bundle", "hil-scenario"),
    ("hil-run-bundle", "hil-source-snapshot"),
    ("hil-runner", "device-lock"),
    ("hil-runner", "devices"),
    ("hil-runner", "hil-analysis"),
    ("hil-runner", "hil-family-system"),
    ("hil-runner", "hil-flash"),
    ("hil-runner", "hil-image"),
    ("hil-runner", "hil-lab"),
    ("hil-runner", "hil-link"),
    ("hil-runner", "hil-observer"),
    ("hil-runner", "hil-run-bundle"),
    ("hil-runner", "hil-scenario"),
    ("hil-runner", "hil-source-snapshot"),
    ("hil-runner", "hil-workload"),
    ("hil-runner", "image"),
    ("hil-runner", "image-bundle"),
    ("hil-runner", "image-check-interrupts"),
    ("hil-runner", "stand-board"),
    ("hil-runner", "stand-journal"),
    ("hil-runner", "stand-owners"),
    ("hil-workload", "device-lock"),
    ("hil-workload", "hil-lab"),
    ("hil-workload", "hil-link"),
    ("hil-workload", "hil-scenario"),
    ("hil-workload", "image-bundle"),
    ("hil-workload", "stand-board"),
    ("image", "esp-idf"),
    ("image", "image-bundle"),
    ("image", "image-encode"),
    ("image-check-interrupts", "image-check-stack"),
    ("image-checks", "image"),
    ("image-checks", "image-check-interrupts"),
    ("image-checks", "image-check-placement"),
    ("image-checks", "image-check-stack"),
    ("phy-vendor-calibration-esp32s31", "phy-calibration-capture"),
    ("probe-macros", "probe-codegen"),
    ("registers", "vendor-provenance"),
    ("stand-arbiter", "device-lock"),
    ("stand-arbiter", "devices"),
    ("stand-arbiter", "stand-claims"),
    ("stand-arbiter", "stand-journal"),
    ("stand-arbiter", "stand-owners"),
    ("stand-arbiter", "stand-power"),
    ("stand-board", "device-lock"),
    ("stand-board", "devices"),
    ("stand-board", "image-bundle"),
    ("stand-board", "stand-power"),
    ("stand-discover", "stand-power"),
    ("stand-doctor", "stand-power"),
    ("stand-fixtures", "stand-ssh"),
    ("stand-journal", "devices"),
    ("stand-journal", "stand-owners"),
    ("stand-power", "devices"),
    ("stand-power", "lab"),
    ("symbol-lineage", "vendor-provenance"),
];

/// The portable parts of `crates/` that `tools/` may use.
const PORTABLE_CRATES: [&str; 5] = [
    "crates/base/",
    "crates/radio/",
    "crates/ieee80211/",
    "crates/bluetooth/",
    "crates/ieee802154/",
];

/// The host-built image layout crates only `lib/service/firmware` may use.
const IMAGE_LAYOUT: [&str; 2] = [
    "firmware/platform/espressif/staged-layout",
    "firmware/platform/esp32s31/layout",
];

#[derive(Clone, Debug, Eq, PartialEq)]
enum Place {
    Crates,
    Cli(String),
    Service(String),
    Analysis,
    Data,
    Base,
    Test,
    Firmware,
    Other,
}

fn place(path: &str) -> Place {
    let part = |prefix: &str| {
        path.strip_prefix(prefix)
            .map(|rest| rest.split('/').next().unwrap_or_default().to_owned())
    };
    if path.starts_with("crates/") {
        Place::Crates
    } else if let Some(name) = part("tools/cli/") {
        Place::Cli(name)
    } else if let Some(name) = part("tools/lib/service/") {
        Place::Service(name)
    } else if path.starts_with("tools/lib/analysis/") {
        Place::Analysis
    } else if path.starts_with("tools/lib/data/") {
        Place::Data
    } else if path.starts_with("tools/lib/base/") {
        Place::Base
    } else if path.starts_with("tools/lib/test/") {
        Place::Test
    } else if path.starts_with("firmware/") {
        Place::Firmware
    } else {
        Place::Other
    }
}

/// Why `from` may not depend on `to`, both repository-relative package
/// directories, or `None` when it may.
fn refusal(from: &str, to: &str) -> Option<String> {
    use Place::*;
    let (source, target) = (place(from), place(to));
    let refused = |why: &str| Some(format!("{from} → {to}: {why}"));
    match (&source, &target) {
        (Crates, Crates) => None,
        (Crates, _) => refused("crates/ never depends on tools/ or firmware/"),
        (Other, _) | (_, Other) => refused("a package outside tools/, crates/ and firmware/"),
        (Firmware, _) => None,
        (_, Crates) if PORTABLE_CRATES.iter().any(|prefix| to.starts_with(prefix)) => None,
        (_, Crates) => refused("tools/ uses only the portable crates (base, radio, the protocols)"),
        (Service(name), Firmware) if name == "firmware" && IMAGE_LAYOUT.contains(&to) => None,
        (_, Firmware) => refused(
            "only lib/service/firmware uses firmware/, and only its host-built image layout",
        ),
        (Cli(_), Cli(_)) => refused("a CLI never depends on another CLI"),
        (Cli(_), Service(_) | Analysis | Data | Base) => None,
        (Service(a), Service(b)) if SERVICE_ORDER.contains(&(a.as_str(), b.as_str())) => None,
        (Service(_), Service(_)) => {
            refused("a service → service edge tools/README.md does not list")
        }
        (Service(_), Analysis | Data | Base) => None,
        (Analysis, Analysis | Base) | (Data, Data | Base) | (Base, Base) => None,
        (Test, Service(_) | Analysis | Data | Base | Test) => None,
        _ => refused("dependencies go only down: cli → service → analysis, data → base"),
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
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

/// The path dependencies of the package at `package` (all kinds, targets
/// included), repository-relative.
fn path_dependencies(root: &Path, package: &str) -> Vec<String> {
    let text = std::fs::read_to_string(root.join(package).join("Cargo.toml")).unwrap();
    let manifest: toml::Table = toml::from_str(&text).unwrap();
    let mut tables = Vec::new();
    let mut collect = |table: &toml::Table| {
        for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(toml::Value::Table(dependencies)) = table.get(kind) {
                tables.push(dependencies.clone());
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
        .flat_map(|dependencies| dependencies.values())
        .filter_map(|dependency| dependency.get("path")?.as_str())
        .map(|path| resolve(package, path))
        .collect()
}

#[test]
fn every_dependency_goes_down_its_layers() {
    let root = root();
    let mut found = Vec::new();
    packages(&root, "tools", &mut found);
    packages(&root, "crates", &mut found);
    assert!(found.len() > 20, "{found:?}");
    // A package that does not build yet answers to the rules when it joins.
    let excluded = workspace_list(&root, "exclude");
    found.retain(|package| !excluded.contains(package));
    let refusals = found
        .iter()
        .flat_map(|package| {
            path_dependencies(&root, package)
                .into_iter()
                .filter_map(|dependency| refusal(package, &dependency))
        })
        .collect::<Vec<_>>();
    assert!(refusals.is_empty(), "{}", refusals.join("\n"));
}

#[test]
fn no_package_sits_inside_another() {
    let root = root();
    let mut found = Vec::new();
    packages(&root, "tools", &mut found);
    packages(&root, "crates", &mut found);
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
fn the_rules_refuse_what_the_readme_forbids() {
    for (from, to) in [
        ("crates/base/memory", "tools/lib/base/process"),
        ("tools/lib/data/stand-file", "tools/lib/service/lab"),
        ("tools/lib/analysis/stack", "tools/lib/data/register-model"),
        ("tools/lib/analysis/elf", "tools/lib/service/image-linker"),
        (
            "tools/lib/service/image-check-stack",
            "tools/lib/service/image-check-interrupts",
        ),
        ("tools/cli/fw", "tools/cli/registers"),
        ("tools/lib/service/lab", "tools/lib/service/firmware"),
        ("tools/lib/service/registers", "crates/esp32s31/pac"),
        ("tools/lib/service/lab", "firmware/platform/esp32s31/layout"),
        ("tools/lib/service/firmware", "firmware/apps/w1"),
    ] {
        assert!(refusal(from, to).is_some(), "{from} → {to}");
    }
    for (from, to) in [
        ("tools/cli/fw", "tools/lib/service/firmware"),
        ("tools/lib/service/firmware", "tools/lib/service/lab"),
        (
            "tools/lib/service/firmware",
            "firmware/platform/esp32s31/layout",
        ),
        (
            "tools/lib/data/register-model",
            "tools/lib/data/register-contracts",
        ),
        ("tools/lib/analysis/stack", "tools/lib/base/process"),
        ("tools/lib/analysis/riscv-lift", "tools/lib/analysis/elf"),
        ("crates/esp32s31/hal", "crates/base/ui-cases"),
    ] {
        assert_eq!(refusal(from, to), None, "{from} → {to}");
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
    // A host package is anything outside firmware/, and the image layout
    // crates the firmware tool reads; the rest of firmware/ builds only for
    // the chip, so a host `cargo test` must not build it.
    let host = |member: &str| !member.starts_with("firmware/") || IMAGE_LAYOUT.contains(&member);
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
    // Every package directory below crates/ and tools/ is a member, or
    // listed in `exclude` with why it does not build yet.
    let excluded = workspace_list(&root, "exclude");
    let mut found = Vec::new();
    packages(&root, "tools", &mut found);
    packages(&root, "crates", &mut found);
    let unlisted = found
        .iter()
        .filter(|package| !members.contains(package) && !excluded.contains(package))
        .collect::<Vec<_>>();
    assert!(unlisted.is_empty(), "not workspace members: {unlisted:?}");
}

/// The package name the directory `path` must carry: `crates/a/b` is
/// `oer-a-b`, `firmware/a/b` is `oer-firmware-a-b`, and `tools/x/y` is
/// `oer-tool-x-y` with the `lib/` level left out.
fn expected_name(path: &str) -> String {
    let parts = path.split('/').collect::<Vec<_>>();
    let parts = match parts.as_slice() {
        ["crates", rest @ ..] => [&["oer"][..], rest].concat(),
        ["firmware", rest @ ..] => [&["oer", "firmware"][..], rest].concat(),
        ["tools", "lib", rest @ ..] | ["tools", rest @ ..] => [&["oer", "tool"][..], rest].concat(),
        _ => parts,
    };
    parts.join("-")
}

#[test]
fn every_package_is_named_after_its_directory() {
    let root = root();
    let mut found = Vec::new();
    for directory in ["tools", "crates", "firmware"] {
        packages(&root, directory, &mut found);
    }
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
        ("crates/base/time", "oer-base-time"),
        (
            "crates/esp32s31/ieee80211/driver",
            "oer-esp32s31-ieee80211-driver",
        ),
        ("firmware/apps/w1", "oer-firmware-apps-w1"),
        ("tools/lib/service/firmware", "oer-tool-service-firmware"),
        ("tools/cli/fw", "oer-tool-cli-fw"),
    ] {
        assert_eq!(expected_name(path), name);
    }
}
