//! `linker/hot-code.x` selects functions by name: a rename would leave a
//! pattern matching nothing, and the code in PSRAM, without any error.

use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The `<length><identifier>` pieces of a mangled pattern, in order. A
/// length that does not fit its identifier is no piece.
fn identifiers(pattern: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in pattern.split('*') {
        let mut rest = word;
        loop {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let Ok(length) = rest[..digits].parse::<usize>() else {
                break;
            };
            let Some(identifier) = rest.get(digits..digits + length) else {
                break;
            };
            found.push(identifier.to_owned());
            rest = &rest[digits + length..];
        }
    }
    found
}

#[test]
fn every_hot_pattern_names_a_function_of_the_sources() {
    let root = repository();
    let list =
        std::fs::read_to_string(root.join("chips/esp32s31/platform/layout/linker/hot-code.x"))
            .unwrap();
    let mut files = Vec::new();
    for directory in ["chips", "protocols"] {
        sources(&root.join(directory), &mut files);
    }
    let code: Vec<String> = files
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .collect();
    let patterns: Vec<&str> = list
        .lines()
        .filter_map(|line| line.strip_prefix("*(.text."))
        .filter_map(|line| line.strip_suffix(')'))
        .collect();
    assert!(!patterns.is_empty());
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    for pattern in patterns {
        let names = identifiers(pattern);
        let (crate_name, _) = names.split_first().unwrap();
        assert!(
            crate_name.starts_with("oer_"),
            "`{pattern}` starts at a crate"
        );
        let package = format!("name = \"{}\"", crate_name.replace('_', "-"));
        assert!(
            lock.contains(&package),
            "`{pattern}` names `{crate_name}`, no package of the workspace"
        );
        for name in names.iter().rev().take(1) {
            let wanted = format!("fn {name}");
            assert!(
                code.iter().any(|source| source.contains(&wanted)),
                "`{pattern}` names `{name}`, no function of the sources"
            );
        }
    }
}

#[test]
fn a_length_that_does_not_fit_ends_the_identifiers() {
    assert_eq!(identifiers("*3abc2de*9short*"), ["abc", "de"]);
}
