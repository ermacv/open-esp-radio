//! Cargo's actual compilation units, bound to the executable that emitted its build record.
use serde_json::{Value, json};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The resolved graph supplies edges only. Features and unit profiles come
/// from Cargo artifacts.
pub fn apply(resolved: &mut Value, artifacts: &[Value]) -> Result<()> {
    for node in resolved["packages"]
        .as_array_mut()
        .ok_or("observer graph missing")?
    {
        let name = node["package"]["name"]
            .as_str()
            .ok_or("package name missing")?;
        let version = node["package"]["version"]
            .as_str()
            .ok_or("package version missing")?;
        let mut units = Vec::new();
        let mut script_flags = Vec::new();
        for artifact in artifacts {
            let id = artifact["package_id"]
                .as_str()
                .ok_or("Cargo package ID missing")?;
            let (location, package) = id.rsplit_once('#').ok_or("invalid Cargo package ID")?;
            let matches = package == format!("{name}@{version}")
                || (package == version && location.rsplit('/').next() == Some(name));
            if !matches {
                continue;
            }
            let source = node["package"]["source"].as_str();
            if source.is_some_and(|source| !source.starts_with(location)) {
                continue;
            }
            if artifact["reason"] == "build-script-executed" {
                let mut flags = json!({"cfgs":artifact["cfgs"],"env":artifact["env"],"linked_libs":artifact["linked_libs"],"linked_paths":artifact["linked_paths"]});
                normalize_output_directory(
                    &mut flags,
                    artifact["out_dir"]
                        .as_str()
                        .ok_or("build script output directory missing")?,
                );
                script_flags.push(flags);
                continue;
            }
            units.push(json!({"kind":artifact["target"]["kind"], "features":artifact["features"], "profile":artifact["profile"]}));
        }
        units.sort_by_key(|v| serde_json::to_string(v).unwrap());
        units.dedup();
        if units.is_empty() {
            return Err(
                format!("no compiled units for observer dependency {name} {version}").into(),
            );
        }
        // Keep unit distinctions: build and normal dependencies need not unify features.
        node["features"] = json!(
            units
                .iter()
                .map(|u| json!({"kind":u["kind"],"features":u["features"]}))
                .collect::<Vec<_>>()
        );
        node["units"] = json!(units);
        script_flags.sort_by_key(|v| serde_json::to_string(v).unwrap());
        script_flags.dedup();
        node["script_flags"] = json!(script_flags);
    }
    resolved["compilation"] = json!("cargo-compiler-artifacts-v1");
    Ok(())
}

fn normalize_output_directory(value: &mut Value, directory: &str) {
    match value {
        Value::String(text) => *text = text.replace(directory, "$OUT_DIR"),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|v| normalize_output_directory(v, directory)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|v| normalize_output_directory(v, directory)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_units_replace_tree_features_without_unifying_build_and_normal_units() {
        let mut graph = json!({"packages":[{"package":{"name":"dep","version":"1","source":"registry+index"},"features":["tree-approximation"]}]});
        let units = vec![
            json!({"package_id":"registry+index#dep@1","target":{"kind":["lib"]},"features":["actual"],"profile":{"test":false}}),
            json!({"package_id":"registry+index#dep@1","target":{"kind":["custom-build"]},"features":["build-only"],"profile":{"test":false}}),
        ];
        let mut units = units;
        units.push(json!({"reason":"build-script-executed","package_id":"registry+index#dep@1","out_dir":"/build/one","cfgs":["counter"],"env":[["TABLE","/build/one/table"]],"linked_libs":[],"linked_paths":[]}));
        apply(&mut graph, &units).unwrap();
        assert_eq!(
            graph["packages"][0]["script_flags"][0]["env"][0][1],
            "$OUT_DIR/table"
        );
        assert_eq!(graph["packages"][0]["units"].as_array().unwrap().len(), 2);
        assert!(!graph.to_string().contains("tree-approximation"));
        assert!(apply(&mut graph, &[]).is_err());
    }
}
