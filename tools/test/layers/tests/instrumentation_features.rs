//! Instrumentation is never a Cargo feature of a library.
//!
//! Features are additive across one build: a feature one image requests on a
//! shared library is on for every image built with it. Observation is
//! switched by the image's `oer_trace`, `oer_profile` and `oer_debug_control`
//! cfgs (`docs/architecture.md`, Instrumentation and probes), so no library
//! below `chips/`, `protocols/` or `portable/` declares a feature, explicit or
//! implied by an optional dependency, whose name carries an instrumentation
//! word.
#![allow(clippy::expect_used)]

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// The trees of libraries; images and tools build programs, not shared code.
const LIBRARY_TREES: [&str; 3] = ["chips/", "protocols/", "portable/"];

/// Words of a feature name, split at `-` and `_`, that name observation,
/// cost measurement, forcing knobs or their storage.
const INSTRUMENTATION_WORDS: &[&str] = &[
    "counter",
    "counters",
    "debug",
    "diagnostic",
    "diagnostics",
    "health",
    "instrument",
    "instrumentation",
    "instrumented",
    "metrics",
    "observation",
    "observe",
    "observer",
    "probe",
    "profile",
    "profiling",
    "stats",
    "telemetry",
    "trace",
    "tracing",
];

/// A library feature the tree still has against the rule, with what removes
/// it. The list only shrinks.
type Deviation = (&'static str, &'static str, &'static str);

const KNOWN_INSTRUMENTATION_FEATURES: &[Deviation] = &[
    (
        "chips/esp32s31/phy",
        "radio-health",
        "PHY observations move to the observer (#848, instrumentation PHY stage)",
    ),
    (
        "protocols/hil/agent",
        "health",
        "health records move to the agent's trace capture service (#848, instrumentation I1)",
    ),
    (
        "protocols/hil/protocol",
        "health",
        "health messages move to the agent's trace capture service (#848, instrumentation I1)",
    ),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root")
}

fn read_toml(path: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(path).expect("read TOML"))
        .expect("valid repository TOML")
}

/// Whether a feature name carries an instrumentation word.
fn instrumentation_name(feature: &str) -> bool {
    feature
        .split(['-', '_'])
        .any(|word| INSTRUMENTATION_WORDS.contains(&word.to_ascii_lowercase().as_str()))
}

/// The features a manifest declares: its `[features]` keys and the implicit
/// feature of each optional dependency that no feature names as `dep:`.
fn declared_features(manifest: &toml::Value) -> BTreeSet<String> {
    let explicit = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    let named_as_dep = explicit
        .values()
        .filter_map(toml::Value::as_array)
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(|value| value.strip_prefix("dep:"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let targets = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values());
    let implicit = core::iter::once(manifest)
        .chain(targets)
        .flat_map(|scope| {
            ["dependencies", "build-dependencies"]
                .into_iter()
                .filter_map(|table| scope.get(table).and_then(toml::Value::as_table))
        })
        .flat_map(|dependencies| dependencies.iter())
        .filter(|(_, spec)| spec.get("optional").and_then(toml::Value::as_bool) == Some(true))
        .map(|(alias, _)| alias.clone())
        .filter(|alias| !named_as_dep.contains(alias));
    explicit.keys().cloned().chain(implicit).collect()
}

/// The `(package path, feature)` pairs of libraries whose feature names carry
/// an instrumentation word.
fn instrumentation_features<'a>(
    manifests: impl IntoIterator<Item = (&'a str, &'a toml::Value)>,
) -> BTreeSet<(String, String)> {
    manifests
        .into_iter()
        .filter(|(path, _)| LIBRARY_TREES.iter().any(|tree| path.starts_with(tree)))
        .flat_map(|(path, manifest)| {
            declared_features(manifest)
                .into_iter()
                .filter(|feature| instrumentation_name(feature))
                .map(move |feature| (path.to_owned(), feature))
        })
        .collect()
}

