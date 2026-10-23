extern crate std;

use super::*;
use crate::{DecodeError, Envelope, FrameDecoder, FrameEncoder, Message, Outbound, WireKind};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};
use std::format;

fn start(ssid: &[u8], passphrase: &[u8]) -> StartStation {
    StartStation(StationStart {
        credentials: NetworkCredentials::try_new(ssid, passphrase).unwrap(),
        power_save: StationPowerSave::Off,
    })
}

fn round_trip<M: Message>(message: M) -> Result<Envelope<StartStation>, DecodeError> {
    let mut encoder = FrameEncoder::new();
    let wire = encoder
        .encode(&Envelope::new(7, 11, 13, 17, message))
        .unwrap();
    let mut result = None;
    FrameDecoder::new().feed(WireKind::Command, wire, |frame| {
        result = Some(frame.unwrap().decode::<StartStation>().unwrap());
    });
    result.unwrap()
}

// A faulty sender uses the real command key but bypasses its constructor.
// Sequence capacity is deliberately larger than the receiver permits.
#[derive(Serialize, Deserialize, Schema)]
struct UncheckedCredentials {
    ssid: [u8; 32],
    ssid_length: u8,
    passphrase: heapless::Vec<u8, 256>,
}

#[derive(Serialize, Deserialize, Schema)]
struct UncheckedStart {
    credentials: UncheckedCredentials,
    power_save: u8,
}

impl Message for UncheckedStart {
    const PATH: &'static str = StartStation::PATH;
    const KEY: crate::Key = StartStation::KEY;
    const WIRE_KIND: WireKind = WireKind::Command;
}

fn unchecked(ssid_length: u8, passphrase_length: usize) -> UncheckedStart {
    UncheckedStart {
        credentials: UncheckedCredentials {
            ssid: [0xa5; 32],
            ssid_length,
            passphrase: heapless::Vec::from_slice(&[0x5a; 256][..passphrase_length]).unwrap(),
        },
        power_save: 0,
    }
}

#[test]
fn valid_bounds_round_trip_and_preserve_identity() {
    for (ssid, passphrase) in [(1, 8), (32, 63)] {
        let decoded = round_trip(start(&[0xa5; 32][..ssid], &[0x5a; 63][..passphrase])).unwrap();
        assert_eq!(decoded.boot_id, 7);
        assert_eq!(decoded.message_sequence, 11);
        assert_eq!(decoded.session_id, 13);
        assert_eq!(decoded.request_id, 17);
        assert_eq!(decoded.body.0.credentials.ssid(), &[0xa5; 32][..ssid]);
        assert_eq!(
            decoded.body.0.credentials.passphrase(),
            &[0x5a; 63][..passphrase]
        );
        assert_eq!(decoded.body.0.power_save, StationPowerSave::Off);
    }
}

#[test]
fn constructor_rejects_invalid_bounds_with_typed_errors() {
    for length in [0, 33, 255] {
        assert_eq!(
            NetworkCredentials::try_new(&[0xa5; 255][..length], b"test-password").unwrap_err(),
            NetworkCredentialsError::SsidLength
        );
    }
    for length in [0, 7, 64, 255] {
        assert_eq!(
            NetworkCredentials::try_new(b"test-network", &[0x5a; 255][..length]).unwrap_err(),
            NetworkCredentialsError::PassphraseLength
        );
    }
}

#[test]
fn wire_decoding_enforces_bounds_before_exposing_credentials() {
    for ssid_length in [0, 33, 255] {
        assert_eq!(
            round_trip(unchecked(ssid_length, 8)).unwrap_err(),
            DecodeError::Payload
        );
    }
    for passphrase_length in [0, 7, 64, 255] {
        assert_eq!(
            round_trip(unchecked(1, passphrase_length)).unwrap_err(),
            DecodeError::Payload
        );
    }
    // Truncation after a partial secret also returns an error.
    let mut storage = [0; 128];
    let wire = postcard::to_slice(&unchecked(1, 63), &mut storage).unwrap();
    assert!(postcard::from_bytes::<StartStation>(&wire[..wire.len() - 2]).is_err());
    let mut unsupported = unchecked(1, 8);
    unsupported.power_save = 1;
    assert_eq!(round_trip(unsupported).unwrap_err(), DecodeError::Payload);
}

