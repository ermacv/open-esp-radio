extern crate std;

use super::*;
use oer_hil_protocol::{
    DecodeError, Envelope, FrameDecoder, FrameEncoder, Message, Outbound, WireKind,
};
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
    const KEY: oer_hil_protocol::Key = StartStation::KEY;
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
    let info = crate::MESSAGES
        .iter()
        .find(|message| message.key == StartStation::KEY)
        .unwrap();
    assert_eq!((info.decode_json)(wire), Some(expected));
    let error =
        serde_json::from_str::<NetworkCredentials>(r#"{"private-test-password":42}"#).unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-test"));
}

fn event_round_trip<M: Message + Copy + PartialEq + core::fmt::Debug>(message: M) {
    let mut encoder = FrameEncoder::new();
    let wire = encoder
        .encode(&Envelope::new(7, 11, 13, 17, message))
        .expect("an event fits a frame");
    let mut decoded = None;
    FrameDecoder::new().feed(WireKind::Event, wire, |frame| {
        decoded = frame
            .expect("an intact frame")
            .decode::<M>()
            .expect("the key names a known message")
            .map(|envelope| envelope.body)
            .ok();
    });
    assert_eq!(decoded, Some(message));
}

#[test]
fn link_events_preserve_epoch_and_typed_disconnect_reason() {
    let reasons = [
        StationDisconnectReason::RetainedSecurityIes(StationRsnFrameError::InvalidRsnIe),
        StationDisconnectReason::RetainedSecurityIes(StationRsnFrameError::InvalidRsnxe),
        StationDisconnectReason::Sae(StationSaeFailure::Crypto {
            peer: [2; 6],
            pwe: StationSaePwe::HuntingAndPecking,
        }),
        StationDisconnectReason::Sae(StationSaeFailure::Crypto {
            peer: [2; 6],
            pwe: StationSaePwe::HashToElement,
        }),
        StationDisconnectReason::InvalidEapol(StationEapolParseError::Truncated),
        StationDisconnectReason::InvalidEapol(StationEapolParseError::LengthMismatch),
        StationDisconnectReason::InvalidEapol(StationEapolParseError::NotKeyPacket),
        StationDisconnectReason::InvalidEapol(StationEapolParseError::NotRsnKeyDescriptor),
        StationDisconnectReason::Sae(StationSaeFailure::Protocol(StationSaeError::Crypto)),
        StationDisconnectReason::LocalRequest,
        StationDisconnectReason::Peer {
            reason_code: 0xffff,
        },
        StationDisconnectReason::AuthenticationFailed,
        StationDisconnectReason::Sae(StationSaeFailure::Timeout),
        StationDisconnectReason::Sae(StationSaeFailure::DeadlineOverflow),
        StationDisconnectReason::Sae(StationSaeFailure::PeerDisconnect {
            kind: StationPeerDisconnectKind::Deauthentication,
            reason_code: 2,
        }),
        StationDisconnectReason::Sae(StationSaeFailure::Rejected { status_code: 77 }),
        StationDisconnectReason::Sae(StationSaeFailure::Protocol(
            StationSaeError::UnsupportedGroup(20),
        )),
        StationDisconnectReason::Sae(StationSaeFailure::Protocol(
            StationSaeError::ConfirmMismatch,
        )),
        StationDisconnectReason::Sae(StationSaeFailure::TokenTooLong),
        StationDisconnectReason::AssociationFailed,
        StationDisconnectReason::NotFound { partial: true },
        StationDisconnectReason::HandshakeFailed,
        StationDisconnectReason::SaQueryTimeout,
        StationDisconnectReason::TransmitRefused(StationTransmitRefusal::PacketNumberExhausted),
        StationDisconnectReason::TransmitRefused(StationTransmitRefusal::Preparation(
            StationPreparationError::BlockAckWindow,
        )),
        StationDisconnectReason::TransmitRefused(StationTransmitRefusal::Preparation(
            StationPreparationError::MissingBodyLoan,
        )),
        StationDisconnectReason::TransmitRefused(StationTransmitRefusal::Layout(
            StationLayoutError::BodyOverlap,
        )),
        StationDisconnectReason::Refused(StationChannelRefusal::TooSoon {
            retry_at_us: u64::MAX,
        }),
        StationDisconnectReason::Refused(StationChannelRefusal::ScheduleExhausted),
        StationDisconnectReason::LeaseExchangeRefused,
        StationDisconnectReason::Arbiter(StationLeaveReason::PortFailed),
        StationDisconnectReason::Arbiter(StationLeaveReason::Unservable),
        StationDisconnectReason::DeadlineOverflow,
        StationDisconnectReason::Encode,
    ];
    for state in [
        StationLinkState::Associated,
        StationLinkState::Authorized {
            bssid: [2; 6],
            channel: 6,
        },
    ]
    .into_iter()
    .chain(
        reasons
            .into_iter()
            .map(|reason| StationLinkState::Disconnected { reason }),
    ) {
        event_round_trip(StationLink {
            epoch: u32::MAX,
            state,
        });
    }
    // Reasons the W4 diagnosis could not tell apart decode as distinct values.
    assert_ne!(
        StationDisconnectReason::Arbiter(StationLeaveReason::PortFailed),
        StationDisconnectReason::LocalRequest
    );
}

