//! The image's critical-section hart, resolved against chip facts.

use std::path::Path;

use oer_chip_profile::Profile;
use serde_json::Value;

use crate::Result;

const KEY: &str = "critical-section-hart";

pub(crate) fn declared(root: &Path, package: &str, profile: &Profile) -> Result<u32> {
    of_metadata(
        &crate::manifest::workspace_metadata(root)?,
        package,
        profile,
    )
}

fn of_metadata(metadata: &Value, package: &str, profile: &Profile) -> Result<u32> {
    let interrupts = profile
        .facts
        .interrupts
        .as_ref()
        .ok_or("critical-section-hart needs the chip's hart facts")?;
    let hart = match crate::manifest::declared(metadata, package, KEY)? {
        Some(Value::String(role)) if role == "boot" => interrupts.boot_hart,
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|hart| u32::try_from(hart).ok())
            .ok_or_else(|| format!("{package}: `{KEY}` requires a hart ID or \"boot\""))?,
        _ => return Err(format!("{package}: declare `{KEY}` as a hart ID or \"boot\"").into()),
    };
    if !interrupts.harts.contains(&hart) {
        return Err(format!("{package}: `{KEY}` names hart {hart} outside the chip facts").into());
    }
    Ok(hart)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(value: Value) -> Value {
        serde_json::json!({"packages": [{"name": "image", "metadata": {
            "open-radio": {"critical-section-hart": value}
        }}]})
    }

    #[test]
    fn boot_role_comes_from_facts_and_explicit_ids_are_checked() {
        let mut profile = Profile::tracked("esp32s31").unwrap();
        let interrupts = profile.facts.interrupts.as_mut().unwrap();
        let hart = *interrupts.harts.last().unwrap();
        interrupts.boot_hart = hart;
        assert_eq!(
            of_metadata(&metadata("boot".into()), "image", &profile).unwrap(),
            hart
        );
        assert_eq!(
            of_metadata(&metadata(hart.into()), "image", &profile).unwrap(),
            hart
        );
        for value in [
            Value::Null,
            "BOOT".into(),
            true.into(),
            (-1).into(),
            u32::MAX.into(),
        ] {
            assert!(of_metadata(&metadata(value), "image", &profile).is_err());
        }
        assert!(of_metadata(&metadata("boot".into()), "missing", &profile).is_err());
    }

    #[test]
    fn every_staged_s31_image_declares_the_fact_derived_boot_role() {
        let profile = Profile::tracked("esp32s31").unwrap();
        let boot_hart = profile.facts.interrupts.as_ref().unwrap().boot_hart;
        for source in [
            include_str!("../../../../firmware/apps/b0/Cargo.toml"),
            include_str!("../../../../firmware/apps/boot-smoke/Cargo.toml"),
            include_str!("../../../../firmware/apps/radio-root/Cargo.toml"),
            include_str!("../../../../firmware/apps/w0/Cargo.toml"),
            include_str!("../../../../firmware/apps/w1/Cargo.toml"),
            include_str!("../../../../firmware/apps/w2/Cargo.toml"),
            include_str!("../../../../firmware/apps/w3/Cargo.toml"),
            include_str!("../../../../firmware/hil/esp32s31/Cargo.toml"),
            include_str!("../../../../firmware/hil/esp32s31-lower-mac/Cargo.toml"),
            include_str!("../../../../firmware/hil/esp32s31-phy/Cargo.toml"),
            include_str!("../../../../firmware/probes/esp32s31-radio/Cargo.toml"),
        ] {
            let manifest: toml::Value = toml::from_str(source).unwrap();
            let package = &manifest["package"];
            let name = package["name"].as_str().unwrap();
            assert_eq!(
                package["metadata"]["open-radio"][KEY].as_str(),
                Some("boot")
            );
            let metadata = serde_json::json!({"packages": [{
                "name": name, "metadata": package.get("metadata"),
            }]});
            assert_eq!(of_metadata(&metadata, name, &profile).unwrap(), boot_hart);
        }
    }
}
