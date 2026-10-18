//! Package classification: the `[package.metadata.open-radio]` table every
//! package declares, typed key by key.
//!
//! This module is the one reader of the table. Every key it may hold is a
//! field of [`Classification`]; an unknown key, a missing required one or a
//! value of the wrong shape is an error naming the package.

use std::{collections::BTreeSet, fmt, str::FromStr};

use toml::{Table, Value};

use crate::manifest::Package;

/// What a package is for, implied by its layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// Shipping code: the architecture audit compiles and bounds it.
    Production,
    /// Experimental code no production package depends on.
    Experimental,
    /// Tools, HIL, verification and applications.
    Development,
}

/// An architecture layer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Layer {
    Contract,
    Protocol,
    Hardware,
    Role,
    Adapter,
    Runtime,
    Service,
    Composition,
    Facade,
    Experiment,
    Tool,
    Hil,
    Verification,
    Application,
    Platform,
}

impl Layer {
    /// Every layer.
    pub const ALL: [Self; 15] = [
        Self::Contract,
        Self::Protocol,
        Self::Hardware,
        Self::Role,
        Self::Adapter,
        Self::Runtime,
        Self::Service,
        Self::Composition,
        Self::Facade,
        Self::Experiment,
        Self::Tool,
        Self::Hil,
        Self::Verification,
        Self::Application,
        Self::Platform,
    ];

    /// Its name in a manifest.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Contract => "contract",
            Self::Protocol => "protocol",
            Self::Hardware => "hardware",
            Self::Role => "role",
            Self::Adapter => "adapter",
            Self::Runtime => "runtime",
            Self::Service => "service",
            Self::Composition => "composition",
            Self::Facade => "facade",
            Self::Experiment => "experiment",
            Self::Tool => "tool",
            Self::Hil => "hil",
            Self::Verification => "verification",
            Self::Application => "application",
            Self::Platform => "platform",
        }
    }

    /// The scope the layer implies.
    pub const fn scope(self) -> Scope {
        match self {
            Self::Contract
            | Self::Protocol
            | Self::Hardware
            | Self::Role
            | Self::Adapter
            | Self::Runtime
            | Self::Service
            | Self::Composition
            | Self::Facade => Scope::Production,
            Self::Experiment => Scope::Experimental,
            Self::Tool | Self::Hil | Self::Verification | Self::Application | Self::Platform => {
                Scope::Development
            }
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Layer {
    type Err = ();

    fn from_str(name: &str) -> Result<Self, ()> {
        Self::ALL
            .into_iter()
            .find(|layer| layer.name() == name)
            .ok_or(())
    }
}

/// Where a package's code runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Platform {
    Portable,
    Host,
    /// One chip, by id.
    Chip(String),
    /// Shared by the chips of one family (`family` in `chip.toml`).
    Family(String),
}

/// What a verification package's code does for the evidence shards: decide
/// a verdict or a recorded set, whose sources a shard records, or only
/// render a report, whose sources a shard never records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Evidence {
    Verdict,
    Report,
}

/// What a HIL package's code does for a run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hil {
    /// Shapes what a run observes and reaches no stand code: the protocol,
    /// scenarios, the link, fixtures, target firmware, evidence.
    Observation,
    /// Drives runs: selection, images, workloads, the runner and its
    /// families. Its code shapes what a run observes, and it may call stand
    /// operation.
    Orchestration,
    /// Operates the stand only: boards, flashing, the stand file. It never
    /// shapes a passed observation, so evidence closures leave it out.
    Operation,
}

