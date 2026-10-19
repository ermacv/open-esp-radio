//! Host build inputs shared by the runner producer and its readers.
use super::cargo_inputs;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RUNNER: &str = "oer-tool-hil-runner";

/// The packages of a resolved graph ([`super::BUILD_SCHEMA`]), the runner
/// first.
pub fn graph(resolved: &Value) -> Result<&Vec<Value>> {
    resolved["packages"]
        .as_array()
        .filter(|packages| !packages.is_empty())
        .ok_or_else(|| "observer resolved graph missing".into())
}

/// The indices of the packages `node` depends on, each within a graph of
/// `len` packages.
fn edges_of(node: &Value, len: usize) -> Result<Vec<usize>> {
    node["edges"]
        .as_array()
        .ok_or("observer graph edges missing")?
        .iter()
        .map(|edge| {
            edge.as_u64()
                .and_then(|edge| usize::try_from(edge).ok())
                .filter(|edge| *edge > 0 && *edge < len)
                .ok_or_else(|| "invalid observer graph edge".into())
        })
        .collect()
}

/// Project Cargo's resolved normal/build graph, retaining shared feature unification.
/// The registry assigns direct dependencies to the mechanisms that use them.
pub fn projection(resolved: &Value, dependencies: &BTreeSet<String>) -> Result<Value> {
    let graph = graph(resolved)?;
    let keys = graph
        .iter()
        .map(|node| {
            let package = &node["package"];
            Ok(format!(
                "{} {}{}",
                package["name"]
                    .as_str()
                    .ok_or("missing observer package name")?,
                package["version"]
                    .as_str()
                    .ok_or("missing package version")?,
                package["source"]
                    .as_str()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default()
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    // The runner and every package reachable from its selected direct
    // dependencies. A package Cargo lists as several nodes (a normal and a
    // build dependency with different features) is one package with the
    // edges of every node reached. Visiting each node once in pre-order
    // lists edges in the order of their first appearance in Cargo's tree.
    fn visit(
        graph: &[Value],
        keys: &[String],
        dependencies: &BTreeSet<String>,
        index: usize,
        visited: &mut [bool],
        packages: &mut BTreeMap<String, Value>,
    ) -> Result<()> {
        if visited[index] {
            return Ok(());
        }
        visited[index] = true;
        let node = &graph[index];
        packages.entry(keys[index].clone()).or_insert_with(|| {
            let mut package = node["package"].clone();
            package["dependencies"] = json!([]);
            package["features"] = node["features"].clone();
            package["script_flags"] = node["script_flags"].clone();
            package["unit_profiles"] = json!(
                node["units"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|u| json!({"kind":u["kind"],"profile":u["profile"]}))
                    .collect::<Vec<_>>()
            );
            package
        });
        for edge in edges_of(node, graph.len())? {
            if index == 0
                && !dependencies.contains(
                    graph[edge]["package"]["name"]
                        .as_str()
                        .ok_or("missing observer package name")?,
                )
            {
                continue;
            }
            let edges = packages
                .get_mut(&keys[index])
                .ok_or("observer graph parent missing")?["dependencies"]
                .as_array_mut()
                .ok_or("missing dependencies")?;
            if !edges.contains(&json!(keys[edge])) {
                edges.push(json!(keys[edge]));
            }
            visit(graph, keys, dependencies, edge, visited, packages)?;
        }
        Ok(())
    }
    let mut packages = BTreeMap::<String, Value>::new();
    visit(
        graph,
        &keys,
        dependencies,
        0,
        &mut vec![false; graph.len()],
        &mut packages,
    )?;
    let mut manifests: BTreeMap<PathBuf, Value> =
        serde_json::from_value(resolved["manifests"].clone())?;
    let runner = manifests
        .get_mut(Path::new("tools/hil-runner/Cargo.toml"))
        .ok_or("runner manifest missing")?;
    for kind in ["dependencies", "build-dependencies"] {
        if let Some(table) = runner[kind].as_object_mut() {
            table.retain(|alias, dep| {
                dependencies.contains(dep["package"].as_str().unwrap_or(alias))
            });
        }
    }
    // Helper binary declarations are not inputs to the runner executable.
    runner
        .as_object_mut()
        .ok_or("invalid runner manifest")?
        .remove("bin");
    cargo_inputs::projection(
        &json!({"package":packages.into_values().collect::<Vec<_>>()}),
        &manifests,
        &[RUNNER.into()],
    )
}

/// Version of `host/hil/model/observer-inputs.json`.
///
/// Schema 4 scopes observer inputs by scenario family: a workload's inputs
/// are the runner package, the path packages reachable from the common and
/// family dependency groups and the registry's non-Cargo `data` files. A
/// workload is identified as `<family>/<kind>`.
pub const REGISTRY_SCHEMA: u64 = 4;

/// Reject a registry of another schema before reading any scope from it.
pub fn check_registry_schema(registry: &Value) -> Result<()> {
    if registry["schema"] != REGISTRY_SCHEMA {
        return Err("unsupported observer input registry".into());
    }
    Ok(())
}

/// The `<family>/<kind>` identity of a schema-5 scenario document.
pub fn workload(document: &Value) -> Option<String> {
    ["system"].into_iter().find_map(|family| {
        let kind = document.get(family)?.get("kind")?;
        Some(format!("{family}/{}", kind.as_str()?))
    })
}

/// Every workload identity the registry classifies.
pub fn workloads(registry: &Value) -> Result<Vec<String>> {
    Ok(registry["timing"]
        .as_object()
        .ok_or("observer workload timing missing")?
        .keys()
        .cloned()
        .collect())
}

/// Registry scope for the selected workload; all direct dependencies must have an owner.
pub fn dependencies(registry: &Value, workload: &str) -> Result<BTreeSet<String>> {
    let groups = registry["dependencies"]
        .as_object()
        .ok_or("observer dependencies missing")?;
    let mut names = BTreeSet::new();
    let family = if workload.is_empty() {
        "common"
    } else {
        if registry["timing"].get(workload).is_none() {
            return Err(format!("observer workload {workload} is not classified").into());
        }
        workload
            .split_once('/')
            .map(|(family, _)| family)
            .ok_or("observer workload lacks a family")?
    };
    for group in ["common", family] {
        for name in groups
            .get(group)
            .and_then(Value::as_array)
            .ok_or("observer dependency group missing")?
        {
            names.insert(
                name.as_str()
                    .ok_or("invalid observer dependency name")?
                    .to_owned(),
            );
        }
    }
    Ok(names)
}

pub fn validate_registry(resolved: &Value, registry: &Value) -> Result<()> {
    let assigned = registry["dependencies"]
        .as_object()
        .ok_or("observer dependency scopes missing")?
        .values()
        .flat_map(|v| v.as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let graph = graph(resolved)?;
    for edge in edges_of(&graph[0], graph.len())? {
        let name = &graph[edge]["package"]["name"];
        if !assigned.contains(name.as_str().ok_or("package name missing")?) {
            return Err(format!("observer dependency {name} has no registered scope").into());
        }
    }
    Ok(())
}

/// The observer build configuration an evaluator requires on this host: the
/// current compiler and the registry's build profile and flags. A producer's
/// embedded configuration must equal it for its evidence to be current.
pub fn required_configuration(root: &Path, registry: &Value) -> Result<Value> {
    let output = oer_os_process::command("rustc")
        .current_dir(root)
        .arg("-vV")
        .output()?;
    if !output.status.success() {
        return Err("cannot identify required observer compiler".into());
    }
    let compiler = String::from_utf8(output.stdout)?;
    let target = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("compiler host missing")?;
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_else(|_| {
        std::env::var("RUSTFLAGS")
            .map(|flags| flags.split_whitespace().collect::<Vec<_>>().join("\u{1f}"))
            .unwrap_or_else(|_| {
                registry["build"]["rustflags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\u{1f}")
            })
    });
    Ok(json!({"compiler":compiler,"environment":{
        "TARGET":target,"PROFILE":registry["build"]["profile"],"OPT_LEVEL":registry["build"]["opt_level"],"DEBUG":registry["build"]["debug"],"CARGO_ENCODED_RUSTFLAGS":flags
    }}))
}

/// Profile differences are reviewed separately from dependency/feature differences.
pub fn take_unit_profiles(projection: &mut Value) -> Value {
    let mut profiles = BTreeMap::new();
    if let Some(packages) = projection["packages"].as_object_mut() {
        for (key, package) in packages {
            if let Some(profile) = package
                .as_object_mut()
                .and_then(|p| p.remove("unit_profiles"))
            {
                profiles.insert(key.clone(), profile);
            }
        }
    }
    json!(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_schema_and_workloads_come_from_the_family_map() {
        let registry = json!({
            "schema": 4,
            "timing": {"system/watchdog": true, "other/probe": true},
            "dependencies": {"common": ["core"], "system": ["system"], "other": ["ble"]},
        });
        check_registry_schema(&registry).unwrap();
        assert_eq!(
            workloads(&registry).unwrap(),
            ["other/probe", "system/watchdog"]
        );
        let watchdog = dependencies(&registry, "system/watchdog").unwrap();
        assert!(
            watchdog.contains("core") && watchdog.contains("system") && !watchdog.contains("ble")
        );
        assert!(dependencies(&registry, "system/boot-smoke").is_err());
        let mut legacy = registry;
        legacy["schema"] = json!(3);
        assert!(check_registry_schema(&legacy).is_err());
        // A table of no runner family names no workload.
        let document = json!({"bluetooth": {"kind": "dtm"}});
        assert_eq!(workload(&document), None);
        let document = json!({"system": {"kind": "boot-smoke"}});
        assert_eq!(workload(&document).as_deref(), Some("system/boot-smoke"));
    }

    fn fixture() -> Value {
        json!({"packages":[
            {"package":{"name":RUNNER,"version":"1"},"features":[],"edges":[1,2]},
            {"package":{"name":"wire","version":"1","source":"registry+test","checksum":"a"},"features":["std"],"edges":[]},
            {"package":{"name":"ble","version":"1","source":"registry+test","checksum":"b"},"features":[],"edges":[]}
        ],"manifests":{
            "Cargo.toml":{"workspace":{"members":["tools/hil-runner"]}},
            "tools/hil-runner/Cargo.toml":{"package":{"name":RUNNER,"version":"1"},"dependencies":{"wire":{"version":"1","features":["std"]},"ble":"1"},"dev-dependencies":{"test-only":"1"}}
        }})
    }

    /// A package shared by a selected and an unselected direct dependency is
    /// projected once with its own edges; one only an unselected dependency
    /// reaches is not projected; an edge outside the graph is an error.
    #[test]
    fn the_graph_projects_what_the_selected_dependencies_reach() {
        let mut resolved = fixture();
        resolved["packages"] = json!([
            {"package":{"name":RUNNER,"version":"1"},"features":[],"edges":[1,2]},
            {"package":{"name":"wire","version":"1","source":"registry+test","checksum":"a"},"features":["std"],"edges":[3]},
            {"package":{"name":"ble","version":"1","source":"registry+test","checksum":"b"},"features":[],"edges":[3,4,5]},
            {"package":{"name":"shared","version":"1","source":"registry+test","checksum":"c"},"features":["alloc"],"edges":[]},
            {"package":{"name":"ble-only","version":"1","source":"registry+test","checksum":"d"},"features":[],"edges":[]},
            // The same package as another of Cargo's nodes, with other
            // features and its own dependency.
            {"package":{"name":"shared","version":"1","source":"registry+test","checksum":"c"},"features":["alloc","std"],"edges":[6]},
            {"package":{"name":"std-only","version":"1","source":"registry+test","checksum":"e"},"features":[],"edges":[]}
        ]);
        let wire = projection(&resolved, &BTreeSet::from(["wire".into()]))
            .unwrap()
            .to_string();
        assert!(wire.contains("shared 1 (registry+test)"), "{wire}");
        assert!(!wire.contains("ble-only"), "{wire}");
        assert!(!wire.contains("std-only"), "{wire}");
        let both = projection(&resolved, &BTreeSet::from(["wire".into(), "ble".into()]))
            .unwrap()
            .to_string();
        assert!(both.contains("ble-only 1 (registry+test)"), "{both}");
        // Both of `shared`'s nodes are reached: one package with the edges
        // of both.
        assert!(both.contains("std-only 1 (registry+test)"), "{both}");
        assert_eq!(both.matches("\"name\":\"shared\"").count(), 1, "{both}");
        validate_registry(
            &resolved,
            &json!({"dependencies": {"common": ["wire", "ble"]}}),
        )
        .unwrap();
        assert!(
            validate_registry(&resolved, &json!({"dependencies": {"common": ["wire"]}})).is_err()
        );
        resolved["packages"][1]["edges"] = json!([9]);
        assert!(projection(&resolved, &BTreeSet::from(["wire".into()])).is_err());
        resolved["packages"][1]["edges"] = json!([0]);
        assert!(projection(&resolved, &BTreeSet::from(["wire".into()])).is_err());
    }

    /// The graph projects exactly what its `--no-dedupe` tree projected. The
    /// expected values are the tree projection's output (the reader removed
    /// in schema 3) for the tree of this graph: `wire` and `ble` share
    /// `shared`, which `ble` also reaches as a second Cargo node with more
    /// features and its own dependency.
    #[test]
    fn the_graph_projects_what_its_tree_projected() {
        let package = |name: &str, checksum: &str| json!({"name":name,"version":"1","source":"registry+test","checksum":checksum});
        let mut resolved = fixture();
        resolved["packages"] = json!([
            {"package":{"name":RUNNER,"version":"1"},"features":[],"edges":[1,3]},
            {"package":package("wire","a"),"features":["std"],"edges":[2]},
            {"package":package("shared","c"),"features":["alloc"],"edges":[]},
            {"package":package("ble","b"),"features":[],"edges":[2,4,5]},
            {"package":package("ble-only","d"),"features":[],"edges":[]},
            {"package":package("shared","c"),"features":["alloc","std"],"edges":[6]},
            {"package":package("std-only","e"),"features":[],"edges":[]}
        ]);
        let wire: Value = serde_json::from_str(
            r#"{"manifests":{"tools/hil-runner/Cargo.toml":{"build-dependencies":null,"dependencies":{"wire":{"features":["std"],"version":"1"}},"package":{"name":"oer-tool-hil-runner","version":"1"}}},"packages":{"oer-tool-hil-runner@\"1\":null":{"dependencies":["wire 1 (registry+test)"],"features":[],"name":"oer-tool-hil-runner","script_flags":null,"unit_profiles":[],"version":"1"},"shared@\"1\":\"registry+test\"":{"checksum":"c","dependencies":[],"features":["alloc"],"name":"shared","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"},"wire@\"1\":\"registry+test\"":{"checksum":"a","dependencies":["shared 1 (registry+test)"],"features":["std"],"name":"wire","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"}},"workspace":{"workspace":{}}}"#,
        )
        .unwrap();
        let both: Value = serde_json::from_str(
            r#"{"manifests":{"tools/hil-runner/Cargo.toml":{"build-dependencies":null,"dependencies":{"ble":"1","wire":{"features":["std"],"version":"1"}},"package":{"name":"oer-tool-hil-runner","version":"1"}}},"packages":{"ble-only@\"1\":\"registry+test\"":{"checksum":"d","dependencies":[],"features":[],"name":"ble-only","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"},"ble@\"1\":\"registry+test\"":{"checksum":"b","dependencies":["ble-only 1 (registry+test)","shared 1 (registry+test)"],"features":[],"name":"ble","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"},"oer-tool-hil-runner@\"1\":null":{"dependencies":["ble 1 (registry+test)","wire 1 (registry+test)"],"features":[],"name":"oer-tool-hil-runner","script_flags":null,"unit_profiles":[],"version":"1"},"shared@\"1\":\"registry+test\"":{"checksum":"c","dependencies":["std-only 1 (registry+test)"],"features":["alloc"],"name":"shared","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"},"std-only@\"1\":\"registry+test\"":{"checksum":"e","dependencies":[],"features":[],"name":"std-only","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"},"wire@\"1\":\"registry+test\"":{"checksum":"a","dependencies":["shared 1 (registry+test)"],"features":["std"],"name":"wire","script_flags":null,"source":"registry+test","unit_profiles":[],"version":"1"}},"workspace":{"workspace":{}}}"#,
        )
        .unwrap();
        assert_eq!(
            projection(&resolved, &BTreeSet::from(["wire".into()])).unwrap(),
            wire
        );
        assert_eq!(
            projection(&resolved, &BTreeSet::from(["wire".into(), "ble".into()])).unwrap(),
            both
        );
    }

    #[test]
    fn wifi_scope_ignores_ble_pins_but_preserves_features_and_dependency_manifests() {
        let mut resolved = fixture();
        let scope = BTreeSet::from(["wire".into()]);
        let expected = projection(&resolved, &scope).unwrap();
        resolved["packages"][2]["package"]["checksum"] = json!("new BLE package");
        resolved["manifests"]["tools/hil-runner/Cargo.toml"]["dependencies"]["ble"] = json!("2");
        resolved["manifests"]["tools/hil-runner/Cargo.toml"]["dev-dependencies"]["test-only"] =
            json!("2");
        assert_eq!(expected, projection(&resolved, &scope).unwrap());
        resolved["packages"][1]["features"] = json!(["std", "shared-feature-enabled-by-ble"]);
        assert_ne!(expected, projection(&resolved, &scope).unwrap());
        resolved["packages"][1]["features"] = json!(["std"]);
        resolved["manifests"]["tools/hil-runner/Cargo.toml"]["dependencies"]["wire"]["features"] =
            json!(["other"]);
        assert_ne!(expected, projection(&resolved, &scope).unwrap());
    }
}
