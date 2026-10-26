//! Direct PAC authority is admitted by exact package path, never by suffix.
use std::path::Path;

const FACTS: &str = "firmware/probes/esp32s31-facts";
const PAC: &str = "oer-esp32s31-pac";
const IMAGE: &str = "oer-firmware-probes-esp32s31-facts";

fn inspect(
    owner: &str,
    value: &toml::Value,
    workspace: Option<&toml::Value>,
    found: &mut Vec<String>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, value) in table {
        if matches!(
            key.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            for (alias, spec) in value.as_table().expect("dependency table") {
                let spec = if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    workspace
                        .and_then(|values| values.get(alias))
                        .unwrap_or(spec)
                } else {
                    spec
                };
                let name = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(alias);
                if name == PAC && !matches!(owner, "chips/esp32s31/hal" | FACTS) {
                    found.push(format!("{owner}: direct PAC dependency"));
                }
                if name == IMAGE {
                    found.push(format!("{owner}: depends on the facts image"));
                }
                if owner == FACTS
                    && key != "build-dependencies"
                    && !matches!(name, PAC | "oer-mmio" | "oer-riscv")
                {
                    found.push(format!("{owner}: runtime dependency {name}"));
                }
            }
        } else {
            inspect(owner, value, workspace, found);
        }
    }
}

#[test]
fn only_the_hal_and_this_facts_image_may_depend_on_the_pac() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let workspace: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    let mut found = Vec::new();
    for member in workspace["workspace"]["members"].as_array().unwrap() {
        let member = member.as_str().unwrap();
        let manifest: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join(member).join("Cargo.toml")).unwrap())
                .unwrap();
        inspect(
            member,
            &manifest,
            workspace["workspace"].get("dependencies"),
            &mut found,
        );
    }
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn aliases_targets_and_other_facts_suffixes_do_not_expand_the_exception() {
    let spec: toml::Value = toml::from_str("[target.'cfg(test)'.dependencies]\nrenamed = { package = 'oer-esp32s31-pac', path = 'pac' }\n").unwrap();
    for owner in [FACTS, "chips/esp32s31/hal"] {
        let mut found = Vec::new();
        inspect(owner, &spec, None, &mut found);
        assert!(found.is_empty());
    }
    for owner in [
        "firmware/probes/esp32s31-radio",
        "firmware/probes/other-facts",
        "firmware/apps/x",
    ] {
        let mut found = Vec::new();
        inspect(owner, &spec, None, &mut found);
        assert_eq!(found.len(), 1);
    }
    let mut found = Vec::new();
    inspect(
        FACTS,
        &toml::from_str("[dependencies]\noer-esp32s31-hal = {}\n").unwrap(),
        None,
        &mut found,
    );
    assert_eq!(found.len(), 1);
}

#[test]
fn workspace_aliases_are_resolved_at_the_actual_consumer() {
    let workspace: toml::Value =
        toml::from_str("renamed = { package = 'oer-esp32s31-pac', path = 'pac' }").unwrap();
    let consumer: toml::Value =
        toml::from_str("[dependencies]\nrenamed = { workspace = true }").unwrap();
    let mut found = Vec::new();
    inspect(
        "firmware/probes/other-facts",
        &consumer,
        Some(&workspace),
        &mut found,
    );
    assert_eq!(found.len(), 1);
}
