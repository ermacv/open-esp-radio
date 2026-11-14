//! The instrumentation classes an image declares in its manifest, each
//! compiled under its own cfg:
//!
//! | Class | Answers | cfg |
//! | --- | --- | --- |
//! | `trace` | events and execution cost, collected by the image | `oer_trace` |
//! | `debug-control` | knobs that force execution paths | `oer_debug_control` |
//! | `fault-injection` | a one-shot fault on a real path, nothing else | `oer_fault_injection` |
//! | `cpu-load` | each hart's idle account: two clock reads and three stores a wake, nothing a poll or a frame | `oer_cpu_load` |
//!
//! An application package declares them as
//! `[package.metadata.open-radio] instrumentation = ["trace"]` (what holds
//! in every build of it) and, for a class one of its own features brings,
//! `[package.metadata.open-radio.features.<feature>] instrumentation = [..]`:
//! a build has the package's classes and those of every feature it enables
//! (its defaults included unless built without them). Only the
//! image pipeline sets the cfgs, for the declaring image alone and in a
//! compile cache of its own, so an image that declares nothing compiles no
//! instrumentation and its build cache never sees the cfgs. The probes of
//! the vendor comparison are no class: `cfg(oer_probe)` belongs to the probe
//! build ([`crate::probe`]).

use std::{collections::BTreeSet, path::Path};

use serde_json::Value;

use crate::Result;

/// One instrumentation class.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Class {
    Trace,
    DebugControl,
    FaultInjection,
    CpuLoad,
}

impl Class {
    const ALL: [Self; 4] = [
        Self::Trace,
        Self::DebugControl,
        Self::FaultInjection,
        Self::CpuLoad,
    ];

    /// The class's name in a manifest.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::DebugControl => "debug-control",
            Self::FaultInjection => "fault-injection",
            Self::CpuLoad => "cpu-load",
        }
    }

    /// The cfg the class's code is compiled under.
    pub const fn cfg(self) -> &'static str {
        match self {
            Self::Trace => "oer_trace",
            Self::DebugControl => "oer_debug_control",
            Self::FaultInjection => "oer_fault_injection",
            Self::CpuLoad => "oer_cpu_load",
        }
    }

    fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.name() == name)
    }
}

/// The classes one image is built with; empty for an image that declares
/// none.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Instrumentation(BTreeSet<Class>);

impl FromIterator<Class> for Instrumentation {
    fn from_iter<I: IntoIterator<Item = Class>>(classes: I) -> Self {
        Self(classes.into_iter().collect())
    }
}

impl Instrumentation {
    /// The classes a build of `application` has, from the workspace at
    /// `root`: its package's own and those of the features it enables.
    ///
    /// # Errors
    ///
    /// `cargo metadata` fails, the workspace has no such package, or its
    /// declaration is malformed (an unknown class, one named twice in a
    /// list, a feature table for a feature the package does not have).
    pub fn declared(root: &Path, application: &crate::Application) -> Result<Self> {
        Self::of_metadata(
            &crate::manifest::workspace_metadata(root)?,
            &application.package,
            &application.features,
            application.default_features,
        )
    }

    /// The classes of a build of `package` with `features` (and its
    /// defaults when `default_features`) in `metadata`, the output of
    /// `cargo metadata --format-version 1`.
    fn of_metadata(
        metadata: &Value,
        package: &str,
        features: &[String],
        default_features: bool,
    ) -> Result<Self> {
        let mut classes = BTreeSet::new();
        if let Some(declared) = crate::manifest::declared(metadata, package, "instrumentation")? {
            classes.extend(Self::classes_of(package, "instrumentation", declared)?);
        }
        let Some(tables) = crate::manifest::declared(metadata, package, "features")? else {
            return Ok(Self(classes));
        };
        let tables = tables.as_object().ok_or_else(|| {
            format!("{package}: `features` is a table of the package's features, not {tables}")
        })?;
        let package_features = crate::manifest::package_features(metadata, package)?;
        let enabled =
            crate::manifest::enabled_features(metadata, package, features, default_features)?;
        for (feature, table) in tables {
            if !package_features.contains(feature) {
                return Err(
                    format!("{package}: `features.{feature}` names no feature of it").into(),
                );
            }
            let Some(declared) = table.get("instrumentation") else {
                continue;
            };
            let named = Self::classes_of(
                package,
                &format!("features.{feature}.instrumentation"),
                declared,
            )?;
            if enabled.contains(feature) {
                classes.extend(named);
            }
        }
        Ok(Self(classes))
    }

