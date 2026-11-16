//! The ordinary transmitter's phases and the management retries, over the
//! transmit queues' double.

use super::*;
use crate::mac::irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};
use crate::mac::tx::{TxDmaState, runtime::WifiTxRuntimePolicy};
use crate::test_hardware::{
    BE, BSSID, Hardware, OFDM24, PEER, Power, STATION, TIMEOUT, Timer, data_frame, entropy,
    interrupt, queued_free_slot, slot,
};
use crate::tx::WifiTxProgress;
use oer_chips_esp32s31_hal::types::{MacLegacyTxResponse, MacTxCompletionObservation};
use oer_protocols_ieee80211_wire::phy::LegacyRate;

#[test]
fn classic_transmitter_lends_only_its_current_phase_and_returns_idle_after_detach() {
    use crate::ordinary_tx::{OrdinaryTxAccess, OrdinaryTxInterface};
    let mut owner = OrdinaryTxOwner::new(WifiTxResources {
        slot: slot(),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: entropy as fn() -> u32,
        timer: Timer::default(),
    });
    let frame = data_frame(PEER);
    let plan = OrdinaryTxPlan {
        frame_length: frame.len(),
        descriptor_capacity: None,
        exchange: MacTxPlan {
            access_category: WmmAccessCategory::BestEffort,
            initial_rate: TxPhyRate::try_from(OFDM24).unwrap(),
            publication_limit: 1,
            publication_timeout: oer_portable_time::Duration::from_micros(TIMEOUT),
        },
        hardware_mic_length: 0,
        hardware_key_selector: 0,
        interface: OrdinaryTxInterface::Station,
        scheduler_priority: 1,
        packet_priority: 1,
        priority_count: 1,
    };
    let mut hardware = Hardware::default();
    let OrdinaryTxAccess::Idle(mut idle) = owner.access() else {
        panic!("a fresh transmitter lends its idle phase");
    };
    idle.buffer_mut().unwrap()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()]
        .copy_from_slice(&frame);
    assert_eq!(
        idle.start_single_attempt(
            &mut hardware,
            plan,
            SingleAttempt::new(SingleAttemptProtection::None)
        )
        .unwrap(),
        WifiTxProgress::Pending
    );
    assert_eq!(owner.slot_state(), TxDmaState::HardwareOwned);
    let mut owner = owner
        .into_idle()
        .err()
        .expect("a live transaction cannot enter queued mode");
    let OrdinaryTxAccess::Active(active) = owner.access() else {
        panic!("a published transaction lends only its active phase");
    };
    let collision = crate::mac::irq::event_mask(
        oer_chips_esp32s31_hal::types::MacInterruptCause::TxQueueCollision.set(),
    );
    assert_eq!(
        active.service(&mut hardware, interrupt(collision)).unwrap(),
        WifiTxProgress::Pending
    );
    assert_eq!(owner.slot_state(), TxDmaState::HardwareOwned);
    assert_eq!(hardware.publications(), 1);
    let OrdinaryTxAccess::Active(active) = owner.access() else {
        panic!("a published transaction lends only its active phase");
    };
    hardware.completion[BE] = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        active
            .service(&mut hardware, interrupt(EVENT_TX_COMPLETE))
            .unwrap(),
        WifiTxProgress::Complete
    );
    assert!(matches!(owner.access(), OrdinaryTxAccess::Idle(_)));
    assert!(owner.into_idle().is_ok());
    assert_eq!(hardware.publications(), 1);
}

#[test]
fn reserved_descriptor_never_lends_classic_idle_authority() {
    use crate::ordinary_tx::OrdinaryTxAccess;
    let mut slot = slot();
    let cookie = queued_free_slot(slot.as_mut()).reserve(64, 32).unwrap();
    let owner = OrdinaryTxOwner::new(WifiTxResources {
        slot,
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: entropy as fn() -> u32,
        timer: Timer::default(),
    });
    let mut owner = owner
        .into_idle()
        .err()
        .expect("a reserved descriptor retains its phase");
    assert!(matches!(
        owner.access(),
        OrdinaryTxAccess::Unavailable(TxDmaState::Reserved)
    ));
    let _ = cookie;
}

#[test]
fn an_irq_legacy_transmitter_refuses_ht_before_reserving_or_publishing_dma() {
    use crate::mac::tx::{HtChannelWidth, HtGuardInterval, HtMcs, HtRate};
    use crate::ordinary_tx::{OrdinaryTxAccess, OrdinaryTxError, OrdinaryTxInterface};
    let mut owner = OrdinaryTxOwner::new_legacy(WifiTxResources {
        slot: slot(),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: entropy as fn() -> u32,
        timer: Timer::default(),
    });
    let mut hardware = Hardware::default();
    let plan = OrdinaryTxPlan {
        frame_length: 24,
        descriptor_capacity: None,
        exchange: MacTxPlan {
            access_category: WmmAccessCategory::Voice,
            initial_rate: TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs0,
                HtGuardInterval::Long800Ns,
                HtChannelWidth::Mhz20,
            )),
            publication_limit: 1,
            publication_timeout: oer_portable_time::Duration::from_micros(TIMEOUT),
        },
        hardware_mic_length: 0,
        hardware_key_selector: 0,
        interface: OrdinaryTxInterface::Station,
        scheduler_priority: 1,
        packet_priority: 1,
        priority_count: 1,
    };
    let OrdinaryTxAccess::Idle(idle) = owner.access() else {
        panic!("fresh owner");
    };
    assert_eq!(
        idle.start_single_attempt(
            &mut hardware,
            plan,
            SingleAttempt::new(SingleAttemptProtection::None)
        ),
        Err(OrdinaryTxError::UnsupportedPhyRate)
    );
    assert_eq!(hardware.publications(), 0);
    assert!(!owner.active());
    assert_eq!(owner.slot_state(), TxDmaState::Free);
}

