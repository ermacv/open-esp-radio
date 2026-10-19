//! Package closure shared by build provenance and applicability evaluation.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The workspace manifest a captured package manifest at `path` inherits
/// from, as Cargo finds it: the nearest captured manifest at or above the
/// package that declares a `[workspace]`, the root workspace otherwise. A
/// path dependency on a package of another workspace (an excluded target
/// workspace's crate a host tool reads) inherits from that workspace.
pub fn workspace_of<'a>(
    manifests: &'a BTreeMap<PathBuf, Value>,
    path: &Path,
) -> Option<(&'a PathBuf, &'a Value)> {
    path.ancestors()
        .skip(1)
        .find_map(|directory| {
            manifests
                .get_key_value(&directory.join("Cargo.toml"))
                .filter(|(_, manifest)| manifest.get("workspace").is_some())
        })
        .or_else(|| manifests.get_key_value(Path::new("Cargo.toml")))
}

pub fn projection(
    lock: &Value,
    manifests: &BTreeMap<PathBuf, Value>,
    roots: &[String],
) -> Result<Value> {
    let workspace = manifests
        .get(Path::new("Cargo.toml"))
        .ok_or("captured workspace manifest is missing")?;
    // Workspaces other than the root one that a selected package inherits
    // from, by manifest path.
    let mut other_workspaces: BTreeMap<PathBuf, Value> = BTreeMap::new();
    let packages = lock["package"]
        .as_array()
        .ok_or("lockfile has no package graph")?;
    let mut local: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for (path, manifest) in manifests {
        if let Some(name) = manifest["package"]["name"].as_str() {
            local.entry(name).or_default().push((path, manifest));
        }
    }
    let mut pending = Vec::new();
    for root in roots {
        let candidates = packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p["name"] == *root && p.get("source").is_none())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let [index] = candidates.as_slice() else {
            return Err(format!(
                "dependency root {root} is absent or ambiguous in the effective lockfile"
            )
            .into());
        };
        pending.push(*index);
    }
    let mut seen = BTreeSet::new();
    let mut bound_manifests = BTreeMap::new();
    let mut selected = BTreeMap::new();
    let mut dependency_names = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if !seen.insert(index) {
            continue;
        }
        let package = &packages[index];
        let name = package["name"]
            .as_str()
            .ok_or("locked package has no name")?;
        dependency_names.insert(name.to_owned());
        let allowed = if package.get("source").is_none() {
            let candidates = local
                .get(name)
                .ok_or_else(|| format!("local dependency {name} has no captured manifest"))?;
            let [(path, manifest)] = candidates.as_slice() else {
                return Err(
                    format!("local dependency {name} has ambiguous captured manifests").into(),
                );
            };
            let mut manifest = (*manifest).clone();
            remove_dev_inputs(&mut manifest);
            let (inherited_path, inherited) =
                workspace_of(manifests, path).ok_or("captured workspace manifest is missing")?;
            let allowed = dependency_names_in(&manifest, inherited)?;
            if inherited_path != Path::new("Cargo.toml") {
                other_workspaces.insert(inherited_path.clone(), inherited.clone());
            }
            bound_manifests.insert((*path).clone(), manifest);
            Some(allowed)
        } else {
            None
        };
        let mut dependencies = Vec::new();
        for dependency in package
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = dependency.as_str().ok_or("invalid locked dependency")?;
            let parts = value.splitn(3, ' ').collect::<Vec<_>>();
            if allowed
                .as_ref()
                .is_some_and(|names| !names.contains(parts[0]))
            {
                continue;
            }
            let candidates = packages
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p["name"] == parts[0]
                        && parts.get(1).is_none_or(|v| p["version"] == *v)
                        && parts.get(2).is_none_or(|v| {
                            p["source"].as_str() == Some(v.trim_matches(['(', ')']))
                        })
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let [index] = candidates.as_slice() else {
                return Err(
                    format!("locked dependency {value} cannot be resolved uniquely").into(),
                );
            };
            pending.push(*index);
            dependencies.push(value.to_owned());
        }
        dependencies.sort();
        let mut bound = package.clone();
        bound["dependencies"] = json!(dependencies);
        selected.insert(
            format!("{}@{}:{}", name, package["version"], package["source"]),
            bound,
        );
    }
    let bound_workspace = |workspace: &Value| {
        let mut workspace = workspace.clone();
        if let Some(table) = workspace
            .get_mut("workspace")
            .and_then(Value::as_object_mut)
        {
            table.remove("members");
            table.remove("default-members");
            table.remove("exclude");
            if let Some(deps) = table.get_mut("dependencies").and_then(Value::as_object_mut) {
                deps.retain(|name, value| {
                    dependency_names
                        .contains(value.get("package").and_then(Value::as_str).unwrap_or(name))
                });
            }
        }
        remove_dev_inputs(&mut workspace);
        workspace
    };
    let mut projection = json!({
        "packages": selected,
        "workspace": bound_workspace(workspace),
        "manifests": bound_manifests,
    });
    // Only a closure that reaches another workspace records it, so every
    // other projection keeps its shape and digest.
    if !other_workspaces.is_empty() {
        projection["workspaces"] = json!(
            other_workspaces
                .iter()
                .map(|(path, workspace)| (path.display().to_string(), bound_workspace(workspace)))
                .collect::<BTreeMap<_, _>>()
        );
    }
    Ok(projection)
}
fn remove_dev_inputs(manifest: &mut Value) {
    if let Some(object) = manifest.as_object_mut() {
        for key in ["dev-dependencies", "test", "bench", "example"] {
            object.remove(key);
        }
        if let Some(targets) = object.get_mut("target").and_then(Value::as_object_mut) {
            for target in targets.values_mut() {
                if let Some(target) = target.as_object_mut() {
                    target.remove("dev-dependencies");
                }
            }
        }
    }
}
/// Resolve normal/build dependency names, including aliases and workspace inheritance.
pub fn dependency_names_in(manifest: &Value, workspace: &Value) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let scopes = std::iter::once(manifest).chain(
        manifest
            .get("target")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|t| t.values()),
    );
    for scope in scopes {
        for kind in ["dependencies", "build-dependencies"] {
            for (alias, dependency) in scope
                .get(kind)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let dependency =
                    if dependency.get("workspace").and_then(Value::as_bool) == Some(true) {
                        workspace
                            .pointer(&format!("/workspace/dependencies/{alias}"))
                            .ok_or("inherited dependency is absent")?
                    } else {
                        dependency
                    };
                names.insert(
                    dependency
                        .get("package")
                        .and_then(Value::as_str)
                        .unwrap_or(alias)
                        .to_owned(),
                );
            }
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host package depending on a crate of an excluded target workspace,
    /// whose dependencies that workspace declares.
    fn graph() -> (Value, BTreeMap<PathBuf, Value>) {
        let lock = json!({"package": [
            {"name": "host", "version": "0.1.0", "dependencies": ["target-crate"]},
            {"name": "target-crate", "version": "0.1.0", "dependencies": ["postcard"]},
            {"name": "postcard", "version": "1.1.3",
             "source": "registry+https://github.com/rust-lang/crates.io-index"},
        ]});
        let manifests = BTreeMap::from([
            (
                PathBuf::from("Cargo.toml"),
                json!({"workspace": {"members": ["host"], "dependencies": {}}}),
            ),
            (
                PathBuf::from("host/Cargo.toml"),
                json!({"package": {"name": "host"},
                       "dependencies": {"target-crate": {"path": "../targets/target-crate"}}}),
            ),
            (
                PathBuf::from("targets/Cargo.toml"),
                json!({"workspace": {"members": ["target-crate"],
                       "dependencies": {"postcard": {"version": "1.1.3"}, "unused": "1"}}}),
            ),
            (
                PathBuf::from("targets/target-crate/Cargo.toml"),
                json!({"package": {"name": "target-crate"},
                       "dependencies": {"postcard": {"workspace": true}}}),
            ),
        ]);
        (lock, manifests)
    }

    #[test]
    fn a_crate_of_another_workspace_inherits_from_that_workspace() {
        let (lock, manifests) = graph();
        let (path, _) =
            workspace_of(&manifests, Path::new("targets/target-crate/Cargo.toml")).unwrap();
        assert_eq!(path, Path::new("targets/Cargo.toml"));
        let (path, _) = workspace_of(&manifests, Path::new("host/Cargo.toml")).unwrap();
        assert_eq!(path, Path::new("Cargo.toml"));
        let projection = projection(&lock, &manifests, &["host".to_owned()]).unwrap();
        assert!(
            projection["packages"]
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.starts_with("postcard@"))
        );
        // The foreign workspace is bound, trimmed to what the closure uses.
        let bound = &projection["workspaces"]["targets/Cargo.toml"]["workspace"];
        assert!(bound["dependencies"].get("postcard").is_some());
        assert!(bound["dependencies"].get("unused").is_none());
        assert!(bound.get("members").is_none());
    }

    #[test]
    fn a_closure_inside_the_root_workspace_records_no_other_workspace() {
        let (lock, mut manifests) = graph();
        manifests.insert(
            PathBuf::from("host/Cargo.toml"),
            json!({"package": {"name": "host"}}),
        );
        let projection = projection(&lock, &manifests, &["host".to_owned()]).unwrap();
        assert!(projection.get("workspaces").is_none());
    }
}