    /// The classes one list `key` of `package` names.
    fn classes_of(package: &str, key: &str, declared: &Value) -> Result<BTreeSet<Class>> {
        let names = declared.as_array().ok_or_else(|| {
            format!("{package}: `{key}` is a list of class names, not {declared}")
        })?;
        let mut classes = BTreeSet::new();
        for name in names {
            let class = name.as_str().and_then(Class::of_name).ok_or_else(|| {
                let known: Vec<_> = Class::ALL.into_iter().map(Class::name).collect();
                format!(
                    "{package}: {name} in `{key}` is no instrumentation class ({})",
                    known.join(", ")
                )
            })?;
            if !classes.insert(class) {
                return Err(format!("{package} names {} twice in `{key}`", class.name()).into());
            }
        }
        Ok(classes)
    }

    /// The classes.
    pub fn classes(&self) -> impl Iterator<Item = Class> + '_ {
        self.0.iter().copied()
    }

    /// The Rust flags that set the classes' cfgs, in class order.
    pub(crate) fn rustflags(&self) -> impl Iterator<Item = String> + '_ {
        self.classes().map(|class| format!("--cfg {}", class.cfg()))
    }

    /// The name of the compile cache of an image built with these classes:
    /// `cargo`, and `cargo-<class>-…` when there are any.
    pub(crate) fn cache_name(&self) -> String {
        self.classes()
            .fold("cargo".to_owned(), |name, class| name + "-" + class.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(instrumentation: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "other", "metadata": null },
            { "name": "image", "metadata": { "open-radio": { "instrumentation": instrumentation } } },
        ] })
    }

    #[test]
    fn an_image_builds_with_the_classes_it_declares() {
        let declared = Instrumentation::of_metadata(
            &metadata(serde_json::json!(["debug-control", "trace"])),
            "image",
            &[],
            true,
        )
        .unwrap();
        assert_eq!(
            declared.classes().collect::<Vec<_>>(),
            [Class::Trace, Class::DebugControl]
        );
        assert_eq!(
            declared.rustflags().collect::<Vec<_>>(),
            ["--cfg oer_trace", "--cfg oer_debug_control"]
        );
        assert_eq!(declared.cache_name(), "cargo-trace-debug-control");
        let control = Instrumentation::of_metadata(
            &metadata(serde_json::json!(["debug-control"])),
            "image",
            &[],
            true,
        )
        .unwrap();
        assert_eq!(
            control.rustflags().collect::<Vec<_>>(),
            ["--cfg oer_debug_control"]
        );
    }

    #[test]
    fn fault_injection_adds_its_own_cfg_and_no_debug_control() {
        let declared = Instrumentation::of_metadata(
            &metadata(serde_json::json!(["trace", "fault-injection"])),
            "image",
            &[],
            true,
        )
        .unwrap();
        assert_eq!(
            declared.rustflags().collect::<Vec<_>>(),
            ["--cfg oer_trace", "--cfg oer_fault_injection"]
        );
    }

    #[test]
    fn the_cpu_load_is_a_class_of_its_own_beside_the_trace() {
        let declared = Instrumentation::of_metadata(
            &metadata(serde_json::json!(["cpu-load"])),
            "image",
            &[],
            true,
        )
        .unwrap();
        assert_eq!(declared.classes().collect::<Vec<_>>(), [Class::CpuLoad]);
        assert_eq!(
            declared.rustflags().collect::<Vec<_>>(),
            ["--cfg oer_cpu_load"]
        );
        assert_eq!(declared.cache_name(), "cargo-cpu-load");
    }

    #[test]
    fn an_image_that_declares_nothing_builds_without_instrumentation() {
        let none =
            Instrumentation::of_metadata(&metadata(Value::Null), "other", &[], true).unwrap();
        assert_eq!(none, Instrumentation::default());
        assert_eq!(none.rustflags().count(), 0);
        // Its cache is the one every plain image shares.
        assert_eq!(none.cache_name(), "cargo");
    }

    #[test]
    fn a_malformed_declaration_fails_the_build() {
        for (declared, expected) in [
            (serde_json::json!(["probe"]), "no instrumentation class"),
            (serde_json::json!(["profile"]), "no instrumentation class"),
            (serde_json::json!(["trace", "trace"]), "twice"),
            (serde_json::json!("trace"), "a list of class names"),
        ] {
            let error = Instrumentation::of_metadata(&metadata(declared), "image", &[], true)
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
        let error = Instrumentation::of_metadata(&metadata(Value::Null), "missing", &[], true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no package missing"), "{error}");
    }

    #[test]
    fn the_workspace_images_declare_their_classes() {
        let packages: Vec<_> = [include_str!(
            "../../../../hil/probes/esp32s31-boot-smoke/Cargo.toml"
        )]
        .into_iter()
        .map(|text| {
            let manifest: toml::Value = toml::from_str(text).unwrap();
            let package = &manifest["package"];
            serde_json::json!({
                "name": package["name"].as_str().unwrap(),
                "metadata": package.get("metadata"),
            })
        })
        .collect();
        let metadata = serde_json::json!({"packages": packages});
        let classes = |package| {
            Instrumentation::of_metadata(&metadata, package, &[], true)
                .unwrap()
                .classes()
                .collect::<Vec<_>>()
        };
        assert_eq!(classes("oer-hil-probes-esp32s31-boot-smoke"), []);
    }

    /// An image whose features name instrumentation: the classes of the
    /// features a build enables, its defaults included, beside the
    /// package's own.
    fn featured() -> Value {
        serde_json::json!({ "packages": [{
            "name": "image",
            "features": { "default": ["trace", "fault"], "trace": ["oer/observe"], "fault": [], "lean": [] },
            "metadata": { "open-radio": {
                "instrumentation": ["debug-control"],
                "features": {
                    "trace": { "instrumentation": ["trace"] },
                    "fault": { "instrumentation": ["fault-injection"] },
                },
            } },
        }] })
    }

    #[test]
    fn a_build_has_the_classes_of_the_features_it_enables() {
        let classes = |features: &[&str], default: bool| {
            let features: Vec<String> = features.iter().map(|f| (*f).to_owned()).collect();
            Instrumentation::of_metadata(&featured(), "image", &features, default)
                .unwrap()
                .classes()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            classes(&[], true),
            [Class::Trace, Class::DebugControl, Class::FaultInjection]
        );
        assert_eq!(
            classes(&[], false),
            [Class::DebugControl],
            "the measuring build"
        );
        assert_eq!(
            classes(&["fault"], false),
            [Class::DebugControl, Class::FaultInjection]
        );
    }

    #[test]
    fn a_feature_table_that_names_an_unknown_class_or_feature_fails_the_build() {
        let mut unknown_class = featured();
        unknown_class["packages"][0]["metadata"]["open-radio"]["features"]["trace"]["instrumentation"] =
            serde_json::json!(["sampler"]);
        let error = Instrumentation::of_metadata(&unknown_class, "image", &[], false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no instrumentation class"), "{error}");
        let mut unknown_feature = featured();
        unknown_feature["packages"][0]["metadata"]["open-radio"]["features"]["probe"] =
            serde_json::json!({ "instrumentation": ["trace"] });
        let error = Instrumentation::of_metadata(&unknown_feature, "image", &[], true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("names no feature"), "{error}");
    }
}
