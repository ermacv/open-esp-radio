//! The registry of every message: identities are unique and well formed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oer_hil_protocol::{FRAMING_VERSION, Key, Kind, MESSAGES_LOCK, MessageInfo, registry};

const MODULES: [&str; 4] = ["base", "system", "network", "telemetry"];

#[test]
fn no_two_messages_share_a_key_or_a_path() {
    let mut keys: BTreeMap<Key, &MessageInfo> = BTreeMap::new();
    let mut paths: BTreeMap<&str, &MessageInfo> = BTreeMap::new();
    for message in registry() {
        if let Some(other) = keys.insert(message.key, message) {
            panic!(
                "{} and {} share key {}",
                other.path, message.path, message.key
            );
        }
        if let Some(other) = paths.insert(message.path, message) {
            panic!(
                "{} is declared twice, with keys {} and {}",
                message.path, other.key, message.key
            );
        }
    }
}

#[test]
fn every_path_lives_in_a_module() {
    for message in registry() {
        let (module, rest) = message
            .path
            .split_once('/')
            .unwrap_or_else(|| panic!("{} names no module", message.path));
        assert!(
            MODULES.contains(&module),
            "{}: unknown module",
            message.path
        );
        assert!(
            !rest.is_empty()
                && rest.split('/').all(|segment| !segment.is_empty()
                    && segment.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-')),
            "{}: a path is lowercase kebab-case segments",
            message.path
        );
    }
}

#[test]
fn a_key_hashes_its_path_and_schema() {
    for message in registry() {
        let hashed = Key(
            postcard_schema::key::hash::fnv1a64_owned::hash_ty_path_owned(
                message.path,
                &message.schema.into(),
            ),
        );
        assert_eq!(hashed, message.key, "{}", message.path);
    }
}

#[test]
fn every_module_declares_something() {
    for module in MODULES {
        assert!(
            registry().any(|message| message.path.starts_with(&format!("{module}/"))),
            "{module} declares no message"
        );
    }
}

#[test]
fn a_property_carries_no_payload() {
    for message in registry().filter(|message| message.kind == Kind::Property) {
        assert!(
            (message.decode_json)(&[]).is_some(),
            "{}: a property decodes from an empty payload",
            message.path
        );
    }
}

/// The variable that makes [`the_lock_is_the_registry`] rewrite the lock.
const WRITE_LOCK: &str = "OER_HIL_PROTOCOL_LOCK";

/// The framework's sources: identity, framing and the envelope. They name
/// no module.
const FRAMEWORK: [&str; 5] = ["lib.rs", "key.rs", "framing.rs", "io.rs", "envelope.rs"];

/// Every Rust source under `path`, tests excluded: a test may use any module
/// without making it a dependency of the wire.
fn sources(path: &Path, found: &mut Vec<PathBuf>) {
    if path.is_file() {
        found.push(path.to_path_buf());
        return;
    }
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if name == "tests" || name == "tests.rs" {
            continue;
        }
        if path.is_dir() || name.ends_with(".rs") {
            sources(&path, found);
        }
    }
}

/// The modules `path`'s sources name through `crate::<module>`.
fn referenced(path: &Path) -> BTreeSet<&'static str> {
    let mut files = Vec::new();
    sources(path, &mut files);
    let mut modules = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (at, _) in text.match_indices("crate::") {
            let rest = &text[at + "crate::".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if let Some(module) = MODULES.iter().find(|module| **module == name) {
                modules.insert(*module);
            }
        }
    }
    modules
}

fn source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Each module and the modules it uses, from its directory's sources.
fn dependencies() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    MODULES
        .iter()
        .map(|module| {
            let mut used = referenced(&source().join(module));
            used.remove(module);
            (*module, used)
        })
        .collect()
}

#[test]
fn the_framework_names_no_module() {
    for file in FRAMEWORK {
        let used = referenced(&source().join(file));
        assert!(used.is_empty(), "{file} uses the modules {used:?}");
    }
}

#[test]
fn modules_depend_without_a_cycle() {
    let dependencies = dependencies();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    while placed.len() < dependencies.len() {
        let ready: Vec<&str> = dependencies
            .iter()
            .filter(|(module, used)| !placed.contains(*module) && used.is_subset(&placed))
            .map(|(module, _)| *module)
            .collect();
        assert!(
            !ready.is_empty(),
            "the modules {:?} depend on each other",
            dependencies
                .keys()
                .filter(|module| !placed.contains(*module))
                .collect::<Vec<_>>()
        );
        placed.extend(ready);
    }
}

fn lock() -> String {
    let mut messages: Vec<&MessageInfo> = registry().collect();
    messages.sort_by_key(|message| message.path);
    let mut lock = format!(
        "# The HIL wire of this revision. Regenerate with\n\
         # `{WRITE_LOCK}=write cargo test -p oer-hil-protocol --features registry --test registry`.\n\
         framing {FRAMING_VERSION}\n\
         [modules]\n"
    );
    // A module and the modules its payloads use: a runner that speaks a
    // module needs these too.
    for (module, used) in dependencies() {
        lock.push_str(module);
        for dependency in used {
            lock.push(' ');
            lock.push_str(dependency);
        }
        lock.push('\n');
    }
    lock.push_str("[messages]\n");
    for message in messages {
        let kind = match message.kind {
            Kind::Endpoint => "endpoint",
            Kind::Topic => "topic",
            Kind::Property => "property",
        };
        lock.push_str(&format!("{} {} {kind}\n", message.path, message.key));
    }
    lock
}

#[test]
fn the_lock_is_the_registry() {
    let lock = lock();
    if std::env::var(WRITE_LOCK).as_deref() == Ok("write") {
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("messages.lock"),
            &lock,
        )
        .unwrap();
        return;
    }
    assert!(
        lock == MESSAGES_LOCK,
        "messages.lock differs from the registry; review the wire change and rerun with \
         {WRITE_LOCK}=write"
    );
}
