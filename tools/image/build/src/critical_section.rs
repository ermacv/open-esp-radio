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

    fn runtime_images(root: &Path) -> Vec<toml::Value> {
        use std::collections::{BTreeMap, BTreeSet};

        let workspace: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
        let inherited = workspace["workspace"].get("dependencies");
        let mut packages = BTreeMap::new();
        for member in workspace["workspace"]["members"].as_array().unwrap() {
            let member = member.as_str().unwrap();
            let manifest: toml::Value = toml::from_str(
                &std::fs::read_to_string(root.join(member).join("Cargo.toml")).unwrap(),
            )
            .unwrap();
            let scopes = std::iter::once(&manifest).chain(
                manifest
                    .get("target")
                    .and_then(toml::Value::as_table)
                    .into_iter()
                    .flat_map(|targets| targets.values()),
            );
            let dependencies: Vec<String> = scopes
                .filter_map(|scope| scope.get("dependencies").and_then(toml::Value::as_table))
                .flat_map(|dependencies| dependencies.iter())
                .map(|(name, spec)| {
                    let spec = if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                    {
                        &inherited.unwrap()[name]
                    } else {
                        spec
                    };
                    spec.get("package")
                        .and_then(toml::Value::as_str)
                        .unwrap_or(name)
                        .to_owned()
                })
                .collect();
            packages.insert(
                manifest["package"]["name"].as_str().unwrap().to_owned(),
                (
                    manifest["package"].clone(),
                    dependencies,
                    Path::new(member).starts_with("firmware"),
                ),
            );
        }
        packages
            .iter()
            .filter_map(|(name, (package, _, firmware))| {
                if !firmware {
                    return None;
                }
                let mut pending = vec![name.as_str()];
                let mut visited = BTreeSet::new();
                while let Some(dependency) = pending.pop() {
                    if dependency == "oer-espressif-staged-runtime" {
                        return Some(package.clone());
                    }
                    if visited.insert(dependency)
                        && let Some((_, dependencies, _)) = packages.get(dependency)
                    {
                        pending.extend(dependencies.iter().map(String::as_str));
                    }
                }
                None
            })
            .collect()
    }

    #[test]
    fn every_runtime_dependent_image_declares_the_fact_derived_boot_role() {
        let profile = Profile::tracked("esp32s31").unwrap();
        let boot_hart = profile.facts.interrupts.as_ref().unwrap().boot_hart;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let packages = runtime_images(&root);
        assert!(!packages.is_empty());
        let metadata = serde_json::json!({"packages": packages});
        for package in metadata["packages"].as_array().unwrap() {
            let name = package["name"].as_str().unwrap();
            eprintln!("critical-section image metadata: {name}");
            assert_eq!(
                package["metadata"]["open-radio"][KEY].as_str(),
                Some("boot"),
                "{name}"
            );
            assert_eq!(of_metadata(&metadata, name, &profile).unwrap(), boot_hart);
        }
    }

    #[test]
    fn discovery_includes_new_target_and_inherited_dependencies_without_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["firmware/apps/new", "firmware/probes/new", "tools/host", "chips/adapter"]
[workspace.dependencies]
rt = { package = "oer-espressif-staged-runtime", version = "0.1" }
"#,
        )
        .unwrap();
        for (member, source) in [
            (
                "firmware/apps/new",
                r#"
[package]
name = "new-app"
[dependencies]
adapter = "0.1"
"#,
            ),
            (
                "firmware/probes/new",
                r#"
[package]
name = "new-probe"
[target.'cfg(target_arch = "riscv32")'.dependencies]
rt = { package = "oer-espressif-staged-runtime", version = "0.1" }
"#,
            ),
            (
                "chips/adapter",
                r#"
[package]
name = "adapter"
[dependencies]
rt.workspace = true
"#,
            ),
            (
                "tools/host",
                r#"
[package]
name = "host-tool"
[dependencies]
oer-espressif-staged-runtime = "0.1"
"#,
            ),
        ] {
            std::fs::create_dir_all(root.join(member)).unwrap();
            std::fs::write(root.join(member).join("Cargo.toml"), source).unwrap();
        }
        let metadata = serde_json::json!({"packages": runtime_images(root)});
        let packages = metadata["packages"].as_array().unwrap();
        assert_eq!(packages.len(), 2);
        let profile = Profile::tracked("esp32s31").unwrap();
        for package in packages {
            let name = package["name"].as_str().unwrap();
            let error = of_metadata(&metadata, name, &profile)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("declare `critical-section-hart`"),
                "{name}: {error}"
            );
        }
    }
}
