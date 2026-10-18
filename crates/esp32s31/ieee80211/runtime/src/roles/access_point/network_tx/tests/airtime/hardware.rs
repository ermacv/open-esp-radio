//! Model hardware for AP runtime publication/completion tests.
use oer_esp32s31_hal::types::*;
use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxPowerPair, WifiTxPowerProfile};
use oer_esp32s31_ieee80211_mac::{
    crypto::CcmpKeyHardware,
    rx::hardware::{RxBlockAckHardware, S31RxBlockAckAgreement, S31RxBlockAckAgreementError},
    tx::ampdu::HtAmpduHardware,
    tx::{HardwareOwnedTxDma, PreparedTxDma},
};
use oer_ieee80211_mac::sequence::SequenceNumber;
#[derive(Default)]
pub(super) struct Hardware {
    pub(super) legacy_publications: usize,
    pub(super) ht_publications: usize,
    pub(super) he_publications: usize,
    pub(super) last_legacy_queue: Option<u8>,
    pub(super) last_ht_queue: Option<u8>,
    pub(super) last_he_queue: Option<u8>,
    pub(super) ordinary_completion: Option<MacTxCompletionObservation>,
    pub(super) aggregate_completion: Option<MacHtAmpduCompletionObservation>,
    pub(super) abort_requests: usize,
    pub(super) timeout_detaches: usize,
    pub(super) timeout_detach_succeeds: bool,
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

pub(super) struct Power;

impl WifiTxPowerProfile for Power {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

pub(super) type Timer = crate::roles::station::tx::test_support::Timer;

impl oer_esp32s31_ieee80211_mac::ap_policy::ApRxPolicyHardware for Hardware {
    fn apply_ap_link_policy(&mut self, _: [u8; 6]) {}
    fn disable_ap_link_policy(&mut self) {}
}
impl oer_esp32s31_ieee80211_mac::ap_tsf::ApTsfHardware for Hardware {
    fn reset_and_start_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
    }
    fn stop_access_point_tsf(
        &mut self,
        _: oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsfWrite,
    ) {
    }
}
impl RxBlockAckHardware for Hardware {
    fn program_rx_block_ack(
        &mut self,
        _: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
    fn clear_rx_block_ack(&mut self, _: u8) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
    fn reset_rx_block_ack_window(
        &mut self,
        _: u8,
        _: u8,
        _: SequenceNumber,
        _: u16,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
    fn program_extra_softap_rx_block_ack(
        &mut self,
        _: S31RxBlockAckAgreement,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
    fn clear_extra_softap_rx_block_ack(
        &mut self,
        _: u8,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
    fn reset_extra_softap_rx_block_ack_window(
        &mut self,
        _: u8,
        _: SequenceNumber,
    ) -> Result<(), S31RxBlockAckAgreementError> {
        unreachable!()
    }
}