#[test]
fn credential_owners_cannot_be_cloned() {
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    struct IsClone;
    impl<T: Clone> AmbiguousIfClone<IsClone> for T {}
    // Adding Clone introduces a second candidate and makes inference fail.
    let _ = <NetworkCredentials as AmbiguousIfClone<_>>::marker;
    let _ = <StationStart as AmbiguousIfClone<_>>::marker;
    let _ = <StartStation as AmbiguousIfClone<_>>::marker;
}

#[test]
fn debug_and_errors_do_not_disclose_credentials_or_wire_bytes() {
    let command = start(b"private-test-network", b"private-test-password");
    assert_eq!(
        format!("{:?}", command.0.credentials),
        "NetworkCredentials(<redacted>)"
    );
    let envelope = Envelope::new(7, 11, 13, 17, command);
    let debug = format!("{envelope:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("private-test"));
    let outbound = Outbound::new(11, 13, 17, &envelope.body).unwrap();
    let debug = format!("{outbound:?}");
    assert!(debug.contains("payload_length:"));
    assert!(!debug.contains("payload: ["));
    let mut encoder = FrameEncoder::new();
    FrameDecoder::new().feed(
        WireKind::Command,
        encoder.encode(&envelope).unwrap(),
        |frame| {
            let debug = format!("{:?}", frame.unwrap());
            assert!(debug.contains("payload_length:"));
            assert!(!debug.contains("payload: ["));
        },
    );
    let error = round_trip(unchecked(255, 8)).unwrap_err();
    assert_eq!(format!("{error:?}"), "Payload");
    assert_eq!(
        format!("{error}"),
        "HIL frame payload does not decode as the type its key names"
    );
    assert_eq!(
        format!("{:?}", NetworkCredentialsError::PassphraseLength),
        "PassphraseLength"
    );
    assert_eq!(
        format!("{}", NetworkCredentialsError::PassphraseLength),
        "passphrase length must be 8..=63 bytes"
    );
}

#[cfg(feature = "registry")]
#[test]
fn readable_serializers_and_registry_redact_the_secret_owner() {
    let command = start(b"private-test-network", b"private-test-password");
    let expected = serde_json::json!({"credentials": "<redacted>", "power_save": "Off"});
    assert_eq!(serde_json::to_value(&command).unwrap(), expected);
    let mut storage = [0; 128];
    let wire = postcard::to_slice(&command, &mut storage).unwrap();
    let info = crate::registry()
        .find(|message| message.key == StartStation::KEY)
        .unwrap();
    assert_eq!((info.decode_json)(wire), Some(expected));
    let error =
        serde_json::from_str::<NetworkCredentials>(r#"{"private-test-password":42}"#).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-test"));
}

#[test]
fn link_events_preserve_epoch_and_typed_disconnect_reason() {
    let reasons = [
        StationDisconnectReason::LocalRequest,
        StationDisconnectReason::Peer {
            reason_code: 0xffff,
        },
        StationDisconnectReason::BeaconTimeout,
        StationDisconnectReason::AuthenticationFailed,
        StationDisconnectReason::AssociationFailed,
        StationDisconnectReason::HandshakeFailed,
        StationDisconnectReason::RadioFailure,
    ];
    for state in [StationLinkState::Associated, StationLinkState::Authorized]
        .into_iter()
        .chain(
            reasons
                .into_iter()
                .map(|reason| StationLinkState::Disconnected { reason }),
        )
    {
        let message = StationLink {
            epoch: u32::MAX,
            state,
        };
        let mut encoder = FrameEncoder::new();
        let wire = encoder
            .encode(&Envelope::new(7, 11, 13, 17, message))
            .unwrap();
        let mut decoded = None;
        FrameDecoder::new().feed(WireKind::Event, wire, |frame| {
            decoded = Some(
                frame
                    .unwrap()
                    .decode::<StationLink>()
                    .unwrap()
                    .unwrap()
                    .body,
            );
        });
        assert_eq!(decoded, Some(message));
    }
}