/// The layer of a host tool, HIL or verification package in
/// the host architecture ([host layers](../../../docs/architecture.md#host-layers)):
/// a package depends only on its own host layer and the layers below it.
/// Listed top-down.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HostLayer {
    /// A command line: argument parsing and calls (`cargo xtask`, `cargo
    /// hil`, the runner, `cargo tidy`, `cargo registers`, Blobray's CLI). Only entry crates spawn `cargo hil` or
    /// `cargo xtask`.
    Entry,
    /// Selection, experiments, analysis and HIL image orchestration.
    Orchestration,
    /// HIL execution: protocol, agent, link, scenarios, workloads,
    /// families, run bundles.
    Execution,
    /// Vendor verification and the register model; never HIL.
    Verification,
    /// The stand: boards, flashing, the arbiter, stand hosts; never HIL
    /// execution.
    Stand,
    /// Images and binary analysis; never HIL execution or the stand.
    Build,
    /// Processes, files, toolchain, chip profiles, the repository model and
    /// vendor pins; nothing above.
    Foundation,
}

impl HostLayer {
    /// Every host layer, top-down.
    pub const ALL: [Self; 7] = [
        Self::Entry,
        Self::Orchestration,
        Self::Execution,
        Self::Verification,
        Self::Stand,
        Self::Build,
        Self::Foundation,
    ];

    /// Its name in a manifest.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::Orchestration => "orchestration",
            Self::Execution => "execution",
            Self::Verification => "verification",
            Self::Stand => "stand",
            Self::Build => "build",
            Self::Foundation => "foundation",
        }
    }
}

impl fmt::Display for HostLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The owner of a host package: an application or a shared library family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostApp {
    Gate,
    Fw,
    Stand,
    Hil,
    Verification,
    Blobray,
    Registers,
    Foundation,
    Devices,
    Images,
    Analysis,
    Formats,
}

impl HostApp {
    pub const ALL: [Self; 12] = [
        Self::Gate,
        Self::Fw,
        Self::Stand,
        Self::Hil,
        Self::Verification,
        Self::Blobray,
        Self::Registers,
        Self::Foundation,
        Self::Devices,
        Self::Images,
        Self::Analysis,
        Self::Formats,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Fw => "fw",
            Self::Stand => "stand",
            Self::Hil => "hil",
            Self::Verification => "verification",
            Self::Blobray => "blobray",
            Self::Registers => "registers",
            Self::Foundation => "foundation",
            Self::Devices => "devices",
            Self::Images => "images",
            Self::Analysis => "analysis",
            Self::Formats => "formats",
        }
    }
}

/// What another application may link: a reviewed library or a data format.
/// Application implementation (including command lines) stays with its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostBoundary {
    Application,
    Library,
    Format,
}

impl HostBoundary {
    pub const ALL: [Self; 3] = [Self::Application, Self::Library, Self::Format];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Library => "library",
            Self::Format => "format",
        }
    }
}

/// The keys `[package.metadata.open-radio]` may hold.
pub const KEYS: &[&str] = &[
    "layer",
    "platform",
    "chip",
    "family",
    "evidence",
    "hil",
    "host-layer",
    "host-app",
    "host-boundary",
    "supported-feature-profiles",
    "test-feature-sets",
    "inputs",
];

/// One package's classification: every key of its
/// `[package.metadata.open-radio]` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Classification {
    pub layer: Layer,
    pub scope: Scope,
    pub platform: Platform,
    /// The role of a verification package in the evidence shards.
    pub evidence: Option<Evidence>,
    /// The role of a HIL package in a run.
    pub hil: Option<Hil>,
    /// The host layer of a development package that runs on the host:
    /// required for a host package, allowed for a portable one, absent
    /// otherwise.
    pub host_layer: Option<HostLayer>,
    /// Application owner and the boundary it exposes to other applications.
    pub host_app: Option<HostApp>,
    pub host_boundary: Option<HostBoundary>,
    /// Feature sets (comma-separated) that replace an all-features build
    /// when the package's features are alternatives.
    pub supported_feature_profiles: Vec<String>,
    /// Feature sets (comma-separated) its tests also run with besides its
    /// default features.
    pub test_feature_sets: Vec<String>,
    /// Patterns of repository files outside the package its tests read
    /// (see [`input_matches`]).
    pub inputs: Vec<String>,
}

/// The one package outside the `oer-` prefix.
pub const FACADE: &str = "open-esp-radio";