/// Hardware observations fed to the actual retained management transmitter.
#[derive(Clone, Copy)]
enum ManagementObservation {
    Delivered,
    AckTimeout,
    CtsTimeout,
    CollisionCompletion,
    Collision,
    HardwareTimeout,
}

/// Drive the production management owner through modeled completions,
/// inspecting semantic HAL publications rather than register images.
fn check_management_transmissions(
    receiver: [u8; 6],
    management: oer_protocols_ieee80211_contracts_port::ManagementTxPlan,
    completions: &[ManagementObservation],
) {
    use crate::ordinary_tx::{OrdinaryTxAccess, OrdinaryTxInterface, OrdinaryTxRetryReport};
    use ManagementObservation::*;
    use oer_chips_esp32s31_hal::types::MacLegacyRate as ChipLegacyRate;
    use oer_protocols_ieee80211_contracts_port::backend::RetryCounters;
    use oer_protocols_ieee80211_wire::frame::FrameControl;

    let mut owner = OrdinaryTxOwner::new(WifiTxResources {
        slot: slot(),
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy: entropy as fn() -> u32,
        timer: Timer::default(),
    });
    let mut frame = [0; 24];
    frame[0] = 0x40; // Probe Request.
    frame[4..10].copy_from_slice(&receiver);
    frame[10..16].copy_from_slice(&STATION);
    frame[16..22].copy_from_slice(&BSSID);
    let plan = OrdinaryTxPlan {
        frame_length: frame.len(),
        descriptor_capacity: None,
        exchange: MacTxPlan {
            access_category: WmmAccessCategory::BestEffort,
            // The management plan must replace both caller values.
            initial_rate: TxPhyRate::Legacy(ChipLegacyRate::Ofdm54M),
            publication_limit: 32,
            publication_timeout: oer_portable_time::Duration::from_micros(TIMEOUT),
        },
        hardware_mic_length: 0,
        hardware_key_selector: 0,
        interface: OrdinaryTxInterface::Station,
        scheduler_priority: 1,
        packet_priority: 1,
        priority_count: 1,
    };
    let mut hardware = Hardware::default();
    let OrdinaryTxAccess::Idle(mut idle) = owner.access() else {
        panic!("a fresh transmitter is idle");
    };
    idle.buffer_mut().unwrap()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()]
        .copy_from_slice(&frame);
    assert_eq!(
        idle.start_management(&mut hardware, plan, management)
            .unwrap(),
        WifiTxProgress::Pending
    );
    let rate = crate::mac::portable::legacy_from_phy_rate(management.rate());
    let mut expected_retries = OrdinaryTxRetryReport::default();
    for (index, &observation) in completions.iter().enumerate() {
        let snapshot = owner.active_snapshot().unwrap();
        assert_eq!(snapshot.current_rate, TxPhyRate::Legacy(rate));
        assert_eq!(usize::from(snapshot.publications), index + 1);
        assert_eq!(hardware.publications(), index + 1);
        assert_eq!(snapshot.retries, expected_retries);
        assert_eq!(
            snapshot.counters,
            RetryCounters {
                mpdu: expected_retries.ack_timeouts,
                short: expected_retries.total(),
                long: 0,
            }
        );
        assert_eq!(snapshot.retry_bit_set, expected_retries.ack_timeouts != 0);
        assert_eq!(
            owner
                .policy()
                .contention_exponent(LegacyTxQueue::BestEffort),
            4 + expected_retries.total().min(6)
        );
        let program = hardware.legacy.last().unwrap().1;
        assert_eq!(program.rate(), rate);
        assert_eq!(
            program.response(),
            if receiver[0] & 1 == 0 {
                MacLegacyTxResponse::Ack
            } else {
                MacLegacyTxResponse::None
            }
        );
        let terminal = index + 1 == completions.len();
        match observation {
            HardwareTimeout => {
                assert!(terminal, "a hardware timeout cannot republish management");
                hardware.timeout_pending[BE] = true;
            }
            Collision => hardware.collision_pending[BE] = true,
            _ => {
                let (status, detail) = match observation {
                    Delivered => (0, 0),
                    AckTimeout => (5, 0),
                    CtsTimeout => (2, 0),
                    CollisionCompletion => (4, 1),
                    _ => unreachable!("handled detached observations"),
                };
                hardware.completion[BE] =
                    Some(MacTxCompletionObservation::new_model(status, detail));
            }
        }
        let OrdinaryTxAccess::Active(active) = owner.access() else {
            panic!("the management frame is still active");
        };
        let progress = active
            .service(
                &mut hardware,
                interrupt(if matches!(observation, HardwareTimeout) {
                    EVENT_TX_TIMEOUT
                } else {
                    EVENT_TX_COMPLETE
                }),
            )
            .unwrap();
        let progress = if matches!(observation, HardwareTimeout) {
            assert_eq!(progress, WifiTxProgress::Pending);
            // The contention window is retained through the bounded abort settle.
            assert_eq!(
                owner
                    .policy()
                    .contention_exponent(LegacyTxQueue::BestEffort),
                4 + expected_retries.total().min(6)
            );
            owner.timer.advance_to(owner.next_deadline().unwrap());
            let OrdinaryTxAccess::Active(active) = owner.access() else {
                panic!("the timeout settle retains its owner");
            };
            active.service(&mut hardware, WifiTxWake::Deadline).unwrap()
        } else {
            progress
        };
        assert_eq!(
            progress,
            if terminal {
                WifiTxProgress::Complete
            } else {
                WifiTxProgress::Pending
            }
        );
        if !terminal {
            match observation {
                AckTimeout => expected_retries.ack_timeouts += 1,
                CtsTimeout => expected_retries.cts_timeouts += 1,
                CollisionCompletion | Collision => expected_retries.collisions += 1,
                Delivered | HardwareTimeout => panic!("terminal observation cannot retry"),
            }
        }
    }
    let report = owner.take_last_outcome().unwrap().report();
    assert_eq!(usize::from(report.status.attempts), completions.len());
    assert_eq!(report.status.final_rate, TxPhyRate::Legacy(rate));
    assert_eq!(owner.slot_state(), TxDmaState::Free);
    assert_eq!(hardware.publications(), completions.len());
    assert_eq!(report.retries, expected_retries);
    assert_eq!(
        owner
            .policy()
            .contention_exponent(LegacyTxQueue::BestEffort),
        4
    );
    let OrdinaryTxAccess::Idle(mut idle) = owner.access() else {
        panic!("a terminal management exchange returns its buffer");
    };
    let returned = &idle.buffer_mut().unwrap()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()];
    let control = FrameControl::new(u16::from_le_bytes(returned[..2].try_into().unwrap())).unwrap();
    assert_eq!(control.retry(), expected_retries.ack_timeouts != 0);
    assert_eq!(
        control
            .with_retry(oer_protocols_ieee80211_wire::frame::Retry::First)
            .get()
            .to_le_bytes(),
        frame[..2]
    );
    assert_eq!(returned[2..], frame[2..]);
}

