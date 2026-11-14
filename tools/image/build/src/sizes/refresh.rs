//! Reanalyse saved ELF bundles; input directories remain read-only.
use super::{FILE, Provenance, SCHEMA, Summary};
use crate::Result;
use oer_tools_image_bundle::files;
use oer_tools_image_checks::sizes::{Measurements, TYPE_MIN_BYTES};
use std::{collections::BTreeMap, path::Path};

/// Regenerate summaries in a separate output tree, retaining original build
/// provenance. `root` supplies the original image configuration, not binaries.
pub fn refresh(input: &Path, output: &Path, root: &Path) -> Result<usize> {
    let input = input.canonicalize()?;
    let output = prospective_path(output)?;
    if output.starts_with(&input) || input.starts_with(&output) {
        return Err("sizes-refresh: input and output trees must be separate".into());
    }
    let count = visit(&input, &output, root)?;
    if count == 0 {
        return Err(format!("no sizes.json in {}", input.display()).into());
    }
    Ok(count)
}

/// The instrumentation classes of the build `provenance` (a summary of
/// schema 1 or 2, which did not record them) describes: what `root`'s
/// manifest declares for its package and features, `root` being the
/// original configuration the refresh is given.
fn recorded_classes(provenance: &serde_json::Value, root: &Path) -> Result<Vec<String>> {
    let application = recorded_application(provenance)?;
    Ok(
        crate::instrumentation::Instrumentation::declared(root, &application)?
            .classes()
            .map(|class| class.name().to_owned())
            .collect(),
    )
}

/// The application an older summary's `provenance` records: its package,
/// binary and features, each required.
fn recorded_application(provenance: &serde_json::Value) -> Result<crate::Application> {
    let text = |key: &str| {
        provenance[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("sizes-refresh: provenance has no {key}"))
    };
    let features = provenance["features"]
        .as_array()
        .ok_or("sizes-refresh: provenance has no features")?
        .iter()
        .map(|feature| {
            feature
                .as_str()
                .map(str::to_owned)
                .ok_or("sizes-refresh: a provenance feature is not a name")
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(crate::Application {
        package: text("package")?,
        binary: text("binary")?,
        features,
        default_features: provenance["default_features"]
            .as_bool()
            .ok_or("sizes-refresh: provenance has no default_features")?,
    })
}

#[cfg(test)]
mod tests {
    use super::recorded_application;

    #[test]
    fn an_older_summary_names_its_application_or_its_refresh_fails() {
        let mut provenance = serde_json::json!({
            "package": "image",
            "binary": "image",
            "features": ["oer/esp32s31"],
            "default_features": false,
        });
        let application = recorded_application(&provenance).unwrap();
        assert_eq!(application.package, "image");
        assert_eq!(application.features, ["oer/esp32s31"]);
        assert!(!application.default_features);
        // No value stands in for a missing one.
        provenance
            .as_object_mut()
            .unwrap()
            .remove("default_features");
        let error = recorded_application(&provenance).unwrap_err().to_string();
        assert!(error.contains("no default_features"), "{error}");
    }
}

fn prospective_path(path: &Path) -> Result<std::path::PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(prospective_path(parent)?.join(
        path.file_name()
            .ok_or("sizes-refresh: output has no name")?,
    ))
}

fn visit(input: &Path, output: &Path, root: &Path) -> Result<usize> {
    if input.join(FILE).is_file() {
        one(input, output, root)?;
        return Ok(1);
    }
    if input.join(oer_tools_image_bundle::MANIFEST).is_file() {
        return Err(format!(
            "sizes-refresh: missing build provenance in {}",
            input.display()
        )
        .into());
    }
    let mut entries = std::fs::read_dir(input)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    let mut count = 0;
    for entry in entries {
        if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            count += visit(&entry.path(), &output.join(entry.file_name()), root)?;
        }
    }
    Ok(count)
}

fn one(input: &Path, output: &Path, root: &Path) -> Result<()> {
    let old: serde_json::Value = serde_json::from_slice(&std::fs::read(input.join(FILE))?)?;
    let schema = old["schema"].as_u64();
    if !matches!(schema, Some(1..=3)) || old["type_min_bytes"].as_u64() != Some(TYPE_MIN_BYTES) {
        return Err("sizes-refresh: unsupported source schema or type threshold".into());
    }
    let mut provenance = old["provenance"].clone();
    if schema != Some(SCHEMA.into()) {
        provenance["instrumentation"] = recorded_classes(&provenance, root)?.into();
    }
    let provenance: Provenance = serde_json::from_value(provenance)?;
    for (name, hash) in &provenance.stack_policy_sources {
        if oer_tools_os_durable::sha256_file(&root.join(name))? != *hash {
            return Err(format!("sizes-refresh: configuration {name} differs; --source-root must supply the original image configuration").into());
        }
    }
    let policy =
        oer_tools_chip_profile::policy::StackPolicy::load(&root.join(&provenance.stack_policy))?;
    let profile = oer_tools_chip_profile::Profile::load(root, &provenance.chip)?;
    if profile.facts.rust_target != provenance.target {
        return Err("sizes-refresh: configuration target differs from original build".into());
    }
    let mut elfs = BTreeMap::new();
    for name in old["elfs"]
        .as_object()
        .ok_or("sizes-refresh: missing ELF inventory")?
        .keys()
    {
        let (file, bootstrap) = match name.as_str() {
            "runtime" => (files::RUNTIME_ELF, false),
            "bootstrap" => (files::BOOTSTRAP_ELF, true),
            _ => return Err(format!("sizes-refresh: unsupported ELF {name}").into()),
        };
        elfs.insert(
            name.clone(),
            Measurements::remeasure(root, &profile, &policy, &input.join(file), bootstrap)?,
        );
    }
    if elfs.is_empty() {
        return Err("sizes-refresh: no ELF measurements".into());
    }
    let mut summary = Summary {
        schema: SCHEMA,
        type_min_bytes: TYPE_MIN_BYTES,
        provenance,
        elfs,
        image_files: BTreeMap::new(),
        embedded_payloads: Vec::new(),
    };
    summary.contents(input)?;
    std::fs::create_dir_all(output)?;
    oer_tools_os_durable::atomic_json(&output.join(FILE), &summary)?;
    eprintln!("sizes-refresh: {} → {}", input.display(), output.display());
    Ok(())
}