/// The classification of `package`.
pub fn of(package: &Package) -> Result<Classification, String> {
    let table = package.open_radio.as_ref().ok_or_else(|| {
        format!(
            "package {} lacks [package.metadata.open-radio]",
            package.name
        )
    })?;
    classify(&package.name, table, &|feature| {
        package.declares_feature(feature)
    })
}

/// Classifies the package `name` from its `[package.metadata.open-radio]`
/// `table`; `declares` says whether the package declares a feature.
pub fn classify(
    name: &str,
    table: &Table,
    declares: &dyn Fn(&str) -> bool,
) -> Result<Classification, String> {
    if let Some(key) = table.keys().find(|key| !KEYS.contains(&key.as_str())) {
        return Err(format!(
            "package {name} has unknown key open-radio.{key}{}",
            if key == "scope" {
                " (the layer implies the scope)"
            } else {
                ""
            }
        ));
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(format!("package {name}: open-radio.{key} must be a string")),
        }
    };
    let required = |key: &str| -> Result<String, String> {
        text(key)?.ok_or_else(|| format!("package {name} lacks open-radio.{key}"))
    };
    let layer_name = required("layer")?;
    let layer: Layer = layer_name
        .parse()
        .map_err(|()| format!("package {name} has unknown architecture layer `{layer_name}`"))?;
    let identifier = |key: &str| -> Result<Option<String>, String> {
        match text(key)? {
            None => Ok(None),
            Some(id) if oer_chip_profile::valid_identifier(&id) => Ok(Some(id)),
            Some(_) => Err(format!(
                "package {name} has invalid open-radio.{key} identifier"
            )),
        }
    };
    let platform = match (
        required("platform")?.as_str(),
        identifier("chip")?,
        identifier("family")?,
    ) {
        ("portable", None, None) => Platform::Portable,
        ("host", None, None) => Platform::Host,
        ("chip", Some(chip), None) => Platform::Chip(chip),
        ("family", None, Some(family)) => Platform::Family(family),
        _ => {
            return Err(format!(
                "package {name} has inconsistent platform/chip classification"
            ));
        }
    };
    let evidence = match (layer == Layer::Verification, text("evidence")?.as_deref()) {
        (true, Some("verdict")) => Some(Evidence::Verdict),
        (true, Some("report")) => Some(Evidence::Report),
        (true, _) => {
            return Err(format!(
                "package {name} of the verification layer needs open-radio.evidence = \"verdict\" or \"report\""
            ));
        }
        (false, None) => None,
        (false, Some(_)) => {
            return Err(format!(
                "package {name} outside the verification layer declares open-radio.evidence"
            ));
        }
    };
    let hil = match (layer == Layer::Hil, text("hil")?.as_deref()) {
        (true, Some("observation")) => Some(Hil::Observation),
        (true, Some("orchestration")) => Some(Hil::Orchestration),
        (true, Some("operation")) => Some(Hil::Operation),
        (true, _) => {
            return Err(format!(
                "package {name} of the hil layer needs open-radio.hil = \"observation\", \"orchestration\" or \"operation\""
            ));
        }
        (false, None) => None,
        (false, Some(_)) => {
            return Err(format!(
                "package {name} outside the hil layer declares open-radio.hil"
            ));
        }
    };
    let host_layer = match text("host-layer")? {
        None if layer.scope() == Scope::Development && platform == Platform::Host => {
            return Err(format!(
                "host package {name} needs open-radio.host-layer ({})",
                HostLayer::ALL.map(HostLayer::name).join(", ")
            ));
        }
        None => None,
        Some(_)
            if layer.scope() != Scope::Development
                || !matches!(platform, Platform::Host | Platform::Portable) =>
        {
            return Err(format!(
                "package {name} declares open-radio.host-layer but is no host or portable development package"
            ));
        }
        Some(value) => Some(
            HostLayer::ALL
                .into_iter()
                .find(|host| host.name() == value)
                .ok_or_else(|| format!("package {name} has unknown host layer `{value}`"))?,
        ),
    };
    let host_app = text("host-app")?
        .map(|value| {
            HostApp::ALL
                .into_iter()
                .find(|app| app.name() == value)
                .ok_or_else(|| format!("package {name} has unknown host application `{value}`"))
        })
        .transpose()?;
    let host_boundary = text("host-boundary")?
        .map(|value| {
            HostBoundary::ALL
                .into_iter()
                .find(|boundary| boundary.name() == value)
                .ok_or_else(|| format!("package {name} has unknown host boundary `{value}`"))
        })
        .transpose()?;
    if host_app.is_some() != host_boundary.is_some() || (host_app.is_some() && host_layer.is_none())
    {
        return Err(format!(
            "package {name}: host-app and host-boundary must be declared together with host-layer"
        ));
    }
    let list = |key: &str| -> Result<Vec<String>, String> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|item| !item.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            format!(
                                "package {name}: open-radio.{key} holds a non-string or empty entry"
                            )
                        })
                })
                .collect(),
            Some(_) => Err(format!(
                "package {name}: open-radio.{key} must be an array of strings"
            )),
        }
    };
    let feature_sets = |key: &str, noun: &str| -> Result<Vec<String>, String> {
        let sets = list(key)?;
        let mut unique = BTreeSet::new();
        for set in &sets {
            if !unique.insert(set) {
                return Err(format!("package {name} repeats {noun} {set}"));
            }
            let mut features = BTreeSet::new();
            for feature in set.split(',') {
                if feature.is_empty() || !features.insert(feature) || !declares(feature) {
                    return Err(format!("package {name} has invalid {noun} {set}"));
                }
            }
        }
        Ok(sets)
    };
    Ok(Classification {
        layer,
        scope: layer.scope(),
        platform,
        evidence,
        hil,
        host_layer,
        host_app,
        host_boundary,
        supported_feature_profiles: feature_sets(
            "supported-feature-profiles",
            "supported feature profile",
        )?,
        test_feature_sets: feature_sets("test-feature-sets", "test feature set")?,
        inputs: list("inputs")?,
    })
}

