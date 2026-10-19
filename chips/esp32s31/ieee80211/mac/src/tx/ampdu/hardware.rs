//! Safe register-side authority required by the A-MPDU lifecycle.

use crate::tx::TxHardware;

use oer_esp32s31_hal::{
    ieee80211::mac::WifiMacHal,
    owner::RadioRuntimeOwner,
    types::{
        MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
        MacHeTriggerTxQueueSnapshot,
    },
};

/// Hardware authority needed specifically by aggregate and HE Trigger-based
/// publication.
pub trait HtAmpduHardware: TxHardware {
    /// Prepare one queue for a future AP Trigger before publishing TX enable.
    ///
    /// Implementations must validate every fallible input before the first
    /// hardware write so an error cannot leave a partially published queue.
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError>;

    /// Remove Trigger eligibility only after DMA ownership has returned.
    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation);

    /// Read back a live Trigger queue while the aggregate owner still retains
    /// the reservation. Test doubles may return `None`.
    fn he_trigger_based_queue_snapshot(
        &self,
        _reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        None
    }
}

impl HtAmpduHardware for WifiMacHal<'_> {
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        WifiMacHal::prepare_he_trigger_based_queue(
            self,
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        WifiMacHal::clear_he_trigger_based_queue(self, reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        Some(WifiMacHal::he_trigger_based_queue_snapshot(
            self,
            reservation,
        ))
    }
}

impl HtAmpduHardware for RadioRuntimeOwner {
    fn prepare_he_trigger_based_queue(
        &mut self,
        policy: MacHeTbTidLimit,
        reservation: MacHeTbLinkReservation,
        tid: MacHeTid,
        mpdu_lengths: &[u16],
        queued_msdu_bytes: u32,
    ) -> Result<MacHeTriggerTxQueueSnapshot, MacHeTbProgramError> {
        HtAmpduHardware::prepare_he_trigger_based_queue(
            &mut self.wifi_mac_hal(),
            policy,
            reservation,
            tid,
            mpdu_lengths,
            queued_msdu_bytes,
        )
    }

    fn clear_he_trigger_based_queue(&mut self, reservation: MacHeTbLinkReservation) {
        HtAmpduHardware::clear_he_trigger_based_queue(&mut self.wifi_mac_hal(), reservation);
    }

    fn he_trigger_based_queue_snapshot(
        &self,
        reservation: MacHeTbLinkReservation,
    ) -> Option<MacHeTriggerTxQueueSnapshot> {
        Some(RadioRuntimeOwner::he_trigger_based_queue_snapshot(
            self,
            reservation,
        ))
    }
}
