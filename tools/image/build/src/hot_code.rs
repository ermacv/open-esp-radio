//! The hot-code list an image declares:
//! `[package.metadata.open-radio] hot-code = "<file>"`, a path relative to
//! its package directory. The image linker moves the listed functions into
//! SRAM ([`oer_image_checks::linker::hot`]).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::Result;

const KEY: &str = "hot-code";

/// The hot-code list `package`, in the workspace at `root`, declares.
///
/// # Errors
///
/// `cargo metadata` fails, the workspace has no `package`, or its
/// declaration is not a path string.
pub fn declared(root: &Path, package: &str) -> Result<Option<PathBuf>> {
    of_metadata(&crate::manifest::workspace_metadata(root)?, package)
}

fn of_metadata(metadata: &Value, package: &str) -> Result<Option<PathBuf>> {
    match crate::manifest::declared(metadata, package, KEY)? {
        None => Ok(None),
        Some(Value::String(file)) => {
            let manifest = metadata
                .get("packages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(package))
                .and_then(|entry| entry.get("manifest_path"))
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{package}: cargo metadata names no manifest path"))?;
            let directory = Path::new(manifest)
                .parent()
                .ok_or_else(|| format!("{package}: its manifest path has no directory"))?;
            Ok(Some(directory.join(file)))
        }
        Some(other) => {
            Err(format!("{package}: `{KEY}` is a path relative to the package, not {other}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(declared: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "other", "metadata": null, "manifest_path": "/w/other/Cargo.toml" },
            {
                "name": "image",
                "manifest_path": "/w/images/image/Cargo.toml",
                "metadata": { "open-radio": { "hot-code": declared } }
            },
        ] })
    }

    #[test]
    fn a_declared_list_resolves_beside_its_manifest_and_nothing_declared_is_none() {
        assert_eq!(
            of_metadata(&metadata(serde_json::json!("hot-code.toml")), "image").unwrap(),
            Some(PathBuf::from("/w/images/image/hot-code.toml"))
        );
        assert_eq!(of_metadata(&metadata(Value::Null), "image").unwrap(), None);
        assert!(of_metadata(&metadata(serde_json::json!(3)), "image").is_err());
        assert!(of_metadata(&metadata(Value::Null), "missing").is_err());
    }
}