/// Every package is `oer-<tokens>` of lowercase letters and digits; the
/// public facade alone is `open-esp-radio`.
pub fn check_name(name: &str, layer: Layer) -> Result<(), String> {
    let valid = if layer == Layer::Facade {
        name == FACADE
    } else {
        name.strip_prefix("oer-").is_some_and(|tokens| {
            tokens.split('-').all(|token| {
                !token.is_empty()
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            })
        })
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "package {name} does not follow the `oer-<tokens>` naming rule (docs/architecture.md)"
        ))
    }
}

/// Whether the repository `path` matches the input `pattern` of a
/// package's `open-radio.inputs`: `/`-separated components where `*`
/// matches within one component and `**` any number of components; a
/// pattern that ends before the path matches everything below it.
pub fn input_matches(pattern: &str, path: &str) -> bool {
    fn component(pattern: &str, text: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == text,
            Some((head, tail)) => {
                text.starts_with(head)
                    && (head.len()..=text.len()).any(|start| component(tail, &text[start..]))
            }
        }
    }
    fn components(pattern: &[&str], path: &[&str]) -> bool {
        match (pattern.split_first(), path.split_first()) {
            (None, _) => true,
            (Some((&"**", rest)), _) => {
                (0..=path.len()).any(|skip| components(rest, &path[skip..]))
            }
            (Some((first, rest)), Some((name, others))) => {
                component(first, name) && components(rest, others)
            }
            (Some(_), None) => false,
        }
    }
    let pattern: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    components(&pattern, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classified(fields: &str) -> Result<Classification, String> {
        let table: Table = fields.parse().unwrap();
        classify("oer-x", &table, &|feature| {
            ["left", "right"].contains(&feature)
        })
    }

    #[test]
    fn the_layer_implies_the_scope() {
        let class = classified("layer = 'role'\nplatform = 'portable'").unwrap();
        assert_eq!((class.layer, class.scope), (Layer::Role, Scope::Production));
        let class = classified("layer = 'experiment'\nplatform = 'host'").unwrap();
        assert_eq!(class.scope, Scope::Experimental);
        let class = classified("layer = 'tool'\nplatform = 'host'\nhost-layer = 'build'").unwrap();
        assert_eq!(class.scope, Scope::Development);
        assert!(classified("layer = 'misc'\nplatform = 'host'").is_err());
        assert!(classified("platform = 'host'").is_err());
        let error = classified("scope = 'production'\nlayer = 'tool'\nplatform = 'host'");
        assert_eq!(
            error.unwrap_err(),
            "package oer-x has unknown key open-radio.scope (the layer implies the scope)"
        );
    }

    #[test]
    fn chip_and_family_identifiers_follow_their_platform() {
        let class = classified("layer = 'hardware'\nplatform = 'chip'\nchip = 'chip-a'").unwrap();
        assert_eq!(class.platform, Platform::Chip("chip-a".into()));
        let class =
            classified("layer = 'hardware'\nplatform = 'family'\nfamily = 'espressif'").unwrap();
        assert_eq!(class.platform, Platform::Family("espressif".into()));
        for invalid in [
            "layer = 'hardware'\nplatform = 'chip'",
            "layer = 'hardware'\nplatform = 'portable'\nchip = 'chip-a'",
            "layer = 'hardware'\nplatform = 'chip'\nchip = 'ESP32'",
            "layer = 'hardware'\nplatform = 'family'\nchip = 'chip-a'",
            // Code shared by chips is a family package or a register
            // library, never one selected per chip by a feature.
            "layer = 'hardware'\nplatform = 'selected'",
            "layer = 1\nplatform = 'host'",
        ] {
            assert!(classified(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn only_verification_and_hil_packages_declare_their_roles() {
        let verdict =
            classified("layer = 'verification'\nplatform = 'host'\nevidence = 'verdict'\nhost-layer = 'verification'").unwrap();
        assert_eq!(verdict.evidence, Some(Evidence::Verdict));
        assert!(classified("layer = 'verification'\nplatform = 'host'").is_err());
        assert!(classified("layer = 'tool'\nplatform = 'host'\nevidence = 'report'").is_err());
        for (role, expected) in [
            ("observation", Hil::Observation),
            ("orchestration", Hil::Orchestration),
            ("operation", Hil::Operation),
        ] {
            let class = classified(&format!(
                "layer = 'hil'\nplatform = 'host'\nhil = '{role}'\nhost-layer = 'execution'"
            ))
            .unwrap();
            assert_eq!(class.hil, Some(expected));
        }
        assert!(classified("layer = 'hil'\nplatform = 'host'\nhil = 'x'").is_err());
        assert!(classified("layer = 'tool'\nplatform = 'host'\nhil = 'operation'").is_err());
    }

    #[test]
    fn every_key_is_typed() {
        let class = classified(
            "layer = 'hil'\nplatform = 'chip'\nchip = 'chip-a'\nhil = 'observation'\n\
             supported-feature-profiles = ['left,right']\ntest-feature-sets = ['left', 'left,right']\n\
             inputs = ['docs/*.md']",
        )
        .unwrap();
        assert_eq!(class.supported_feature_profiles, ["left,right"]);
        assert_eq!(class.test_feature_sets, ["left", "left,right"]);
        assert_eq!(class.inputs, ["docs/*.md"]);
        for (invalid, message) in [
            (
                "test-feature-sets = ['left', 'left']",
                "repeats test feature set left",
            ),
            (
                "test-feature-sets = ['left,left']",
                "invalid test feature set left,left",
            ),
            (
                "supported-feature-profiles = ['missing']",
                "invalid supported feature profile missing",
            ),
            ("test-feature-sets = 'left'", "must be an array of strings"),
            ("inputs = ['']", "non-string or empty entry"),
        ] {
            let error = classified(&format!(
                "layer = 'tool'\nplatform = 'host'\nhost-layer = 'build'\n{invalid}"
            ))
            .unwrap_err();
            assert!(error.contains(message), "{invalid}: {error}");
        }
        // Every declared key is a field above; the list and the parser agree.
        assert_eq!(KEYS.len(), 12);
    }

    #[test]
    fn input_patterns_match_prefixes_and_globs() {
        assert!(input_matches("registers", "registers/esp32s31/a.toml"));
        assert!(input_matches(
            ".github/workflows/",
            ".github/workflows/ci.yml"
        ));
        assert!(!input_matches("registers", "registersx/a.toml"));
        let manifests = "hil/targets/**/Cargo.toml";
        assert!(input_matches(
            manifests,
            "hil/targets/chip-a/agent/Cargo.toml"
        ));
        assert!(input_matches(manifests, "hil/targets/Cargo.toml"));
        assert!(!input_matches(
            manifests,
            "hil/targets/chip-a/agent/src/main.rs"
        ));
        assert!(input_matches("docs/*.md", "docs/guide.md"));
        assert!(input_matches("a/*-x*/b", "a/one-x2/b"));
        assert!(!input_matches("docs/*.md", "docs/guide.txt"));
    }

    #[test]
    fn package_names_share_one_prefix_and_only_the_facade_is_branded() {
        for (name, layer) in [
            ("oer-ieee80211-sta", Layer::Protocol),
            ("oer-chip-a-ieee80211-embassy-net-owned", Layer::Adapter),
            ("oer-hil-runner", Layer::Hil),
            ("open-esp-radio", Layer::Facade),
        ] {
            check_name(name, layer).unwrap();
        }
        for (name, layer) in [
            ("open-esp-radio-hil-runner", Layer::Hil),
            ("oer-Wifi", Layer::Protocol),
            ("oer--mac", Layer::Protocol),
            ("oer-", Layer::Protocol),
            ("oer-radio", Layer::Facade),
        ] {
            assert!(check_name(name, layer).is_err(), "{name}");
        }
    }

    #[test]
    fn a_host_package_names_its_host_layer_and_only_development_code_has_one() {
        let class =
            classified("layer = 'tool'\nplatform = 'host'\nhost-layer = 'foundation'").unwrap();
        assert_eq!(class.host_layer, Some(HostLayer::Foundation));
        let error = classified("layer = 'tool'\nplatform = 'host'").unwrap_err();
        assert!(error.contains("needs open-radio.host-layer"), "{error}");
        // A portable development package may have one; it needs none.
        let class = classified("layer = 'tool'\nplatform = 'portable'").unwrap();
        assert_eq!(class.host_layer, None);
        let class =
            classified("layer = 'tool'\nplatform = 'portable'\nhost-layer = 'build'").unwrap();
        assert_eq!(class.host_layer, Some(HostLayer::Build));
        for invalid in [
            "layer = 'role'\nplatform = 'portable'\nhost-layer = 'build'",
            "layer = 'tool'\nplatform = 'chip'\nchip = 'chip-a'\nhost-layer = 'build'",
            "layer = 'tool'\nplatform = 'host'\nhost-layer = 'middle'",
        ] {
            assert!(classified(invalid).is_err(), "{invalid}");
        }
        assert!(HostLayer::Entry < HostLayer::Foundation);
    }

    #[test]
    fn application_ownership_and_boundary_are_typed_together() {
        let host = "layer = 'tool'\nplatform = 'host'\nhost-layer = 'entry'\n";
        let class = classified(&format!(
            "{host}host-app = 'fw'\nhost-boundary = 'application'"
        ))
        .unwrap();
        assert_eq!(class.host_app, Some(HostApp::Fw));
        assert_eq!(class.host_boundary, Some(HostBoundary::Application));
        for fields in [
            "host-app = 'fw'",
            "host-boundary = 'library'",
            "host-app = 'unknown'\nhost-boundary = 'application'",
            "host-app = 'fw'\nhost-boundary = 'unknown'",
        ] {
            assert!(classified(&format!("{host}{fields}")).is_err(), "{fields}");
        }
        assert!(classified("layer = 'hardware'\nplatform = 'portable'\nhost-app = 'fw'\nhost-boundary = 'application'").is_err());
    }
}
