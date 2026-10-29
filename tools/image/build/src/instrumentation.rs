//! The instrumentation classes an image declares in its manifest, each
//! compiled under its own cfg:
//!
//! | Class | Answers | cfg |
//! | --- | --- | --- |
//! | `trace` | events and execution cost, collected by the image | `oer_trace` |
//! | `debug-control` | knobs that force execution paths | `oer_debug_control` |
//!
//! An application package declares them as
//! `[package.metadata.open-radio] instrumentation = ["trace"]`. Only the
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
}

impl Class {
    const ALL: [Self; 2] = [Self::Trace, Self::DebugControl];

    /// The class's name in a manifest.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::DebugControl => "debug-control",
        }
    }

    /// The cfg the class's code is compiled under.
    pub const fn cfg(self) -> &'static str {
        match self {
            Self::Trace => "oer_trace",
            Self::DebugControl => "oer_debug_control",
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
    /// The classes `package` declares, from the workspace at `root`.
    ///
    /// # Errors
    ///
    /// `cargo metadata` fails, the workspace has no `package`, or its
    /// declaration is malformed.
    pub fn declared(root: &Path, package: &str) -> Result<Self> {
        Self::of_metadata(&crate::manifest::workspace_metadata(root)?, package)
    }

    /// The classes `package` declares in `metadata`, the output of
    /// `cargo metadata --format-version 1`.
    fn of_metadata(metadata: &Value, package: &str) -> Result<Self> {
        let Some(declared) = crate::manifest::declared(metadata, package, "instrumentation")?
        else {
            return Ok(Self::default());
        };
        let names = declared.as_array().ok_or_else(|| {
            format!("{package}: `instrumentation` is a list of class names, not {declared}")
        })?;
        let mut classes = BTreeSet::new();
        for name in names {
            let class = name.as_str().and_then(Class::of_name).ok_or_else(|| {
                let known: Vec<_> = Class::ALL.into_iter().map(Class::name).collect();
                format!(
                    "{package}: {name} is no instrumentation class ({})",
                    known.join(", ")
                )
            })?;
            if !classes.insert(class) {
                return Err(format!("{package} declares {} twice", class.name()).into());
            }
        }
        Ok(Self(classes))
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
        let control =
            Instrumentation::of_metadata(&metadata(serde_json::json!(["debug-control"])), "image")
                .unwrap();
        assert_eq!(
            control.rustflags().collect::<Vec<_>>(),
            ["--cfg oer_debug_control"]
        );
    }

    #[test]
    fn an_image_that_declares_nothing_builds_without_instrumentation() {
        let none = Instrumentation::of_metadata(&metadata(Value::Null), "other").unwrap();
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
            let error = Instrumentation::of_metadata(&metadata(declared), "image")
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
        let error = Instrumentation::of_metadata(&metadata(Value::Null), "missing")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no package missing"), "{error}");
    }

    #[test]
    fn the_workspace_images_declare_their_classes() {
        let packages: Vec<_> = [include_str!(
            "../../../../firmware/apps/boot-smoke/Cargo.toml"
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
            Instrumentation::of_metadata(&metadata, package)
                .unwrap()
                .classes()
                .collect::<Vec<_>>()
        };
        assert_eq!(classes("oer-firmware-apps-boot-smoke"), []);
    }
}