#[test]
fn service_end_preserves_the_fault_and_its_typed_cause() {
    let backend = [
        StationBackendFault::Core(StationLowerMacFault::Tx),
        StationBackendFault::Core(StationLowerMacFault::KeyUseMismatch),
        StationBackendFault::Receive(StationReceiveFault::Service),
        StationBackendFault::Receive(StationReceiveFault::ForeignExecutor),
        StationBackendFault::Deadline,
        StationBackendFault::Capacity,
        StationBackendFault::Clock,
        StationBackendFault::Retune(StationRetuneFault::MacActive { state: u8::MAX }),
        StationBackendFault::Retune(StationRetuneFault::RxWalkerEnabled),
        StationBackendFault::Retune(StationRetuneFault::Refused),
        StationBackendFault::Retune(StationRetuneFault::ClientFailed),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::TrackingTick,
            cause: StationRadioPoisonCause::Tracking(StationPhyFault::EdgeTimedOut),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::Request {
                client: StationRadioClient::Wifi,
                request: StationRadioRequest::Retune,
            },
            cause: StationRadioPoisonCause::Channel(StationPhyFault::UnexpectedBinding),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::TrackingTick,
            cause: StationRadioPoisonCause::CalibrationTracking(
                StationCalibrationTrackingFault::Step(StationPhyFault::EdgeTimedOut),
            ),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::TrackingTick,
            cause: StationRadioPoisonCause::CalibrationTracking(
                StationCalibrationTrackingFault::Radio(StationCalibrationTrackingFailure::Dcode),
            ),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::Prepare,
            cause: StationRadioPoisonCause::Registration(StationRegistrationFault::Lowering {
                stage: StationRegisterStage::FinalI2cRead,
                error: StationRegisterBindingError::UnexpectedOutcome,
            }),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::Prepare,
            cause: StationRadioPoisonCause::Registration(StationRegistrationFault::Radio(
                StationRegistrationFailure::I2cMasterResetTimedOut {
                    index: u8::MAX,
                    samples: u16::MAX,
                },
            )),
        })),
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin: StationRadioOperation::TrackingTick,
            cause: StationRadioPoisonCause::TrackingRefused(StationPhyDomainError::TimeReversed {
                previous_us: u64::MAX,
                now_us: 0,
            }),
        })),
        StationBackendFault::Abandoned,
    ];
    let mut faults = std::vec![
        StationServiceFault::RxBlockAckCapacity,
        StationServiceFault::RxBlockAckEncode,
        StationServiceFault::Contract(StationContractViolation::UnexpectedTerminal(
            StationPendingTerminal::Transmit(u8::MAX),
        )),
        StationServiceFault::Contract(StationContractViolation::StationKeyBank(
            StationKeyContractViolation::IncompleteBank,
        )),
        StationServiceFault::Contract(StationContractViolation::GroupSlotMismatch),
        StationServiceFault::Contract(StationContractViolation::StationKeyUnwrap),
    ];
    for cause in backend {
        faults.extend([
            StationServiceFault::EnableRefused(cause),
            StationServiceFault::Backend(cause),
            StationServiceFault::Submission(cause),
            StationServiceFault::DrainedWithOutstanding {
                cause,
                missing: StationPendingTerminal::Control(StationControlTerminal::GateClosed),
            },
        ]);
    }
    for fault in faults {
        for end in [
            StationServiceEnd::Failed(fault),
            StationServiceEnd::Poisoned(fault),
            StationServiceEnd::Quarantined(fault),
        ] {
            event_round_trip(StationServiceEnded {
                epoch: u32::MAX,
                end,
            });
        }
    }
    event_round_trip(StationServiceEnded {
        epoch: u32::MAX,
        end: StationServiceEnd::Unavailable,
    });
    // A retune refused by an earlier failed tick and one that failed itself
    // stay distinct.
    let poisoned = |origin| {
        StationBackendFault::Retune(StationRetuneFault::Poisoned(StationRadioPoison {
            origin,
            cause: StationRadioPoisonCause::Tracking(StationPhyFault::EdgeTimedOut),
        }))
    };
    assert_ne!(
        poisoned(StationRadioOperation::TrackingTick),
        poisoned(StationRadioOperation::Request {
            client: StationRadioClient::Wifi,
            request: StationRadioRequest::Retune,
        })
    );
    // A failed calibration-tracking child is not a binding error of the
    // tracking parent.
    assert_ne!(
        StationRadioPoisonCause::CalibrationTracking(StationCalibrationTrackingFault::Step(
            StationPhyFault::EdgeTimedOut
        )),
        StationRadioPoisonCause::Tracking(StationPhyFault::UnexpectedBinding)
    );
    // Enabling and submitting faults with the same cause stay distinct.
    let cause = StationBackendFault::Deadline;
    assert_ne!(
        StationServiceFault::EnableRefused(cause),
        StationServiceFault::Backend(cause)
    );
}

