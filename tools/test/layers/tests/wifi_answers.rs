//! Every public enum of the Wi-Fi contracts and cores is in the answers
//! registry (`wifi-answers.toml`), and every variant of an answer has its
//! kind: the check stands at the door (the enum's declaration), not at a
//! spelling.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration-test helpers report malformed source and broken fixture invariants"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, Visibility};

/// The packages whose public enums the registry holds. A package of the
/// Wi-Fi path's cores joins as it is rewritten, and never leaves.
const PACKAGES: &[&str] = &[
    "protocols/ieee80211/contracts/port",
    "protocols/ieee80211/core/installation",
    "protocols/ieee80211/core/datapath",
];

/// Each package's `own-state` and `kept-out` variants. Both only shrink: a
/// rise fails, and so does a fall the entry does not follow.
const COUNTS: &[(&str, usize, usize)] = &[
    ("protocols/ieee80211/contracts/port", 47, 5),
    ("protocols/ieee80211/core/installation", 0, 0),
    ("protocols/ieee80211/core/datapath", 5, 1),
];

/// The kinds of a variant of an answer, and what each needs.
const KINDS: &[(&str, Needs)] = &[
    ("outcome", Needs::Nothing),
    ("hardware", Needs::Nothing),
    ("air", Needs::Nothing),
    ("peer", Needs::Nothing),
    ("host", Needs::Nothing),
    ("input", Needs::Nothing),
    ("load", Needs::Next),
    ("own-state", Needs::By),
    ("kept-out", Needs::By),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Needs {
    Nothing,
    /// The event, waker or instant that ends the load.
    Next,
    /// The step that removes it.
    By,
}

/// The public enums of one package: each name with its variants.
type Enums = BTreeMap<String, Vec<String>>;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repository root")
}

fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr.parse_args::<syn::Meta>().is_ok_and(
                    |meta| matches!(meta, syn::Meta::Path(path) if path.is_ident("test")),
                ))
    })
}

fn collect(items: &[Item], enums: &mut Enums) {
    for item in items {
        match item {
            Item::Enum(item) if !test_only(&item.attrs) => {
                if matches!(item.vis, Visibility::Public(_)) {
                    let variants = item
                        .variants
                        .iter()
                        .map(|variant| variant.ident.to_string())
                        .collect();
                    enums.insert(item.ident.to_string(), variants);
                }
            }
            Item::Mod(module) if !test_only(&module.attrs) => {
                if let Some((_, items)) = &module.content {
                    collect(items, enums);
                }
            }
            _ => {}
        }
    }
}

/// The non-test sources of `directory`: every `.rs` file but a `tests.rs`
/// and those under a `tests` directory.
fn sources(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = std::fs::read_dir(directory)
        .expect("read a source directory")
        .map(|entry| entry.expect("a directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if path.is_dir() {
            if name != "tests" {
                sources(&path, files);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            files.push(path);
        }
    }
}

fn package_enums(package: &Path) -> Enums {
    let mut files = Vec::new();
    sources(&package.join("src"), &mut files);
    let mut enums = Enums::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read a source file");
        let parsed = syn::parse_file(&text).expect("a source file parses");
        collect(&parsed.items, &mut enums);
    }
    enums
}

/// What is wrong between the sources' enums and the registry's entries,
/// with each package's two counts.
fn check(
    found: &BTreeMap<String, Enums>,
    registry: &toml::Value,
    counts: &[(&str, usize, usize)],
) -> Vec<String> {
    let mut wrong = Vec::new();
    let kinds = KINDS.iter().copied().collect::<BTreeMap<_, _>>();
    for (package, enums) in found {
        let Some(table) = registry.get(package).and_then(toml::Value::as_table) else {
            wrong.push(format!("{package}: no table in the registry"));
            continue;
        };
        let mut own_state = 0;
        let mut kept_out = 0;
        for (name, variants) in enums {
            let Some(entry) = table.get(name) else {
                wrong.push(format!("{package}: {name} is not in the registry"));
                continue;
            };
            if entry.as_str() == Some("vocabulary") {
                continue;
            }
            if entry.get("kind").and_then(toml::Value::as_str) != Some("answer") {
                wrong.push(format!(
                    "{package}: {name} is neither \"vocabulary\" nor kind = \"answer\""
                ));
                continue;
            }
            let listed = entry
                .get("variants")
                .and_then(toml::Value::as_table)
                .cloned()
                .unwrap_or_default();
            for variant in variants {
                let Some(variant_entry) = listed.get(variant) else {
                    wrong.push(format!("{package}: {name}::{variant} has no kind"));
                    continue;
                };
                let kind = variant_entry.get("kind").and_then(toml::Value::as_str);
                let next = variant_entry
                    .get("next")
                    .and_then(toml::Value::as_str)
                    .filter(|text| !text.trim().is_empty());
                let by = variant_entry
                    .get("by")
                    .and_then(toml::Value::as_str)
                    .filter(|text| !text.trim().is_empty());
                match kind.and_then(|kind| kinds.get(kind)) {
                    None => wrong.push(format!(
                        "{package}: {name}::{variant} has no kind of {:?}",
                        KINDS.iter().map(|(kind, _)| *kind).collect::<Vec<_>>()
                    )),
                    Some(Needs::Next) if next.is_none() || by.is_some() => wrong.push(format!(
                        "{package}: {name}::{variant} is load: it names its next, and no by"
                    )),
                    Some(Needs::By) if by.is_none() || next.is_some() => wrong.push(format!(
                        "{package}: {name}::{variant} is {}: it names the step that removes it, and no next",
                        kind.unwrap_or_default()
                    )),
                    Some(Needs::Nothing) if by.is_some() || next.is_some() => wrong.push(format!(
                        "{package}: {name}::{variant} is {}: it names neither next nor by",
                        kind.unwrap_or_default()
                    )),
                    _ => {}
                }
                match kind {
                    Some("own-state") => own_state += 1,
                    Some("kept-out") => kept_out += 1,
                    _ => {}
                }
            }
            for variant in listed.keys() {
                if !variants.contains(variant) {
                    wrong.push(format!(
                        "{package}: {name}::{variant} is gone, remove its entry"
                    ));
                }
            }
        }
        for name in table.keys() {
            if !enums.contains_key(name) {
                wrong.push(format!("{package}: {name} is gone, remove its entry"));
            }
        }
        match counts.iter().find(|(listed, ..)| listed == package) {
            None => wrong.push(format!("{package}: no counts")),
            Some((_, own, kept)) => {
                if (own_state, kept_out) != (*own, *kept) {
                    wrong.push(format!(
                        "{package}: own-state {own_state} and kept-out {kept_out}, listed {own} and {kept}: they only shrink, and the list follows"
                    ));
                }
            }
        }
    }
    wrong
}

