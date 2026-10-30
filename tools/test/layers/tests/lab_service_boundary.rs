//! The stand's clients reach its hardware only through the LAB service:
//! `cargo fw`, `cargo hil`, `cargo stand` and the HIL harness build against
//! its protocol and client (`oer-lab-client`), never against the service,
//! the board drivers or the router's SSH, directly or through another crate.
use std::collections::{BTreeMap, BTreeSet};

/// The service's implementation and the drivers only it runs.
const IMPLEMENTATION: [&str; 3] = ["oer-tool-lab-service", "oer-lab-board", "oer-lab-router"];
/// The service's clients.
const CLIENTS: [&str; 5] = [
    "oer-lab-client",
    "oer-hil-harness",
    "oer-tool-hil-cli",
    "oer-tool-image-cli",
    "oer-tool-lab-cli",
];

/// Every package of `Cargo.lock` with the names of its dependencies.
fn lock() -> BTreeMap<String, Vec<String>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let text = std::fs::read_to_string(root.join("Cargo.lock")).expect("Cargo.lock is readable");
    let lock: toml::Value = toml::from_str(&text).expect("Cargo.lock is TOML");
    lock.get("package")
        .and_then(toml::Value::as_array)
        .expect("Cargo.lock lists packages")
        .iter()
        .map(|package| {
            let name = package["name"].as_str().expect("a package has a name");
            let dependencies = package
                .get("dependencies")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                // `name version` when two versions are locked.
                .map(|dependency| {
                    dependency
                        .split(' ')
                        .next()
                        .unwrap_or(dependency)
                        .to_owned()
                })
                .collect();
            (name.to_owned(), dependencies)
        })
        .collect()
}

/// `package` and every package it reaches.
fn closure(lock: &BTreeMap<String, Vec<String>>, package: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([package.to_owned()]);
    let mut pending = vec![package.to_owned()];
    while let Some(next) = pending.pop() {
        for dependency in lock.get(&next).into_iter().flatten() {
            if seen.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    seen
}

#[test]
fn clients_reach_the_hardware_only_through_the_lab_service() {
    let lock = lock();
    for client in CLIENTS {
        assert!(lock.contains_key(client), "{client} is not in Cargo.lock");
        let reached = closure(&lock, client);
        let forbidden = IMPLEMENTATION
            .iter()
            .filter(|implementation| reached.contains(**implementation))
            .collect::<Vec<_>>();
        assert!(
            forbidden.is_empty(),
            "{client} builds against {forbidden:?}: use oer-lab-client and a service operation"
        );
    }
    assert!(closure(&lock, "oer-tool-lab-service").contains("oer-lab-board"));
}
