use oer_espressif_ieee802154_mac_values::COEX_DISABLED_PTI;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154MacControl,
    Ieee802154MultipanEnableState, Ieee802154PanIdentity,
};
use std::vec::Vec;

use crate::ieee802154::mac::{Ieee802154FoundationSnapshot, Ieee802154Pti};

use super::{
    Ieee802154MacPolicy, Ieee802154MacPolicyBackend, Ieee802154MacPolicyCheckpoint,
    Ieee802154MacPolicyReadback, Ieee802154MacPolicySnapshot, Ieee802154MacPolicyWrites,
    configure_ieee802154_mac_policy, verify_ieee802154_mac_policy,
};
use crate::ieee802154::lifecycle::Ieee802154ReadbackError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Channel(u8),
    CcaMode(Ieee802154CcaMode),
    CcaThreshold(i8),
    MacControl(Ieee802154MacControl),
    AckTimeout(u16),
    PrimaryIdentity(Ieee802154PanIdentity),
    Fence,
    Snapshot,
}

#[derive(Debug)]
struct FakeBackend {
    owner_id: u32,
    operations: Vec<Operation>,
    readback: Ieee802154MacPolicyReadback,
}

impl Ieee802154MacPolicyWrites for FakeBackend {
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.operations.push(Operation::Channel(channel.number()));
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.operations.push(Operation::CcaMode(mode));
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.operations.push(Operation::CcaThreshold(threshold));
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.operations.push(Operation::MacControl(control));
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.operations.push(Operation::AckTimeout(timeout.units()));
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.operations.push(Operation::PrimaryIdentity(identity));
    }

    fn order_device_accesses(&mut self) {
        self.operations.push(Operation::Fence);
    }
}

impl Ieee802154MacPolicyBackend for FakeBackend {
    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback {
        self.operations.push(Operation::Snapshot);
        self.readback
    }
}

fn policy() -> Ieee802154MacPolicy {
    Ieee802154MacPolicy::new(
        Ieee802154Channel::new(20).expect("standard channel"),
        Ieee802154CcaMode::CarrierAndEnergyDetection,
        -67,
        Ieee802154AckTimeout::from_microseconds(1_729).expect("bounded timeout"),
        Ieee802154MacControl::new(true, true, true, true, true, true),
        Ieee802154PanIdentity::new(
            0x1a2b,
            0x3c4d,
            [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87],
        ),
    )
}

fn valid_foundation() -> Ieee802154FoundationSnapshot {
    Ieee802154FoundationSnapshot::new(
        true,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        true,
    )
}

fn backend(snapshot: Ieee802154MacPolicySnapshot) -> FakeBackend {
    backend_with_foundation(valid_foundation(), snapshot)
}

fn backend_with_foundation(
    foundation: Ieee802154FoundationSnapshot,
    snapshot: Ieee802154MacPolicySnapshot,
) -> FakeBackend {
    FakeBackend {
        owner_id: 0x154,
        operations: Vec::new(),
        readback: Ieee802154MacPolicyReadback::new(foundation, snapshot),
    }
}

#[test]
fn static_policy_uses_exact_deterministic_order_then_one_snapshot() {
    let policy = policy();
    let configured = configure_ieee802154_mac_policy(
        backend(Ieee802154MacPolicySnapshot::from_policy(policy)),
        policy,
    )
    .expect("matching readback");

    assert_eq!(
        configured.operations,
        [
            Operation::Channel(20),
            Operation::CcaMode(Ieee802154CcaMode::CarrierAndEnergyDetection),
            Operation::CcaThreshold(-67),
            Operation::MacControl(policy.control()),
            Operation::AckTimeout(109),
            Operation::PrimaryIdentity(policy.identity()),
            Operation::Fence,
            Operation::Snapshot,
        ]
    );
}

