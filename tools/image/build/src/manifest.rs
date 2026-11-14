//! What an application package declares under its
//! `[package.metadata.open-radio]`, read from `cargo metadata`.

use std::path::Path;

use serde_json::Value;

use crate::Result;

/// The workspace's `cargo metadata --format-version 1`.
pub(crate) fn workspace_metadata(root: &Path) -> Result<Value> {
    let output = crate::cargo::command()
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()
        .map_err(|error| format!("run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// `package`'s own feature table in `metadata`.
///
/// # Errors
///
/// The workspace has no such package, or its entry has no feature table
/// (`cargo metadata` gives every package one, empty or not).
fn feature_table<'a>(
    metadata: &'a Value,
    package: &str,
) -> Result<&'a serde_json::Map<String, Value>> {
    metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|candidate| candidate.get("name").and_then(Value::as_str) == Some(package))
        .ok_or_else(|| format!("the workspace has no package {package}"))?
        .get("features")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("the metadata of {package} has no feature table").into())
}

/// The features a build of `package` enables: `features`, with its
/// `default` when `default_features`, and every feature they name through
/// the package's own feature table, each once (a dependency's feature, such
/// as `oer/group`, is named and not followed).
///
/// # Errors
///
/// As [`feature_table`].
pub(crate) fn enabled_features(
    metadata: &Value,
    package: &str,
    features: &[String],
    default_features: bool,
) -> Result<std::collections::BTreeSet<String>> {
    let table = feature_table(metadata, package)?;
    let mut pending: Vec<String> = features.to_vec();
    if default_features {
        pending.push("default".to_owned());
    }
    let mut enabled = std::collections::BTreeSet::new();
    while let Some(feature) = pending.pop() {
        if !enabled.insert(feature.clone()) {
            continue;
        }
        pending.extend(
            table
                .get(&feature)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    Ok(enabled)
}

/// The names in `package`'s own feature table.
///
/// # Errors
///
/// As [`feature_table`].
pub(crate) fn package_features(
    metadata: &Value,
    package: &str,
) -> Result<std::collections::BTreeSet<String>> {
    Ok(feature_table(metadata, package)?.keys().cloned().collect())
}

/// What `package` declares under `key` in `metadata`; `None` when it
/// declares nothing.
pub(crate) fn declared<'a>(
    metadata: &'a Value,
    package: &str,
    key: &str,
) -> Result<Option<&'a Value>> {
    let entry = metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(package))
        .ok_or_else(|| format!("the workspace has no package {package}"))?;
    Ok(entry
        .get("metadata")
        .and_then(|metadata| metadata.get("open-radio"))
        .and_then(|open_radio| open_radio.get(key))
        .filter(|declared| !declared.is_null()))
}
