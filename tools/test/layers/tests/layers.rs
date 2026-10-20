//! The repository's two structural rules, read from every package manifest:
//! a package is named after its directory, and every crate above the unsafe
//! boundary forbids unsafe code.

use std::path::{Path, PathBuf};

/// The roots that hold packages.
const ROOTS: [&str; 5] = ["protocols", "portable", "chips", "firmware", "tools"];

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

/// Every package of the roots, repository-relative.
fn all_packages(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for directory in ROOTS {
        packages(root, directory, &mut found);
    }
    found.sort();
    found
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
fn every_package_is_named_after_its_directory() {
    let root = root();
    let found = all_packages(&root);
    let misnamed = found
        .iter()
        .filter_map(|package| {
            let text = std::fs::read_to_string(root.join(package).join("Cargo.toml")).unwrap();
            let manifest: toml::Table = toml::from_str(&text).unwrap();
            let expected = expected_name(package, !executables(&root, package).is_empty());
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
    ] {
        assert_eq!(expected_name(path, executable), name);
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
    "chips/esp32c5/hal",
    "chips/esp32s31/hal",
    "chips/esp32s31/bluetooth/memory",
    "chips/esp32c5/board",
    "chips/esp32s31/board",
    "chips/espressif/staged-bootstrap",
    "chips/espressif/staged-layout",
    "chips/espressif/staged-runtime",
    "chips/espressif/executor",
    "chips/espressif/esp-hal-irq",
    "chips/espressif/esp-hal/",
    "chips/esp32c5/esp-hal-soc",
    "portable/embassy/",
    "portable/memory",
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
fn every_crate_below_the_unsafe_boundary_denies_undocumented_unsafe_code() {
    let root = root();
    let missing = all_packages(&root)
        .into_iter()
        .filter(|package| below_the_boundary(package) && !package.ends_with("pac-raw"))
        .flat_map(|package| {
            // Bootstrap and probe images are unsafe throughout: they deny
            // only undocumented unsafe blocks (docs/unsafe.md).
            let image = package.starts_with("firmware/bootstrap/")
                || package.starts_with("firmware/probes/");
            crate_roots(&root, &package)
                .into_iter()
                .map(move |path| (path, image))
        })
        .filter(|(path, image)| {
            !std::fs::read_to_string(path).unwrap().lines().any(|line| {
                let line = line.trim();
                line == "#![forbid(unsafe_code)]"
                    || line == "#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]"
                    || (*image && line == "#![deny(clippy::undocumented_unsafe_blocks)]")
            })
        })
        .map(|(path, _)| path.strip_prefix(&root).unwrap().display().to_string())
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn the_boundary_is_named_by_package_or_domain() {
    for package in [
        "chips/esp32c5/hal",
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
