//! The shared MAC dictionary stays below execution; Apache-only dependencies
//! determine their consumers' effective licenses across the workspace.
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

fn apache_license_violations(
    graph: &BTreeMap<String, BTreeSet<String>>,
    licenses: &BTreeMap<String, String>,
) -> Vec<String> {
    let apache_only = licenses
        .iter()
        .filter(|(_, license)| license.as_str() == "Apache-2.0")
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    licenses
        .iter()
        .filter(|(name, license)| {
            license.as_str() != "Apache-2.0"
                && !normal_closure(graph, name).is_disjoint(&apache_only)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

#[test]
fn the_dictionary_stays_below_execution_and_apache_consumers_state_the_license() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let workspace: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let mut licenses = BTreeMap::new();
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
        let license = &manifest["package"]["license"];
        let license = if license.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
            &workspace["workspace"]["package"]["license"]
        } else {
            license
        };
        licenses.insert(name, license.as_str().unwrap().to_owned());
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
    assert_eq!(
        apache_license_violations(&graph, &licenses),
        Vec::<String>::new(),
        "packages whose effective license must be Apache-2.0"
    );
}

#[test]
fn apache_license_follows_normal_edges_and_requires_exact_apache() {
    let target: toml::Value = toml::from_str(
        "[target.'cfg(unix)'.dependencies]\nforward = { workspace = true, optional = true }\n",
    )
    .unwrap();
    let workspace: toml::Value =
        toml::from_str("forward = { package = 'apache-only', version = '1' }\n").unwrap();
    let dev_and_build: toml::Value = toml::from_str(
        "[dev-dependencies]\napache-only = '1'\n[build-dependencies]\napache-only = '1'\n",
    )
    .unwrap();
    let graph = BTreeMap::from([
        ("direct".into(), BTreeSet::from(["apache-only".into()])),
        ("transitive".into(), BTreeSet::from(["direct".into()])),
        ("accepted".into(), BTreeSet::from(["apache-only".into()])),
        ("combined".into(), BTreeSet::from(["apache-only".into()])),
        ("target".into(), normal_dependencies(&target, &workspace)),
        (
            "dev-and-build".into(),
            normal_dependencies(&dev_and_build, &workspace),
        ),
    ]);
    let licenses = [
        ("apache-only", "Apache-2.0"),
        ("direct", "MIT OR Apache-2.0"),
        ("transitive", "MIT"),
        ("accepted", "Apache-2.0"),
        ("combined", "Apache-2.0 AND MIT"),
        ("target", "MIT OR Apache-2.0"),
        ("dev-and-build", "MIT OR Apache-2.0"),
        ("unrelated", "MIT OR Apache-2.0"),
    ]
    .into_iter()
    .map(|(name, license)| (name.to_owned(), license.to_owned()))
    .collect();
    assert_eq!(
        apache_license_violations(&graph, &licenses),
        ["combined", "direct", "target", "transitive"]
    );
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
