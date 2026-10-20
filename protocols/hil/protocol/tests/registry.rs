//! The registry of every message: identities are unique and well formed.

use std::collections::BTreeMap;

use oer_hil_protocol::{Key, Kind, MessageInfo, registry};

const MODULES: [&str; 6] = ["base", "system", "network", "bluetooth", "telemetry", "phy"];

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
