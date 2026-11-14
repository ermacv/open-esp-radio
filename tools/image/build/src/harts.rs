//! The harts an image runs: `[package.metadata.open-radio] harts`, a list of
//! hart IDs or `"boot"`, resolved against the chip facts. An image on the
//! application entry declares none: it runs the boot hart, and a second one
//! when it selects the entry's task `group` and the chip has one. Any other
//! image that declares nothing runs the boot hart alone. A declared secondary hart's
//! stacks come from its stack policy's launch template
//! (`StackPolicy::for_image`); every other template stays unused.

use std::path::Path;

use oer_tools_chip_profile::Profile;
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
pub(crate) fn declared(
    root: &Path,
    application: &crate::Application,
    profile: &Profile,
) -> Result<Vec<u32>> {
    let metadata = crate::manifest::workspace_metadata(root)?;
    let mut harts = of_metadata(&metadata, &application.package, profile)?;
    if harts.len() == 1
        && crate::manifest::declared(&metadata, &application.package, KEY)?.is_none()
        && selects_a_group(
            &metadata,
            &application.package,
            &application.features,
            application.default_features,
        )?
    {
        let interrupts = profile
            .facts
            .interrupts
            .as_ref()
            .ok_or("`harts` needs the chip's hart facts")?;
        if let Some(second) = interrupts
            .harts
            .iter()
            .copied()
            .find(|hart| *hart != interrupts.boot_hart)
        {
            harts.push(second);
            harts.sort_unstable();
        }
    }
    Ok(harts)
}

/// Whether the build's `features` of `package` (with its defaults when
/// `default_features`) reach the application entry's `group` feature
/// through the package's own features.
///
/// # Errors
///
/// The metadata has no such package or no feature table of it.
fn selects_a_group(
    metadata: &Value,
    package: &str,
    features: &[String],
    default_features: bool,
) -> Result<bool> {
    Ok(
        crate::manifest::enabled_features(metadata, package, features, default_features)?
            .iter()
            .any(|feature| ["oer/group", "oer?/group"].contains(&feature.as_str())),
    )
}

/// The harts `package` declares in the workspace `metadata`, as [`declared`].
pub(crate) fn of_metadata(metadata: &Value, package: &str, profile: &Profile) -> Result<Vec<u32>> {
    let interrupts = profile
        .facts
        .interrupts
        .as_ref()
        .ok_or("`harts` needs the chip's hart facts")?;
    let boot = interrupts.boot_hart;
    let Some(declared) = crate::manifest::declared(metadata, package, KEY)? else {
        let second = interrupts.harts.iter().copied().find(|hart| *hart != boot);
        return Ok(match second {
            Some(second) if takes_a_group(metadata, package) => {
                let mut harts = vec![boot, second];
                harts.sort_unstable();
                harts
            }
            _ => vec![boot],
        });
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

/// Whether `package` selects the application entry's task group (`oer`
/// with its `group` feature), which a chip with a second hart runs there.
fn takes_a_group(metadata: &Value, package: &str) -> bool {
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|candidate| candidate["name"].as_str() == Some(package))
        .flat_map(|candidate| candidate["dependencies"].as_array().into_iter().flatten())
        .any(|dependency| {
            dependency["name"].as_str() == Some("oer")
                && dependency["features"]
                    .as_array()
                    .is_some_and(|features| features.iter().any(|feature| feature == "group"))
        })
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
    fn an_image_with_the_entrys_task_group_runs_the_chips_second_hart() {
        let profile = Profile::tracked("esp32s31").unwrap();
        let metadata = |features: Value| {
            serde_json::json!({"packages": [{
                "name": "image",
                "metadata": null,
                "dependencies": [{"name": "oer", "features": features}],
            }]})
        };
        assert_eq!(
            of_metadata(
                &metadata(serde_json::json!(["group", "wifi"])),
                "image",
                &profile
            )
            .unwrap(),
            [0, 1]
        );
        assert_eq!(
            of_metadata(&metadata(serde_json::json!(["wifi"])), "image", &profile).unwrap(),
            [0]
        );
    }

    #[test]
    fn a_build_feature_that_reaches_the_entrys_group_selects_it() {
        let metadata = serde_json::json!({"packages": [{
            "name": "image",
            "features": {"station": ["oer/station", "oer/group"], "default": []},
        }]});
        assert!(selects_a_group(&metadata, "image", &["station".to_owned()], true).unwrap());
        assert!(!selects_a_group(&metadata, "image", &[], true).unwrap());
        // A package the metadata does not hold is an error, not "no group".
        let missing = selects_a_group(&metadata, "other", &["station".to_owned()], true);
        assert!(
            missing
                .unwrap_err()
                .to_string()
                .contains("no package other")
        );
        let tableless = serde_json::json!({"packages": [{"name": "image"}]});
        let tableless = selects_a_group(&tableless, "image", &[], true);
        assert!(
            tableless
                .unwrap_err()
                .to_string()
                .contains("no feature table")
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
            "../../../../hil/probes/esp32s31-boot-smoke/Cargo.toml"
        ))
        .unwrap();
        let package = &manifest["package"];
        let metadata = serde_json::json!({"packages": [{
            "name": package["name"].as_str().unwrap(),
            "metadata": package.get("metadata"),
        }]});
        let profile = Profile::tracked("esp32s31").unwrap();
        assert_eq!(
            of_metadata(&metadata, "oer-hil-probes-esp32s31-boot-smoke", &profile).unwrap(),
            [0, 1]
        );
    }
}