#[test]
fn management_unicast_keeps_its_rate_for_seven_attempts() {
    use ManagementObservation::*;
    for rate in [LegacyRate::Ofdm6M, LegacyRate::Ofdm12M] {
        for failures in [
            [AckTimeout; 7],
            [CtsTimeout; 7],
            [Collision; 7],
            [CollisionCompletion; 7],
            [
                CtsTimeout,
                Collision,
                AckTimeout,
                CtsTimeout,
                CollisionCompletion,
                AckTimeout,
                AckTimeout,
            ],
        ] {
            check_management_transmissions(
                PEER,
                oer_protocols_ieee80211_core_upper_mac::management::unicast(rate),
                &failures,
            );
        }
        check_management_transmissions(
            PEER,
            oer_protocols_ieee80211_core_upper_mac::management::unicast(rate),
            &[Delivered],
        );
    }
}

#[test]
fn management_retries_preserve_retry_bit_counters_and_reset_edca_at_success_or_timeout() {
    use ManagementObservation::*;
    for observations in [
        &[AckTimeout, Delivered][..],
        &[CtsTimeout, Delivered],
        &[Collision, Delivered],
        &[CollisionCompletion, Delivered],
        &[CtsTimeout, Collision, CtsTimeout, Delivered],
        &[AckTimeout, CtsTimeout, Collision, Delivered],
        &[AckTimeout, HardwareTimeout],
        &[CtsTimeout, Collision, HardwareTimeout],
    ] {
        check_management_transmissions(
            PEER,
            oer_protocols_ieee80211_core_upper_mac::management::unicast(LegacyRate::Ofdm12M),
            observations,
        );
    }
}

#[test]
fn management_broadcast_never_retries() {
    use ManagementObservation::*;
    for rate in [LegacyRate::Ofdm6M, LegacyRate::Ofdm12M] {
        for completion in [
            Delivered,
            AckTimeout,
            CtsTimeout,
            Collision,
            CollisionCompletion,
            HardwareTimeout,
        ] {
            check_management_transmissions(
                [0xff; 6],
                oer_protocols_ieee80211_core_upper_mac::management::broadcast(rate),
                &[completion],
            );
        }
    }
}
