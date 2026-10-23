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
