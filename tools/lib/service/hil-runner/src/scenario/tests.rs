use super::*;
use oer_hil_scenario::identity::normalize;
use oer_hil_schema::image::ImageClass;

fn catalog() -> Catalog {
    Catalog::load(&oer_process::built_root().join("hil/scenarios")).unwrap()
}

/// Top-level fields that are not part of a scenario's procedure.
const PRESENTATION: [&str; 4] = ["description", "role", "tags", "unsupported"];

/// Set to regenerate `hil/schema/scenario-v5-defaults.json` from the typed
/// scenario families instead of checking it.
const BLESS_DEFAULTS: &str = "OER_HIL_BLESS_SCENARIO_DEFAULTS";

/// Remove null values, which typed serialization writes for absent options.
fn without_nulls(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => object
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.clone(), without_nulls(value)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        serde_json::Value::Array(values) => values.iter().map(without_nulls).collect(),
        other => other.clone(),
    }
}

/// The defaults document the typed families imply: every value a typed
/// scenario holds that its source document does not write, placed under
/// the `$kind` variant of a tagged table and under a `?` key for a table
/// that only some documents of the same variant hold.
struct Defaults {
    document: serde_json::Map<String, serde_json::Value>,
    /// Table paths a typed scenario omitted, so their defaults are optional.
    optional: std::collections::BTreeSet<Vec<String>>,
    /// Table paths a typed scenario filled although its document omitted them.
    implied: std::collections::BTreeSet<Vec<String>>,
}

impl Defaults {
    fn slot<'a>(
        document: &'a mut serde_json::Map<String, serde_json::Value>,
        path: &[String],
    ) -> &'a mut serde_json::Map<String, serde_json::Value> {
        path.iter().fold(document, |node, key| {
            node.entry(key.clone())
                .or_insert_with(|| serde_json::Value::Object(Default::default()))
                .as_object_mut()
                .expect("a defaults table")
        })
    }

    /// Record the values `typed` adds to `raw`, both the table at `path`.
    fn collect(
        &mut self,
        raw: &serde_json::Map<String, serde_json::Value>,
        typed: &serde_json::Map<String, serde_json::Value>,
        path: &[String],
        scenario: &str,
    ) {
        // A tagged table's defaults belong to its variant, except the tag.
        let mut variant = path.to_vec();
        if let Some(kind) = typed.get("kind").and_then(serde_json::Value::as_str) {
            variant.extend([String::from("$kind"), kind.to_owned()]);
        }
        for (key, value) in typed {
            if path.is_empty() && PRESENTATION.contains(&key.as_str()) {
                continue;
            }
            let base = if key == "kind" { path } else { &variant[..] };
            let mut child = base.to_vec();
            child.push(key.clone());
            match (raw.get(key), value) {
                (Some(serde_json::Value::Object(raw)), serde_json::Value::Object(typed)) => {
                    self.collect(raw, typed, &child, scenario)
                }
                (Some(_), _) => {}
                (None, serde_json::Value::Object(typed)) => {
                    self.implied.insert(child.clone());
                    Self::slot(&mut self.document, &child);
                    self.collect(&Default::default(), typed, &child, scenario);
                }
                (None, value) => {
                    let slot = Self::slot(&mut self.document, base);
                    let previous = slot.insert(key.clone(), value.clone());
                    assert!(
                        previous.is_none_or(|previous| previous == *value),
                        "{scenario}: two defaults for {child:?}"
                    );
                }
            }
        }
        for (key, value) in raw {
            assert!(
                !value.is_object() || typed.contains_key(key),
                "{scenario}: the typed scenario drops the table {key}"
            );
        }
        // A table with defaults that this typed scenario does not hold is
        // optional within its variant.
        for key in self.tables_at(&variant) {
            if !typed.contains_key(&key) {
                let mut child = variant.clone();
                child.push(key);
                self.optional.insert(child);
            }
        }
    }

    /// The tables already recorded directly below `path`.
    fn tables_at(&self, path: &[String]) -> Vec<String> {
        let mut node = &self.document;
        for key in path {
            match node.get(key).and_then(serde_json::Value::as_object) {
                Some(next) => node = next,
                None => return Vec::new(),
            }
        }
        node.iter()
            .filter(|(key, value)| value.is_object() && !key.starts_with('$'))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Mark optional tables with `?`; a table both implied and omitted
    /// within one variant has no single default.
    fn finish(mut self) -> serde_json::Value {
        let mut optional = self.optional.iter().cloned().collect::<Vec<_>>();
        // Deepest first, so renaming a table keeps its parents' paths valid.
        optional.sort_by_key(|path| std::cmp::Reverse(path.len()));
        for path in optional {
            assert!(
                !self.implied.contains(&path),
                "the table {path:?} is both filled and omitted by typed scenarios"
            );
            let (key, parent) = path.split_last().expect("a table path");
            let parent = Self::slot(&mut self.document, parent);
            if let Some(table) = parent.remove(key) {
                parent.insert(format!("?{key}"), table);
            }
        }
        serde_json::Value::Object(self.document)
    }
}

/// Every catalog scenario's defaults, generated from the typed families.
fn generated_defaults(catalog: &Catalog) -> serde_json::Value {
    let mut defaults = Defaults {
        document: Default::default(),
        optional: Default::default(),
        implied: Default::default(),
    };
    // Two passes: the second sees every table the first recorded, so an
    // omission is found wherever it occurs in catalog order.
    for _ in 0..2 {
        for scenario in catalog.all() {
            let raw: toml::Table =
                toml::from_str(&std::fs::read_to_string(scenario.source()).unwrap()).unwrap();
            let raw = without_nulls(&serde_json::to_value(raw).unwrap());
            let typed = without_nulls(&serde_json::to_value(scenario).unwrap());
            defaults.collect(
                raw.as_object().unwrap(),
                typed.as_object().unwrap(),
                &[],
                scenario.id(),
            );
        }
    }
    defaults.finish()
}

