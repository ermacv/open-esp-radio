//! The hot code an image takes, in `[package.metadata.open-radio]`:
//! `hot = N`, the highest level of `#[hot(N)]` functions the link places in
//! SRAM (`oer-portable-hot`); absent, none. The image linker writes the
//! fragment of those levels (`oer_tools_image_checks::linker::hot`).

use oer_tools_image_checks::linker::hot::Level;
use serde_json::Value;

use crate::Result;

const KEY: &str = "hot";

/// The highest hot level `package` takes in the workspace `metadata`.
///
/// # Errors
///
/// The workspace has no `package`, or `hot` is not a level the attribute
/// takes.
pub(crate) fn of_metadata(metadata: &Value, package: &str) -> Result<Option<Level>> {
    match crate::manifest::declared(metadata, package, KEY)? {
        None => Ok(None),
        Some(value) => {
            let level = value
                .as_u64()
                .and_then(|level| u8::try_from(level).ok())
                .ok_or_else(|| format!("{package}: `{KEY}` is a level, not {value}"))?;
            Ok(Some(
                Level::try_from(level).map_err(|error| format!("{package}: `{KEY}`: {error}"))?,
            ))
        }
    }
}

/// The highest hot level `package` takes, from the workspace at `root`.
pub(crate) fn declared(root: &std::path::Path, package: &str) -> Result<Option<Level>> {
    of_metadata(&crate::manifest::workspace_metadata(root)?, package)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(open_radio: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "image", "metadata": { "open-radio": open_radio } },
        ] })
    }

    #[test]
    fn an_image_names_its_highest_level_or_none() {
        assert_eq!(
            of_metadata(&metadata(serde_json::json!({})), "image").unwrap(),
            None
        );
        assert_eq!(
            of_metadata(&metadata(serde_json::json!({ "hot": 2 })), "image")
                .unwrap()
                .map(u8::from),
            Some(2)
        );
    }

    #[test]
    fn a_level_the_attribute_does_not_take_is_refused() {
        for value in [
            serde_json::json!(0),
            serde_json::json!(4),
            serde_json::json!(300),
            serde_json::json!(-1),
            serde_json::json!("1"),
        ] {
            let error = of_metadata(&metadata(serde_json::json!({ "hot": value })), "image")
                .unwrap_err()
                .to_string();
            assert!(error.starts_with("image: `hot`"), "{error}");
        }
    }
}