#[test]
fn only_a_running_station_admits_stop() {
    use oer_hil_protocol::base::RejectReason::InvalidState;
    assert_eq!(StopStation::refusal(StationRole::Running), None);
    for role in [StationRole::Idle, StationRole::Stopping, StationRole::Ended] {
        assert_eq!(StopStation::refusal(role), Some(InvalidState));
    }
}

#[test]
fn pending_commands_preserve_identity_cause_and_each_command_credit() {
    use oer_ieee80211_runtime::station::{self as runtime, contract::Command};
    use oer_memory::WakingSpscQueue;
    let mut queue = WakingSpscQueue::<runtime::ServiceCommand<()>, 1>::new();
    let (send, receive) = queue.split();
    let state = runtime::State::new();
    let (mut control, _runner) = runtime::Control::new(send, &state).unwrap();
    let pending = PendingCommands::new();
    let first = oer_hil_protocol::RequestIdentity {
        boot_id: 7,
        session_id: 91,
        request_id: 13,
    };
    let second = oer_hil_protocol::RequestIdentity {
        session_id: 92,
        ..first
    };
    assert_eq!(
        pending.admit(&mut control, first, StationCommandKind::Connect),
        StationAdmission::Accepted
    );
    assert_eq!(
        pending.admit(&mut control, second, StationCommandKind::Connect),
        StationAdmission::InFlight
    );
    assert_eq!(
        pending.admit(&mut control, second, StationCommandKind::BackgroundScan),
        StationAdmission::Refused(StationControlError::QueueFull)
    );
    let Some(runtime::ServiceCommand::Station(request)) = receive.try_receive() else {
        panic!("original queued request missing")
    };
    assert_eq!(request.command(), Command::Connect);
    assert_ne!(request.correlation(), u64::from(first.request_id));
    let result = StationCommandResult::Refused(StationCommandRefusal::Transitioning);
    assert!(
        pending
            .finish(runtime::Request::new(1, Command::Connect), result)
            .is_none()
    );
    let (identity, event) = pending.finish(request, result).unwrap();
    assert_eq!(identity, first);
    assert_eq!(event.result, result);
    assert_eq!(
        pending.admit(&mut control, second, StationCommandKind::Connect),
        StationAdmission::Accepted
    );
    let status = StationStatus::from(control.status());
    assert_eq!(
        (status.epoch, status.linked, status.stopped, status.notice),
        (0, false, false, None)
    );
}