/// A new pair and a known entry whose pair is gone both fail.
fn deviation_differences(found: &BTreeSet<(String, String)>, known: &[Deviation]) -> Vec<String> {
    let known = known
        .iter()
        .map(|(path, feature, _)| ((*path).to_owned(), (*feature).to_owned()))
        .collect::<BTreeSet<_>>();
    let new = found
        .difference(&known)
        .map(|(path, feature)| format!("new: {path} declares feature `{feature}`"));
    let stale = known
        .difference(found)
        .map(|(path, feature)| format!("gone, drop the entry: {path} `{feature}`"));
    new.chain(stale).collect()
}

#[test]
fn no_library_declares_instrumentation_as_a_cargo_feature() {
    let root = root();
    let workspace = read_toml(&root.join("Cargo.toml"));
    let manifests = workspace["workspace"]["members"]
        .as_array()
        .expect("explicit workspace members")
        .iter()
        .map(|member| {
            let path = member.as_str().expect("member path").to_owned();
            let manifest = read_toml(&root.join(&path).join("Cargo.toml"));
            (path, manifest)
        })
        .collect::<Vec<_>>();
    assert!(manifests.len() > 100, "the workspace members are read");
    let found = instrumentation_features(
        manifests
            .iter()
            .map(|(path, manifest)| (path.as_str(), manifest)),
    );
    let differences = deviation_differences(&found, KNOWN_INSTRUMENTATION_FEATURES);
    assert!(
        differences.is_empty(),
        "instrumentation is the image's cfg, not a library feature:\n{}",
        differences.join("\n")
    );
}

#[test]
fn the_feature_rule_reads_words_implicit_features_and_only_library_trees() {
    let manifest = |text: &str| -> toml::Value { toml::from_str(text).expect("manifest") };
    let library = manifest(
        r#"
[features]
default = []
radio-health = []
test-support = []
traces = []
oer_trace = []
with-probe = ["dep:probe-tool"]
[dependencies]
probe-tool = { version = "1", optional = true }
debug-log = { version = "1", optional = true }
[target.'cfg(target_os = "none")'.dependencies]
oer-trace = { workspace = true, optional = true }
[build-dependencies]
profile-gen = { version = "1", optional = true }
[dev-dependencies]
health-fixture = "1"
"#,
    );
    let image = manifest("[features]\ndiagnostic-phy = []\n");
    let found = instrumentation_features([
        ("chips/esp32s31/example", &library),
        ("firmware/hil/example", &image),
        ("tools/example", &image),
    ]);
    let pairs = |names: &[&str]| {
        names
            .iter()
            .map(|feature| ("chips/esp32s31/example".to_owned(), (*feature).to_owned()))
            .collect::<BTreeSet<_>>()
    };
    // `traces` is not the word `trace`; `with-probe` names its optional
    // dependency through `dep:`, so `probe-tool` implies no feature; the
    // dev-dependency implies none either.
    assert_eq!(
        found,
        pairs(&[
            "debug-log",
            "oer-trace",
            "oer_trace",
            "profile-gen",
            "radio-health",
            "with-probe",
        ])
    );
}

#[test]
fn the_feature_deviation_list_fails_on_a_new_feature_and_on_a_removed_one() {
    let known: &[Deviation] = &[("a", "trace", "a stage"), ("b", "health", "a stage")];
    let pair = |path: &str, feature: &str| (path.to_owned(), feature.to_owned());
    assert!(
        deviation_differences(
            &BTreeSet::from([pair("a", "trace"), pair("b", "health")]),
            known
        )
        .is_empty()
    );
    assert_eq!(
        deviation_differences(
            &BTreeSet::from([pair("a", "trace"), pair("c", "probe")]),
            known
        ),
        [
            "new: c declares feature `probe`",
            "gone, drop the entry: b `health`",
        ]
    );
}
