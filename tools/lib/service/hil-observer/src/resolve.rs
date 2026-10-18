//! Producer-only Cargo graph resolution. Evaluators read prepared descriptors.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RUNNER: &str = "oer-hil-runner";

pub fn resolve(root: &Path, target: &str) -> Result<Value> {
    let output = Command::new(oer_toolchain::cargo_program())
        .current_dir(root)
        .args([
            "tree",
            "--locked",
            "-p",
            RUNNER,
            "--edges",
            "normal,build",
            "--prefix",
            "depth",
            // `CARGO_TERM_COLOR=always` would wrap the `(*)` marker in
            // escape sequences.
            "--color",
            "never",
            "--format",
            "{p}|{f}",
            "--target",
            target,
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "observer Cargo resolution failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let lock: Value = toml_edit::de::from_str(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let packages = lock["package"].as_array().ok_or("missing lock packages")?;
    // The graph: each of Cargo's nodes once, the runner first, with the
    // indices of the nodes it depends on in Cargo's order. Cargo prints a
    // node's dependencies under its first occurrence only and marks the
    // later ones `(*)`, so every edge appears once under its parent.
    let mut graph = Vec::<Value>::new();
    let mut index = BTreeMap::<String, usize>::new();
    let mut parents = Vec::<usize>::new();
    let mut manifests = BTreeMap::new();
    manifests.insert(
        PathBuf::from("Cargo.toml"),
        toml_edit::de::from_str::<Value>(&fs::read_to_string(root.join("Cargo.toml"))?)?,
    );
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let boundary = line
            .find(|c: char| !c.is_ascii_digit())
            .ok_or("invalid Cargo tree depth")?;
        let depth: usize = line[..boundary].parse()?;
        let (identity, features) = line[boundary..]
            .split_once('|')
            .ok_or("invalid Cargo tree node")?;
        let features = features.strip_suffix(" (*)").unwrap_or(features);
        let identity = identity.replace(" (proc-macro)", "");
        let mut words = identity.split_whitespace();
        let name = words.next().ok_or("missing package name")?;
        let version = words
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .ok_or("missing package version")?;
        let candidates = packages
            .iter()
            .filter(|p| {
                p["name"] == name
                    && p["version"] == version
                    && match identity
                        .split_once(" (")
                        .map(|(_, s)| s.trim_end_matches(')'))
                    {
                        None => p["source"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("registry+")),
                        Some(location) if location.starts_with('/') => p.get("source").is_none(),
                        Some(location) => p["source"].as_str().is_some_and(|s| {
                            s.strip_prefix("git+")
                                .is_some_and(|s| s.starts_with(location))
                        }),
                    }
            })
            .collect::<Vec<_>>();
        let [package] = candidates.as_slice() else {
            return Err(format!("ambiguous observer package {identity}").into());
        };
        if package.get("source").is_none() {
            let start = identity.find(" (").ok_or("local package has no path")?;
            let directory = Path::new(
                identity[start + 2..]
                    .strip_suffix(')')
                    .ok_or("invalid local package path")?,
            );
            let canonical_root = root.canonicalize()?;
            let manifest = directory.join("Cargo.toml");
            let relative = manifest.strip_prefix(&canonical_root)?.to_path_buf();
            manifests.insert(
                relative,
                toml_edit::de::from_str::<Value>(&fs::read_to_string(&manifest)?)?,
            );
            // A package of another workspace inherits from that workspace's
            // manifest, the nearest one above it that declares one.
            for ancestor in directory.ancestors().skip(1) {
                let Ok(relative) = ancestor.strip_prefix(&canonical_root) else {
                    break;
                };
                if relative.as_os_str().is_empty() {
                    break;
                }
                let candidate = ancestor.join("Cargo.toml");
                if !candidate.is_file() {
                    continue;
                }
                let value = toml_edit::de::from_str::<Value>(&fs::read_to_string(&candidate)?)?;
                if value.get("workspace").is_some() {
                    manifests.insert(relative.join("Cargo.toml"), value);
                    break;
                }
            }
        }
        let features = features
            .split(',')
            .filter(|f| !f.is_empty())
            .collect::<BTreeSet<_>>();
        // A node is Cargo's: a package with its features. A package that is
        // both a normal and a build dependency can be two nodes, each with
        // its own dependencies.
        let key = serde_json::to_string(&json!([
            package["name"],
            package["version"],
            package["source"],
            features
        ]))?;
        let node = match index.get(&key) {
            Some(&node) => node,
            None => {
                graph.push(json!({"package":package,"features":features,"edges":[]}));
                index.insert(key, graph.len() - 1);
                graph.len() - 1
            }
        };
        // One root, the runner, first; a node is at most one level below
        // the one before it.
        if depth > parents.len() || (depth == 0) != (node == 0) {
            return Err("invalid Cargo tree depth".into());
        }
        parents.truncate(depth);
        if let Some(&parent) = parents.last() {
            let edges = graph[parent]["edges"]
                .as_array_mut()
                .ok_or("observer edges missing")?;
            if !edges.contains(&json!(node)) {
                edges.push(json!(node));
            }
        }
        parents.push(node);
    }
    let config_path = root.join(".cargo/config.toml");
    let mut config: Value = if config_path.is_file() {
        toml_edit::de::from_str(&fs::read_to_string(config_path)?)?
    } else {
        json!({})
    };
    config
        .as_object_mut()
        .ok_or("invalid Cargo configuration")?
        .retain(|key, _| matches!(key.as_str(), "build" | "target" | "env"));
    if graph.is_empty() {
        return Err("observer Cargo resolution printed no packages".into());
    }
    Ok(json!({"packages":graph,"manifests":manifests,"cargo_config":config}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package two others depend on is one node with every edge to it,
    /// although Cargo lists it once in full and then marks it `(*)`.
    #[test]
    fn a_shared_package_is_one_node_with_every_edge() {
        let root =
            std::env::temp_dir().join(format!("oer-observer-resolve-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        let package = |name: &str, dependencies: &[&str]| {
            let directory = root.join(name);
            fs::create_dir_all(directory.join("src")).unwrap();
            let dependencies = dependencies
                .iter()
                .map(|d| format!("{d} = {{ path = \"../{d}\" }}\n"))
                .collect::<String>();
            fs::write(
                directory.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n{dependencies}"
                ),
            )
            .unwrap();
            fs::write(directory.join("src/lib.rs"), "").unwrap();
        };
        package("leaf", &[]);
        package("shared", &["leaf"]);
        package("left", &["shared"]);
        package("right", &["shared"]);
        package(RUNNER, &["left", "right"]);
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[workspace]\nresolver = \"3\"\nmembers = [\"{RUNNER}\", \"left\", \"right\", \"shared\", \"leaf\"]\n"
            ),
        )
        .unwrap();
        let status = Command::new(oer_toolchain::cargo_program())
            .current_dir(&root)
            .args(["generate-lockfile", "--offline"])
            .status()
            .unwrap();
        assert!(status.success());
        let compiler =
            String::from_utf8(Command::new("rustc").arg("-vV").output().unwrap().stdout).unwrap();
        let target = compiler
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        let resolved = resolve(&root, target).unwrap();
        let graph = resolved["packages"].as_array().unwrap();
        let names = graph
            .iter()
            .map(|node| node["package"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, [RUNNER, "left", "shared", "leaf", "right"]);
        let edges = |index: usize| graph[index]["edges"].clone();
        assert_eq!(edges(0), json!([1, 4]));
        assert_eq!(edges(1), json!([2]));
        assert_eq!(edges(2), json!([3]));
        assert_eq!(edges(3), json!([]));
        assert_eq!(edges(4), json!([2]), "the `(*)` occurrence keeps its edge");
        fs::remove_dir_all(root).unwrap();
    }
}
