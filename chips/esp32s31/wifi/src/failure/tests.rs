use super::*;
#[test]
fn completion_status_and_receive_credit_observations_survive_the_boundary() {
    use oer_esp32s31_ieee80211_driver::{
        lower_mac::LowerMacFault as Core,
        mac::rx::pool::RxStageTransactionError as Stage,
        mac::tx::{CompletionFault as Completion, TxError as Mac},
        ordinary_tx::OrdinaryTxError as Tx,
        port::Fault,
        rx::control::WifiRxCause,
    };
    let value = Fault::Core(Core::Tx(Tx::Tx(Mac::Completion(
        Completion::InvalidStatus { status: 0xf3 },
    ))));
    assert_eq!(
        super::convert::backend_fault(value),
        BackendFault::Core(LowerMacFault::Tx(OrdinaryTxError::Tx(TxError::Completion(
            CompletionFault::InvalidStatus { status: 0xf3 }
        ))))
    );
    let value = Fault::Receive(WifiRxCause::Service(Stage::InvalidCreditReserve {
        reserved: 5,
        pool_slots: 3,
        queue_depth: 2,
    }));
    assert_eq!(
        super::convert::backend_fault(value),
        BackendFault::Receive(ReceiveFault::Service(
            RxStageTransactionError::InvalidCreditReserve {
                reserved: 5,
                pool_slots: 3,
                queue_depth: 2
            }
        ))
    );
}
#[test]
fn radio_origin_and_tracking_class_keep_their_original_clock_values() {
    use oer_esp32s31_phy::{
        concurrent::ConcurrentPhyError,
        state::client::{PhyPllTrackClass as Class, PhyTrackTimeError as Time},
    };
    use oer_esp32s31_radio_client::{Poison, PoisonCause, PoisonOrigin, RequestKind};
    let value = Poison {
        origin: PoisonOrigin::Request {
            client: oer_esp32s31_hal::domain::RadioClient::Wifi,
            request: RequestKind::Retune,
        },
        cause: PoisonCause::Domain(ConcurrentPhyError::Time(Time::TimeReversed {
            class: Class::BluetoothIeee802154,
            previous_micros: 199,
            now_micros: 7,
        })),
    };
    assert_eq!(
        super::convert::radio_poison(value),
        RadioPoison {
            origin: RadioOperation::Request {
                client: RadioClient::Wifi,
                request: RadioRequest::Retune
            },
            cause: RadioPoisonCause::Domain(PhyDomainError::Time(
                PhyTrackTimeError::TimeReversed {
                    class: PhyPllTrackClass::BluetoothIeee802154,
                    previous_micros: 199,
                    now_micros: 7
                }
            ))
        }
    );
}
#[test]
fn a_failed_retirement_keeps_the_station_failure_and_every_lifecycle_observation() {
    use oer_radio_port::{LifecycleError as Source, SlotObservation as Observed, SlotStorageState};
    let observed = Observed {
        phase: 7,
        storage: SlotStorageState::Vacant,
        invocation_present: true,
    };
    assert_eq!(
        super::convert::retirement(None, Source::InvalidState(observed), false),
        RetirementFailure {
            failure: None,
            network: None,
            cause: RetirementCause::InvalidState(SlotObservation {
                phase: 7,
                storage: SlotStorage::Vacant,
                invocation_present: true,
            }),
            hardware_stopped: false,
        }
    );
    let failure = StationFailure::Backend(BackendFault::Deadline);
    for (source, cause) in [
        (Source::Cancelled, RetirementCause::Cancelled),
        (
            Source::FailStopRequested,
            RetirementCause::FailStopRequested,
        ),
        (Source::Construction, RetirementCause::Construction),
        (
            Source::BodyAlreadyIssued,
            RetirementCause::BodyAlreadyIssued,
        ),
        (Source::Deadline, RetirementCause::Deadline),
        (Source::StopUnconfirmed, RetirementCause::StopUnconfirmed),
    ] {
        let failed = FailedStation {
            failure,
            network: None,
        };
        assert_eq!(
            super::convert::retirement(Some(failed), source, true),
            RetirementFailure {
                failure: Some(failure),
                network: None,
                cause,
                hardware_stopped: true,
            }
        );
    }
}
#[test]
fn a_failed_network_drain_is_the_retirement_cause_beside_the_station_failure() {
    use oer_radio_port::LifecycleError as Source;
    let drain = NetworkDrain {
        cause: NetworkDrainCause::Deadline,
        held: 3,
    };
    let failure = StationFailure::Backend(BackendFault::Deadline);
    // The port's refusal to retire is the drain's, and the original
    // failure is kept beside it.
    assert_eq!(
        super::convert::retirement(
            Some(FailedStation {
                failure,
                network: Some(drain),
            }),
            Source::StopUnconfirmed,
            true,
        ),
        RetirementFailure {
            failure: Some(failure),
            network: Some(drain),
            cause: RetirementCause::NetworkDrain(drain),
            hardware_stopped: true,
        }
    );
    // A requested stop fails with the drain itself.
    let lost = NetworkDrain {
        cause: NetworkDrainCause::ParticipantLost,
        held: 1,
    };
    assert_eq!(
        super::convert::network_drain(oer_ieee80211_runtime::station::NetworkDrainFailure {
            cause: oer_ieee80211_runtime::station::NetworkDrainCause::ParticipantLost,
            held: 1,
        }),
        lost
    );
}
#[test]
fn a_release_into_a_poisoned_domain_or_with_an_active_mac_is_not_a_stop() {
    use oer_esp32s31_hal::owner::maintenance::Error as Maintenance;
    use oer_esp32s31_radio_client::DomainHealth;
    use oer_esp32s31_radio_client::{Poison, PoisonCause, PoisonOrigin};
    assert_eq!(super::convert::released(DomainHealth::Healthy), Ok(()));
    let poison = Poison {
        origin: PoisonOrigin::CloseRf,
        cause: PoisonCause::Domain(oer_esp32s31_phy::concurrent::ConcurrentPhyError::Time(
            oer_esp32s31_phy::state::client::PhyTrackTimeError::TimeReversed {
                class: oer_esp32s31_phy::state::client::PhyPllTrackClass::BluetoothIeee802154,
                previous_micros: 9,
                now_micros: 2,
            },
        )),
    };
    assert_eq!(
        super::convert::released(DomainHealth::Poisoned(poison)),
        Err(ReleaseCause::DomainPoisoned(super::convert::radio_poison(
            poison
        )))
    );
    assert_eq!(
        super::convert::still_active(Maintenance::MacActive { state: 3 }),
        ReleaseCause::StillActive(MaintenanceError::MacActive { state: 3 })
    );
    assert_eq!(
        super::convert::still_active(Maintenance::RxWalkerEnabled),
        ReleaseCause::StillActive(MaintenanceError::RxWalkerEnabled)
    );
}
