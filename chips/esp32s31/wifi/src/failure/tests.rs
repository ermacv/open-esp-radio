use super::*;
#[test]
fn completion_status_and_receive_credit_observations_survive_the_boundary() {
    use oer_chips_esp32s31_ieee80211_driver::{
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
    use oer_chips_esp32s31_phy::{
        concurrent::ConcurrentPhyError,
        state::client::{PhyPllTrackClass as Class, PhyTrackTimeError as Time},
    };
    use oer_chips_esp32s31_radio_client::{Poison, PoisonCause, PoisonOrigin, RequestKind};
    let value = Poison {
        origin: PoisonOrigin::Request {
            client: oer_chips_esp32s31_hal::domain::RadioClient::Wifi,
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
fn a_release_into_a_poisoned_domain_or_with_an_active_mac_is_not_a_stop() {
    use oer_chips_esp32s31_hal::owner::maintenance::Error as Maintenance;
    use oer_chips_esp32s31_radio_client::DomainHealth;
    use oer_chips_esp32s31_radio_client::{Poison, PoisonCause, PoisonOrigin};
    assert_eq!(super::convert::released(DomainHealth::Healthy), Ok(()));
    let poison = Poison {
        origin: PoisonOrigin::CloseRf,
        cause: PoisonCause::Domain(
            oer_chips_esp32s31_phy::concurrent::ConcurrentPhyError::Time(
                oer_chips_esp32s31_phy::state::client::PhyTrackTimeError::TimeReversed {
                    class:
                        oer_chips_esp32s31_phy::state::client::PhyPllTrackClass::BluetoothIeee802154,
                    previous_micros: 9,
                    now_micros: 2,
                },
            ),
        ),
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
#[test]
fn a_port_fault_keeps_its_whole_path_under_the_lifecycle_failure() {
    use oer_chips_esp32s31_ieee80211_driver::{
        lower_mac::arena::ArenaFault as Arena, mac::tx::ampdu::HtAmpduTxError as Chain, port::Fault,
    };
    let failure = LifecycleFailure::Port(super::convert::backend_fault(Fault::Arena(
        Arena::Chain(Chain::DetachFailed),
    )));
    assert_eq!(
        failure,
        LifecycleFailure::Port(BackendFault::Arena(ArenaFault::Chain(
            AggregateTxError::DetachFailed
        )))
    );
    let path = oer_portable_diagnostics::DiagPath::of(&failure);
    assert_eq!(path.names(), ["Port", "Arena", "Chain", "DetachFailed"]);
    assert!(path.words().is_empty() && !path.overflowed());
    // A received buffer dropped without its recycle is named as such.
    let lost = LifecycleFailure::Port(super::convert::backend_fault(Fault::ReceiveLost));
    assert_eq!(lost, LifecycleFailure::Port(BackendFault::ReceiveLost));
    assert_eq!(
        oer_portable_diagnostics::DiagPath::of(&lost).names(),
        ["Port", "ReceiveLost"]
    );
}
#[test]
fn a_named_driver_fault_keeps_its_queue_and_operation_across_the_boundary() {
    use oer_chips_esp32s31_ieee80211_driver::port::{CoreOp, Fault};
    assert_eq!(
        super::convert::backend_fault(Fault::DuplicateCompletion { queue: 2 }),
        BackendFault::DuplicateCompletion { queue: 2 }
    );
    assert_eq!(
        super::convert::backend_fault(Fault::CoreViewUnavailable(CoreOp::Tbtt)),
        BackendFault::CoreViewUnavailable(CoreOperation::Tbtt)
    );
    assert_eq!(
        super::convert::backend_fault(Fault::CommandExpired(
            oer_protocols_ieee80211_contracts_port::CommandKind::InstallGroup
        )),
        BackendFault::CommandExpired(PortCommandKind::InstallGroup)
    );
    let path = oer_portable_diagnostics::DiagPath::of(&BackendFault::QueueFull { queue: 3 });
    assert_eq!(path.names(), &["QueueFull"]);
    assert_eq!(path.words(), &[3]);
}

#[test]
fn a_lifecycle_failure_names_its_cause_and_keeps_its_observations() {
    use oer_portable_diagnostics::DiagPath;
    let reinstall = DiagPath::of(&LifecycleFailure::Reinstall(ReinstallCause::Epochs));
    assert_eq!(reinstall.names(), ["Reinstall", "Epochs"]);
    let stop = DiagPath::of(&LifecycleFailure::StopUnconfirmed(
        oer_protocols_ieee80211_runtime::datapath::StopObserved {
            port: None,
            deadline: true,
            arena_on_air: false,
            frames_out: 2,
        },
    ));
    assert_eq!(stop.names(), ["StopUnconfirmed"]);
    assert_eq!(stop.words(), [1, 0, 2]);
    let actor = DiagPath::of(&LifecycleFailure::Actor(
        oer_protocols_ieee80211_runtime::datapath::ActorViolation::PairwiseCustody,
    ));
    assert_eq!(actor.names(), ["Actor", "PairwiseCustody"]);
}

#[test]
fn a_refused_start_keeps_the_retune_cause_in_its_path() {
    use oer_chips_esp32s31_ieee80211_driver::port::{Fault, WifiRetuneError};
    use oer_chips_esp32s31_radio_client::Failure;
    use oer_portable_diagnostics::DiagPath;
    use oer_protocols_ieee80211_contracts_port::StartRefused;
    let refused = super::convert::start_refused(StartRefused::Retune(Fault::Retune(
        WifiRetuneError::Radio(Failure::Refused),
    )));
    assert_eq!(
        refused,
        StartRefused::Retune(BackendFault::Retune(RetuneError::Radio(
            super::convert::radio_failure(Failure::Refused)
        )))
    );
    let path = DiagPath::of(&LifecycleFailure::StartRefused(refused));
    assert_eq!(path.names(), ["StartRefused", "Retune", "Retune", "Radio"]);
    assert!(path.overflowed(), "the radio failure's subtree is cut");
    let permit = DiagPath::of(&LifecycleFailure::StartRefused(
        super::convert::start_refused(StartRefused::Retune(Fault::ChannelNotPermitted)),
    ));
    assert_eq!(
        permit.names(),
        ["StartRefused", "Retune", "ChannelNotPermitted"]
    );
    let device = DiagPath::of(&LifecycleFailure::StartRefused(
        super::convert::start_refused(StartRefused::Device),
    ));
    assert_eq!(device.names(), ["StartRefused", "Device"]);
}
