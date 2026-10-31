//! The harts an image runs: `[package.metadata.open-radio] harts`, a list of
//! hart IDs or `"boot"`, resolved against the chip facts. An image that
//! declares nothing runs the boot hart alone. A declared secondary hart's
//! stacks come from its stack policy's launch template
//! (`StackPolicy::for_image`); every other template stays unused.

use std::path::Path;

use oer_chip_profile::Profile;
use serde_json::Value;

use crate::Result;

const KEY: &str = "harts";

/// The harts `package`, in the workspace at `root`, declares, ascending.
///
/// # Errors
///
/// `cargo metadata` fails, the workspace has no `package`, or its
/// declaration is malformed, names a hart outside the chip twice or omits the
/// boot hart.
pub(crate) fn declared(root: &Path, package: &str, profile: &Profile) -> Result<Vec<u32>> {
    of_metadata(
        &crate::manifest::workspace_metadata(root)?,
        package,
        profile,
    )
}

fn of_metadata(metadata: &Value, package: &str, profile: &Profile) -> Result<Vec<u32>> {
    let interrupts = profile
        .facts
        .interrupts
        .as_ref()
        .ok_or("`harts` needs the chip's hart facts")?;
    let boot = interrupts.boot_hart;
    let Some(declared) = crate::manifest::declared(metadata, package, KEY)? else {
        return Ok(vec![boot]);
    };
    let entries = declared
        .as_array()
        .ok_or_else(|| format!("{package}: `{KEY}` is a list of hart IDs or \"boot\""))?;
    let mut harts = Vec::new();
    for entry in entries {
        let hart = match entry {
            Value::String(role) if role == "boot" => boot,
            Value::Number(number) => number
                .as_u64()
                .and_then(|hart| u32::try_from(hart).ok())
                .ok_or_else(|| format!("{package}: `{KEY}` names an invalid hart {number}"))?,
            other => {
                return Err(
                    format!("{package}: `{KEY}` holds hart IDs or \"boot\", not {other}").into(),
                );
            }
        };
        if !interrupts.harts.contains(&hart) {
            return Err(
                format!("{package}: `{KEY}` names hart {hart} outside the chip facts").into(),
            );
        }
        if harts.contains(&hart) {
            return Err(format!("{package}: `{KEY}` names hart {hart} twice").into());
        }
        harts.push(hart);
    }
    if !harts.contains(&boot) {
        return Err(format!("{package}: `{KEY}` omits the boot hart").into());
    }
    harts.sort_unstable();
    Ok(harts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(value: Value) -> Value {
        serde_json::json!({"packages": [
            {"name": "image", "metadata": {"open-radio": {"harts": value}}},
            {"name": "plain", "metadata": null},
        ]})
    }

    #[test]
    fn an_image_runs_the_boot_hart_unless_it_declares_more() {
        let profile = Profile::tracked("esp32s31").unwrap();
        assert_eq!(
            of_metadata(&metadata(Value::Null), "plain", &profile).unwrap(),
            [0]
        );
        assert_eq!(
            of_metadata(&metadata(serde_json::json!(["boot"])), "image", &profile).unwrap(),
            [0]
        );
        assert_eq!(
            of_metadata(&metadata(serde_json::json!([1, "boot"])), "image", &profile).unwrap(),
            [0, 1]
        );
    }

    #[test]
    fn malformed_unknown_duplicate_or_bootless_declarations_are_refused() {
        let profile = Profile::tracked("esp32s31").unwrap();
        for value in [
            serde_json::json!("boot"),
            serde_json::json!([1]),
            serde_json::json!(["boot", 2]),
            serde_json::json!(["boot", 1, 1]),
            serde_json::json!(["boot", 0]),
            serde_json::json!(["boot", -1]),
            serde_json::json!(["boot", "secondary"]),
            serde_json::json!(["boot", true]),
        ] {
            assert!(
                of_metadata(&metadata(value.clone()), "image", &profile).is_err(),
                "{value}"
            );
        }
        assert!(of_metadata(&metadata(Value::Null), "missing", &profile).is_err());
    }

    #[test]
    fn the_boot_smoke_image_declares_hart_1() {
        let manifest: toml::Value = toml::from_str(include_str!(
            "../../../../firmware/apps/boot-smoke/Cargo.toml"
        ))
        .unwrap();
        let package = &manifest["package"];
        let metadata = serde_json::json!({"packages": [{
            "name": package["name"].as_str().unwrap(),
            "metadata": package.get("metadata"),
        }]});
        let profile = Profile::tracked("esp32s31").unwrap();
        assert_eq!(
            of_metadata(&metadata, "oer-firmware-apps-boot-smoke", &profile).unwrap(),
            [0, 1]
        );
    }
}