fn registry() -> toml::Value {
    let path = root().join("tools/test/layers/wifi-answers.toml");
    toml::from_str(&std::fs::read_to_string(path).expect("read the answers registry"))
        .expect("the answers registry is TOML")
}

#[test]
fn every_public_enum_of_the_wifi_contracts_and_cores_is_registered_with_its_kind() {
    let root = root();
    let found = PACKAGES
        .iter()
        .map(|package| ((*package).to_owned(), package_enums(&root.join(package))))
        .collect::<BTreeMap<_, _>>();
    let registry = registry();
    let mut wrong = check(&found, &registry, COUNTS);
    let registered = registry
        .as_table()
        .map(|table| table.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    for package in registered {
        if !PACKAGES.contains(&package.as_str()) {
            wrong.push(format!("{package}: in the registry, not in PACKAGES"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_registry_s_door_finds_each_wrong_entry() {
    let registry: toml::Value = toml::from_str(
        r#"
["p"]
Rate = "vocabulary"
Refusal.kind = "answer"
Refusal.variants.Busy = { kind = "own-state", by = "B3d" }
Refusal.variants.Later = { kind = "load", next = "the waker" }
"#,
    )
    .expect("fixture");
    let enums = |extra: &[(&str, &[&str])]| {
        let mut enums = Enums::new();
        enums.insert("Rate".into(), vec!["Fast".into()]);
        enums.insert("Refusal".into(), vec!["Busy".into(), "Later".into()]);
        for (name, variants) in extra {
            enums.insert(
                (*name).into(),
                variants
                    .iter()
                    .map(|variant| (*variant).to_owned())
                    .collect(),
            );
        }
        [("p".to_owned(), enums)]
            .into_iter()
            .collect::<BTreeMap<_, _>>()
    };
    let counts: &[(&str, usize, usize)] = &[("p", 1, 0)];
    assert_eq!(check(&enums(&[]), &registry, counts), Vec::<String>::new());
    // A variant added to an answer.
    assert_eq!(
        check(
            &enums(&[("Refusal", &["Busy", "Later", "Stopping"])]),
            &registry,
            counts
        ),
        ["p: Refusal::Stopping has no kind"]
    );
    // A new public enum.
    assert_eq!(
        check(&enums(&[("Quarantine", &["Held"])]), &registry, counts),
        ["p: Quarantine is not in the registry"]
    );
    // An own-state variant re-kinded as outcome, its count left.
    let rekinded: toml::Value = toml::from_str(
        r#"
["p"]
Rate = "vocabulary"
Refusal.kind = "answer"
Refusal.variants.Busy = { kind = "outcome" }
Refusal.variants.Later = { kind = "load", next = "the waker" }
"#,
    )
    .expect("fixture");
    assert_eq!(
        check(&enums(&[]), &rekinded, counts),
        ["p: own-state 0 and kept-out 0, listed 1 and 0: they only shrink, and the list follows"]
    );
    // A load without its next.
    let no_next: toml::Value = toml::from_str(
        r#"
["p"]
Rate = "vocabulary"
Refusal.kind = "answer"
Refusal.variants.Busy = { kind = "own-state", by = "B3d" }
Refusal.variants.Later = { kind = "load" }
"#,
    )
    .expect("fixture");
    assert_eq!(
        check(&enums(&[]), &no_next, counts),
        ["p: Refusal::Later is load: it names its next, and no by"]
    );
    // An own-state variant without its step, and an entry that is gone.
    let no_by: toml::Value = toml::from_str(
        r#"
["p"]
Rate = "vocabulary"
Gone = "vocabulary"
Refusal.kind = "answer"
Refusal.variants.Busy = { kind = "own-state" }
Refusal.variants.Later = { kind = "load", next = "the waker" }
"#,
    )
    .expect("fixture");
    assert_eq!(
        check(&enums(&[]), &no_by, counts),
        [
            "p: Refusal::Busy is own-state: it names the step that removes it, and no next",
            "p: Gone is gone, remove its entry",
        ]
    );
}

#[test]
fn the_door_sees_public_enums_of_inline_modules_and_skips_test_code() {
    let file = syn::parse_file(
        r#"
pub enum Kept { A }
enum Private { B }
pub(crate) enum Crate { C }
#[cfg(test)]
pub enum OnlyInTests { D }
pub mod inner { pub enum Inner { E } }
#[cfg(test)]
mod tests { pub enum InTests { F } }
"#,
    )
    .expect("fixture");
    let mut enums = Enums::new();
    collect(&file.items, &mut enums);
    assert_eq!(
        enums.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Inner", "Kept"]
    );
}