#[test]
fn every_checkpoint_fails_closed_and_preserves_the_owner() {
    let policy = policy();
    let valid = Ieee802154MacPolicySnapshot::from_policy(policy);
    let cases = [
        Ieee802154MacPolicyCheckpoint::EventsMasked,
        Ieee802154MacPolicyCheckpoint::RxAbortsMasked,
        Ieee802154MacPolicyCheckpoint::TxAbortsMasked,
        Ieee802154MacPolicyCheckpoint::EdSampleAverage,
        Ieee802154MacPolicyCheckpoint::TxrxPtiDisabled,
        Ieee802154MacPolicyCheckpoint::AckPtiDisabled,
        Ieee802154MacPolicyCheckpoint::TxOnDelayApplied,
        Ieee802154MacPolicyCheckpoint::Channel,
        Ieee802154MacPolicyCheckpoint::CcaMode,
        Ieee802154MacPolicyCheckpoint::CcaThreshold,
        Ieee802154MacPolicyCheckpoint::TxAutoAck,
        Ieee802154MacPolicyCheckpoint::RxAutoAck,
        Ieee802154MacPolicyCheckpoint::EnhancedAckTx,
        Ieee802154MacPolicyCheckpoint::Coordinator,
        Ieee802154MacPolicyCheckpoint::Promiscuous,
        Ieee802154MacPolicyCheckpoint::EnhancedPending,
        Ieee802154MacPolicyCheckpoint::AckTimeout,
        Ieee802154MacPolicyCheckpoint::PrimaryPanEnabled,
        Ieee802154MacPolicyCheckpoint::PanId,
        Ieee802154MacPolicyCheckpoint::ShortAddress,
        Ieee802154MacPolicyCheckpoint::ExtendedAddress,
    ];

    for checkpoint in cases {
        assert_eq!(
            checkpoint.invalidates_foundation(),
            matches!(
                checkpoint,
                Ieee802154MacPolicyCheckpoint::EventsMasked
                    | Ieee802154MacPolicyCheckpoint::RxAbortsMasked
                    | Ieee802154MacPolicyCheckpoint::TxAbortsMasked
                    | Ieee802154MacPolicyCheckpoint::EdSampleAverage
                    | Ieee802154MacPolicyCheckpoint::TxrxPtiDisabled
                    | Ieee802154MacPolicyCheckpoint::AckPtiDisabled
                    | Ieee802154MacPolicyCheckpoint::TxOnDelayApplied
            )
        );
        let mut snapshot = valid;
        let foundation = match checkpoint {
            Ieee802154MacPolicyCheckpoint::EventsMasked => Ieee802154FoundationSnapshot::new(
                false,
                true,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::RxAbortsMasked => Ieee802154FoundationSnapshot::new(
                true,
                false,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::TxAbortsMasked => Ieee802154FoundationSnapshot::new(
                true,
                true,
                false,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::EdSampleAverage => Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                false,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::TxrxPtiDisabled => Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI - 1).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::AckPtiDisabled => Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI - 1).expect("five-bit PTI"),
                true,
            ),
            Ieee802154MacPolicyCheckpoint::TxOnDelayApplied => Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                true,
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
                false,
            ),
            _ => valid_foundation(),
        };
        match checkpoint {
            Ieee802154MacPolicyCheckpoint::EventsMasked
            | Ieee802154MacPolicyCheckpoint::RxAbortsMasked
            | Ieee802154MacPolicyCheckpoint::TxAbortsMasked
            | Ieee802154MacPolicyCheckpoint::EdSampleAverage
            | Ieee802154MacPolicyCheckpoint::TxrxPtiDisabled
            | Ieee802154MacPolicyCheckpoint::AckPtiDisabled
            | Ieee802154MacPolicyCheckpoint::TxOnDelayApplied => {}
            Ieee802154MacPolicyCheckpoint::Channel => snapshot.frequency_code ^= 1,
            Ieee802154MacPolicyCheckpoint::CcaMode => {
                snapshot.cca_mode = Ieee802154CcaMode::Carrier
            }
            Ieee802154MacPolicyCheckpoint::CcaThreshold => snapshot.cca_threshold_code += 1,
            Ieee802154MacPolicyCheckpoint::TxAutoAck => {
                snapshot.control = Ieee802154MacControl::new(
                    false,
                    snapshot.control.rx_auto_ack(),
                    snapshot.control.enhanced_ack_tx(),
                    snapshot.control.coordinator(),
                    snapshot.control.promiscuous(),
                    snapshot.control.enhanced_pending(),
                )
            }
            Ieee802154MacPolicyCheckpoint::RxAutoAck => {
                snapshot.control = Ieee802154MacControl::new(
                    snapshot.control.tx_auto_ack(),
                    false,
                    snapshot.control.enhanced_ack_tx(),
                    snapshot.control.coordinator(),
                    snapshot.control.promiscuous(),
                    snapshot.control.enhanced_pending(),
                )
            }
            Ieee802154MacPolicyCheckpoint::EnhancedAckTx => {
                snapshot.control = Ieee802154MacControl::new(
                    snapshot.control.tx_auto_ack(),
                    snapshot.control.rx_auto_ack(),
                    false,
                    snapshot.control.coordinator(),
                    snapshot.control.promiscuous(),
                    snapshot.control.enhanced_pending(),
                )
            }
            Ieee802154MacPolicyCheckpoint::Coordinator => {
                snapshot.control = Ieee802154MacControl::new(
                    snapshot.control.tx_auto_ack(),
                    snapshot.control.rx_auto_ack(),
                    snapshot.control.enhanced_ack_tx(),
                    false,
                    snapshot.control.promiscuous(),
                    snapshot.control.enhanced_pending(),
                )
            }
            Ieee802154MacPolicyCheckpoint::Promiscuous => {
                snapshot.control = Ieee802154MacControl::new(
                    snapshot.control.tx_auto_ack(),
                    snapshot.control.rx_auto_ack(),
                    snapshot.control.enhanced_ack_tx(),
                    snapshot.control.coordinator(),
                    false,
                    snapshot.control.enhanced_pending(),
                )
            }
            Ieee802154MacPolicyCheckpoint::EnhancedPending => {
                snapshot.control = Ieee802154MacControl::new(
                    snapshot.control.tx_auto_ack(),
                    snapshot.control.rx_auto_ack(),
                    snapshot.control.enhanced_ack_tx(),
                    snapshot.control.coordinator(),
                    snapshot.control.promiscuous(),
                    false,
                )
            }
            Ieee802154MacPolicyCheckpoint::AckTimeout => {
                snapshot.ack_timeout = Ieee802154AckTimeout::from_units(0)
            }
            Ieee802154MacPolicyCheckpoint::PrimaryPanEnabled => {
                snapshot.multipan_enable_state = Ieee802154MultipanEnableState::NONE
            }
            Ieee802154MacPolicyCheckpoint::PanId => {
                snapshot.identity = Ieee802154PanIdentity::new(
                    snapshot.identity.pan_id() ^ 1,
                    snapshot.identity.short_address(),
                    snapshot.identity.extended_address(),
                )
            }
            Ieee802154MacPolicyCheckpoint::ShortAddress => {
                snapshot.identity = Ieee802154PanIdentity::new(
                    snapshot.identity.pan_id(),
                    snapshot.identity.short_address() ^ 1,
                    snapshot.identity.extended_address(),
                )
            }
            Ieee802154MacPolicyCheckpoint::ExtendedAddress => {
                let mut address = snapshot.identity.extended_address();
                address[7] ^= 1;
                snapshot.identity = Ieee802154PanIdentity::new(
                    snapshot.identity.pan_id(),
                    snapshot.identity.short_address(),
                    address,
                )
            }
        }

        let failure =
            configure_ieee802154_mac_policy(backend_with_foundation(foundation, snapshot), policy)
                .expect_err("mismatched readback must fail");
        assert_eq!(
            failure.error(),
            Ieee802154ReadbackError {
                checkpoint,
                expected: true,
                observed: false,
            }
        );
        let recovered = failure.into_backend();
        assert_eq!(recovered.owner_id, 0x154);
        assert_eq!(recovered.operations.last(), Some(&Operation::Snapshot));
        assert_eq!(
            recovered
                .operations
                .iter()
                .filter(|operation| **operation == Operation::Snapshot)
                .count(),
            1
        );
    }
}

