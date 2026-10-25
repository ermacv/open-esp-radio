//! No esp-* or vendor PAC dependency in the S31 workspace. Host espflash stays.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Only build outputs, Git internals and the workspace excludes are skipped.
const SKIPPED: [&str; 2] = ["target", ".git"];

fn dependency_uses(name: &str, spec: &toml::Value, host: bool) -> usize {
    let package = spec
        .get("package")
        .and_then(toml::Value::as_str)
        .unwrap_or(name);
    let from_fork = spec
        .get("git")
        .and_then(toml::Value::as_str)
        .is_some_and(|url| url.contains("/esp-hal") || url.contains("/esp-pacs"));
    usize::from(
        package.starts_with("esp-")
            || package.starts_with("esp32")
            || from_fork
            || (package == "espflash" && !host),
    )
}

fn manifest_uses(value: &toml::Value, host: bool) -> usize {
    let Some(table) = value.as_table() else {
        return 0;
    };
    let mut count = 0;
    for (key, child) in table {
        match (key.as_str(), child.as_table()) {
            ("dependencies" | "dev-dependencies" | "build-dependencies", Some(dependencies)) => {
                count += dependencies
                    .iter()
                    .map(|(name, spec)| dependency_uses(name, spec, host))
                    .sum::<usize>();
            }
            ("patch", Some(sources)) => {
                for patches in sources.values().filter_map(toml::Value::as_table) {
                    count += patches
                        .iter()
                        .map(|(name, spec)| dependency_uses(name, spec, host))
                        .sum::<usize>();
                }
            }
            _ => count += manifest_uses(child, host),
        }
    }
    count
}

/// The directories `[workspace] exclude` of the manifest at `root` names.
fn excluded(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return Vec::new();
    };
    let manifest: toml::Value = toml::from_str(&text).expect("manifest parses");
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("exclude"))
        .and_then(toml::Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(toml::Value::as_str)
                .map(|path| root.join(path))
                .collect()
        })
        .unwrap_or_default()
}

fn visit(directory: &Path, excluded: &[PathBuf], files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source tree is readable") {
        let path = entry.expect("directory entry is readable").path();
        let name = path.file_name().map(|name| name.to_string_lossy());
        let name = name.as_deref().unwrap_or("");
        if path.is_dir() {
            if !SKIPPED.contains(&name) && !excluded.contains(&path) {
                visit(&path, excluded, files);
            }
        } else if name == "Cargo.toml" {
            files.push(path);
        }
    }
}

/// Uses per file below `root`, only files that have any.
fn scan(root: &Path) -> BTreeMap<String, usize> {
    let mut files = Vec::new();
    visit(root, &excluded(root), &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let host = path
                .strip_prefix(root)
                .expect("file is below root")
                .starts_with("tools");
            let count = manifest_uses(
                &toml::from_str::<toml::Value>(&text).expect("manifest parses"),
                host,
            );
            let relative = path.strip_prefix(root).expect("file is below root");
            (count != 0).then(|| (relative.to_string_lossy().into_owned(), count))
        })
        .collect()
}

#[test]
fn the_workspace_has_no_esp_dependencies() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let found = scan(&root);
    assert!(
        found.is_empty(),
        "esp-* and esp-pacs dependencies are forbidden: {found:?}"
    );
}

#[test]
fn aliases_target_dependencies_and_patches_cannot_hide_esp_packages() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
esp-hal = { workspace = true }
alias = { package = "esp-config", version = "1" }
espflash = "4"
[target.'cfg(target_os = "none")'.dependencies]
pac = { package = "esp32s31", version = "1" }
[patch.crates-io]
other = { git = "https://github.com/ermacv/esp-pacs" }
"#,
    )
    .expect("manifest");
    assert_eq!(manifest_uses(&manifest, true), 4);
}

#[test]
fn host_espflash_and_our_packages_are_allowed() {
    let manifest: toml::Value = toml::from_str(
        r#"
[dependencies]
espflash = "4"
oer-espressif-boot = { path = "boot" }
"#,
    )
    .expect("manifest");
    assert_eq!(manifest_uses(&manifest, true), 0);
}

#[test]
fn excluded_c5_manifests_are_not_scanned() {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("Cargo.toml"),
        "[workspace]\nexclude = [\"c5\"]\n",
    )
    .expect("root manifest");
    std::fs::create_dir(directory.path().join("c5")).expect("excluded directory");
    std::fs::write(
        directory.path().join("c5/Cargo.toml"),
        "[dependencies]\nesp-hal = \"1\"\n",
    )
    .expect("excluded manifest");
    assert!(scan(directory.path()).is_empty());
}

#[test]
fn espflash_is_a_host_tool_only() {
    let manifest: toml::Value =
        toml::from_str("[dependencies]\nespflash = \"4\"\n").expect("manifest");
    assert_eq!(manifest_uses(&manifest, false), 1);
}

#[test]
fn the_lock_has_no_vendor_platform_packages() {
    let lock: toml::Value =
        toml::from_str(include_str!("../../../../Cargo.lock")).expect("lock parses");
    let packages = lock["package"].as_array().expect("locked packages");
    for package in packages {
        let name = package["name"].as_str().expect("package name");
        if name == "espflash" {
            continue;
        }
        if name == "esp-idf-part" {
            // espflash uses this host-only partition format internally. No
            // workspace manifest may depend on it; only espflash may pull it in.
            for user in packages {
                if user
                    .get("dependencies")
                    .and_then(toml::Value::as_array)
                    .is_some_and(|deps| {
                        deps.iter().any(|dep| {
                            dep.as_str()
                                .is_some_and(|dep| dep.split(' ').next() == Some(name))
                        })
                    })
                {
                    assert_eq!(user["name"].as_str(), Some("espflash"));
                }
            }
            continue;
        }
        assert!(
            !name.starts_with("esp-") && !name.starts_with("esp32"),
            "forbidden locked package: {name}"
        );
    }
}

#[test]
fn facts_and_raw_pac_manifests_cannot_hide_platform_dependencies() {
    let directory = tempfile::tempdir().expect("temporary directory");
    for path in ["chips/facts", "chips/pac-raw"] {
        let package = directory.path().join(path);
        std::fs::create_dir_all(&package).expect("package directory");
        std::fs::write(
            package.join("Cargo.toml"),
            "[dependencies]\nesp-config = \"1\"\n",
        )
        .expect("manifest");
    }
    assert_eq!(scan(directory.path()).len(), 2);
}
