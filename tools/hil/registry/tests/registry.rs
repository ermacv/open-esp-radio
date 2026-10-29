//! Only the explicit lifecycle and generic capture migrations change the reviewed wire.
use oer_hil_protocol::{FRAMING_VERSION, Key, Kind, MessageInfo};
use oer_hil_registry::{LOCKS, messages_lock, registry};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn station_control_migration_preserves_every_other_reviewed_message()
-> Result<(), oer_hil_registry::LockError> {
    assert_eq!(messages_lock()?, migrated_wire()?);
    Ok(())
}

fn migrated_wire() -> Result<String, oer_hil_registry::LockError> {
    use oer_hil_protocol::{
        Message,
        lifecycle::{RequestReset, ResetAccepted},
    };
    let previous = include_str!("before.messages.lock")
        .replace("[modules]\n", "[modules]\nlifecycle\n")
        .replace(
            "system/panic/inject 4a8069a58696abc1 endpoint",
            &format!("{} {} endpoint", RequestReset::PATH, RequestReset::KEY),
        )
        .replace(
            "system/panic/injected 9f469405c60e21be topic",
            &format!("{} {} topic", ResetAccepted::PATH, ResetAccepted::KEY),
        );
    let previous = previous
        .lines()
        .filter(|line| {
            ![
                "wifi/station/link ",
                "wifi/station/stopped ",
                "wifi/station/service-ended ",
                "wifi/station/probe ",
                "wifi/station/probe/ready ",
                "wifi/station/probe/finished ",
                "lower-mac/case/trace ",
                "health/counters ",
                "health/counters/read ",
                "health/signals ",
                "health/signals/read ",
                "health/tracking ",
                "health/tracking/read ",
                "health/measurements ",
                "health/measurements/read ",
            ]
            .iter()
            .any(|path| line.starts_with(path))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut previous = previous.replace("[modules]\n", "[modules]\ncapture\n");
    previous.push('\n');
    for message in oer_hil_protocol::capture::MESSAGES {
        let kind = match message.kind {
            Kind::Endpoint => "endpoint",
            Kind::Topic => "topic",
            Kind::Property => "property",
        };
        previous.push_str(&format!("{} {} {}\n", message.path, message.key, kind));
    }
    for message in oer_ieee80211_control::MESSAGES.iter().filter(|message| {
        matches!(
            message.path,
            "wifi/station/link"
                | "wifi/station/stopped"
                | "wifi/station/service-ended"
                | "wifi/station/transmit/refused"
                | "wifi/station/msdu/refused"
                | "wifi/station/connect"
                | "wifi/station/disconnect"
                | "wifi/station/scan"
                | "wifi/station/status/get"
                | "wifi/station/status"
                | "wifi/station/admission"
                | "wifi/station/command/finished"
        )
    }) {
        let kind = match message.kind {
            Kind::Endpoint => "endpoint",
            Kind::Topic => "topic",
            Kind::Property => "property",
        };
        previous.push_str(&format!("{} {} {}\n", message.path, message.key, kind));
    }
    for message in oer_network_control::MESSAGES.iter().filter(|message| {
        matches!(
            message.path,
            "network/datagrams/start" | "network/datagrams/stop" | "network/datagrams/finished"
        )
    }) {
        let kind = match message.kind {
            Kind::Endpoint => "endpoint",
            Kind::Topic => "topic",
            Kind::Property => "property",
        };
        previous.push_str(&format!("{} {} {kind}\n", message.path, message.key));
    }
    let message = oer_ieee80211_conformance_control::MESSAGES
        .iter()
        .find(|message| message.path == "lower-mac/case/trace")
        .expect("conformance topic");
    previous.push_str(&format!("{} {} topic\n", message.path, message.key));
    oer_hil_registry::merge_locks([previous.as_str()])
}

#[test]
fn lock_parts_cover_the_registry_exactly() {
    let mut keys = BTreeSet::new();
    let mut paths = BTreeMap::new();
    for message in registry() {
        assert!(keys.insert(message.key), "duplicate key: {}", message.path);
        assert!(
            paths.insert(message.path, message).is_none(),
            "duplicate path: {}",
            message.path
        );
        let hashed = Key(
            postcard_schema::key::hash::fnv1a64_owned::hash_ty_path_owned(
                message.path,
                &message.schema.into(),
            ),
        );
        assert_eq!(hashed, message.key, "{}", message.path);
        assert!(message.path.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }));
        if message.kind == Kind::Property {
            assert!((message.decode_json)(&[]).is_some());
        }
    }
    let mut locked = BTreeSet::new();
    for (path, lock) in LOCKS {
        assert!(
            lock.lines()
                .any(|line| line == format!("framing {FRAMING_VERSION}")),
            "{path}"
        );
        let (_, lines) = lock.split_once("[messages]\n").expect("messages section");
        for line in lines.lines() {
            let parts: Vec<_> = line.split_whitespace().collect();
            assert_eq!(parts.len(), 3, "{path}: {line}");
            assert!(locked.insert(parts[0]), "duplicate lock entry: {line}");
            let message: &&MessageInfo = paths.get(parts[0]).expect("registered lock entry");
            assert_eq!(parts[1], message.key.to_string(), "{}", message.path);
            let kind = match message.kind {
                Kind::Endpoint => "endpoint",
                Kind::Topic => "topic",
                Kind::Property => "property",
            };
            assert_eq!(parts[2], kind, "{}", message.path);
        }
    }
    assert_eq!(locked, paths.keys().copied().collect());
}

#[test]
fn malformed_or_ambiguous_locks_cannot_become_a_wire_identity() {
    use oer_hil_registry::{LockError, merge_locks};
    let base = oer_hil_protocol::MESSAGES_LOCK;
    assert_eq!(merge_locks([]), Err(LockError::Incomplete));
    assert!(merge_locks(["framing 2\n[modules]\nbase\n[messages]\n"]).is_err());
    assert!(merge_locks([base, base]).is_err());
    let different = oer_ieee80211_control::MESSAGES_LOCK.replace("framing 2", "framing 3");
    assert_eq!(
        merge_locks([base, &different]),
        Err(LockError::FramingMismatch)
    );
    let invalid = format!("{base}not-a-message\n");
    assert!(merge_locks([invalid.as_str()]).is_err());
}
