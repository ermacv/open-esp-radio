use core::pin::Pin;
use oer_ieee80211_mac::sequence::SequenceNumber;

use crate::connection_coex::ReconnectFramePriority;

use oer_esp32s31_hal::types::{
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome,
    MacTxDetachReason, MacTxProtection, MacTxQueueDetached,
};
use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
    tx::protection::BssProtection,
    tx::{HardwareOwnedTxDma, PreparedTxDma, TxSlot, TxSlotState},
};
use oer_ieee80211_mac::protection::ErpProtection;
use oer_ieee80211_mac::station::StaTxSequenceCounters;

use super::*;
use crate::single_mpdu_tx::{ConnectedTxSecurity, SingleMpduTxConfig};
use oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerPair;

#[derive(Default)]
struct Hardware {
    prepare: bool,
    publications: u8,
    completions: [Option<MacTxCompletionObservation>; 2],
    completion_index: usize,
    timeout: bool,
    legacy: Option<(u8, MacLegacyTxProgram)>,
}

impl TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy = Some((queue, program));
        self.prepare
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications += 1;
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        let completion = self.completions.get_mut(self.completion_index)?.take();
        if completion.is_some() {
            self.completion_index += 1;
        }
        completion
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        self.timeout
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Timeout if !self.timeout => MacTxDetachOutcome::NoEvent,
            MacTxDetachReason::Timeout => {
                self.timeout = false;
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Collision => MacTxDetachOutcome::NoEvent,
        }
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        _index: u8,
        _identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {}
}

#[derive(Clone, Copy)]
struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

type Timer = oer_time_virtual::SkipClock;

fn completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
}

fn make_tx<'a>(
    slot: Pin<&'a mut TxSlot<256>>,
) -> ControlTransmitter<'a, Power, fn() -> u32, Timer, 256> {
    fn entropy() -> u32 {
        0x1234_5678
    }
    ControlTransmitter::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        ControlTxConfig {
            completion_timeout: oer_time::Duration::from_micros(10),
            poll_interval: oer_time::Duration::from_micros(1),
        },
    )
}

#[test]
fn authentication_is_encoded_and_completed_by_the_shared_owner() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: SequenceNumber::new(7).unwrap(),
        },
        None,
    ));

    assert!(matches!(result, Ok(completion) if completion.status() == 0));
    assert_eq!(hardware.publications, 1);
    let (_, program) = hardware.legacy.expect("management publication");
    assert_eq!(program.interface(), MacInterface::Station);
    assert_eq!(program.scheduler_priority(), 1);
    assert_eq!(program.packet_priority(), 1);
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::Free);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_eq!(&bytes[TX_METADATA_SIZE..TX_METADATA_SIZE + 2], &[0xb0, 0]);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
        &[0x70, 0]
    );
}

#[test]
/// Join-path EAPOL follows the default interface schedule (1 Mbit/s DSSS),
/// which ERP protection never covers: only OFDM rates need CTS-to-Self.
fn erp_protected_join_eapol_leaves_at_the_dsss_schedule_rate_without_protection() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());
    tx.install_bss_protection(BssProtection {
        erp: ErpProtection::new(true, false),
        ..BssProtection::UNPROTECTED
    });
    let result = crate::test_support::block_on(tx.transmit_protected_data(
        &mut hardware,
        StaProtectedDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            destination: [0xff; 6],
            sequence_number: SequenceNumber::new(7).unwrap(),
            user_priority: 0,
            peer_qos: true,
            ccmp_header: [1, 0, 0, 0x20, 0, 0, 0, 0],
            ether_type: 0x888e,
            payload: &[1, 2, 3, 4],
        },
        LegacyTxQueue::Voice,
        1,
        None,
    ));
    assert!(result.is_ok());
    assert_eq!(hardware.publications, 1);
    let (_, program) = hardware.legacy.unwrap();
    assert_eq!(program.control().protection, MacTxProtection::None);
}

