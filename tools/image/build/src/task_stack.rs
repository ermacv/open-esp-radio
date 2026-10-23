//! Whether an image declares its task stacks proven:
//! `[package.metadata.open-radio] task-stack = "proven"`. The check then
//! fails the build on any unresolved path of a task stack's bound, where an
//! image that declares nothing passes it with a warning.

use std::path::Path;

use serde_json::Value;

use crate::Result;

const KEY: &str = "task-stack";
const PROVEN: &str = "proven";

/// Whether `package`, in the workspace at `root`, declares its task stacks
/// proven.
///
/// # Errors
///
/// `cargo metadata` fails, the workspace has no `package`, or its
/// declaration is malformed.
pub fn declared_proven(root: &Path, package: &str) -> Result<bool> {
    proven_of_metadata(&crate::manifest::workspace_metadata(root)?, package)
}

fn proven_of_metadata(metadata: &Value, package: &str) -> Result<bool> {
    match crate::manifest::declared(metadata, package, KEY)? {
        None => Ok(false),
        Some(Value::String(value)) if value == PROVEN => Ok(true),
        Some(other) => Err(format!(
            "{package}: `{KEY}` is the string \"{PROVEN}\" (the only value), not {other}"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(declared: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "other", "metadata": null },
            { "name": "image", "metadata": { "open-radio": { "task-stack": declared } } },
        ] })
    }

    #[test]
    fn proven_is_accepted_and_nothing_declared_is_not_proven() {
        assert!(proven_of_metadata(&metadata(serde_json::json!("proven")), "image").unwrap());
        assert!(!proven_of_metadata(&metadata(Value::Null), "image").unwrap());
        assert!(!proven_of_metadata(&metadata(Value::Null), "other").unwrap());
    }

    #[test]
    fn any_other_value_or_type_is_an_error() {
        for declared in [
            serde_json::json!("partial"),
            serde_json::json!("Proven"),
            serde_json::json!(true),
            serde_json::json!(["proven"]),
        ] {
            let error = proven_of_metadata(&metadata(declared), "image")
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("`task-stack` is the string \"proven\""),
                "{error}"
            );
        }
        let error = proven_of_metadata(&metadata(Value::Null), "missing")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no package missing"), "{error}");
    }

    #[test]
    fn the_workspace_images_declare_their_task_stacks() {
        let packages: Vec<_> = [
            include_str!("../../../../firmware/apps/w0/Cargo.toml"),
            include_str!("../../../../firmware/apps/boot-smoke/Cargo.toml"),
            include_str!("../../../../firmware/apps/radio-root/Cargo.toml"),
        ]
        .into_iter()
        .map(|text| {
            let manifest: toml::Value = toml::from_str(text).unwrap();
            let package = &manifest["package"];
            serde_json::json!({
                "name": package["name"].as_str().unwrap(),
                "metadata": package.get("metadata"),
            })
        })
        .collect();
        let metadata = serde_json::json!({"packages": packages});
        let proven = |package| proven_of_metadata(&metadata, package).unwrap();
        assert!(!proven("oer-firmware-apps-w0"));
        assert!(proven("oer-firmware-apps-boot-smoke"));
        assert!(proven("oer-firmware-apps-radio-root"));
    }
}
