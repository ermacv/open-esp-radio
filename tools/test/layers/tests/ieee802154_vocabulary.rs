//! The shared MAC dictionary stays below execution and protocol policy.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const VALUES: &str = "oer-espressif-ieee802154-mac-values";
const KNOWLEDGE: &str = "oer-esp32s31-knowledge";
const ENGINE: &str = "oer-espressif-ieee802154-engine";

fn normal_dependencies(value: &toml::Value, workspace: &toml::Value) -> BTreeSet<String> {
    let mut tables = vec![value];
    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        tables.extend(targets.values());
    }
    let mut dependencies = BTreeSet::new();
    for table in tables {
        let Some(entries) = table.get("dependencies").and_then(toml::Value::as_table) else {
            continue;
        };
        for (alias, spec) in entries {
            let spec = if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                &workspace[alias]
            } else {
                spec
            };
            dependencies.insert(
                spec.get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias)
                    .to_owned(),
            );
        }
    }
    dependencies
}

fn normal_closure(graph: &BTreeMap<String, BTreeSet<String>>, root: &str) -> BTreeSet<String> {
    let mut pending = vec![root.to_owned()];
    let mut found = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if found.insert(name.clone()) {
            if let Some(dependencies) = graph.get(&name) {
                pending.extend(dependencies.iter().cloned());
            }
        }
    }
    found
}

#[test]
fn the_dictionary_and_knowledge_do_not_reach_execution_and_consumers_state_the_license() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let workspace: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let mut manifests = BTreeMap::new();
    let mut graph = BTreeMap::new();
    for member in workspace["workspace"]["members"].as_array().unwrap() {
        let manifest: toml::Value = toml::from_str(
            &std::fs::read_to_string(root.join(member.as_str().unwrap()).join("Cargo.toml"))
                .unwrap(),
        )
        .unwrap();
        let name = manifest["package"]["name"].as_str().unwrap().to_owned();
        graph.insert(
            name.clone(),
            normal_dependencies(&manifest, &workspace["workspace"]["dependencies"]),
        );
        manifests.insert(name, manifest);
    }
    for owner in [VALUES, KNOWLEDGE] {
        let dependencies = normal_closure(&graph, owner);
        let forbidden = dependencies
            .iter()
            .filter(|name| {
                name.as_str() == ENGINE
                    || name.as_str() == "oer-esp32s31-hal"
                    || name.starts_with("oer-ieee802154-")
            })
            .collect::<Vec<_>>();
        assert!(forbidden.is_empty(), "{owner} reaches {forbidden:?}");
    }
    for (name, manifest) in &manifests {
        if normal_closure(&graph, name).contains(VALUES) {
            assert_eq!(
                manifest["package"]["license"].as_str(),
                Some("Apache-2.0"),
                "effective license of {name}"
            );
        }
    }
}

#[test]
fn normal_closure_follows_target_aliases_and_workspace_names_but_excludes_dev_edges() {
    let manifest: toml::Value = toml::from_str("[target.'cfg(test)'.dependencies]\nforward = { workspace = true }\n[dev-dependencies]\nignored = '1'\n[build-dependencies]\nalso_ignored = '1'\n[package.metadata.dependencies]\nnot_a_dependency = '1'\n").unwrap();
    let workspace: toml::Value =
        toml::from_str("forward = { package = 'bridge', version = '1' }\n").unwrap();
    let graph = BTreeMap::from([
        (
            VALUES.to_owned(),
            normal_dependencies(&manifest, &workspace),
        ),
        ("bridge".to_owned(), BTreeSet::from([ENGINE.to_owned()])),
    ]);
    let dependencies = normal_closure(&graph, VALUES);
    assert!(dependencies.contains(ENGINE));
    assert!(!dependencies.contains("ignored"));
    assert!(!dependencies.contains("also_ignored"));
    assert!(!dependencies.contains("not_a_dependency"));
}