#[test]
fn nested_runtime_causes_round_trip_without_erasure() {
    use oer_ieee80211_runtime::station::contract as runtime;
    let outcome = runtime::CommandOutcome::BackgroundScan {
        interrupted: Some(runtime::BackgroundScanEnd::LinkEnded(
            runtime::DisconnectReason::Sae(runtime::SaeFailure::Crypto {
                peer: [9; 6],
                pwe: runtime::SaePwe::HashToElement,
            }),
        )),
    };
    let event = StationCommandFinished {
        correlation: u64::MAX,
        origin: StationCommandOrigin::Requested,
        command: StationCommandKind::BackgroundScan,
        result: StationCommandResult::Completed(outcome.into()),
    };
    let mut bytes = [0; 128];
    let encoded = postcard::to_slice(&event, &mut bytes).unwrap();
    assert_eq!(
        postcard::from_bytes::<StationCommandFinished>(encoded).unwrap(),
        event
    );
    assert_eq!(
        event.result,
        StationCommandResult::Completed(StationCommandOutcome::BackgroundScan {
            interrupted: Some(StationBackgroundScanEnd::LinkEnded(
                StationDisconnectReason::Sae(StationSaeFailure::Crypto {
                    peer: [9; 6],
                    pwe: StationSaePwe::HashToElement
                })
            ))
        })
    );
}

#[test]
fn stopped_control_reports_unexecuted_requests_once_without_losing_identities() {
    use oer_ieee80211_runtime::station as runtime;
    use oer_memory::WakingSpscQueue;
    let mut queue = WakingSpscQueue::<runtime::ServiceCommand<()>, 1>::new();
    let (send, _receive) = queue.split();
    let state = runtime::State::new();
    let (mut control, runner) = runtime::Control::new(send, &state).unwrap();
    let pending = PendingCommands::new();
    let status = control.status_reader();
    let request = oer_hil_protocol::RequestIdentity {
        boot_id: 7,
        session_id: 91,
        request_id: 13,
    };
    assert_eq!(
        pending.admit(&mut control, request, StationCommandKind::Connect),
        StationAdmission::Accepted
    );
    assert!(
        pending
            .finish_stopped(control.status())
            .iter()
            .all(Option::is_none)
    );
    assert!(!status.status().stopped);
    drop(runner);
    assert!(control.status().stopped);
    assert!(status.status().stopped);
    let terminals = pending
        .finish_stopped(control.status())
        .into_iter()
        .flatten()
        .collect::<std::vec::Vec<_>>();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].0, request);
    assert_eq!(
        terminals[0].1.result,
        StationCommandResult::NotExecuted(StationControlError::Stopped)
    );
    assert!(
        pending
            .finish_stopped(control.status())
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(
        pending.admit(&mut control, request, StationCommandKind::Connect),
        StationAdmission::Refused(StationControlError::Stopped)
    );
}
