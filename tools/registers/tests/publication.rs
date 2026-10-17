//! Repository publication checks exercise reviewed inputs, not generated addresses.
use std::{fs, path::PathBuf};
fn manifest() -> (tempfile::TempDir, PathBuf, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    // The first chip with a register publication.
    let base = std::fs::read_dir(root.join("registers"))
        .unwrap()
        .map(|chip| chip.unwrap().path().join("publication"))
        .filter(|base| base.join("registers.toml").is_file())
        .min()
        .unwrap();
    let source = fs::read_to_string(base.join("registers.toml")).unwrap();
    // All relative inputs/outputs in this concrete composition start with ../.
    let source = source.replace("\"../", &format!("\"{}/../", base.display()));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registers.toml");
    (dir, path, source)
}
#[test]
fn reviewed_inputs_validate_without_vendor_artifacts() {
    let (_dir, path, source) = manifest();
    fs::write(&path, source).unwrap();
    oer_register_tool::Publication::load(&path).unwrap();
}
#[test]
fn missing_evidence_wrong_applicability_and_output_alias_are_rejected() {
    for change in ["evidence", "chip", "alias", "schema"] {
        let (_dir, path, source) = manifest();
        let mut document: toml_edit::DocumentMut = source.parse().unwrap();
        match change {
            "evidence" => document["evidence"][0] = toml_edit::value("/missing/evidence.toml"),
            "chip" => document["applicability"]["chip"] = toml_edit::value("wrong-chip"),
            "alias" => document["outputs"]["svd"] = document["model"].clone(),
            "schema" => document["schema"] = toml_edit::value(9),
            _ => unreachable!(),
        }
        fs::write(&path, document.to_string()).unwrap();
        assert!(
            oer_register_tool::Publication::load(&path).is_err(),
            "{change}"
        );
    }
}
#[test]
fn publication_uses_loaded_policy_after_source_disappears() {
    let (dir, path, source) = manifest();
    let mut document: toml_edit::DocumentMut = source.parse().unwrap();
    let api = dir.path().join("api.toml");
    fs::copy(document["api"].as_str().unwrap(), &api).unwrap();
    document["api"] = toml_edit::value(api.to_str().unwrap());
    fs::write(&path, document.to_string()).unwrap();
    let publication = oer_register_tool::Publication::load(&path).unwrap();
    fs::remove_file(api).unwrap();
    publication.generate(true).unwrap();
}

#[test]
fn publication_dependency_graph_has_no_binary_execution_authority() {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .no_deps()
        .exec()
        .unwrap();
    let mut pending = vec!["oer-register-tool".to_owned()];
    let mut visited = std::collections::BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        assert!(
            !name.starts_with("blobray"),
            "forbidden publication dependency: {name}"
        );
        let package = metadata
            .packages
            .iter()
            .find(|p| p.name.as_str() == name)
            .unwrap();
        pending.extend(
            package
                .dependencies
                .iter()
                .filter(|d| d.path.is_some() && d.kind == cargo_metadata::DependencyKind::Normal)
                .map(|d| d.name.clone()),
        );
    }
}

#[test]
fn memory_policy_rejects_undeclared_overlap_and_inexact_alias() {
    for alias in [false, true] {
        let (dir, path, source) = manifest();
        let mut publication: toml_edit::DocumentMut = source.parse().unwrap();
        let original = fs::read_to_string(publication["memory"].as_str().unwrap()).unwrap();
        let mut memory: toml_edit::DocumentMut = original.parse().unwrap();
        let regions = memory["regions"].as_array_of_tables_mut().unwrap();
        let mut duplicate = regions.get(0).unwrap().clone();
        let original_name = duplicate["name"].as_str().unwrap().to_owned();
        duplicate["name"] = toml_edit::value("ambiguous-region");
        if alias {
            duplicate["alias-of"] = toml_edit::value(original_name);
            duplicate["end-exclusive"] =
                toml_edit::value(duplicate["end-exclusive"].as_integer().unwrap() - 1);
        }
        regions.push(duplicate);
        let memory_path = dir.path().join("memory.toml");
        fs::write(&memory_path, memory.to_string()).unwrap();
        publication["memory"] = toml_edit::value(memory_path.to_str().unwrap());
        fs::write(&path, publication.to_string()).unwrap();
        let error = match oer_register_tool::Publication::load(&path) {
            Ok(_) => panic!("invalid memory policy accepted"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(if alias {
                "invalid memory alias"
            } else {
                "overlap"
            }),
            "{error}"
        );
    }
}
