//! Whether an image declares its task stacks proven:
//! `[package.metadata.open-radio] task-stack = "proven"`. The check then
//! fails the build on any unresolved path of a task stack's bound, where an
//! image that declares nothing passes it with a warning.
//!
//! An image that declares them proven may choose their reserve,
//! `task-stack-reserve-bytes` (zero included; absent, the stack policy's
//! `minimum_free_bytes`). Under a partial bound the reserve covers the
//! unresolved sites and stays the policy's: a declaration then fails.

use std::path::Path;

use serde_json::Value;

use crate::Result;

const KEY: &str = "task-stack";
const PROVEN: &str = "proven";
const RESERVE: &str = "task-stack-reserve-bytes";

/// What an image chose for its task stacks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskStackChoices {
    /// The bounds are declared proven.
    pub proven: bool,
    /// The reserve above a proven bound; `None` takes the policy's.
    pub reserve: Option<u32>,
}

/// Whether `package`, in the workspace at `root`, declares its task stacks
/// proven.
///
/// # Errors
///
/// `cargo metadata` fails, the workspace has no `package`, or its
/// declaration is malformed.
pub fn declared(root: &Path, package: &str) -> Result<TaskStackChoices> {
    of_metadata(&crate::manifest::workspace_metadata(root)?, package)
}

pub(crate) fn of_metadata(metadata: &Value, package: &str) -> Result<TaskStackChoices> {
    let proven = proven_of_metadata(metadata, package)?;
    let reserve = match crate::manifest::declared(metadata, package, RESERVE)? {
        None => None,
        Some(_) if !proven => {
            return Err(format!(
                "{package}: `{RESERVE}` needs `{KEY} = \"{PROVEN}\"`: under a partial bound the \
                 reserve covers the unresolved sites and stays the stack policy's"
            )
            .into());
        }
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|bytes| u32::try_from(bytes).ok())
                .ok_or_else(|| {
                    format!("{package}: `{RESERVE}` is a number of bytes, not {value}")
                })?,
        ),
    };
    Ok(TaskStackChoices { proven, reserve })
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

    fn image(open_radio: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "image", "metadata": { "open-radio": open_radio } },
        ] })
    }

    #[test]
    fn a_proven_image_chooses_its_reserve_zero_included_else_the_policy_s() {
        let proven = serde_json::json!({ "task-stack": "proven" });
        assert_eq!(
            of_metadata(&image(proven), "image").unwrap(),
            TaskStackChoices {
                proven: true,
                reserve: None
            }
        );
        for bytes in [0, 4096] {
            let choices = of_metadata(
                &image(serde_json::json!({ "task-stack": "proven", "task-stack-reserve-bytes": bytes })),
                "image",
            )
            .unwrap();
            assert_eq!(
                choices,
                TaskStackChoices {
                    proven: true,
                    reserve: Some(bytes)
                }
            );
        }
    }

    #[test]
    fn a_reserve_without_a_proven_bound_fails_naming_why() {
        let error = of_metadata(
            &image(serde_json::json!({ "task-stack-reserve-bytes": 0 })),
            "image",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("partial bound"), "{error}");
        let error = of_metadata(
            &image(serde_json::json!({ "task-stack": "proven", "task-stack-reserve-bytes": -1 })),
            "image",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains(RESERVE), "{error}");
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
        let packages: Vec<_> = [include_str!(
            "../../../../hil/probes/esp32s31-boot-smoke/Cargo.toml"
        )]
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
        assert!(proven("oer-hil-probes-esp32s31-boot-smoke"));
    }
}
