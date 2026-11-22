//! What an image's console speaks, in `[package.metadata.open-radio]`:
//! `console = "hil"`, the HIL protocol through the agent (`hil/agent`), the
//! verdict its scenario's; absent, text lines ending in `OER-RESULT:`.

use serde_json::Value;

use crate::Result;

const KEY: &str = "console";

/// What an image's console speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Console {
    /// Text lines, the verdict an `OER-RESULT:` line.
    Text,
    /// The HIL protocol: the verdict is its scenario's.
    Hil,
}

/// What `package`'s console speaks in the workspace `metadata`.
///
/// # Errors
///
/// The workspace has no `package`, or `console` names no protocol.
pub(crate) fn of_metadata(metadata: &Value, package: &str) -> Result<Console> {
    match crate::manifest::declared(metadata, package, KEY)? {
        None => Ok(Console::Text),
        Some(value) => match value.as_str() {
            Some("hil") => Ok(Console::Hil),
            _ => Err(format!("{package}: `{KEY}` is \"hil\" or absent, not {value}").into()),
        },
    }
}

/// What the console of `image` speaks, from the workspace at `root`.
///
/// # Errors
///
/// The image names no package, or its `console` names no protocol.
pub fn of_image(root: &std::path::Path, image: &str) -> Result<Console> {
    let package = crate::images::package(root, image)?;
    of_metadata(&crate::manifest::workspace_metadata(root)?, &package)
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
    fn an_image_speaks_text_unless_it_declares_the_hil_protocol() {
        assert_eq!(
            of_metadata(&metadata(serde_json::json!({})), "image").unwrap(),
            Console::Text
        );
        assert_eq!(
            of_metadata(&metadata(serde_json::json!({ "console": "hil" })), "image").unwrap(),
            Console::Hil
        );
        let error =
            of_metadata(&metadata(serde_json::json!({ "console": "rpc" })), "image").unwrap_err();
        assert!(error.to_string().contains("\"hil\" or absent"), "{error}");
    }
}
