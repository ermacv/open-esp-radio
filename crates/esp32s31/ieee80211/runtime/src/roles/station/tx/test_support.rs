//! Host models for exercising the station TX owner outside this crate.
//!
//! The model hardware accepts every publication, records it and returns the
//! completions a test installs. It is available to crate tests and, with the
//! `test-support` feature, to host tests of other packages; the underlying
//! MAC/PAC model constructors exist only on 64-bit hosts.

use core::pin::Pin;

use oer_esp32s31_hal::types::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerTxQueueSnapshot, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtTxProgram,
    MacKeyInstallOutcome, MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome,
    MacTxDetachReason, MacTxQueueDetached,
};

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxPowerPair, WifiTxResources};

use oer_esp32s31_ieee80211_mac::{
    crypto::{CcmpKeyHardware, install_sta_pairwise_ccmp},
    tx::{
        HardwareOwnedTxDma, HtChannelWidth, HtGuardInterval, HtMcs, HtRate, PreparedTxDma, TxSlot,
        ampdu::HtAmpduHardware, runtime::WifiTxRuntimePolicy,
    },
};

use oer_esp32s31_ieee80211_sta::single_mpdu_tx::{
    ConnectedTxHandoff, ConnectedTxSecurity, SingleMpduTx, SingleMpduTxConfig,
};

use oer_ieee80211_mac::{
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
    station::{STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaTxSequenceCounters},
};

use super::WifiTxPowerProfile;

pub const STATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
pub const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
pub const TEST_FRAME_CAPACITY: usize = 64;
pub const TEST_HEADROOM: usize = oer_esp32s31_ieee80211_mac::tx::ampdu::TX_AMPDU_METADATA_SIZE
    + STA_PROTECTED_QOS_ETHERNET_HEADROOM;
pub const TEST_TRAILER: usize = 12;
pub const TEST_QUEUE_DEPTH: usize = 3;
pub const TEST_SLOTS: usize = 3;
pub const TEST_BUFFER_SIZE: usize = 256;
pub const TEST_RATE: HtRate = HtRate::new(
    HtMcs::Mcs7,
    HtGuardInterval::Short400Ns,
    HtChannelWidth::Mhz20,
);

#[derive(Default)]
pub struct Hardware {
    pub legacy_publications: usize,
    pub ht_publications: usize,
    pub he_publications: usize,
    pub last_legacy_queue: Option<u8>,
    pub last_ht_queue: Option<u8>,
    pub last_he_queue: Option<u8>,
    pub ordinary_completion: Option<MacTxCompletionObservation>,
    pub aggregate_completion: Option<MacHtAmpduCompletionObservation>,
    pub abort_requests: usize,
    pub timeout_detaches: usize,
    pub timeout_detach_succeeds: bool,
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

impl oer_esp32s31_ieee80211_mac::tx::TxHardware for Hardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        self.legacy_publications += 1;
        self.last_legacy_queue = Some(queue);
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        _program: MacHtTxProgram,
    ) -> bool {
        self.ht_publications += 1;
        self.last_ht_queue = Some(queue);
        true
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        _program: MacHeTxProgram,
    ) -> bool {
        self.he_publications += 1;
        self.last_he_queue = Some(queue);
        true
    }

    fn start_bound_he_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.ordinary_completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        self.abort_requests += 1;
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Timeout => {
                self.timeout_detaches += 1;
                if self.timeout_detach_succeeds {
                    MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                        expected_descriptor_head,
                    )))
                } else {
                    MacTxDetachOutcome::Failed
                }
            }
            MacTxDetachReason::Collision | MacTxDetachReason::Completed => {
                MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                    expected_descriptor_head,
                )))
            }
        }
    }

    fn take_block_ack_completion(&mut self, _queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        self.aggregate_completion.take()
    }
}

impl HtAmpduHardware for Hardware {
    fn prepare_he_trigger_based_queue(
        &mut self,
        _policy: MacHeTbTidLimit,
        _reservation: MacHeTbLinkReservation,
        _tid: MacHeTid,
        _mpdu_lengths: &[u16],
        _queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        unreachable!("HT tests never publish a trigger-based HE queue")
    }

    fn clear_he_trigger_based_queue(&mut self, _reservation: MacHeTbLinkReservation) {}
}

pub struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

/// Two model clocks with different epochs and a common tick rate. Waiting
/// advances both readings; a generation change remains a separate operation.
/// TX requires only the read-only MAC sample capability, so this model does
/// not implement reception conversion or RF wake notification.
pub struct Timer {
    monotonic: oer_time_virtual::SkipClock,
    radio: core::cell::Cell<oer_ieee80211_lower_mac::Ieee80211Instant>,
    pub generation: core::cell::Cell<u32>,
    pub available: core::cell::Cell<bool>,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            monotonic: oer_time_virtual::SkipClock::new(),
            radio: core::cell::Cell::new(oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(
                5_000,
            )),
            generation: core::cell::Cell::new(1),
            available: core::cell::Cell::new(true),
        }
    }
}
impl oer_time::Clock for Timer {
    fn now(&self) -> oer_time::Instant {
        oer_time::Clock::now(&self.monotonic)
    }
}
impl oer_time::Timer for Timer {
    async fn wait_until(&self, at: oer_time::Instant) {
        let before = oer_time::Clock::now(&self.monotonic);
        oer_time::Timer::wait_until(&self.monotonic, at).await;
        let elapsed = oer_time::Clock::now(&self.monotonic).saturating_duration_since(before);
        self.radio.set(
            self.radio
                .get()
                .checked_add(oer_time::RadioDuration::from_micros(elapsed.as_micros()))
                .expect("model tick advance stays in its radio epoch"),
        );
    }
}
impl crate::mac_clock::MacClockReader for Timer {
    fn sample(&self) -> Option<oer_ieee80211_lower_mac::Ieee80211ClockSample> {
        self.available
            .get()
            .then(|| oer_ieee80211_lower_mac::Ieee80211ClockSample {
                source: self.radio.get(),
                target: oer_time::Clock::now(self),
                uncertainty: oer_time::Duration::from_micros(0),
                generation: crate::clock_generation(self.generation.get()),
            })
    }
}

pub fn aggregate_completion(
    starting_sequence: u16,
    bitmap: u64,
) -> MacHtAmpduCompletionObservation {
    MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(0, 0),
        0,
        starting_sequence,
        bitmap,
        true,
    )
}

pub fn make_ordinary<'a, const BUFFER_SIZE: usize>(
    slot: Pin<&'a mut TxSlot<BUFFER_SIZE>>,
    hardware: &mut Hardware,
) -> SingleMpduTx<'a, Power, fn() -> u32, Timer, BUFFER_SIZE> {
    fn entropy() -> u32 {
        0x1234_5678
    }

    let key = install_sta_pairwise_ccmp(hardware, BSSID, &[0x5a; 16]).unwrap();
    SingleMpduTx::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: Power,
            entropy,
            timer: Timer::default(),
        },
        ConnectedTxHandoff {
            security: ConnectedTxSecurity::Wpa2Personal(key),
            sequences: StaTxSequenceCounters::new(SequenceNumber::new(7).unwrap()),
            config: SingleMpduTxConfig {
                station_address: STATION,
                bssid: BSSID,
                ssid: oer_ieee80211_mac::ssid::WifiSsid::new(b"test").unwrap(),
                peer_qos: true,
                management_protection: false,
                access_category: WmmAccessCategory::BestEffort,
                control_schedule:
                    oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE,
                publication_timeout: oer_time::Duration::from_micros(250_000),
            },
        },
    )
}
