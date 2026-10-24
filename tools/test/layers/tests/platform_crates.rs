//! No new use of esp-hal, esp-pacs or the crates that travel with them.
//!
//! The platform is being moved to our own HAL and PAC. Every remaining use is
//! listed in `platform_crates.allowlist` with its file and its number of
//! places; a use anywhere else fails, and so does a listed place that is gone,
//! so each domain change removes its lines and the list only shrinks.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Crate names as written in Rust paths.
const RUST_NAMES: [&str; 12] = [
    "esp_hal",
    "esp_riscv_rt",
    "esp_metadata",
    "esp_println",
    "esp_backtrace",
    "esp_bootloader_esp_idf",
    "esp_sync",
    "esp_rom_sys",
    "esp32s31",
    "esp32c5",
    "esp32s31_pac",
    "esp32c5_pac",
];

/// Package names as written in a manifest, and the repositories they come from.
const PACKAGES: [&str; 10] = [
    "esp-hal",
    "esp-riscv-rt",
    "esp-metadata",
    "esp-println",
    "esp-backtrace",
    "esp-bootloader-esp-idf",
    "esp-sync",
    "esp-rom-sys",
    "esp32s31",
    "esp32c5",
];
const REPOSITORIES: [&str; 2] = ["ermacv/esp-hal", "ermacv/esp-pacs"];

/// Generated or foreign trees are not scanned.
const SKIPPED: [&str; 5] = ["target", ".git", "pac-raw", "published", "facts"];

fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Counts `name::` path starts of the listed crates in one line of code.
fn rust_uses(line: &str) -> usize {
    let code = line.split("//").next().unwrap_or("");
    let bytes = code.as_bytes();
    let mut count = 0;
    for name in RUST_NAMES {
        for (start, _) in code.match_indices(name) {
            let before = start.checked_sub(1).and_then(|index| bytes.get(index));
            let after = start + name.len();
            let rest = code.get(after..).unwrap_or("").trim_start();
            let starts_path = before.is_none_or(|byte| !is_identifier(*byte) && *byte != b':');
            if starts_path && rest.starts_with("::") {
                count += 1;
            }
        }
    }
    count
}

fn dependency_uses(name: &str, spec: &toml::Value) -> usize {
    let package = spec
        .get("package")
        .and_then(toml::Value::as_str)
        .unwrap_or(name);
    let from_fork = spec
        .get("git")
        .and_then(toml::Value::as_str)
        .is_some_and(|url| {
            REPOSITORIES
                .iter()
                .any(|repository| url.contains(repository))
        });
    usize::from(PACKAGES.contains(&package) || from_fork)
}

fn manifest_uses(value: &toml::Value) -> usize {
    let Some(table) = value.as_table() else {
        return 0;
    };
    let mut count = 0;
    for (key, child) in table {
        match (key.as_str(), child.as_table()) {
            ("dependencies" | "dev-dependencies" | "build-dependencies", Some(dependencies)) => {
                count += dependencies
                    .iter()
                    .map(|(name, spec)| dependency_uses(name, spec))
                    .sum::<usize>();
            }
            ("patch", Some(sources)) => {
                for patches in sources.values().filter_map(toml::Value::as_table) {
                    count += patches
                        .iter()
                        .map(|(name, spec)| dependency_uses(name, spec))
                        .sum::<usize>();
                }
            }
            _ => count += manifest_uses(child),
        }
    }
    count
}

fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source tree is readable") {
        let path = entry.expect("directory entry is readable").path();
        let name = path.file_name().map(|name| name.to_string_lossy());
        let name = name.as_deref().unwrap_or("");
        if path.is_dir() {
            if !SKIPPED.contains(&name) {
                visit(&path, files);
            }
        } else if name == "Cargo.toml" || name.ends_with(".rs") {
            files.push(path);
        }
    }
}

/// Uses per file below `root`, only files that have any.
fn scan(root: &Path) -> BTreeMap<String, usize> {
    let mut files = Vec::new();
    visit(root, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let count = if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                manifest_uses(&toml::from_str::<toml::Value>(&text).expect("manifest parses"))
            } else {
                text.lines().map(rust_uses).sum()
            };
            let relative = path.strip_prefix(root).expect("file is below root");
            (count != 0).then(|| (relative.to_string_lossy().into_owned(), count))
        })
        .collect()
}

fn parse_allowlist(text: &str) -> BTreeMap<String, usize> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (path, count) = line.rsplit_once(' ').expect("a line is `<path> <count>`");
            (
                path.to_owned(),
                count.parse().expect("the count is a number"),
            )
        })
        .collect()
}

/// The lines that differ between what the tree uses and what is allowed.
fn differences(actual: &BTreeMap<String, usize>, allowed: &BTreeMap<String, usize>) -> Vec<String> {
    let mut lines = Vec::new();
    for (path, count) in actual {
        match allowed.get(path) {
            None => lines.push(format!("new: {path} has {count} use(s), none allowed")),
            Some(limit) if count > limit => {
                lines.push(format!("new: {path} has {count} use(s), {limit} allowed"));
            }
            Some(limit) if count < limit => {
                lines.push(format!("stale: {path} has {count} use(s), {limit} listed"));
            }
            Some(_) => {}
        }
    }
    for path in allowed.keys().filter(|path| !actual.contains_key(*path)) {
        lines.push(format!("stale: {path} no longer uses them"));
    }
    lines
}

#[test]
fn the_tree_uses_esp_hal_and_esp_pacs_only_where_listed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let allowed = parse_allowlist(include_str!("platform_crates.allowlist"));
    let found = differences(&scan(&root), &allowed);
    assert!(
        found.is_empty(),
        "esp-hal and esp-pacs go away; do not add uses, and drop listed lines you removed:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_new_rust_use_outside_the_list_fails() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("src");
    std::fs::create_dir(&source).expect("source directory");
    std::fs::write(
        source.join("lib.rs"),
        "// esp_hal::ignored\nfn f() { let _ = esp_hal::init(); let _ = oer::esp32s31::X; }\n",
    )
    .expect("source file");
    let actual = scan(directory.path());
    assert_eq!(actual.get("src/lib.rs"), Some(&1));
    assert_eq!(differences(&actual, &BTreeMap::new()).len(), 1);
    let allowed = parse_allowlist("src/lib.rs 1\n");
    assert!(differences(&actual, &allowed).is_empty());
}

#[test]
fn a_new_dependency_outside_the_list_fails() {
    let directory = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        directory.path().join("Cargo.toml"),
        "[features]\nesp32s31 = []\n[dependencies]\nesp-hal = { workspace = true }\n\
         pac = { package = \"esp32c5\", version = \"1\" }\n\
         other = { git = \"https://github.com/ermacv/esp-pacs\" }\nserde = \"1\"\n",
    )
    .expect("manifest");
    let actual = scan(directory.path());
    assert_eq!(actual.get("Cargo.toml"), Some(&3));
    assert!(!differences(&actual, &BTreeMap::new()).is_empty());
}

#[test]
fn a_listed_place_that_is_gone_fails() {
    let allowed = parse_allowlist("src/gone.rs 2\n");
    assert_eq!(differences(&BTreeMap::new(), &allowed).len(), 1);
}