#[test]
fn the_scenario_defaults_document_is_generated_from_the_typed_families() {
    let catalog = catalog();
    let path = oer_process::built_root().join("hil/schema/scenario-v5-defaults.json");
    let mut generated = serde_json::to_string_pretty(&generated_defaults(&catalog)).unwrap();
    generated.push('\n');
    if std::env::var_os(BLESS_DEFAULTS).is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap();
    assert!(
        committed == generated,
        "{} is not the document the typed scenario families imply; regenerate it with \
         {BLESS_DEFAULTS}=1 cargo test -p oer-hil-runner scenario_defaults",
        path.display()
    );
}

#[test]
fn every_catalog_scenario_has_the_same_identity_before_and_after_typed_defaults() {
    let catalog = catalog();
    for scenario in catalog.all() {
        let raw: toml::Table =
            toml::from_str(&std::fs::read_to_string(scenario.source()).unwrap()).unwrap();
        let typed = serde_json::to_value(scenario).unwrap();
        assert_eq!(
            normalize(&serde_json::to_value(raw).unwrap()),
            normalize(&typed),
            "{}",
            scenario.id()
        );
        let snapshot = Scenario::from_json(&serde_json::to_vec(&typed).unwrap()).unwrap();
        assert_eq!(snapshot.family, scenario.family, "{}", scenario.id());
    }
}

#[test]
fn a_document_names_exactly_one_known_family() {
    let header = "schema = 5\nid = 'x'\ndescription = 'd'\nrole = 'investigation'\n";
    let parse = |family: &str| {
        Scenario::from_toml(&format!("{header}{family}"), std::path::Path::new("x.toml"))
    };
    parse("[system]\nkind = 'boot-smoke'\n").unwrap();
    for family in [
        "",
        "[system]\nkind = 'boot-smoke'\n[bluetooth]\nkind = 'gatt'\n",
        "[bluetooth]\nkind = 'gatt'\n",
        "[thread]\nkind = 'boot-smoke'\n",
        "image = 'boot-smoke'\n[system]\nkind = 'boot-smoke'\n",
    ] {
        assert!(parse(family).is_err(), "accepted {family:?}");
    }
}

#[test]
fn every_image_class_is_reached_by_a_family_plan() {
    let catalog = catalog();
    for class in ImageClass::ALL {
        assert!(
            catalog
                .all()
                .iter()
                .any(|scenario| scenario.image() == class),
            "no scenario runs {}",
            class.id()
        );
    }
}

#[test]
fn the_system_scenarios_need_no_fixture_service() {
    let catalog = catalog();
    let selected = catalog.all().iter().collect::<Vec<_>>();
    assert!(requirements(&selected) == Requirements::default());
}

#[test]
fn a_scenario_is_marked_unsupported_exactly_when_no_current_image_serves_it() {
    let root = oer_process::built_root();
    // A chip whose agent builds every class.
    let staged = oer_chip_profile::Profile::all(&root)
        .unwrap()
        .into_iter()
        .find(|profile| oer_hil_image_class::declares(&profile.id, "system-panic-reset"))
        .unwrap();
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(staged.hil_agent_manifest(&root)).unwrap())
            .unwrap();
    let features = manifest["features"].as_table().unwrap();
    // The firmware builds a class only while it declares every feature of
    // the class's recipe.
    let built = |class: ImageClass| {
        class
            .runtime_features()
            .split(',')
            .all(|feature| features.contains_key(feature))
    };
    for scenario in catalog().all() {
        let class = scenario.image();
        let served = if class == ImageClass::BootSmoke {
            built(class)
        } else {
            oer_hil_image_class::image_keys_on(class, &staged.id)
                .is_some_and(|reported| scenario.family.served_by(&reported))
        };
        assert_eq!(
            served,
            scenario.header.unsupported.is_none(),
            "scenario `{}` on image `{}`: served = {served}",
            scenario.id(),
            class.id()
        );
    }
}

#[test]
fn an_explicit_selection_refuses_an_unsupported_scenario_and_a_tag_skips_it() {
    let directory = tempfile::tempdir().unwrap();
    for (id, unsupported) in [("served", ""), ("unserved", "unsupported = 'no image'\n")] {
        std::fs::write(
            directory.path().join(format!("{id}.toml")),
            format!(
                "schema = 5\nid = '{id}'\ndescription = 'd'\nrole = 'investigation'\ntags = ['shared']\n{unsupported}\n[system]\nkind = 'watchdog'\n"
            ),
        )
        .unwrap();
    }
    let catalog = Catalog::load(directory.path()).unwrap();
    let unsupported = catalog
        .all()
        .iter()
        .find(|scenario| scenario.header.unsupported.is_some())
        .expect("an unsupported scenario");
    let explicit = crate::cli::Selection {
        scenario: Some(unsupported.id().to_owned()),
        tag: Vec::new(),
        role: None,
        chip: None,
    };
    assert!(explicit.resolve(&catalog).is_err());
    let tagged = crate::cli::Selection {
        scenario: None,
        tag: unsupported.header.tags.clone(),
        role: None,
        chip: None,
    };
    let selected = tagged.resolve(&catalog).unwrap_or_default();
    assert!(
        selected
            .iter()
            .all(|scenario| scenario.id() != unsupported.id())
    );
    assert!(
        crate::execution::orchestration::named_scenarios(&catalog, &[unsupported.id().to_owned()])
            .is_err()
    );
}