#[test]
fn ack_timeout_reuses_sequence_and_marks_the_retry_bit() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(5)), Some(completion(0))],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: SequenceNumber::new(11).unwrap(),
        },
        None,
    ));

    assert!(result.is_ok());
    assert_eq!(hardware.publications, 2);
    let bytes = tx.ordinary.slot.as_mut().buffer_mut().unwrap();
    assert_ne!(bytes[TX_METADATA_SIZE + 1] & 0x08, 0);
    assert_eq!(
        &bytes[TX_METADATA_SIZE + 22..TX_METADATA_SIZE + 24],
        &[0xb0, 0]
    );
}

#[test]
fn eapol_uses_the_recovered_voice_data_priority() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_unprotected_data(
        &mut hardware,
        StaDataFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: SequenceNumber::new(8).unwrap(),
            ether_type: 0x888e,
            payload: &[1, 2, 3, 4],
        },
        None,
    ));

    assert!(result.is_ok());
    let (_, program) = hardware.legacy.expect("EAPOL publication");
    assert_eq!(program.scheduler_priority(), 3);
    assert_eq!(program.packet_priority(), 3);
}

#[test]
fn a_reconnecting_station_publishes_connection_frames_at_event_46_priority() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());

    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: SequenceNumber::new(7).unwrap(),
        },
        Some(ReconnectFramePriority {
            packet: 10,
            scheduler: 5,
        }),
    ));

    assert!(result.is_ok());
    let (_, program) = hardware.legacy.expect("management publication");
    assert_eq!(program.packet_priority(), 10);
    assert_eq!(program.scheduler_priority(), 5);
    assert_eq!(program.priority_count(), ReconnectFramePriority::COUNT);
}

#[test]
fn missing_hardware_timeout_edge_quarantines_without_drop_panic() {
    let mut slot = std::boxed::Box::pin(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    {
        let mut tx = make_tx(slot.as_mut());

        let result = crate::test_support::block_on(tx.transmit_open_authentication(
            &mut hardware,
            OpenAuthenticationRequest {
                source: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                sequence_number: SequenceNumber::new(13).unwrap(),
            },
            None,
        ));

        assert_eq!(
            result,
            Err(ControlTxError::RadioResetRequired(
                TxResetReason::ExecutorDeadline
            ))
        );
        assert_eq!(tx.ordinary.slot.state(), TxSlotState::ResetRequired);
        assert!(tx.ordinary.slot.as_mut().reserve(64, 32).is_err());
    }
    let drop_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(slot)));
    assert!(drop_result.is_ok());
}

#[test]
fn passive_fallback_requires_a_proven_quiescent_tx_owner() {
    assert!(ControlTxError::HardwareTimeout.retains_quiescent_owner());
    assert!(ControlTxError::CollisionLimit.retains_quiescent_owner());
    assert!(!ControlTxError::Busy.retains_quiescent_owner());
    assert!(
        !ControlTxError::RadioResetRequired(TxResetReason::ExecutorDeadline)
            .retains_quiescent_owner()
    );
}

#[test]
fn connected_handoff_preserves_the_descriptor_and_association_policy() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware::default();
    let mut tx = make_tx(slot.as_mut());
    tx.install_he_bss_color(37);
    let key = install_sta_pairwise_ccmp(
        &mut hardware,
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        &[0x5a; 16],
    )
    .unwrap();

    let connected = tx
        .try_into_connected(ConnectedTxHandoff {
            security: ConnectedTxSecurity::Wpa2Personal(key),
            sequences: StaTxSequenceCounters::new(SequenceNumber::new(9).unwrap()),
            config: SingleMpduTxConfig {
                station_address: [2, 3, 4, 5, 6, 7],
                bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
                ssid: oer_ieee80211_mac::ssid::WifiSsid::new(b"test").unwrap(),
                peer_qos: true,
                management_protection: false,
                access_category: LegacyTxQueue::BestEffort.access_category(),
                control_schedule:
                    oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE,
                publication_timeout: oer_time::Duration::from_micros(10),
            },
        })
        .unwrap_or_else(|_| panic!("idle owner must transfer"));

    assert_eq!(connected.policy().he_bss_color(), 37);
    assert!(!connected.active());
}