#[test]
fn readback_reports_the_first_failed_checkpoint() {
    let policy = policy();
    let mut snapshot = Ieee802154MacPolicySnapshot::from_policy(policy);
    snapshot.cca_mode = Ieee802154CcaMode::Carrier;
    let mut address = snapshot.identity.extended_address();
    address[0] ^= 1;
    snapshot.identity = Ieee802154PanIdentity::new(
        snapshot.identity.pan_id(),
        snapshot.identity.short_address(),
        address,
    );

    let failure =
        configure_ieee802154_mac_policy(backend(snapshot), policy).expect_err("two corrupt fields");
    assert_eq!(
        failure.error().checkpoint,
        Ieee802154MacPolicyCheckpoint::CcaMode
    );
}

#[test]
fn returning_owner_is_proved_by_readback_alone() {
    let policy = policy();
    let verified = verify_ieee802154_mac_policy(
        backend(Ieee802154MacPolicySnapshot::from_policy(policy)),
        policy,
    )
    .expect("matching readback");

    // No policy or foundation field is rewritten on the way back.
    assert_eq!(verified.operations, [Operation::Snapshot]);
}

#[test]
fn returning_owner_with_live_event_enables_invalidates_the_foundation() {
    let policy = policy();
    let foundation = Ieee802154FoundationSnapshot::new(
        false,
        true,
        true,
        true,
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        Ieee802154Pti::new(COEX_DISABLED_PTI).expect("five-bit PTI"),
        true,
    );
    let failure = verify_ieee802154_mac_policy(
        backend_with_foundation(foundation, Ieee802154MacPolicySnapshot::from_policy(policy)),
        policy,
    )
    .expect_err("an interrupt teardown that left events enabled must fail");

    assert_eq!(
        failure.error().checkpoint,
        Ieee802154MacPolicyCheckpoint::EventsMasked
    );
    assert!(failure.error().checkpoint.invalidates_foundation());
    assert_eq!(failure.into_backend().operations, [Operation::Snapshot]);
}
