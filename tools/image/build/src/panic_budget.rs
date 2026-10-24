//! The panic check's contract of an image: the crate layer of every
//! workspace package, from its directory, and the image's per-layer budget,
//! `[package.metadata.open-radio.panic-budget]`:
//!
//! ```toml
//! [package.metadata.open-radio.panic-budget]
//! runtime = { hal = 2, core = 14 }
//! bootstrap = { core = 3 }
//! ```
//!
//! A layer the budget does not name has 0; the PAC layer is always 0.

use std::path::Path;

use serde_json::Value;

use oer_image_checks::panics::{Budget, Contract, Layers};

use crate::Result;

const KEY: &str = "panic-budget";

/// The panic contract of `package`, in the workspace at `root`.
///
/// # Errors
///
/// `cargo metadata` fails, the workspace has no `package`, or its budget is
/// malformed.
pub fn declared(root: &Path, package: &str) -> Result<Contract> {
    contract_of_metadata(&crate::manifest::workspace_metadata(root)?, package)
}

fn contract_of_metadata(metadata: &Value, package: &str) -> Result<Contract> {
    let budget = Budget::parse(crate::manifest::declared(metadata, package, KEY)?)
        .map_err(|error| format!("{package}: {error}"))?;
    let workspace = metadata
        .get("workspace_root")
        .and_then(Value::as_str)
        .map(Path::new)
        .ok_or("cargo metadata names no workspace root")?;
    let mut packages = Vec::new();
    for entry in metadata
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(manifest)) = (
            entry.get("name").and_then(Value::as_str),
            entry.get("manifest_path").and_then(Value::as_str),
        ) else {
            return Err("a cargo metadata package has no name or manifest path".into());
        };
        let directory = Path::new(manifest)
            .parent()
            .and_then(|directory| directory.strip_prefix(workspace).ok())
            .ok_or_else(|| format!("{name}'s manifest is outside the workspace"))?;
        packages.push((name, directory));
    }
    Ok(Contract {
        layers: Layers::of_packages(packages),
        budget,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_image_checks::CheckedElf;
    use oer_image_checks::panics::Layer;

    fn metadata(declared: Value) -> Value {
        serde_json::json!({
            "workspace_root": "/w",
            "packages": [
                { "name": "oer-esp32s31-pac", "manifest_path": "/w/chips/esp32s31/pac/Cargo.toml",
                  "metadata": null },
                { "name": "oer-esp32s31-hal", "manifest_path": "/w/chips/esp32s31/hal/Cargo.toml",
                  "metadata": null },
                { "name": "image", "manifest_path": "/w/firmware/apps/image/Cargo.toml",
                  "metadata": { "open-radio": { "panic-budget": declared } } },
            ],
        })
    }

    #[test]
    fn the_budget_and_the_layers_of_the_workspace_are_read() {
        let contract = contract_of_metadata(
            &metadata(serde_json::json!({ "runtime": { "hal": 2 }, "bootstrap": { "core": 1 } })),
            "image",
        )
        .unwrap();
        assert_eq!(contract.budget.of(CheckedElf::Runtime, Layer::Hal), 2);
        assert_eq!(contract.budget.of(CheckedElf::Runtime, Layer::Core), 0);
        assert_eq!(contract.budget.of(CheckedElf::Bootstrap, Layer::Core), 1);
        assert_eq!(contract.layers.of("oer_esp32s31_pac::x"), Layer::Pac);
        assert_eq!(contract.layers.of("oer_esp32s31_hal::x"), Layer::Hal);
        assert_eq!(contract.layers.of("image::main"), Layer::Firmware);
        let nothing = contract_of_metadata(&metadata(Value::Null), "image").unwrap();
        assert_eq!(nothing.budget, Budget::default());
    }

    #[test]
    fn a_malformed_budget_names_the_package() {
        for declared in [
            serde_json::json!({ "runtime": { "pac": 0 } }),
            serde_json::json!({ "runtime": { "drivers": 1 } }),
            serde_json::json!({ "stage-two": { "core": 1 } }),
            serde_json::json!({ "runtime": { "core": -1 } }),
            serde_json::json!(3),
        ] {
            let error = contract_of_metadata(&metadata(declared), "image")
                .unwrap_err()
                .to_string();
            assert!(error.starts_with("image: "), "{error}");
        }
    }
}
