#![cfg_attr(test, allow(clippy::disallowed_macros))]
//! The registry of every message: identities are unique and well formed.

use std::collections::{BTreeMap, BTreeSet};

use oer_hil_protocol::{FRAMING_VERSION, Key, Kind, MESSAGES_LOCK, MessageInfo, registry};

const MODULES: [&str; 9] = [
    "base",
    "system",
    "network",
    "bluetooth",
    "telemetry",
    "phy",
    "lower-mac",
    "health",
    "wifi",
];

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

/// Framework files name no module; module files may depend on each other.
const FRAMEWORK: [&str; 5] = ["lib.rs", "key.rs", "framing.rs", "io.rs", "envelope.rs"];

const SOURCES: &[(&str, &str)] = &[
    ("health/mod.rs", include_str!("../src/health/mod.rs")),
    (
        "health/records.rs",
        include_str!("../src/health/records.rs"),
    ),
    ("base/boot.rs", include_str!("../src/base/boot.rs")),
    (
        "base/image_keys.rs",
        include_str!("../src/base/image_keys.rs"),
    ),
    ("base/mod.rs", include_str!("../src/base/mod.rs")),
    (
        "bluetooth/gatt.rs",
        include_str!("../src/bluetooth/gatt.rs"),
    ),
    ("bluetooth/mod.rs", include_str!("../src/bluetooth/mod.rs")),
    (
        "bluetooth/payload.rs",
        include_str!("../src/bluetooth/payload.rs"),
    ),
    (
        "bluetooth/secure_gatt.rs",
        include_str!("../src/bluetooth/secure_gatt.rs"),
    ),
    ("envelope.rs", include_str!("../src/envelope.rs")),
    ("feature_keys.rs", include_str!("../src/feature_keys.rs")),
    ("framing.rs", include_str!("../src/framing.rs")),
    ("image_keys.rs", include_str!("../src/image_keys.rs")),
    ("io.rs", include_str!("../src/io.rs")),
    ("key.rs", include_str!("../src/key.rs")),
    ("lib.rs", include_str!("../src/lib.rs")),
    ("lower_mac/mod.rs", include_str!("../src/lower_mac/mod.rs")),
    (
        "lower_mac/payload.rs",
        include_str!("../src/lower_mac/payload.rs"),
    ),
    ("network/mod.rs", include_str!("../src/network/mod.rs")),
    (
        "network/session.rs",
        include_str!("../src/network/session.rs"),
    ),
    ("network/state.rs", include_str!("../src/network/state.rs")),
    (
        "network/stream_pattern.rs",
        include_str!("../src/network/stream_pattern.rs"),
    ),
    (
        "network/traffic.rs",
        include_str!("../src/network/traffic.rs"),
    ),
    (
        "network/udp_probe.rs",
        include_str!("../src/network/udp_probe.rs"),
    ),
    ("phy/cache.rs", include_str!("../src/phy/cache.rs")),
    ("phy/capture.rs", include_str!("../src/phy/capture.rs")),
    ("phy/hold.rs", include_str!("../src/phy/hold.rs")),
    ("phy/mod.rs", include_str!("../src/phy/mod.rs")),
    ("system/mod.rs", include_str!("../src/system/mod.rs")),
    (
        "system/payload.rs",
        include_str!("../src/system/payload.rs"),
    ),
    ("telemetry/mod.rs", include_str!("../src/telemetry/mod.rs")),
    (
        "telemetry/profile.rs",
        include_str!("../src/telemetry/profile.rs"),
    ),
    ("trace_header.rs", include_str!("../src/trace_header.rs")),
    ("wifi/mod.rs", include_str!("../src/wifi/mod.rs")),
    (
        "wifi/credentials.rs",
        include_str!("../src/wifi/credentials.rs"),
    ),
    ("wifi/link.rs", include_str!("../src/wifi/link.rs")),
];

/// Read only source text embedded at compile time, excluding test modules.
fn referenced(path: &str) -> BTreeSet<&'static str> {
    let mut modules = BTreeSet::new();
    for (_, text) in SOURCES
        .iter()
        .filter(|(file, _)| *file == path || file.starts_with(&format!("{path}/")))
    {
        for (at, _) in text.match_indices("crate::") {
            let rest = &text[at + "crate::".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if let Some(module) = MODULES
                .iter()
                .find(|module| module.replace('-', "_") == name)
            {
                modules.insert(*module);
            }
        }
    }
    modules
}

/// Each module and the modules it uses, from its directory's sources.
fn dependencies() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    MODULES
        .iter()
        .map(|module| {
            let mut used = referenced(&module.replace('-', "_"));
            used.remove(module);
            (*module, used)
        })
        .collect()
}

#[test]
fn the_framework_names_no_module() {
    for file in FRAMEWORK {
        let used = referenced(file);
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
        "# The HIL wire of this revision.\n\
         # The registry test prints the complete replacement when it differs.\n\
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
    assert_eq!(
        lock, MESSAGES_LOCK,
        "review the printed wire change before replacing messages.lock"
    );
}