#[test]
fn active_handoff_returns_tx_and_crypto_resources_for_later_retry() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut hardware = Hardware {
        prepare: true,
        ..Hardware::default()
    };
    let mut tx = make_tx(slot.as_mut());
    let frame_length = OpenAuthenticationRequest {
        source: [2, 3, 4, 5, 6, 7],
        bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: SequenceNumber::new(15).unwrap(),
    }
    .encode(&mut tx.ordinary.buffer_mut().unwrap()[TX_METADATA_SIZE..])
    .unwrap();
    tx.ordinary
        .start(
            &mut hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::Voice.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                    publication_limit: 1,
                    publication_timeout: oer_time::Duration::from_micros(10),
                },
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
                priority_count: 1,
            },
        )
        .unwrap();
    let refused = Timer::starting_at(oer_time::Instant::from_micros(999));
    let Err((ControlTxError::Busy, refused)) = tx.rebind_timer(refused) else {
        panic!("an active publication retains its clock capability");
    };
    assert_eq!(
        oer_time::Clock::now(&refused),
        oer_time::Instant::from_micros(999)
    );
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::HardwareOwned);
    assert_eq!(hardware.publications, 1);

    let key = install_sta_pairwise_ccmp(
        &mut hardware,
        [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        &[0x5a; 16],
    )
    .unwrap();
    let key_index = key.hardware_index();
    let handoff = ConnectedTxHandoff {
        security: ConnectedTxSecurity::Wpa2Personal(key),
        sequences: StaTxSequenceCounters::new(SequenceNumber::new(9).unwrap()),
        config: SingleMpduTxConfig {
            station_address: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            ssid: oer_ieee80211_mac::ssid::WifiSsid::new(b"test").unwrap(),
            peer_qos: true,
            management_protection: false,
            access_category: LegacyTxQueue::BestEffort.access_category(),
            control_schedule:
                oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE,
            publication_timeout: oer_time::Duration::from_micros(10),
        },
    };

    let (mut tx, handoff) = match tx.try_into_connected(handoff) {
        Err(resources) => resources,
        Ok(_) => panic!("hardware-owned descriptor must reject handoff"),
    };
    assert_eq!(tx.ordinary.slot.state(), TxSlotState::HardwareOwned);
    assert!(matches!(
        &handoff.security,
        ConnectedTxSecurity::Wpa2Personal(key) if key.hardware_index() == key_index
    ));

    hardware.completions[0] = Some(completion(0));
    assert_eq!(
        crate::test_support::block_on(
            tx.ordinary
                .service_polling(&mut hardware, oer_time::Duration::from_micros(1))
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(tx.try_into_connected(handoff).is_ok());
}

#[test]
fn idle_timer_rebind_preserves_policy_and_terminal_observation() {
    let mut slot = core::pin::pin!(TxSlot::<256>::new_model());
    let mut tx = make_tx(slot.as_mut());
    let mut hardware = Hardware {
        prepare: true,
        completions: [Some(completion(0)), None],
        ..Hardware::default()
    };
    let result = crate::test_support::block_on(tx.transmit_open_authentication(
        &mut hardware,
        OpenAuthenticationRequest {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
            sequence_number: SequenceNumber::new(7).unwrap(),
        },
        None,
    ))
    .unwrap();
    let terminal = tx.ordinary.last_outcome();
    tx.policy_mut().install_he_bss_color(27);
    let old = tx
        .rebind_timer(Timer::starting_at(oer_time::Instant::from_micros(900)))
        .unwrap_or_else(|_| panic!("the completed owner is idle"));
    assert!(oer_time::Clock::now(&old) < oer_time::Instant::from_micros(900));
    assert_eq!(
        oer_time::Clock::now(&tx.ordinary.timer),
        oer_time::Instant::from_micros(900)
    );
    assert_eq!(tx.ordinary.last_outcome(), terminal);
    assert_eq!(tx.policy().he_bss_color(), 27);
    assert_eq!(result.status(), 0);
    assert_eq!(hardware.publications, 1);
}
