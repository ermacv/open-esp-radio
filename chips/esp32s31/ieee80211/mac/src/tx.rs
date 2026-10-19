//! Bounded q0 legacy TX descriptor, submission and completion ownership.

#![forbid(unsafe_code)]

pub use oer_ieee80211_softmac::tx_cost::TxContention;

use core::{
    num::{NonZeroU32, NonZeroU64},
    pin::Pin,
};

#[cfg(any(test, feature = "test-support"))]
extern crate alloc;
#[cfg(any(test, feature = "test-support"))]
use alloc::boxed::Box;

pub use oer_esp32s31_hal::types::MacLegacyTxResponse;
pub use oer_memory::{HardwareOwnedTxDma, PreparedTxDma};

use oer_esp32s31_hal::types::{
    MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTbTidLimit, MacHeTid,
    MacHeTxFormat, MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation,
    MacHtChannelWidth, MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate,
    MacHtTxFormat, MacHtTxParameters, MacHtTxProgram, MacInterface, MacLegacyRate,
    MacLegacyTxParameters, MacLegacyTxProgram, MacPartialRuPowerSelector,
    MacTxCompletionObservation, MacTxControlFrame, MacTxDetachOutcome, MacTxDetachReason,
    MacTxProtection, MacTxQueueDetached,
};

pub use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaState as TxSlotState;
#[cfg(any(test, feature = "test-support"))]
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage;

use oer_esp32s31_hal::ieee80211::dma::tx_storage::{PinnedTxDmaStorage, TxDmaStorageError};

use oer_ieee80211_lower_mac::TxStatus;
use oer_ieee80211_mac::qos::WmmAccessCategory;

pub use oer_ieee80211_mac::trigger::HeResourceUnit;

use crate::rate::{
    low::{MacLowRateGateProbe, MacLowRateTransitionError, probe_phy_low_rate_gate},
    schedule::{RateScheduleKind, RateScheduleRef},
};

use oer_espressif_ieee80211_policy::retry_ladder;

use oer_esp32s31_hal::{ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner};

use oer_ieee80211_mac::{
    he::HeDcmConstellation,
    ht::HtDuplicateMcs32,
    trigger::{
        TriggerCommonInfo, TriggerFrame, TriggerGiLtf, TriggerParseError, TriggerRuAllocation,
        TriggerType, TriggerUserSpatialStreamInfo, parse_trigger_user_spatial_stream,
    },
};

mod completion;
mod cost;
pub use completion::{CompletionFault, CompletionObservation, CompletionRaw, decode};

const LEGACY_FCS_LENGTH: u16 = 4;
// SOURCE: HIL_VENDOR_HE20_MCS9_SU_2026_07_29. Two synchronous vendor HE SU
// A-MPDU formatter captures used descriptor flags 0xc0403009, entry class one
// and the bounded BCC/non-STBC A2 low control image 0x105.

/// Hardware lifetime class selected by the descriptor's A-MPDU-container bit.
///
/// This is not an EDCA access category or a management/data distinction.
/// Complete `libpp[lmac.o]::lmacSetTxFrame` tests descriptor flag
/// `0x0040_0000`: clear loads `lmacConfMib + 0x08`, while set loads
/// `lmacConfMib + 0x00`. Complete `lmacInit` initializes those two lifetimes
/// to `0x400` and `0x600`, shifts the selected value left by ten, and passes it
/// through `ppProcessLifeTime`. A freshly submitted direct queue image has one
/// elapsed lifetime unit removed: `0x03ff` for a direct MPDU and `0x05ff` for
/// an A-MPDU container.
///
/// These values apply only when encoding immediately before publication. A
/// future queued scheduler must retain enqueue TSF and recompute the remaining
/// lifetime instead of reusing these values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxLifetimeClass {
    DirectMpdu,
    AmpduContainer,
}

impl TxLifetimeClass {
    pub const fn fresh_queue_timeout(self) -> u16 {
        match self {
            Self::DirectMpdu => 0x03ff,
            Self::AmpduContainer => 0x05ff,
        }
    }
}

/// The four ordinary EDCA hardware queues recovered from `ppTxPkt`.
///
/// The names follow the standard user-priority mapping: priorities 6/7 use
/// voice, 4/5 video, 0/3 best effort, and 1/2 background.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LegacyTxQueue {
    #[default]
    Voice = 0,
    Video = 1,
    BestEffort = 2,
    Background = 3,
}

impl LegacyTxQueue {
    pub(crate) const fn index(self) -> u8 {
        self.hardware_index()
    }

    /// Return the queue number used by the ESP32-S31 MAC transaction.
    ///
    /// This is intentionally chip-specific: callers must not infer it from
    /// portable WMM access-category numbering.
    pub const fn hardware_index(self) -> u8 {
        self as u8
    }

    /// Translate the portable EDCA category into the ESP32-S31 queue order.
    ///
    /// The hardware order is VO/VI/BE/BK rather than the standard ACI order,
    /// so this mapping belongs in the chip-specific MAC adapter.
    pub const fn from_access_category(category: WmmAccessCategory) -> Self {
        match category {
            WmmAccessCategory::Voice => Self::Voice,
            WmmAccessCategory::Video => Self::Video,
            WmmAccessCategory::BestEffort => Self::BestEffort,
            WmmAccessCategory::Background => Self::Background,
        }
    }

    pub const fn access_category(self) -> WmmAccessCategory {
        match self {
            Self::Voice => WmmAccessCategory::Voice,
            Self::Video => WmmAccessCategory::Video,
            Self::BestEffort => WmmAccessCategory::BestEffort,
            Self::Background => WmmAccessCategory::Background,
        }
    }

    /// Packet PTI assigned by complete vendor data encapsulation.
    ///
    /// Complete `libnet80211[ieee80211_output.o]::ieee80211_encap_esfbuf` maps voice/video/best-effort/background to
    /// coexistence events 10/11/12/13. The complete pinned
    /// `libcoexist.a[coexist_core.o]::coex_pti_tab` maps those events to
    /// 3/2/1/1.
    pub const fn vendor_data_packet_priority(self) -> u8 {
        match self {
            Self::Voice => 3,
            Self::Video => 2,
            Self::BestEffort | Self::Background => 1,
        }
    }

    /// Queue scheduler PTI selected by complete vendor data encapsulation.
    ///
    /// Complete `libpp[hal_mac.o]::mac_tx_set_pti` takes the
    /// unsigned minimum of the packet PTI and coexistence event-one PTI 5.
    /// Every ordinary data priority is below five, so it is retained.
    pub const fn vendor_data_scheduler_priority(self) -> u8 {
        self.vendor_data_packet_priority()
    }
}

/// Finite ordinary-queue hardware authority used by one owned TX slot.
pub trait TxHardware {
    /// Exercise the complete reviewed PHY low-rate gate and restore its entry
    /// state before returning.
    ///
    /// Pure queue backends do not implicitly gain a PHY owner. Production
    /// implementations override this method only when they can serialize the
    /// three ROM-proved register edges with ordinary TX.
    fn probe_phy_low_rate_gate(
        &mut self,
    ) -> Result<MacLowRateGateProbe, MacLowRateTransitionError> {
        Ok(MacLowRateGateProbe::OwnerUnavailable)
    }

    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool;

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8);

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacHtTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacHeTxProgram,
    ) -> bool {
        false
    }

    fn start_bound_he_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {}
    /// Sample one ordinary queue without changing or acknowledging it.
    ///
    /// Pure/model backends may omit this low-frequency hardware diagnostic.
    fn ordinary_tx_queue_snapshot(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacOrdinaryTxQueueSnapshot> {
        None
    }
    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation>;
    /// Take the completion of a PPDU that solicited a BlockAck: an A-MPDU or
    /// a BlockAckReq. The queue's three BlockAck words are sampled before
    /// the completion edge is acknowledged, while that edge still owns them,
    /// so this cannot be replaced by [`Self::take_tx_completion`].
    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation>;
    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool;
    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R>;
}

impl TxHardware for WifiMacHal<'_> {
    fn probe_phy_low_rate_gate(
        &mut self,
    ) -> Result<MacLowRateGateProbe, MacLowRateTransitionError> {
        probe_phy_low_rate_gate(self)
    }

    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        WifiMacHal::prepare_bound_legacy_tx(self, dma, queue, program)
    }

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        WifiMacHal::start_bound_tx(self, dma, queue);
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        WifiMacHal::prepare_bound_ht_tx(self, dma, queue, program)
    }

    fn start_bound_ht_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        WifiMacHal::start_bound_tx(self, dma, queue);
    }

    fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        WifiMacHal::prepare_bound_he_tx(self, dma, queue, program)
    }

    fn start_bound_he_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        WifiMacHal::start_bound_tx(self, dma, queue);
    }

    fn ordinary_tx_queue_snapshot(
        &mut self,
        queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacOrdinaryTxQueueSnapshot> {
        Some(WifiMacHal::ordinary_tx_queue_snapshot(self, queue))
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        WifiMacHal::take_tx_completion(self, queue)
    }

    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        WifiMacHal::take_block_ack_completion(self, queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        WifiMacHal::begin_tx_timeout_abort(self, queue)
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        _expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        WifiMacHal::with_tx_queue_detached(self, queue, reason, detached)
    }
}

impl TxHardware for RadioRuntimeOwner {
    fn probe_phy_low_rate_gate(
        &mut self,
    ) -> Result<MacLowRateGateProbe, MacLowRateTransitionError> {
        TxHardware::probe_phy_low_rate_gate(&mut self.wifi_mac_hal())
    }

    fn prepare_bound_legacy_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_legacy_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_legacy_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_legacy_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn prepare_bound_ht_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_ht_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_ht_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_ht_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn prepare_bound_he_tx(
        &mut self,
        dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHeTxProgram,
    ) -> bool {
        TxHardware::prepare_bound_he_tx(&mut self.wifi_mac_hal(), dma, queue, program)
    }

    fn start_bound_he_tx(&mut self, dma: &dyn HardwareOwnedTxDma, queue: u8) {
        TxHardware::start_bound_he_tx(&mut self.wifi_mac_hal(), dma, queue);
    }

    fn ordinary_tx_queue_snapshot(
        &mut self,
        queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacOrdinaryTxQueueSnapshot> {
        Some(self.wifi_mac_hal().ordinary_tx_queue_snapshot(queue))
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        TxHardware::take_tx_completion(&mut self.wifi_mac_hal(), queue)
    }

    fn take_block_ack_completion(&mut self, queue: u8) -> Option<MacHtAmpduCompletionObservation> {
        TxHardware::take_block_ack_completion(&mut self.wifi_mac_hal(), queue)
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        TxHardware::begin_tx_timeout_abort(&mut self.wifi_mac_hal(), queue)
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        TxHardware::with_tx_queue_detached(
            &mut self.wifi_mac_hal(),
            queue,
            expected_descriptor_head,
            reason,
            detached,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCookie(pub u32);

/// Completed-publication metadata, constructed only after Observed(status).
/// Its retained status proof is independent of optional diagnostic fields.
/// NoEffect and CompletionFault cannot construct this value.
///
/// ```compile_fail,E0451
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// # use oer_esp32s31_ieee80211_mac::tx::{TxCompletion, TxCookie};
/// # use oer_esp32s31_hal::types::MacTxCompletionObservation;
/// # use oer_ieee80211_lower_mac::TxStatus;
/// fn forge(raw: MacTxCompletionObservation) -> TxCompletion {
///     TxCompletion { cookie: TxCookie(1), observation: raw, tx_status: TxStatus::Success }
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxCompletion {
    cookie: TxCookie,
    observation: MacTxCompletionObservation,
    tx_status: TxStatus,
}

impl TxCompletion {
    /// Hardware completion status used by the vendor ACK-timeout path.
    ///
    /// SOURCE: the six-entry jump table in complete
    /// `libpp[lmac.o]::lmacProcessTxComplete` maps status five to
    /// `libpp[lmac.o]::lmacProcessAckTimeout` (status zero maps to ordinary TX success).
    pub const ACK_TIMEOUT_STATUS: u8 = 5;

    pub const fn cookie(&self) -> TxCookie {
        self.cookie
    }

    pub const fn status(&self) -> u8 {
        self.observation.status()
    }

    /// Whether hardware associated this completion with a Trigger-based flow.
    pub const fn is_trigger_flow(&self) -> bool {
        self.observation.trigger_flow()
    }

    pub const fn used_alternate_record(&self) -> bool {
        self.observation.used_alternate()
    }

    /// Low completion detail byte selected and decoded by the PAC owner.
    pub const fn detail(&self) -> u8 {
        self.observation.detail()
    }

    /// The status retained when the pure decoder observed this completion.
    /// NoEffect and decoder faults never construct a TxCompletion.
    pub const fn tx_status(&self) -> TxStatus {
        self.tx_status
    }

    /// Return whether this completion belongs to a Trigger-based transmit flow.
    pub const fn is_trigger_based(&self) -> bool {
        self.is_trigger_flow()
    }

    /// Count the ordinary Trigger-based packets reported by hardware.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessTxComplete`
    /// stores completion extension word zero bits 19:13 at per-queue state
    /// byte `+0x30`. Complete `lmacProcess{Short,Long}RetryFail` consumes that
    /// byte before deciding whether to enter `lmacProcessTBSuccess`.
    pub const fn trigger_based_packet_count(&self) -> u8 {
        self.observation.trigger_based_packet_count()
    }

    /// Return whether the completion selects the additional TB packet count.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessTxComplete`
    /// copies completion extension word zero bit 20 to per-queue state byte
    /// `+0x2e`. The retry leaves add byte `+0x2f` only when this flag is set.
    pub const fn last_tx_was_trigger_based(&self) -> bool {
        self.observation.last_tx_was_trigger_based()
    }

    /// Decode the conditional secondary TB packet count.
    ///
    /// Complete `hal_mac_get_txq_complete` reconstructs extension word one as
    /// `(A[21:20]) | (C[13:7] << 2)`. `lmacProcessTxComplete` shifts that word
    /// right by two before storing byte `+0x2f`, so the consumed seven-bit
    /// count is exactly raw completion word C bits 13:7.
    pub const fn secondary_trigger_based_packet_count(&self) -> u8 {
        self.observation.secondary_trigger_based_packet_count()
    }

    /// Reproduce the vendor's narrow ACK-timeout-to-TB-success predicate.
    ///
    /// This does not reinterpret an arbitrary failure as an ACK. Status five
    /// dispatches through `lmacProcessAckTimeout`, which invokes the retry
    /// leaves with their collision/error selector clear. Complete
    /// `lmacProcess{Short,Long}RetryFail` then calls `lmacProcessTBSuccess`
    /// only when the queue is in Trigger flow and the sum of its applicable
    /// hardware packet counts is zero.
    pub const fn completes_vendor_trigger_flow(&self) -> bool {
        if self.status() != Self::ACK_TIMEOUT_STATUS || !self.is_trigger_flow() {
            return false;
        }
        let mut packets = self.trigger_based_packet_count() as u16;
        if self.last_tx_was_trigger_based() {
            packets += self.secondary_trigger_based_packet_count() as u16;
        }
        packets == 0
    }

    /// Decode the signed ACK-SNR sample consumed by vendor rate control.
    ///
    /// This value is meaningful only for a successful completion. Complete
    /// `hal_mac_get_txq_complete` copies `PRIMARY[23:16]` into result byte two;
    /// complete `lmacProcessTxSuccess` writes that byte to the descriptor's
    /// ACK-SNR slot; complete `rcUpdateTxDone` adds `wDevCtrl[0x2e]`, whose
    /// pinned initialized value is `0x60`, and narrows the sum to a signed
    /// byte before calling `rcUpdateAckSnr`.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::hal_mac_get_txq_complete`, `libpp[lmac.o]::lmacProcessTxSuccess`, and `libpp[trc.o]::rcUpdateTxDone`.
    pub const fn ack_snr_sample(&self) -> Option<i8> {
        if self.status() != 0 {
            return None;
        }
        let encoded = self.observation.ack_snr_encoded();
        Some(encoded.wrapping_add(0x60) as i8)
    }

    /// Construct a semantic completion for a native protocol model.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn new_model(
        cookie: TxCookie,
        status: u8,
        detail: u8,
    ) -> Result<CompletionObservation<Self>, CompletionFault> {
        decode_tx_completion(
            cookie,
            MacTxCompletionObservation::new_model(status, detail),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_trigger_flow_model(mut self, trigger_flow: bool) -> Self {
        self.observation = self.observation.with_trigger_flow_model(trigger_flow);
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_trigger_packet_counts_model(
        mut self,
        primary: u8,
        last_tx_was_trigger_based: bool,
        secondary: u8,
    ) -> Self {
        self.observation = self.observation.with_trigger_packet_counts_model(
            primary,
            last_tx_was_trigger_based,
            secondary,
        );
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn with_ack_snr_encoded_model(mut self, encoded: u8) -> Self {
        self.observation = self.observation.with_ack_snr_encoded_model(encoded);
        self
    }
}

/// Attach the status proof to diagnostic metadata only on Observed.
pub(crate) const fn decode_tx_completion(
    cookie: TxCookie,
    observation: MacTxCompletionObservation,
) -> Result<CompletionObservation<TxCompletion>, CompletionFault> {
    match decode(CompletionRaw::ordinary(observation)) {
        Ok(CompletionObservation::Observed(tx_status)) => {
            Ok(CompletionObservation::Observed(TxCompletion {
                cookie,
                observation,
                tx_status,
            }))
        }
        Ok(CompletionObservation::NoEffect) => Ok(CompletionObservation::NoEffect),
        Err(fault) => Err(fault),
    }
}

/// A hardware decoding fault always quarantines its owner's storage before
/// returning the existing completion fault. NoEffect leaves storage alone.
pub(crate) fn decode_or_quarantine(
    cookie: TxCookie,
    observation: MacTxCompletionObservation,
    quarantine: impl FnOnce(),
) -> Result<CompletionObservation<TxCompletion>, CompletionFault> {
    decode_tx_completion(cookie, observation).inspect_err(|_| quarantine())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxError {
    Busy,
    Invalid,
    QueueActive,
    Stale,
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
    /// A completion contradicted the publication contract; retain DMA for reset.
    Completion(CompletionFault),
}

/// Hardware encoding of an ESP32-S31 non-HT transmit rate.
///
/// These values are not ordered by bitrate.  They are the exact indices used
/// by the MAC PLCP formatter and by the PHY target-power table.
///
/// SOURCE: sibling oracle
/// `../esp-wifi-sys/esp-wifi-sys-esp32s31/src/include.rs`, generated from
/// Espressif's `esp_wifi_types_generic.h` (`wifi_phy_rate_t`), cross-checked
/// against `libpp.a` rate schedules and the recovered
/// `phy_rate_to_index` ROM routine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum LegacyRate {
    #[default]
    Dsss1MLong = 0x00,
    Dsss2MLong = 0x01,
    Cck5M5Long = 0x02,
    Cck11MLong = 0x03,
    Dsss2MShort = 0x05,
    Cck5M5Short = 0x06,
    Cck11MShort = 0x07,
    Ofdm48M = 0x08,
    Ofdm24M = 0x09,
    Ofdm12M = 0x0a,
    Ofdm6M = 0x0b,
    Ofdm54M = 0x0c,
    Ofdm36M = 0x0d,
    Ofdm18M = 0x0e,
    Ofdm9M = 0x0f,
}

impl LegacyRate {
    /// HAL selector published for this rate.
    pub const fn pac_rate(self) -> MacLegacyRate {
        match self {
            Self::Dsss1MLong => MacLegacyRate::Dsss1MLong,
            Self::Dsss2MLong => MacLegacyRate::Dsss2MLong,
            Self::Cck5M5Long => MacLegacyRate::Cck5M5Long,
            Self::Cck11MLong => MacLegacyRate::Cck11MLong,
            Self::Dsss2MShort => MacLegacyRate::Dsss2MShort,
            Self::Cck5M5Short => MacLegacyRate::Cck5M5Short,
            Self::Cck11MShort => MacLegacyRate::Cck11MShort,
            Self::Ofdm48M => MacLegacyRate::Ofdm48M,
            Self::Ofdm24M => MacLegacyRate::Ofdm24M,
            Self::Ofdm12M => MacLegacyRate::Ofdm12M,
            Self::Ofdm6M => MacLegacyRate::Ofdm6M,
            Self::Ofdm54M => MacLegacyRate::Ofdm54M,
            Self::Ofdm36M => MacLegacyRate::Ofdm36M,
            Self::Ofdm18M => MacLegacyRate::Ofdm18M,
            Self::Ofdm9M => MacLegacyRate::Ofdm9M,
        }
    }

    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0x00 => Some(Self::Dsss1MLong),
            0x01 => Some(Self::Dsss2MLong),
            0x02 => Some(Self::Cck5M5Long),
            0x03 => Some(Self::Cck11MLong),
            0x05 => Some(Self::Dsss2MShort),
            0x06 => Some(Self::Cck5M5Short),
            0x07 => Some(Self::Cck11MShort),
            0x08 => Some(Self::Ofdm48M),
            0x09 => Some(Self::Ofdm24M),
            0x0a => Some(Self::Ofdm12M),
            0x0b => Some(Self::Ofdm6M),
            0x0c => Some(Self::Ofdm54M),
            0x0d => Some(Self::Ofdm36M),
            0x0e => Some(Self::Ofdm18M),
            0x0f => Some(Self::Ofdm9M),
            _ => None,
        }
    }

    /// Select this rate's vendor 802.11g retry-ladder entry.
    ///
    /// `failed_attempts` is the number of ACK/CTS failures already observed
    /// for the same MPDU. The first transmission therefore passes zero. The
    /// complete 54M ladder is `54M x2, 48M x2, 6M x3, 5.5M x25`; the other
    /// legacy rates use their corresponding records. `None` reports that the
    /// record's complete retry budget has been consumed.
    ///
    /// SOURCE: `libpp[trc.o]::rcGetRate, libpp[trc.o]::rcUpdatePhyMode` and the
    /// exact Rust-owned schedule arenas in [`crate::rate::schedule`],
    /// cross-checked against `SOURCE[PROMOTED_LMAC_TX]`
    /// `lmac.rs::select_basic_retry_rate`.
    pub fn vendor_retry_rate(self, failed_attempts: u8) -> Option<Self> {
        retry_ladder::legacy_retry_rate(self.phy_rate(), failed_attempts).map(Self::from_phy_rate)
    }

    /// Complete number of hardware publications admitted by this rate's
    /// vendor retry record, including the initial publication.
    ///
    /// This must not be inferred from the first retry-pair count. The vendor
    /// `rcReachRetryLimit` body reads byte `0x08`, while `rcGetRate` consumes
    /// the four `(rate, count)` pairs independently.
    pub fn vendor_retry_publication_limit(self) -> Option<u8> {
        retry_ladder::legacy_retry_publication_limit(self.phy_rate())
    }

    /// Return the basic protection rate selected by the vendor MAC.
    ///
    /// Despite the vendor symbol name, `mac_tx_get_rts_rate`, the result is
    /// always published by `mac_tx_set_len` in `TX_Q_LENGTH_CONTROL`; it is
    /// therefore part of every legacy PPDU image, not only frames which
    /// request explicit RTS/CTS protection.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_get_rts_rate` (size `0x96`) and the exhaustive reviewed
    /// production reconstruction.
    pub const fn vendor_control_rate(self) -> Self {
        match self {
            Self::Dsss1MLong => Self::Dsss1MLong,
            Self::Dsss2MLong | Self::Cck5M5Long | Self::Cck11MLong => Self::Dsss2MLong,
            Self::Dsss2MShort | Self::Cck5M5Short | Self::Cck11MShort => Self::Dsss2MShort,
            Self::Ofdm48M | Self::Ofdm24M | Self::Ofdm54M | Self::Ofdm36M => Self::Ofdm24M,
            Self::Ofdm12M | Self::Ofdm18M => Self::Ofdm12M,
            Self::Ofdm6M | Self::Ofdm9M => Self::Ofdm6M,
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        match self {
            Self::Dsss1MLong => 1_000,
            Self::Dsss2MLong | Self::Dsss2MShort => 2_000,
            Self::Cck5M5Long | Self::Cck5M5Short => 5_500,
            Self::Cck11MLong | Self::Cck11MShort => 11_000,
            Self::Ofdm6M => 6_000,
            Self::Ofdm9M => 9_000,
            Self::Ofdm12M => 12_000,
            Self::Ofdm18M => 18_000,
            Self::Ofdm24M => 24_000,
            Self::Ofdm36M => 36_000,
            Self::Ofdm48M => 48_000,
            Self::Ofdm54M => 54_000,
        }
    }
}

/// One-spatial-stream 802.11n modulation and coding scheme.
///
/// The ESP32-S31 is 1T1R, so the open HT path intentionally exposes only the
/// standard single-stream MCS0..MCS7 set. MCS8..MCS31 describe additional
/// spatial streams and cannot be made valid by passing an unchecked integer.
/// The special duplicate-mode MCS32 is represented separately by
/// [`HtDuplicateRate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HtMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs2 = 2,
    Mcs3 = 3,
    Mcs4 = 4,
    Mcs5 = 5,
    Mcs6 = 6,
    Mcs7 = 7,
}

impl HtMcs {
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Mcs0),
            1 => Some(Self::Mcs1),
            2 => Some(Self::Mcs2),
            3 => Some(Self::Mcs3),
            4 => Some(Self::Mcs4),
            5 => Some(Self::Mcs5),
            6 => Some(Self::Mcs6),
            7 => Some(Self::Mcs7),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtGuardInterval {
    #[default]
    Long800Ns,
    Short400Ns,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtChannelWidth {
    #[default]
    Mhz20,
    Mhz40,
}

/// Peer-derived HT minimum MPDU start-spacing class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HtProtectionSpacing {
    /// IEEE HT A-MPDU Parameters spacing codes zero through four.
    #[default]
    Density0To4,
    /// IEEE HT A-MPDU Parameters spacing code five.
    Density5,
    /// IEEE HT A-MPDU Parameters spacing code six.
    Density6,
    /// IEEE HT A-MPDU Parameters spacing code seven.
    Density7,
}

impl HtProtectionSpacing {
    /// Derive the exact finite hardware value from a complete HT A-MPDU
    /// Parameters byte (HT Capabilities IE payload byte two).
    ///
    /// SOURCE: complete `libpp[trc.o]::rcUpdateAMPDUParam`,
    /// size `0xde`, cross-checked against a coherent hardware-owned vendor HT
    /// queue containing three copies of value 40 (`0x0280_a028`).
    pub const fn from_ampdu_parameters(parameters: u8) -> Self {
        Self::from_density(HtAmpduDensity::from_ampdu_parameters(parameters))
    }

    pub const fn from_density(density: HtAmpduDensity) -> Self {
        match density.encoding() {
            0..=4 => Self::Density0To4,
            5 => Self::Density5,
            6 => Self::Density6,
            _ => Self::Density7,
        }
    }

    const fn pac_spacing(self) -> MacHtProtectionSpacing {
        match self {
            Self::Density0To4 => MacHtProtectionSpacing::Density0To4,
            Self::Density5 => MacHtProtectionSpacing::Density5,
            Self::Density6 => MacHtProtectionSpacing::Density6,
            Self::Density7 => MacHtProtectionSpacing::Density7,
        }
    }
}

/// Peer-advertised HT minimum MPDU start spacing.
///
/// The enum retains all eight IEEE encodings even though the S31 queue's
/// [`HtProtectionSpacing`] field collapses encodings zero through four.
/// HE delimiter construction needs the original encoding: the complete
/// vendor `rcUpdateAMPDUParam` converts it through the integer-microsecond
/// table `[0, 1, 1, 1, 2, 4, 8, 16]` before regenerating the HE minimum
/// subframe tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HtAmpduDensity {
    #[default]
    NoRestriction = 0,
    QuarterMicrosecond = 1,
    HalfMicrosecond = 2,
    OneMicrosecond = 3,
    TwoMicroseconds = 4,
    FourMicroseconds = 5,
    EightMicroseconds = 6,
    SixteenMicroseconds = 7,
}

impl HtAmpduDensity {
    /// Decode bits 4:2 of the complete HT A-MPDU Parameters byte.
    pub const fn from_ampdu_parameters(parameters: u8) -> Self {
        match (parameters >> 2) & 0x07 {
            0 => Self::NoRestriction,
            1 => Self::QuarterMicrosecond,
            2 => Self::HalfMicrosecond,
            3 => Self::OneMicrosecond,
            4 => Self::TwoMicroseconds,
            5 => Self::FourMicroseconds,
            6 => Self::EightMicroseconds,
            _ => Self::SixteenMicroseconds,
        }
    }

    pub const fn encoding(self) -> u8 {
        self as u8
    }

    /// Integer-microsecond value consumed by the pinned PP blob.
    ///
    /// SOURCE: complete `libpp[trc.o]::rcUpdateAMPDUParam`.
    /// Its two stack constants form the exact byte table
    /// `[0, 1, 1, 1, 2, 4, 8, 16]`; the selected byte is stored in ROM global
    /// `s_ht_ampdu_density_us` before `libpp[trc.o]::he_get_min_subframe_len`
    /// and its `_dcm` variant use it.
    pub const fn vendor_integer_microseconds(self) -> u8 {
        match self {
            Self::NoRestriction => 0,
            Self::QuarterMicrosecond | Self::HalfMicrosecond | Self::OneMicrosecond => 1,
            Self::TwoMicroseconds => 2,
            Self::FourMicroseconds => 4,
            Self::EightMicroseconds => 8,
            Self::SixteenMicroseconds => 16,
        }
    }
}

/// Complete runtime interpretation of one peer HT A-MPDU Parameters byte.
///
/// Keeping these three derived values together prevents upper layers from
/// independently reimplementing the vendor exponent and spacing branches.
/// The original byte is an association input, not a C-layout state image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtPeerAmpduParameters {
    density: HtAmpduDensity,
    protection_spacing: HtProtectionSpacing,
    maximum_aggregate_bytes: u16,
}

impl HtPeerAmpduParameters {
    /// Decode the peer's HT Capabilities A-MPDU Parameters byte.
    ///
    /// SOURCE: complete `libpp[trc.o]::rcUpdateAMPDUParam`.
    /// Bits 1:0 select exactly `0x1fff`, `0x3fff`, `0x7fff` or `0xffff`;
    /// bits 4:2 select the retained IEEE density and its collapsed S31 queue
    /// protection value.
    pub const fn from_capability_byte(parameters: u8) -> Self {
        let density = HtAmpduDensity::from_ampdu_parameters(parameters);
        let maximum_aggregate_bytes = match parameters & 0x03 {
            0 => 0x1fff,
            1 => 0x3fff,
            2 => 0x7fff,
            _ => 0xffff,
        };
        Self {
            density,
            protection_spacing: HtProtectionSpacing::from_density(density),
            maximum_aggregate_bytes,
        }
    }

    pub const fn density(self) -> HtAmpduDensity {
        self.density
    }

    pub const fn protection_spacing(self) -> HtProtectionSpacing {
        self.protection_spacing
    }

    pub const fn maximum_aggregate_bytes(self) -> u16 {
        self.maximum_aggregate_bytes
    }
}

impl Default for HtPeerAmpduParameters {
    fn default() -> Self {
        Self::from_capability_byte(0)
    }
}

/// Complete typed HT PHY rate selected for one transmit attempt.
///
/// Channel width is both a PHY channel-engine precondition and part of the
/// queue vector. The same numeric rate code is used for HT20 and HT40, while
/// HT-SIG1 bit 7 and PLCP1 bit 29 publish CBW for the selected PPDU.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HtRate {
    pub mcs: HtMcs,
    pub guard_interval: HtGuardInterval,
    pub channel_width: HtChannelWidth,
}

impl HtRate {
    pub const fn new(
        mcs: HtMcs,
        guard_interval: HtGuardInterval,
        channel_width: HtChannelWidth,
    ) -> Self {
        Self {
            mcs,
            guard_interval,
            channel_width,
        }
    }

    const fn pac_rate(self) -> MacHtRate {
        let mcs = match self.mcs {
            HtMcs::Mcs0 => MacHtMcs::Mcs0,
            HtMcs::Mcs1 => MacHtMcs::Mcs1,
            HtMcs::Mcs2 => MacHtMcs::Mcs2,
            HtMcs::Mcs3 => MacHtMcs::Mcs3,
            HtMcs::Mcs4 => MacHtMcs::Mcs4,
            HtMcs::Mcs5 => MacHtMcs::Mcs5,
            HtMcs::Mcs6 => MacHtMcs::Mcs6,
            HtMcs::Mcs7 => MacHtMcs::Mcs7,
        };
        let guard_interval = match self.guard_interval {
            HtGuardInterval::Long800Ns => MacHtGuardInterval::Long800Ns,
            HtGuardInterval::Short400Ns => MacHtGuardInterval::Short400Ns,
        };
        let channel_width = match self.channel_width {
            HtChannelWidth::Mhz20 => MacHtChannelWidth::Mhz20,
            HtChannelWidth::Mhz40 => MacHtChannelWidth::Mhz40,
        };
        MacHtRate {
            mcs,
            guard_interval,
            channel_width,
        }
    }

    /// Exact S31 non-HE MAC rate code.
    ///
    /// SOURCE: sibling S31 `wifi_phy_rate_t`, complete
    /// `libpp[hal_mac_tx.o]::mac_tx_set_htsig`, and the recovered
    /// Dot11N rate schedules. Long-GI MCS0 starts at 16; short-GI MCS0 starts
    /// at 26.
    pub const fn code(self) -> u8 {
        let base = match self.guard_interval {
            HtGuardInterval::Long800Ns => 16,
            HtGuardInterval::Short400Ns => 26,
        };
        base + self.mcs.index()
    }

    /// Rate code used for target-power lookup.
    ///
    /// `hal_mac_tx_set_ppdu` subtracts ten from an SGI rate before loading the
    /// power pair, so LGI and SGI of the same MCS share one calibrated entry.
    pub const fn power_lookup_code(self) -> u8 {
        16 + self.mcs.index()
    }

    /// Legacy basic rate used by the protection/length vector.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_get_rts_rate` (size 0x96). Both GI code ranges select 6M for
    /// MCS0, 12M for MCS1/2 and 24M for MCS3..7.
    pub const fn vendor_control_rate(self) -> LegacyRate {
        match self.mcs {
            HtMcs::Mcs0 => LegacyRate::Ofdm6M,
            HtMcs::Mcs1 | HtMcs::Mcs2 => LegacyRate::Ofdm12M,
            HtMcs::Mcs3 | HtMcs::Mcs4 | HtMcs::Mcs5 | HtMcs::Mcs6 | HtMcs::Mcs7 => {
                LegacyRate::Ofdm24M
            }
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        const HT20_LGI: [u32; 8] = [
            6_500, 13_000, 19_500, 26_000, 39_000, 52_000, 58_500, 65_000,
        ];
        const HT20_SGI: [u32; 8] = [
            7_200, 14_400, 21_700, 28_900, 43_300, 57_800, 65_000, 72_200,
        ];
        const HT40_LGI: [u32; 8] = [
            13_500, 27_000, 40_500, 54_000, 81_000, 108_000, 121_500, 135_000,
        ];
        const HT40_SGI: [u32; 8] = [
            15_000, 30_000, 45_000, 60_000, 90_000, 120_000, 135_000, 150_000,
        ];
        let table = match (self.channel_width, self.guard_interval) {
            (HtChannelWidth::Mhz20, HtGuardInterval::Long800Ns) => &HT20_LGI,
            (HtChannelWidth::Mhz20, HtGuardInterval::Short400Ns) => &HT20_SGI,
            (HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns) => &HT40_LGI,
            (HtChannelWidth::Mhz40, HtGuardInterval::Short400Ns) => &HT40_SGI,
        };
        table[self.mcs.index() as usize]
    }

    /// Additional rate-dependent A-MPDU byte ceiling used by the vendor.
    ///
    /// SOURCE: complete `libpp[trc.o]::rx11NRate2AMPDULimit` and its complete 18-entry halfword table. The
    /// selector consumes only the HT rate code; width is therefore not an
    /// independent input. A zero table cell means that this leaf contributes
    /// no additional ceiling, leaving the negotiated/static A-MPDU limit in
    /// force.
    pub const fn vendor_ampdu_byte_limit(self) -> Option<u16> {
        const LIMITS: [u16; 18] = [
            6_490, 9_600, 19_200, 25_600, 43_200, 50_000, 57_600, 65_535, 0, 6_490, 9_600, 19_200,
            25_600, 43_200, 50_000, 57_600, 65_535, 0,
        ];
        let index = self.code().wrapping_sub(0x10) as usize;
        if index >= LIMITS.len() || LIMITS[index] == 0 {
            None
        } else {
            Some(LIMITS[index])
        }
    }

    /// Select one exact vendor Dot11N retry-ladder attempt.
    ///
    /// The recovered table has explicit records for every LGI MCS0..7 and for
    /// SGI MCS7, the vendor maximum-throughput starting point. Other fixed SGI
    /// MCS values are valid queue rates but have no independent record in the
    /// complete `rcUpdatePhyMode` mapping and therefore return `None` here
    /// instead of inventing a fallback policy.
    pub fn vendor_retry_rate(self, failed_attempts: u8) -> Option<TxPhyRate> {
        let oer_ieee80211_mac::phy::PhyRate::Ht(rate) = TxPhyRate::Ht(self).phy_rate() else {
            unreachable!("an S31 HT rate is a portable HT rate");
        };
        TxPhyRate::try_from(retry_ladder::ht_retry_rate(rate, failed_attempts)?).ok()
    }
}

/// Protocol-valid HT Duplicate MCS32 rate, kept outside [`HtMcs`].
///
/// MCS32 repeats one coded stream across both halves of a 40-MHz channel. It
/// is neither a fifth spatial stream nor a throughput successor to MCS7. This
/// type therefore fixes the channel width at 40 MHz and carries only the
/// independently selected guard interval.
///
/// The ESP32-S31 queue-rate, length, protection and calibrated-power encoding
/// for this mode has no reviewed oracle. Consequently this type deliberately
/// exposes none of the `code`, `power_lookup_code`, or retry-table methods of
/// [`HtRate`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HtDuplicateRate {
    mcs: HtDuplicateMcs32,
    guard_interval: HtGuardInterval,
}

impl HtDuplicateRate {
    pub const fn new(guard_interval: HtGuardInterval) -> Self {
        Self {
            mcs: HtDuplicateMcs32::new(),
            guard_interval,
        }
    }

    pub const fn mcs(self) -> HtDuplicateMcs32 {
        self.mcs
    }

    pub const fn mcs_index(self) -> u8 {
        HtDuplicateMcs32::INDEX
    }

    pub const fn guard_interval(self) -> HtGuardInterval {
        self.guard_interval
    }

    pub const fn channel_width(self) -> HtChannelWidth {
        HtChannelWidth::Mhz40
    }

    pub const fn nominal_kbps(self) -> u32 {
        match self.guard_interval {
            HtGuardInterval::Long800Ns => 6_000,
            HtGuardInterval::Short400Ns => 6_700,
        }
    }
}

/// One independently reviewable field required by the S31 MCS32 formatter.
///
/// The ordinary HT formatter cannot supply any of these fields by analogy:
/// its reviewed rate domain is exactly 16..=35 and its MCS reconstruction
/// yields only the ordinary MCS0..MCS9 numeric range. Keeping the gaps finite
/// lets diagnostics state the minimum missing oracle without publishing a
/// guessed raw rate code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HtDuplicateTxOracleField {
    /// Descriptor byte/flag which selects duplicate-mode MCS32.
    DescriptorSelector = 1 << 0,
    /// Complete PLCP0, PLCP1 and HT-SIG image for a known PSDU.
    PlcpAndHtSig = 1 << 1,
    /// DATA_LENGTH/LENGTH_CONTROL mapping and PPDU-duration enforcement.
    Length = 1 << 2,
    /// RTS/basic-rate selection and queue protection image.
    Protection = 1 << 3,
    /// Calibrated power-table lookup index and resulting queue power pair.
    Power = 1 << 4,
    /// Retry transition which preserves duplicate mode on every attempt.
    Retry = 1 << 5,
}

/// Set of reviewed formatter facts which are still absent for S31 MCS32.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtDuplicateTxOracleGaps(u8);

impl HtDuplicateTxOracleGaps {
    /// Exact frontier at the current reviewed-source boundary.
    pub const ESP32S31: Self = Self(
        HtDuplicateTxOracleField::DescriptorSelector as u8
            | HtDuplicateTxOracleField::PlcpAndHtSig as u8
            | HtDuplicateTxOracleField::Length as u8
            | HtDuplicateTxOracleField::Protection as u8
            | HtDuplicateTxOracleField::Power as u8
            | HtDuplicateTxOracleField::Retry as u8,
    );

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, field: HtDuplicateTxOracleField) -> bool {
        self.0 & field as u8 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Runtime/hardware evidence gate kept separate from source formatter fields.
///
/// A complete source reconstruction can build a candidate queue plan, while
/// this independent gate decides whether production may publish that plan.
/// Keeping both facts separate avoids describing an on-air observation as a
/// register field required to construct the image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HtDuplicateTxQualificationField {
    /// A controlled peer decodes MCS32 and returns the expected terminal ACK
    /// or BlockAck for the exact source-reconstructed queue image.
    OnAirAck = 1 << 0,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtDuplicateTxQualificationGaps(u8);

impl HtDuplicateTxQualificationGaps {
    pub const ESP32S31: Self = Self(HtDuplicateTxQualificationField::OnAirAck as u8);

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, field: HtDuplicateTxQualificationField) -> bool {
        self.0 & field as u8 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Complete evidence frontier reported by the fail-closed S31 boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtDuplicateTxEvidenceGaps {
    formatter: HtDuplicateTxOracleGaps,
    qualification: HtDuplicateTxQualificationGaps,
}

impl HtDuplicateTxEvidenceGaps {
    pub const ESP32S31: Self = Self {
        formatter: HtDuplicateTxOracleGaps::ESP32S31,
        qualification: HtDuplicateTxQualificationGaps::ESP32S31,
    };

    pub const fn formatter(self) -> HtDuplicateTxOracleGaps {
        self.formatter
    }

    pub const fn qualification(self) -> HtDuplicateTxQualificationGaps {
        self.qualification
    }
}

/// Explicit fail-closed boundary between protocol MCS32 and S31 TX hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtDuplicateTxUnavailable {
    /// Reviewed formatter fields and/or the independent qualification gate
    /// remain incomplete.
    Esp32s31EvidenceIncomplete(HtDuplicateTxEvidenceGaps),
}

impl HtDuplicateTxUnavailable {
    pub const fn evidence_gaps(self) -> HtDuplicateTxEvidenceGaps {
        match self {
            Self::Esp32s31EvidenceIncomplete(gaps) => gaps,
        }
    }
}

/// Explicit, finite request for the HT Duplicate certification path.
///
/// This request is deliberately separate from [`HtMcs`] and the recovered
/// rate-control schedules. Ordinary link adaptation can therefore never rank
/// MCS32 beside MCS0..MCS7. The duration is a caller-owned upper bound for one
/// PPDU; zero is retained as an invalid input so a planner can report the exact
/// rejection instead of silently substituting a policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtDuplicateCertificationRequest {
    channel_width: HtChannelWidth,
    guard_interval: HtGuardInterval,
    maximum_ppdu_duration_micros: u32,
}

/// Hardware-admitted MCS32 plan kept outside the ordinary PHY-rate domain.
///
/// This value can only be created by the private reviewed-hardware boundary.
/// In particular, it is not convertible to [`TxPhyRate`]: that enum selects
/// the ordinary legacy/HT/HE formatters and cannot represent duplicate mode.
/// The private fields also prevent an upper layer from manufacturing a plan
/// after performing only protocol capability checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtDuplicateTxPlan {
    rate: HtDuplicateRate,
    maximum_ppdu_duration_micros: NonZeroU32,
}

impl HtDuplicateTxPlan {
    pub const fn rate(self) -> HtDuplicateRate {
        self.rate
    }

    pub const fn maximum_ppdu_duration_micros(self) -> NonZeroU32 {
        self.maximum_ppdu_duration_micros
    }
}

impl HtDuplicateCertificationRequest {
    pub const fn new(
        channel_width: HtChannelWidth,
        guard_interval: HtGuardInterval,
        maximum_ppdu_duration_micros: u32,
    ) -> Self {
        Self {
            channel_width,
            guard_interval,
            maximum_ppdu_duration_micros,
        }
    }

    pub const fn channel_width(self) -> HtChannelWidth {
        self.channel_width
    }

    pub const fn guard_interval(self) -> HtGuardInterval {
        self.guard_interval
    }

    pub const fn maximum_ppdu_duration_micros(self) -> u32 {
        self.maximum_ppdu_duration_micros
    }
}

/// Negotiated facts consumed by the MCS32-only selector.
///
/// `channel_width == None` represents a legacy association. Keeping this
/// small value independent of STA/AP peer records gives both role planners
/// one shared validation and hardware-admission boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtDuplicateTxLinkCapabilities {
    channel_width: Option<HtChannelWidth>,
    peer_supports_mcs32: bool,
    peer_supports_short_guard_interval: bool,
}

impl HtDuplicateTxLinkCapabilities {
    pub const fn new(
        channel_width: Option<HtChannelWidth>,
        peer_supports_mcs32: bool,
        peer_supports_short_guard_interval: bool,
    ) -> Self {
        Self {
            channel_width,
            peer_supports_mcs32,
            peer_supports_short_guard_interval,
        }
    }

    pub const fn channel_width(self) -> Option<HtChannelWidth> {
        self.channel_width
    }

    pub const fn peer_supports_mcs32(self) -> bool {
        self.peer_supports_mcs32
    }

    pub const fn peer_supports_short_guard_interval(self) -> bool {
        self.peer_supports_short_guard_interval
    }
}

/// Exact reason an explicit MCS32 request was not selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtDuplicateTxRejection {
    ZeroMaximumPpduDuration,
    RequestedWidthMustBe40Mhz,
    LinkIsNot40Mhz,
    PeerDoesNotSupportMcs32,
    PeerDoesNotSupportShortGuardInterval,
    Hardware(HtDuplicateTxUnavailable),
}

/// Value-only planner/telemetry result for the independent MCS32 request.
///
/// A rejected request never changes the ordinary STA/AP fallback rate. A
/// future `Selected` value must come only from the hardware boundary below;
/// it is not a candidate in the recovered rate-control schedule.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HtDuplicateTxSelection {
    #[default]
    NotRequested,
    Rejected {
        request: HtDuplicateCertificationRequest,
        reason: HtDuplicateTxRejection,
    },
    Selected {
        request: HtDuplicateCertificationRequest,
        plan: HtDuplicateTxPlan,
    },
}

impl HtDuplicateTxSelection {
    pub const fn request(self) -> Option<HtDuplicateCertificationRequest> {
        match self {
            Self::NotRequested => None,
            Self::Rejected { request, .. } | Self::Selected { request, .. } => Some(request),
        }
    }

    pub const fn rejection(self) -> Option<HtDuplicateTxRejection> {
        match self {
            Self::Rejected { reason, .. } => Some(reason),
            Self::NotRequested | Self::Selected { .. } => None,
        }
    }

    pub const fn plan(self) -> Option<HtDuplicateTxPlan> {
        match self {
            Self::Selected { plan, .. } => Some(plan),
            Self::NotRequested | Self::Rejected { .. } => None,
        }
    }
}

/// Evaluate one fixed MCS32 request against negotiated link facts and the
/// reviewed ESP32-S31 hardware contract.
///
/// Width, GI and the caller's finite PPDU-duration bound are rejected before
/// touching the hardware boundary. Today every protocol-valid candidate then
/// fails closed because no tracked oracle proves the special queue selector,
/// HT-SIG/DATA_LENGTH image, protection rate or calibrated power lookup.
pub fn select_esp32s31_ht_duplicate_tx(
    request: Option<HtDuplicateCertificationRequest>,
    link: HtDuplicateTxLinkCapabilities,
) -> HtDuplicateTxSelection {
    let Some(request) = request else {
        return HtDuplicateTxSelection::NotRequested;
    };
    let rejection = if request.maximum_ppdu_duration_micros() == 0 {
        Some(HtDuplicateTxRejection::ZeroMaximumPpduDuration)
    } else if request.channel_width() != HtChannelWidth::Mhz40 {
        Some(HtDuplicateTxRejection::RequestedWidthMustBe40Mhz)
    } else if link.channel_width() != Some(HtChannelWidth::Mhz40) {
        Some(HtDuplicateTxRejection::LinkIsNot40Mhz)
    } else if !link.peer_supports_mcs32 {
        Some(HtDuplicateTxRejection::PeerDoesNotSupportMcs32)
    } else if request.guard_interval() == HtGuardInterval::Short400Ns
        && !link.peer_supports_short_guard_interval
    {
        Some(HtDuplicateTxRejection::PeerDoesNotSupportShortGuardInterval)
    } else {
        None
    };
    if let Some(reason) = rejection {
        return HtDuplicateTxSelection::Rejected { request, reason };
    }

    let Some(maximum_ppdu_duration_micros) =
        NonZeroU32::new(request.maximum_ppdu_duration_micros())
    else {
        return HtDuplicateTxSelection::Rejected {
            request,
            reason: HtDuplicateTxRejection::ZeroMaximumPpduDuration,
        };
    };
    match esp32s31_ht_duplicate_hardware_boundary(
        HtDuplicateRate::new(request.guard_interval()),
        maximum_ppdu_duration_micros,
    ) {
        Ok(plan) => HtDuplicateTxSelection::Selected { request, plan },
        Err(error) => HtDuplicateTxSelection::Rejected {
            request,
            reason: HtDuplicateTxRejection::Hardware(error),
        },
    }
}

/// Sole source boundary at which reviewed S31 MCS32 formatter evidence can be
/// attached. Until that evidence identifies every queue-vector and power
/// input, a protocol-valid certification request remains unpublishable.
fn esp32s31_ht_duplicate_hardware_boundary(
    _rate: HtDuplicateRate,
    _maximum_ppdu_duration_micros: NonZeroU32,
) -> Result<HtDuplicateTxPlan, HtDuplicateTxUnavailable> {
    Err(HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(
        HtDuplicateTxEvidenceGaps::ESP32S31,
    ))
}

/// One-spatial-stream 802.11ax modulation and coding scheme.
///
/// The ESP32-S31 is 1T1R. HE SU therefore admits MCS0..MCS9 for this first
/// bounded formatter; unchecked integers cannot request a second spatial
/// stream or the still-unqualified MCS10/11 modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HeMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs2 = 2,
    Mcs3 = 3,
    Mcs4 = 4,
    Mcs5 = 5,
    Mcs6 = 6,
    Mcs7 = 7,
    Mcs8 = 8,
    Mcs9 = 9,
}

impl HeMcs {
    pub const fn index(self) -> u8 {
        self as u8
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Mcs0),
            1 => Some(Self::Mcs1),
            2 => Some(Self::Mcs2),
            3 => Some(Self::Mcs3),
            4 => Some(Self::Mcs4),
            5 => Some(Self::Mcs5),
            6 => Some(Self::Mcs6),
            7 => Some(Self::Mcs7),
            8 => Some(Self::Mcs8),
            9 => Some(Self::Mcs9),
            _ => None,
        }
    }

    const fn pac_mcs(self) -> MacHeMcs {
        match self {
            Self::Mcs0 => MacHeMcs::Mcs0,
            Self::Mcs1 => MacHeMcs::Mcs1,
            Self::Mcs2 => MacHeMcs::Mcs2,
            Self::Mcs3 => MacHeMcs::Mcs3,
            Self::Mcs4 => MacHeMcs::Mcs4,
            Self::Mcs5 => MacHeMcs::Mcs5,
            Self::Mcs6 => MacHeMcs::Mcs6,
            Self::Mcs7 => MacHeMcs::Mcs7,
            Self::Mcs8 => MacHeMcs::Mcs8,
            Self::Mcs9 => MacHeMcs::Mcs9,
        }
    }
}

/// Forward-error-correction profile carried by the S31 HE-SIG-A2 control.
///
/// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_set_hesig`
/// (size `0x324`) and ROM rev0 `rom[]::mac_tx_set_hesig` at `0x2f8350a8`.
/// The blob's `esp_wifi_cert_tx_bcc` selector produces intermediate halfword
/// `0x017f` for BCC and `0x01ff` for LDPC. Its final `>> 6` transformation
/// changes the queue's HE-SIG-A2 control image from `0x105` to `0x107`.
/// SOURCE\[HIL_OPEN_HE20_LDPC_MATRIX_2026_07_30]: the open formatter's LDPC
/// image completed three 30-profile MCS0..9 by GI/LTF A-MPDU matrices against
/// an LDPC-capable FRITZ peer with no failed profiles or terminal retries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeFecCoding {
    #[default]
    Bcc,
    Ldpc,
}

/// HE DCM MCS values valid in the currently owned BCC SU profile.
///
/// This intentionally does not expose an unchecked integer. The pinned
/// `libpp[trc.o]::rcGetDCMMaxRate` selects exactly the internal
/// rate-control fallback codes `0x10`, `0x11`, and `0x13` for BPSK, QPSK,
/// and 16-QAM DCM. Its RU242 DCM table additionally contains MCS4, but that
/// requires the separately owned LDPC profile and therefore must not be
/// combined with the `HE_SU_A2_CONTROL_BCC` image. Use
/// [`HeRate::ldpc_dcm`] for that coding domain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HeBccDcmMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs3 = 3,
}

impl HeBccDcmMcs {
    pub const fn mcs(self) -> HeMcs {
        match self {
            Self::Mcs0 => HeMcs::Mcs0,
            Self::Mcs1 => HeMcs::Mcs1,
            Self::Mcs3 => HeMcs::Mcs3,
        }
    }
}

/// HE DCM MCS values valid with the separately owned LDPC SU profile.
///
/// SOURCE: ROM rev0 `he_rates_dcm_ru_242` at `0x2f84e07c` contains three
/// GI rows by five MCS columns. Its valid columns are MCS0, MCS1, MCS3 and
/// MCS4; MCS2 is zero. MCS4 is deliberately available only through this LDPC
/// type, so it cannot be combined with [`HeRate::bcc_dcm`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum HeLdpcDcmMcs {
    #[default]
    Mcs0 = 0,
    Mcs1 = 1,
    Mcs3 = 3,
    Mcs4 = 4,
}

impl HeLdpcDcmMcs {
    pub const fn mcs(self) -> HeMcs {
        match self {
            Self::Mcs0 => HeMcs::Mcs0,
            Self::Mcs1 => HeMcs::Mcs1,
            Self::Mcs3 => HeMcs::Mcs3,
            Self::Mcs4 => HeMcs::Mcs4,
        }
    }
}

/// One EDCA TXOP limit in the 32-us units used by the 802.11 WMM parameter.
///
/// A raw value of zero selects the vendor's default HE PPDU-duration policy;
/// a nonzero value bounds the exchange duration and therefore changes the
/// maximum APEP independently for every MCS and GI/LTF combination. Keeping
/// the raw unit in this type avoids accidentally passing microseconds to the
/// recovered table producer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeEdcaTxopLimit {
    units_32_us: u8,
}

impl HeEdcaTxopLimit {
    /// Vendor default used when the advertised EDCA TXOP field is zero.
    pub const DEFAULT: Self = Self { units_32_us: 0 };
    /// Largest value retained by the complete vendor WMM parser.
    pub const MAXIMUM_SUPPORTED: Self = Self {
        units_32_us: u8::MAX,
    };

    /// Admit a standard 16-bit WMM TXOP when the vendor data path can retain it.
    ///
    /// Complete `ieee80211_parse_wmeparams` stores only the low byte in its
    /// seven-byte per-AC record. Returning `None` for a larger standard value
    /// makes that implementation boundary explicit instead of truncating it.
    pub const fn from_units_32_us(units_32_us: u16) -> Option<Self> {
        if units_32_us <= u8::MAX as u16 {
            Some(Self {
                units_32_us: units_32_us as u8,
            })
        } else {
            None
        }
    }

    pub const fn units_32_us(self) -> u8 {
        self.units_32_us
    }

    pub const fn is_default(self) -> bool {
        self.units_32_us == 0
    }
}

/// Typed HE20 SU transmit rate for the single S31 spatial stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeRate {
    mcs: HeMcs,
    guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    fec_coding: HeFecCoding,
    dcm: bool,
}

impl HeRate {
    pub const fn new(mcs: HeMcs, guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf) -> Self {
        Self {
            mcs,
            guard_interval_and_ltf,
            fec_coding: HeFecCoding::Bcc,
            dcm: false,
        }
    }

    /// Construct one HE SU LDPC rate without DCM.
    ///
    /// The coding selector is independently represented even though the
    /// descriptor rate code remains `0x1a + MCS`.
    pub const fn ldpc(
        mcs: HeMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self {
            mcs,
            guard_interval_and_ltf,
            fec_coding: HeFecCoding::Ldpc,
            dcm: false,
        }
    }

    /// Construct one standard-valid HE SU BCC+DCM rate.
    ///
    /// SOURCE: complete `libpp[trc.o]::rcGetDCMMaxRate,
    /// libpp[trc.o]::he_get_min_subframe_len_dcm`, the RU242 DCM rate table, and complete
    /// `libpp[hal_mac_tx.o]::mac_tx_set_hesig`.
    pub const fn bcc_dcm(
        mcs: HeBccDcmMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self {
            mcs: mcs.mcs(),
            guard_interval_and_ltf,
            fec_coding: HeFecCoding::Bcc,
            dcm: true,
        }
    }

    /// Construct one standard-valid HE SU LDPC+DCM rate.
    ///
    /// Unlike the bounded BCC constructor, this admits the ROM-evidenced
    /// MCS4 column. It still excludes the zero MCS2 column and every
    /// unsupported MCS5..9 DCM combination at the type boundary.
    pub const fn ldpc_dcm(
        mcs: HeLdpcDcmMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self {
            mcs: mcs.mcs(),
            guard_interval_and_ltf,
            fec_coding: HeFecCoding::Ldpc,
            dcm: true,
        }
    }

    pub const fn mcs(self) -> HeMcs {
        self.mcs
    }

    pub const fn guard_interval_and_ltf(self) -> crate::rx::HeGuardIntervalAndLtf {
        self.guard_interval_and_ltf
    }

    pub const fn fec_coding(self) -> HeFecCoding {
        self.fec_coding
    }

    pub const fn is_ldpc(self) -> bool {
        matches!(self.fec_coding, HeFecCoding::Ldpc)
    }

    pub const fn is_dcm(self) -> bool {
        self.dcm
    }

    const fn pac_rate(self) -> MacHeRate {
        let guard_interval_and_ltf = match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns => MacHeGuardIntervalAndLtf::OneLtf800Ns,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => MacHeGuardIntervalAndLtf::TwoLtf800Ns,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => {
                MacHeGuardIntervalAndLtf::TwoLtf1600Ns
            }
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => {
                MacHeGuardIntervalAndLtf::FourLtf3200Ns
            }
        };
        MacHeRate {
            mcs: self.mcs.pac_mcs(),
            guard_interval_and_ltf,
            fec_coding: match self.fec_coding {
                HeFecCoding::Bcc => MacHeFecCoding::Bcc,
                HeFecCoding::Ldpc => MacHeFecCoding::Ldpc,
            },
            dcm: self.dcm,
        }
    }

    /// Canonical S31 HE descriptor rate code.
    ///
    /// Explicit DCM and non-DCM HE submissions both use `0x1a + MCS`; DCM is
    /// carried independently in descriptor-state bit 15 and HE-SIG-A1 bit 7.
    /// `HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29` qualified descriptor rate
    /// `0x1a`, PLCP1 `0x0401a000`, and HE-SIG-A1 `0xfc204087`.
    ///
    /// Do not confuse this with [`Self::rate_control_dcm_fallback_code`].
    /// Complete `libpp[trc.o]::rcGetDCMMaxRate` can rewrite the
    /// descriptor to a separate `0x10 + MCS` domain when its internal
    /// rate-control state requests a DCM fallback.
    pub const fn code(self) -> u8 {
        0x1a + self.mcs.index()
    }

    /// Internal vendor rate-control fallback code for an explicit DCM rate.
    ///
    /// SOURCE: complete `libpp[trc.o]::rcGetDCMMaxRate`. That
    /// function first requires descriptor-state word bit 21, selects the
    /// peer-bounded DCM constellation from bits 1:0, then stores exactly
    /// `0x10`, `0x11`, or `0x13` and sets descriptor byte `+0x31` bit 7.
    /// This is exposed for a future owned rate-control port; the direct queue
    /// formatter must continue to use [`Self::code`].
    pub const fn rate_control_dcm_fallback_code(self) -> Option<u8> {
        match (self.dcm, self.mcs) {
            (true, HeMcs::Mcs0 | HeMcs::Mcs1 | HeMcs::Mcs3) => Some(0x10 + self.mcs.index()),
            _ => None,
        }
    }

    /// HE and HT use the same MCS-indexed calibrated power pair.
    ///
    /// Complete `hal_mac_tx_set_ppdu` subtracts ten from HE codes 26..35
    /// before indexing the 43-pair MAC power table.
    pub const fn power_lookup_code(self) -> u8 {
        16 + self.mcs.index()
    }

    pub const fn vendor_control_rate(self) -> LegacyRate {
        match self.mcs {
            HeMcs::Mcs0 => LegacyRate::Ofdm6M,
            HeMcs::Mcs1 | HeMcs::Mcs2 => LegacyRate::Ofdm12M,
            HeMcs::Mcs3
            | HeMcs::Mcs4
            | HeMcs::Mcs5
            | HeMcs::Mcs6
            | HeMcs::Mcs7
            | HeMcs::Mcs8
            | HeMcs::Mcs9 => LegacyRate::Ofdm24M,
        }
    }

    /// Select one exact vendor ordinary-MPDU Dot11Ax retry-ladder attempt.
    ///
    /// `rcGetRate` walks the four `(rate, count)` pairs of the current
    /// schedule record after an ordinary MPDU failure. The aggregate retry
    /// path is deliberately different: complete
    /// `libpp[lmac.o]::lmacRetryTxFrame` branches around
    /// `rcGetRate` when its state byte is four, and
    /// `lmacProcessLongRetryFail` installs that state immediately before an
    /// A-MPDU retry. The HE20 records use one dedicated 800-ns MCS9 entry and
    /// ten 1600-ns entries for MCS9 through MCS0. The ordinary retry code may
    /// eventually leave the HE domain; returning [`TxPhyRate`] keeps that
    /// boundary explicit.
    ///
    /// FEC is not part of the schedule byte. When the selected retry remains
    /// HE, retain the caller's independently negotiated BCC/LDPC choice.
    /// DCM has a separate producer and is rejected here instead of being
    /// silently combined with the ordinary Dot11Ax table.
    ///
    /// SOURCE: complete pinned `libpp[trc.o]::rcGetRate`, size `0xf6`,
    /// and the pinned `rcUpdatePhyMode` mapping represented by the Rust-owned
    /// [`RateScheduleKind::Dot11Ax`] arena.
    pub fn vendor_retry_rate(self, failed_attempts: u8) -> Option<TxPhyRate> {
        if self.dcm {
            return None;
        }
        let oer_ieee80211_mac::phy::PhyRate::He(rate) = TxPhyRate::He(self).phy_rate() else {
            unreachable!("an S31 HE rate is a portable HE rate");
        };
        TxPhyRate::try_from(retry_ladder::he_retry_rate(rate, failed_attempts)?).ok()
    }

    pub const fn nominal_kbps(self) -> u32 {
        self.nominal_kbps_for_resource_unit(HeResourceUnit::Ru242)
    }

    /// Return the exact blob/ROM nominal rate for one HE resource unit.
    ///
    /// SOURCE: complete `libpp.a[trc.o]` objects
    /// `he_rates_ru_{26,52,106,242}` and
    /// `he_rates_dcm_ru_{26,52,106,242}`, independently present in the rev0
    /// ROM ELF at `0x2f07f5cc/0x2f07f644/0x2f07f4dc/0x2f07f554` and
    /// `0x2f84e0b8/0x2f84e0f4/0x2f84e040/0x2f84e07c`, respectively.
    /// Keeping the RU width typed prevents a Trigger scheduler from using
    /// the 114.7-Mbit/s RU242 MCS9 rate for an 11.8-Mbit/s RU26 allocation.
    ///
    /// Tables are stored in 100-kbit/s units by the oracle and converted to
    /// kbit/s here. DCM constructors exclude every unsupported MCS, including
    /// the anomalous nonzero RU106/MCS2 table cell that no typed rate can
    /// select.
    pub const fn nominal_kbps_for_resource_unit(self, resource_unit: HeResourceUnit) -> u32 {
        const RU26: [[u16; 10]; 3] = [
            [9, 18, 26, 35, 53, 71, 79, 88, 106, 118],
            [8, 17, 25, 33, 50, 67, 75, 83, 100, 111],
            [8, 15, 23, 30, 45, 60, 68, 75, 90, 100],
        ];
        const RU52: [[u16; 10]; 3] = [
            [18, 35, 53, 71, 106, 141, 159, 176, 212, 235],
            [17, 33, 50, 67, 100, 133, 150, 167, 200, 222],
            [15, 30, 45, 60, 90, 120, 135, 150, 180, 200],
        ];
        const RU106: [[u16; 10]; 3] = [
            [38, 75, 113, 150, 225, 300, 338, 375, 450, 500],
            [35, 71, 106, 142, 213, 283, 319, 354, 425, 472],
            [32, 64, 96, 128, 191, 255, 287, 319, 383, 425],
        ];
        const RU242: [[u16; 10]; 3] = [
            [86, 172, 258, 344, 516, 688, 774, 860, 1_032, 1_147],
            [81, 163, 244, 325, 488, 650, 731, 813, 975, 1_083],
            [73, 146, 219, 293, 439, 585, 658, 731, 878, 975],
        ];
        const DCM_RU26: [[u16; 5]; 3] = [[4, 9, 0, 18, 26], [4, 8, 0, 17, 25], [4, 8, 0, 15, 23]];
        const DCM_RU52: [[u16; 5]; 3] =
            [[9, 18, 0, 35, 53], [8, 17, 0, 33, 50], [8, 15, 0, 30, 45]];
        // The complete blob and ROM both contain 113 in the RU106/MCS2
        // cells. MCS2 is not exposed by either typed DCM constructor.
        const DCM_RU106: [[u16; 5]; 3] = [
            [18, 38, 113, 75, 113],
            [17, 35, 106, 71, 106],
            [16, 32, 96, 64, 96],
        ];
        const DCM_RU242: [[u16; 5]; 3] = [
            [43, 86, 0, 172, 258],
            [40, 81, 0, 163, 244],
            [36, 73, 0, 146, 219],
        ];

        let gi = match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => 0,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => 1,
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => 2,
        };
        let mcs = self.mcs.index() as usize;
        let units_100_kbps = if self.dcm {
            match resource_unit {
                HeResourceUnit::Ru26 => DCM_RU26[gi][mcs],
                HeResourceUnit::Ru52 => DCM_RU52[gi][mcs],
                HeResourceUnit::Ru106 => DCM_RU106[gi][mcs],
                HeResourceUnit::Ru242 => DCM_RU242[gi][mcs],
            }
        } else {
            match resource_unit {
                HeResourceUnit::Ru26 => RU26[gi][mcs],
                HeResourceUnit::Ru52 => RU52[gi][mcs],
                HeResourceUnit::Ru106 => RU106[gi][mcs],
                HeResourceUnit::Ru242 => RU242[gi][mcs],
            }
        };
        (units_100_kbps as u32) * 100
    }

    /// Maximum APEP bytes for the vendor's zero-TXOP HE SU policy.
    ///
    /// SOURCE: ROM rev0 `he_max_apep_length` at `0x2f84fd40`, complete
    /// `libpp[trc.o]::rx11AXRate2AMPDULimit`, and complete
    /// `libpp[pp_he.o]::ppCheckTxHEAMPDUlength`.
    ///
    /// The ROM object contains three GI rows by ten MCS columns. The selector
    /// maps both 0.8-us GI encodings to row zero. `libpp[pp_he.o]::ppCheckTxHEAMPDUlength`
    /// consumes the selected value as a `u16` and halves it when descriptor
    /// state bit 15 selects DCM. This limit is independent of the peer's
    /// advertised maximum A-MPDU exponent; an owner must satisfy both.
    ///
    /// A nonzero EDCA TXOP limit uses the separately generated
    /// `rx11AXRate2AMPDULimit_update` table and is deliberately not presented
    /// as this zero-TXOP result.
    pub const fn maximum_default_apep_bytes(self) -> u16 {
        const GI_800: [u16; 10] = [
            3_700, 7_400, 11_100, 14_800, 22_500, 30_000, 33_600, 37_600, 45_000, 50_000,
        ];
        const GI_1600: [u16; 10] = [
            3_500, 7_000, 10_500, 14_000, 21_000, 28_200, 31_500, 35_200, 42_300, 47_000,
        ];
        const GI_3200: [u16; 10] = [
            3_200, 6_400, 9_600, 12_800, 19_000, 25_200, 28_700, 32_000, 37_800, 42_000,
        ];
        let row = match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => &GI_800,
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => &GI_1600,
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => &GI_3200,
        };
        let limit = row[self.mcs.index() as usize];
        if self.dcm { limit / 2 } else { limit }
    }

    /// Checked maximum APEP bytes for this rate and one EDCA TXOP limit.
    ///
    /// SOURCE: complete `libpp[trc.o]::rx11AXRate2AMPDULimit_update` (size `0x136`), complete
    /// `libpp[pp_he.o]::get_estimated_batime` (size `0x1a`), and
    /// `libpp.a[hal_mac_ctl.o]::{he_preamble_ersu,
    /// he_time_per_sym,he_data_bits_per_sym}`.
    ///
    /// The blob generates one three-GI-by-ten-MCS `u32` table for each of the
    /// four EDCA access categories. For a nonzero TXOP it subtracts 36 us,
    /// the rate-dependent estimated BlockAck time, and the GI-dependent
    /// preamble, converts the remaining time to symbols, performs one fused
    /// `NDBPS * symbols - 22` operation, truncates toward zero, and divides by
    /// eight. The `-22` term is the service/tail-bit allowance. The exact
    /// recovered constants are:
    ///
    /// - BlockAck: 68 us for MCS0, 44 us for MCS1/2, 32 us for MCS3..9;
    /// - preamble: 31.2, 32, or 40 us;
    /// - symbol: 13.6, 14.4, or 16 us;
    /// - RU242 NDBPS: 117, 234, 351, 468, 702, 936, 1053, 1170, 1404, 1560.
    ///
    /// A zero limit retains the ROM table exposed by
    /// [`Self::maximum_default_apep_bytes`]. A nonzero limit whose complete
    /// duration calculation leaves no positive payload budget returns
    /// `None`. The blob converts that negative signed result to an unsigned
    /// table entry, but an untrusted peer can advertise such a short TXOP;
    /// treating the wrap as a large APEP would fail open.
    pub const fn checked_maximum_apep_bytes(self, txop: HeEdcaTxopLimit) -> Option<u32> {
        let Some(units) = core::num::NonZeroU8::new(txop.units_32_us()) else {
            return Some(self.maximum_default_apep_bytes() as u32);
        };

        let signed_bytes = self.nonzero_txop_apep_bytes(units);
        if signed_bytes <= 0 {
            return None;
        }
        let limit = self.vendor_unchecked_maximum_apep_bytes(txop);
        if limit == 0 { None } else { Some(limit) }
    }

    /// Fail-closed maximum used by compatibility callers which do not need
    /// to distinguish a non-positive duration budget from a zero-byte limit.
    pub const fn maximum_apep_bytes(self, txop: HeEdcaTxopLimit) -> u32 {
        match self.checked_maximum_apep_bytes(txop) {
            Some(limit) => limit,
            None => 0,
        }
    }

    /// Preserve the complete blob conversion as a private comparison oracle.
    /// Production admission calls [`Self::checked_maximum_apep_bytes`] first,
    /// so a negative signed result can never become aggregate capacity.
    /// The exact rational producer below is exhaustively compared with the
    /// blob's f32 instruction sequence across every peer-admitted TXOP byte,
    /// all three GI/LTF rows and all ten MCS values.
    const fn vendor_unchecked_maximum_apep_bytes(self, txop: HeEdcaTxopLimit) -> u32 {
        let Some(units) = core::num::NonZeroU8::new(txop.units_32_us()) else {
            return self.maximum_default_apep_bytes() as u32;
        };
        let limit = self.nonzero_txop_apep_bytes(units) as u32;
        if self.dcm { limit / 2 } else { limit }
    }

    /// A non-default duration is established by the caller's NonZeroU8;
    /// the default PPDU policy cannot enter this exchange-budget formula.
    const fn nonzero_txop_apep_bytes(self, txop: core::num::NonZeroU8) -> i64 {
        const DATA_BITS_PER_SYMBOL_RU242: [i32; 10] =
            [117, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
        let mcs = self.mcs.index() as usize;
        let estimated_block_ack_us = match self.mcs {
            HeMcs::Mcs0 => 68,
            HeMcs::Mcs1 | HeMcs::Mcs2 => 44,
            HeMcs::Mcs3
            | HeMcs::Mcs4
            | HeMcs::Mcs5
            | HeMcs::Mcs6
            | HeMcs::Mcs7
            | HeMcs::Mcs8
            | HeMcs::Mcs9 => 32,
        };
        let exchange_budget_us = txop.get() as i64 * 32 - 36;
        let data_bits_per_symbol = DATA_BITS_PER_SYMBOL_RU242[mcs] as i64;
        match self.guard_interval_and_ltf {
            crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
            | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => {
                // ((budget - BA - 31.2) / 13.6 * NDBPS - 22) / 8
                ((5 * (exchange_budget_us - estimated_block_ack_us) - 156) * data_bits_per_symbol
                    - 22 * 68)
                    / (68 * 8)
            }
            crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => {
                // ((budget - BA - 32) / 14.4 * NDBPS - 22) / 8
                (5 * (exchange_budget_us - estimated_block_ack_us - 32) * data_bits_per_symbol
                    - 22 * 72)
                    / (72 * 8)
            }
            crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => {
                // ((budget - BA - 40) / 16 * NDBPS - 22) / 8
                ((exchange_budget_us - estimated_block_ack_us - 40) * data_bits_per_symbol
                    - 22 * 16)
                    / (16 * 8)
            }
        }
    }

    /// Minimum HE A-MPDU subframe length for a negotiated density.
    ///
    /// This reproduces complete `libpp[trc.o]::he_get_min_subframe_len,libpp[trc.o]::he_get_min_subframe_len_dcm`: select the
    /// ordinary or DCM RU242 rate in 100-kbit/s units, multiply by the blob's
    /// integer-microsecond density, truncate by eight, then divide by ten and
    /// round up when a remainder remains.
    ///
    /// The value is the required complete A-MPDU subframe length in bytes.
    /// The caller still owns the distinction between MPDU bytes, the
    /// four-byte delimiter, four-byte alignment, and any extra empty
    /// delimiters.
    pub const fn minimum_ampdu_subframe_bytes(self, density: HtAmpduDensity) -> u16 {
        let density_us = density.vendor_integer_microseconds();
        let rate_100_kbps = self.nominal_kbps() / 100;
        let truncated_byte_rate = rate_100_kbps * density_us as u32 / 8;
        let bytes = truncated_byte_rate / 10 + (!truncated_byte_rate.is_multiple_of(10)) as u32;
        bytes as u16
    }

    /// Empty four-byte delimiters required after one HE A-MPDU PSDU.
    ///
    /// `psdu_length` includes the hardware MIC and FCS. Complete
    /// `libpp[pp_he.o]::ppCheckTxHEAMPDUlength` passes
    /// `round_up(psdu_length + 4, 4)` to `ppCalDeliNum`; with the owned
    /// metadata profile this is `round_up(psdu_length, 4) + 4`.
    /// `ppCalDeliNum` writes `ceil((minimum-current)/4)` to metadata byte
    /// four when the subframe is short and zero otherwise.
    pub const fn ampdu_empty_delimiters(
        self,
        psdu_length: u16,
        density: HtAmpduDensity,
    ) -> Option<u8> {
        if psdu_length == 0 || psdu_length > 0x3fff {
            return None;
        }
        let minimum = self.minimum_ampdu_subframe_bytes(density) as u32;
        let Some(delimiters) =
            oer_ieee80211_mac::ampdu::empty_delimiters(psdu_length as u32, minimum)
        else {
            return None;
        };
        if delimiters > u8::MAX as u32 {
            None
        } else {
            Some(delimiters as u8)
        }
    }
}

/// A standard-valid HE SU DCM rate suitable for a capability-gated override.
///
/// `HeRate` also represents ordinary non-DCM HE rates. Keeping the DCM
/// subset in this newtype prevents a rate-policy caller from publishing a
/// nominally "DCM" override whose HE-SIG-A DCM bit is actually clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeDcmRate(HeRate);

impl HeDcmRate {
    pub const fn bcc(
        mcs: HeBccDcmMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self(HeRate::bcc_dcm(mcs, guard_interval_and_ltf))
    }

    pub const fn ldpc(
        mcs: HeLdpcDcmMcs,
        guard_interval_and_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Self {
        Self(HeRate::ldpc_dcm(mcs, guard_interval_and_ltf))
    }

    pub const fn rate(self) -> HeRate {
        self.0
    }

    /// Smallest DCM constellation the receiving peer must advertise.
    ///
    /// SOURCE: complete `libpp[trc.o]::rcGetDCMMaxRate` maps peer
    /// capability levels BPSK/QPSK/16-QAM to MCS0/MCS1/MCS3. ROM
    /// `he_rates_dcm_ru_242` adds MCS4 only in the separately typed LDPC
    /// column, which still uses 16-QAM.
    pub const fn required_peer_constellation(self) -> HeDcmConstellation {
        match self.0.mcs() {
            HeMcs::Mcs0 => HeDcmConstellation::Bpsk,
            HeMcs::Mcs1 => HeDcmConstellation::Qpsk,
            HeMcs::Mcs3 | HeMcs::Mcs4 => HeDcmConstellation::Qam16,
            // The private field and typed constructors make these variants
            // unreachable. Fail closed nonetheless if that invariant changes:
            // no peer advertises support for an invalid DCM MCS.
            HeMcs::Mcs2 | HeMcs::Mcs5 | HeMcs::Mcs6 | HeMcs::Mcs7 | HeMcs::Mcs8 | HeMcs::Mcs9 => {
                HeDcmConstellation::NotSupported
            }
        }
    }

    pub const fn is_supported_by(
        self,
        peer_receive: HeDcmConstellation,
        peer_supports_ldpc: bool,
    ) -> bool {
        let constellation_supported = match self.required_peer_constellation() {
            HeDcmConstellation::NotSupported => false,
            HeDcmConstellation::Bpsk => peer_receive.supports_bpsk(),
            HeDcmConstellation::Qpsk => peer_receive.supports_qpsk(),
            HeDcmConstellation::Qam16 => peer_receive.supports_16qam(),
        };
        constellation_supported && (!self.0.is_ldpc() || peer_supports_ldpc)
    }
}

/// One scheduled 1T1R HE-TB rate recovered from Trigger Common/User Info.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeTriggerScheduledRate {
    pub rate: HeRate,
    /// Exact Trigger Common Info selector retained independently from the
    /// HE-SU formatter's different GI/LTF encoding.
    pub trigger_gi_ltf: TriggerGiLtf,
    pub resource_unit: HeResourceUnit,
    pub resource_unit_index: u8,
    pub resource_unit_region: bool,
    pub partial_ru_power_selector: MacPartialRuPowerSelector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeTriggerScheduledRateError {
    UnsupportedTriggerType,
    UnsupportedBandwidth,
    AssociationIdMismatch,
    AssociationIdNotScheduled,
    DuplicateAssociationId,
    MalformedUserInfo(TriggerParseError),
    UnsupportedSpatialStreams,
    UnsupportedResourceUnit,
    UnsupportedMcs,
    UnsupportedGiLtf,
    UnsupportedDcmCombination,
}

impl HeTriggerScheduledRate {
    /// Select this station from a complete, possibly multi-user Trigger.
    ///
    /// [`TriggerFrame::users`] supplies the type-dependent, allocation-free
    /// User Info iteration and its joint AID12/RU padding sentinel;
    /// [`Self::new`] then derives the scheduled user's RU and rate.
    ///
    /// The entire iterator is consumed before returning. A malformed trailing
    /// user or a duplicate assignment therefore cannot be hidden by placing a
    /// superficially valid assignment for this station first.
    pub fn from_trigger_frame(
        frame: &TriggerFrame<'_>,
        association_id: u16,
    ) -> Result<Self, HeTriggerScheduledRateError> {
        if !matches!(frame.common.trigger_type, TriggerType::Basic) {
            return Err(HeTriggerScheduledRateError::UnsupportedTriggerType);
        }
        if frame.common.uplink_bandwidth_encoding != 0 {
            return Err(HeTriggerScheduledRateError::UnsupportedBandwidth);
        }
        if association_id == 0 || association_id > 0x0fff {
            return Err(HeTriggerScheduledRateError::AssociationIdMismatch);
        }

        let mut assigned_user = None;
        for field in frame.users() {
            let field = field.map_err(HeTriggerScheduledRateError::MalformedUserInfo)?;
            if field.aid12() != association_id {
                continue;
            }
            if assigned_user.is_some() {
                return Err(HeTriggerScheduledRateError::DuplicateAssociationId);
            }
            assigned_user = Some(
                parse_trigger_user_spatial_stream(field.user_info)
                    .map_err(HeTriggerScheduledRateError::MalformedUserInfo)?,
            );
        }
        let user = assigned_user.ok_or(HeTriggerScheduledRateError::AssociationIdNotScheduled)?;
        Self::new(frame.common, user, association_id)
    }

    /// Admit the Trigger user assigned to this 1T1R HE20 station.
    ///
    /// SOURCE: complete pinned `libpp[hal_debug.o]::dbg_dump_trig_common_info` and `dbg_dump_trig_user_ss`, and complete
    /// `libpp[hal_utilities.o]::ru2str`, which decode the UL bandwidth,
    /// GI/LTF, RU allocation, UL HE-MCS, UL FEC, DCM and spatial-stream
    /// fields. IEEE Std 802.11ax-2021 9.3.1.22 and 27.3.2 define their
    /// meaning: the RU class follows from the raw allocation, and the rate
    /// from the scheduled user's MCS, coding, GI and spatial-stream
    /// allocation.
    ///
    /// Wider bandwidths, non-NSS1 assignments and unsupported DCM/MCS
    /// combinations fail before they can become a transmit rate. AID zero
    /// random-access users use a different policy and are intentionally not
    /// accepted by this scheduled-user constructor.
    pub const fn new(
        common: TriggerCommonInfo,
        user: TriggerUserSpatialStreamInfo,
        association_id: u16,
    ) -> Result<Self, HeTriggerScheduledRateError> {
        if !matches!(common.trigger_type, TriggerType::Basic) {
            return Err(HeTriggerScheduledRateError::UnsupportedTriggerType);
        }
        if common.uplink_bandwidth_encoding != 0 {
            return Err(HeTriggerScheduledRateError::UnsupportedBandwidth);
        }
        if user.aid12 != association_id || association_id == 0 || association_id > 0x0fff {
            return Err(HeTriggerScheduledRateError::AssociationIdMismatch);
        }
        if user.starting_spatial_stream != 1 || user.spatial_stream_count != 1 {
            return Err(HeTriggerScheduledRateError::UnsupportedSpatialStreams);
        }
        let Some(allocation) = TriggerRuAllocation::from_encoding(user.ru_allocation) else {
            return Err(HeTriggerScheduledRateError::UnsupportedResourceUnit);
        };
        let TriggerRuAllocation::Narrow {
            resource_unit,
            one_based_index,
        } = allocation
        else {
            return Err(HeTriggerScheduledRateError::UnsupportedResourceUnit);
        };
        // Complete hal_mac_{get,set}_tb_max_pwr provides the hardware's
        // narrower HE20 admission oracle. In particular ru2str can diagnose
        // raw allocation 62 as a second RU242, but the runtime power jump
        // tables reject it; a 20-MHz 1T1R station must not schedule it.
        let Some(partial_ru_power_selector) =
            MacPartialRuPowerSelector::from_trigger_encoding(user.ru_allocation)
        else {
            return Err(HeTriggerScheduledRateError::UnsupportedResourceUnit);
        };
        let Some(mcs) = HeMcs::from_index(user.mcs) else {
            return Err(HeTriggerScheduledRateError::UnsupportedMcs);
        };
        let gi_ltf = match common.gi_ltf {
            // The HE-SU rate type does not encode the Trigger-only 1x-LTF
            // distinction. Both selectors share the same 1.6-us data-symbol
            // rate; `trigger_gi_ltf` below retains the exact LTF count for
            // eventual HE-TB vector programming.
            TriggerGiLtf::OneLtf1600Ns | TriggerGiLtf::TwoLtf1600Ns => {
                crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns
            }
            TriggerGiLtf::FourLtf3200Ns => crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns,
            TriggerGiLtf::Reserved => {
                return Err(HeTriggerScheduledRateError::UnsupportedGiLtf);
            }
        };
        let rate = match (user.dcm, user.coding_type, mcs) {
            (false, false, _) => HeRate::new(mcs, gi_ltf),
            (false, true, _) => HeRate::ldpc(mcs, gi_ltf),
            (true, false, HeMcs::Mcs0) => HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, gi_ltf),
            (true, false, HeMcs::Mcs1) => HeRate::bcc_dcm(HeBccDcmMcs::Mcs1, gi_ltf),
            (true, false, HeMcs::Mcs3) => HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, gi_ltf),
            (true, true, HeMcs::Mcs0) => HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs0, gi_ltf),
            (true, true, HeMcs::Mcs1) => HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs1, gi_ltf),
            (true, true, HeMcs::Mcs3) => HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs3, gi_ltf),
            (true, true, HeMcs::Mcs4) => HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, gi_ltf),
            (true, _, _) => {
                return Err(HeTriggerScheduledRateError::UnsupportedDcmCombination);
            }
        };

        Ok(Self {
            rate,
            trigger_gi_ltf: common.gi_ltf,
            resource_unit,
            resource_unit_index: one_based_index,
            resource_unit_region: user.ru_allocation_region,
            partial_ru_power_selector,
        })
    }

    pub const fn nominal_kbps(self) -> u32 {
        self.rate.nominal_kbps_for_resource_unit(self.resource_unit)
    }
}

/// One finite legacy, HT, or HE SU rate used by the open transmit path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxPhyRate {
    Legacy(LegacyRate),
    Ht(HtRate),
    He(HeRate),
}

impl TxPhyRate {
    /// Control rate published in LENGTH_CONTROL for an unprotected PPDU.
    ///
    /// SOURCE: complete `libpp[hal_mac_tx.o]::mac_tx_get_rts_rate`
    /// (size `0x96`), whose result complete `libpp[hal_mac_tx.o]::mac_tx_set_len` writes for every
    /// PPDU.
    pub const fn vendor_control_rate(self) -> LegacyRate {
        match self {
            Self::Legacy(rate) => rate.vendor_control_rate(),
            Self::Ht(rate) => rate.vendor_control_rate(),
            Self::He(rate) => rate.vendor_control_rate(),
        }
    }

    pub const fn from_code(code: u8, ht_width: HtChannelWidth) -> Option<Self> {
        if let Some(rate) = LegacyRate::from_code(code) {
            return Some(Self::Legacy(rate));
        }
        let (mcs, guard_interval) = if code >= 16 && code <= 23 {
            (code - 16, HtGuardInterval::Long800Ns)
        } else if code >= 26 && code <= 33 {
            (code - 26, HtGuardInterval::Short400Ns)
        } else {
            return None;
        };
        let Some(mcs) = HtMcs::from_index(mcs) else {
            return None;
        };
        Some(Self::Ht(HtRate::new(mcs, guard_interval, ht_width)))
    }

    /// Decode one rate byte in the context of its owning rate-control arena.
    ///
    /// The numeric `wifi_phy_rate_t` MCS domains overlap: `0x10..=0x19`
    /// means HT long GI in a Dot11N arena, but HE MCS0..9 with 1600-ns GI in
    /// a Dot11Ax arena. Likewise `0x1a..=0x23` means HT short GI or HE
    /// MCS0..9 with 800-ns GI. Requiring the arena identity prevents a caller
    /// from publishing a semantically HE byte through the HT formatter.
    ///
    /// `he_800ns_gi_ltf` supplies the peer-qualified LTF count for the HE
    /// 800-ns domain. The 1600-ns domain has exactly two LTFs. LDPC and DCM
    /// remain independent rate-control state and are intentionally not
    /// inferred from the shared rate byte.
    ///
    /// SOURCE: sibling S31 `esp_wifi_types_generic.h::wifi_phy_rate_t`, whose
    /// rate table explicitly assigns HE20 1600 ns to `MCS*_LGI` and HE20
    /// 800 ns to `MCS*_SGI`; complete
    /// `libpp[trc.o]::rcGetRate,libpp[trc.o]::rcGetSMPDURate`; and the exact
    /// Rust-owned Dot11Ax/Dot11N schedule arenas.
    pub const fn from_rate_control_code(
        kind: RateScheduleKind,
        code: u8,
        ht_width: HtChannelWidth,
        he_800ns_gi_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Option<Self> {
        if let Some(rate) = LegacyRate::from_code(code) {
            return Some(Self::Legacy(rate));
        }
        if matches!(kind, RateScheduleKind::Dot11Ax) {
            let (index, gi_ltf) = if code >= 0x10 && code <= 0x19 {
                (code - 0x10, crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns)
            } else if code >= 0x1a && code <= 0x23 {
                if !matches!(
                    he_800ns_gi_ltf,
                    crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
                        | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns
                ) {
                    return None;
                }
                (code - 0x1a, he_800ns_gi_ltf)
            } else {
                return None;
            };
            let Some(mcs) = HeMcs::from_index(index) else {
                return None;
            };
            return Some(Self::He(HeRate::new(mcs, gi_ltf)));
        }
        Self::from_code(code, ht_width)
    }

    /// The rate-control byte of this rate: the inverse of
    /// [`Self::from_rate_control_code`]. Legacy and HT bytes are their queue
    /// codes; an HE byte carries the guard interval, `0x10 + MCS` for
    /// 1600 ns and `0x1a + MCS` for 800 ns, and 3200 ns has none.
    pub const fn rate_control_code(self) -> Option<u8> {
        match self {
            Self::Legacy(rate) => Some(rate.code()),
            Self::Ht(rate) => Some(rate.code()),
            Self::He(rate) => match rate.guard_interval_and_ltf() {
                crate::rx::HeGuardIntervalAndLtf::TwoLtf1600Ns => Some(0x10 + rate.mcs().index()),
                crate::rx::HeGuardIntervalAndLtf::OneLtf800Ns
                | crate::rx::HeGuardIntervalAndLtf::TwoLtf800Ns => Some(0x1a + rate.mcs().index()),
                crate::rx::HeGuardIntervalAndLtf::FourLtf3200Ns => None,
            },
        }
    }

    /// Decode the primary rate of one complete Rust-owned schedule record.
    pub fn from_rate_control_schedule(
        schedule: RateScheduleRef,
        ht_width: HtChannelWidth,
        he_800ns_gi_ltf: crate::rx::HeGuardIntervalAndLtf,
    ) -> Option<Self> {
        let code = crate::rate::schedule::schedule_state(schedule).rate;
        Self::from_rate_control_code(schedule.kind(), code, ht_width, he_800ns_gi_ltf)
    }

    pub const fn code(self) -> u8 {
        match self {
            Self::Legacy(rate) => rate.code(),
            Self::Ht(rate) => rate.code(),
            Self::He(rate) => rate.code(),
        }
    }

    pub const fn nominal_kbps(self) -> u32 {
        match self {
            Self::Legacy(rate) => rate.nominal_kbps(),
            Self::Ht(rate) => rate.nominal_kbps(),
            Self::He(rate) => rate.nominal_kbps(),
        }
    }
}

/// Control frame published with one PPDU.
///
/// `protection` comes from [`protection::WifiTxProtectionPolicy::select`].
/// The power pair is the calibrated entry for [`Self::rate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxControlFrame {
    pub protection: protection::TxProtection,
    pub power_primary: u8,
    pub power_alternate: u8,
}

impl TxControlFrame {
    pub const UNPROTECTED: Self = Self {
        protection: protection::TxProtection::None,
        power_primary: 0,
        power_alternate: 0,
    };

    /// Rate of the control frame, or the vendor LENGTH_CONTROL image for an
    /// unprotected PPDU at `data`.
    pub const fn rate(self, data: TxPhyRate) -> LegacyRate {
        match self.protection.control_rate() {
            Some(rate) => rate,
            None => data.vendor_control_rate(),
        }
    }

    const fn pac(self, data: TxPhyRate) -> MacTxControlFrame {
        MacTxControlFrame {
            protection: match self.protection {
                protection::TxProtection::None => MacTxProtection::None,
                protection::TxProtection::CtsToSelf { .. } => MacTxProtection::CtsToSelf,
                protection::TxProtection::RtsCts { .. } => MacTxProtection::RtsCts,
            },
            rate: self.rate(data).pac_rate(),
            power_primary: self.power_primary,
            power_alternate: self.power_alternate,
        }
    }
}

/// Inputs for one finite non-HE q0 attempt.
///
/// For the direct raw q0 management path, `signal` is the transmitted
/// `MPDU + FCS` byte length written to `TX_Q_PLCP1`; it is not a vendor
/// descriptor snapshot. Power values are indices in the PHY gain table, not
/// dBm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyTxConfig {
    /// Control frame published with this PPDU.
    pub control: TxControlFrame,
    pub rate: LegacyRate,
    pub signal: u16,
    pub data_power: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: MacInterface,
    /// Priority written to the ordinary queue scheduler field.
    ///
    /// This can differ from [`Self::pti`]: the pinned vendor
    /// `mac_tx_set_pti` raises it to at least coexistence event one's PTI
    /// while retaining the original packet PTI in the four vector lanes.
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    /// Response the MAC awaits: none for a group address, an ACK for an
    /// individual frame, a BlockAck for a BlockAckReq.
    ///
    /// SOURCE: `libpp[pp.o]::ppTxProtoProc` copies the low bit of
    /// address one into descriptor flag `0x0000_0002`.
    /// `libpp[hal_mac_tx.o]::mac_tx_set_plcp0` then preserves PLCP0
    /// format zero when that flag is set. For the bounded plain legacy profile
    /// represented here, an ordinary individual address follows the recovered
    /// zero-flags branch and selects format one. This distinction is qualified
    /// by the vendor authentication capture and by repeated open STA
    /// authentication/association/WPA2/DHCP runs. A BlockAckReq selects
    /// format two ([`MacLegacyTxResponse::BlockAck`]).
    pub response: MacLegacyTxResponse,
    /// Six-bit key-entry index. Zero is plaintext; protected traffic uses its
    /// owned hardware key slot. The formatter combines this with
    /// [`Self::interface`] in the recovered PLCP1 descriptor-control byte.
    pub hardware_key_selector: u8,
}

/// Inputs for one finite, non-aggregate HT MPDU attempt.
///
/// `length` is the complete on-air PSDU byte count produced by hardware:
/// encoded MPDU plus any hardware-generated security MIC and the four-byte
/// FCS. Power bytes are calibrated PHY gain-table indices, not dBm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtTxConfig {
    /// Control frame published with this PPDU.
    pub control: TxControlFrame,
    pub rate: HtRate,
    pub protection_spacing: HtProtectionSpacing,
    pub length: u16,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: MacInterface,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HtTxConfig {
    /// Construct a single-MPDU HT profile from the encoded MPDU length.
    pub const fn single_mpdu(
        rate: HtRate,
        mpdu_length: u16,
        hardware_mic_length: u8,
    ) -> Option<Self> {
        let Some(length) = mpdu_length.checked_add(hardware_mic_length as u16) else {
            return None;
        };
        let Some(length) = length.checked_add(LEGACY_FCS_LENGTH) else {
            return None;
        };
        if length == 0 {
            return None;
        }
        Some(Self {
            control: TxControlFrame::UNPROTECTED,
            rate,
            protection_spacing: HtProtectionSpacing::Density0To4,
            length,
            data_power_primary: 0,
            data_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: TxLifetimeClass::DirectMpdu.fresh_queue_timeout(),
            interface: MacInterface::Station,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    const fn pac_parameters(self) -> MacHtTxParameters {
        MacHtTxParameters {
            control: self.control.pac(TxPhyRate::Ht(self.rate)),
            rate: self.rate.pac_rate(),
            format: MacHtTxFormat::SingleMpdu,
            length: self.length,
            descriptor_count: 1,
            data_power_primary: self.data_power_primary,
            data_power_alternate: self.data_power_alternate,
            protection_spacing: self.protection_spacing.pac_spacing(),
            timeout: self.timeout,
            scheduler_priority: self.scheduler_priority,
            packet_priority: self.pti,
            priority_count: self.pti_count,
            aifsn: self.aifsn,
            contention_window: self.contention_window,
            interface: self.interface,
            hardware_key_selector: self.hardware_key_selector,
            txop: false,
        }
    }
}

/// Inputs for one HE20 SU S-MPDU PPDU.
///
/// An HE single MPDU is not the direct non-aggregate HT layout. IEEE 802.11ax
/// carries it in an S-MPDU container, and the pinned PP implementation adds a
/// delimiter around an MPDU length which already includes the FCS before
/// publishing APEP length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeSmpduTxConfig {
    /// Control frame published with this PPDU.
    pub control: TxControlFrame,
    pub rate: HeRate,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    /// Encoded 802.11 MPDU bytes before the hardware-generated FCS.
    pub mpdu_length: u16,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: MacInterface,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
    pub protection_spacing: u16,
}

impl HeSmpduTxConfig {
    /// Construct the exact bounded vendor S-MPDU profile.
    ///
    /// SOURCE: complete `libpp[pp_he.o]::ppCalTxHESMPDULength` (`0x58` bytes) first rounds the descriptor
    /// payload length to four, calls `libpp[pp_he.o]::ppCalDeliNum` and
    /// `libpp[pp_he.o]::ppCalSubFrameLength`, then sets descriptor byte `+0x32` bit two,
    /// frame byte `+0x26` to one and descriptor byte `+0x2a` to one.
    /// Complete `libpp[pp_he.o]::ppCalSubFrameLength` (`0x3e` bytes) adds the mandatory
    /// four-byte delimiter. The live vendor raw oracle independently captured
    /// metadata word `0x0100_001c`: payload length 28 is the 24-byte encoded
    /// MPDU plus hardware FCS, metadata bit 24 is set, and metadata byte seven
    /// (the optional extra four-byte term) is zero. Therefore APEP is
    /// `round_up(mpdu + FCS, 4) + delimiter`, equivalent to
    /// `round_up(mpdu, 4) + 8`.
    pub const fn new(rate: HeRate, bss_color: u8, mpdu_length: u16) -> Option<Self> {
        if bss_color > 0x3f || mpdu_length == 0 || mpdu_length > 0x3ff7 {
            return None;
        }
        Some(Self {
            control: TxControlFrame::UNPROTECTED,
            rate,
            bss_color,
            spatial_reuse: 0,
            mpdu_length,
            data_power_primary: 0,
            data_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: TxLifetimeClass::AmpduContainer.fresh_queue_timeout(),
            interface: MacInterface::Station,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
            protection_spacing: 0x31,
        })
    }

    /// Complete HE S-MPDU APEP length written to HE-SIG-A2.
    pub const fn apep_length(self) -> u16 {
        ((self.mpdu_length + 3) & !3) + 8
    }

    pub(crate) const fn valid(self) -> bool {
        self.bss_color <= 0x3f
            && self.spatial_reuse <= 0x0f
            && self.mpdu_length != 0
            && self.mpdu_length <= 0x3ff7
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.hardware_key_selector <= 0x3f
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
            && self.protection_spacing <= 0x03ff
    }

    const fn pac_parameters(self) -> MacHeTxParameters {
        MacHeTxParameters {
            control: self.control.pac(TxPhyRate::He(self.rate)),
            rate: self.rate.pac_rate(),
            format: MacHeTxFormat::Smpdu,
            apep_length: self.apep_length(),
            descriptor_count: 1,
            bss_color: self.bss_color,
            spatial_reuse: self.spatial_reuse,
            software_he_control: None,
            data_power_primary: self.data_power_primary,
            data_power_alternate: self.data_power_alternate,
            protection_spacing: self.protection_spacing,
            timeout: self.timeout,
            scheduler_priority: self.scheduler_priority,
            packet_priority: self.pti,
            priority_count: self.pti_count,
            aifsn: self.aifsn,
            contention_window: self.contention_window,
            interface: self.interface,
            hardware_key_selector: self.hardware_key_selector,
        }
    }
}

/// Inputs for one basic-HT A-MPDU PPDU.
///
/// Unlike [`HtTxConfig`], `aggregate_length` is the already assembled A-MPDU
/// byte count, including each delimiter and all non-final alignment/empty
/// delimiters. It must come from the bounded aggregate ownership path; this
/// type never adds a second FCS or guesses delimiter padding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtAmpduTxConfig {
    /// Control frame published with this PPDU.
    pub control: TxControlFrame,
    pub rate: HtRate,
    pub protection_spacing: HtProtectionSpacing,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: MacInterface,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HtAmpduTxConfig {
    /// Construct an aggregate PPDU profile from an already validated length.
    ///
    /// `subframes` is retained as a finite 1..=32 value because hardware and
    /// the vendor formatter can represent a one-subframe A-MPDU, even though
    /// the ordinary batching policy normally waits for at least two frames.
    pub const fn new(rate: HtRate, aggregate_length: u16, subframes: u8) -> Option<Self> {
        if aggregate_length == 0 || subframes == 0 || subframes > 32 {
            return None;
        }
        Some(Self {
            control: TxControlFrame::UNPROTECTED,
            rate,
            protection_spacing: HtProtectionSpacing::Density0To4,
            aggregate_length,
            subframes,
            data_power_primary: 0,
            data_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: TxLifetimeClass::AmpduContainer.fresh_queue_timeout(),
            interface: MacInterface::Station,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    pub(crate) const fn pac_parameters(self) -> MacHtTxParameters {
        MacHtTxParameters {
            control: self.control.pac(TxPhyRate::Ht(self.rate)),
            rate: self.rate.pac_rate(),
            format: MacHtTxFormat::Ampdu,
            length: self.aggregate_length,
            descriptor_count: self.subframes,
            data_power_primary: self.data_power_primary,
            data_power_alternate: self.data_power_alternate,
            protection_spacing: self.protection_spacing.pac_spacing(),
            timeout: self.timeout,
            scheduler_priority: self.scheduler_priority,
            packet_priority: self.pti,
            priority_count: self.pti_count,
            aifsn: self.aifsn,
            contention_window: self.contention_window,
            interface: self.interface,
            hardware_key_selector: self.hardware_key_selector,
            txop: false,
        }
    }
}

/// Optional Trigger-based eligibility for an owned HE A-MPDU queue.
///
/// This prepares the exact queue/MPLEN/BSR state consumed by an AP Trigger;
/// it does not change the immediately submitted HE-SU PPDU into a TB PPDU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeTriggerBasedTxConfig {
    tid_limit: MacHeTbTidLimit,
    tid: MacHeTid,
    runtime_handoff_window_micros: Option<NonZeroU64>,
}

impl HeTriggerBasedTxConfig {
    /// Select one QoS TID admitted by the exact vendor eligibility table.
    ///
    /// SOURCE: complete `libpp[if_hecfg.o]::wifi_he_get_hetb_tid_bitmap`. Keeping the validation here prevents a
    /// later MMIO transaction from silently doing nothing for an ineligible
    /// TID, as the complete `libpp[hal_mac_tx.o]::mac_tx_set_tb` body does.
    pub const fn new(tid_limit: MacHeTbTidLimit, tid: MacHeTid) -> Option<Self> {
        if tid_limit.contains(tid) {
            Some(Self {
                tid_limit,
                tid,
                runtime_handoff_window_micros: None,
            })
        } else {
            None
        }
    }

    /// Enable the bounded software handoff from an RX Trigger/NDPA event to
    /// the unique connected TX owner.
    ///
    /// This caller-supplied duration is measured from the first executor-side
    /// completed-RX handoff. It is intentionally not a fabricated IEEE SIFS or
    /// HE-TB air-timing constant. The final TX boundary still fails closed
    /// until a reviewed HE-TB PHY publication contract exists.
    pub const fn with_runtime_handoff_window_micros(mut self, window: NonZeroU64) -> Self {
        self.runtime_handoff_window_micros = Some(window);
        self
    }

    pub const fn tid_limit(self) -> MacHeTbTidLimit {
        self.tid_limit
    }

    pub const fn tid(self) -> MacHeTid {
        self.tid
    }

    pub const fn runtime_handoff_window_micros(self) -> Option<NonZeroU64> {
        self.runtime_handoff_window_micros
    }
}

/// Inputs for one bounded HE20 SU A-MPDU PPDU.
///
/// Trigger eligibility is optional and remains separate from the ordinary
/// HE-SU vector. Enabling it does not turn an SU transmission into a TB PPDU;
/// it prepares the owned queue and BSR/MPLEN state needed if the AP later
/// sends a matching Trigger frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeAmpduTxConfig {
    /// Control frame published with this PPDU.
    pub control: TxControlFrame,
    rate: HeRate,
    ampdu_density: HtAmpduDensity,
    txop_limit: HeEdcaTxopLimit,
    trigger_based: Option<HeTriggerBasedTxConfig>,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    /// Three repeated ten-bit descriptor-derived timing values.
    ///
    /// The complete formatter proves the mapping. Its higher-level vendor
    /// state producer is not yet promoted, so callers must retain the
    /// negotiated/qualified finite value explicitly.
    protection_spacing: u16,
    pub aggregate_length: u16,
    pub subframes: u8,
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub aifsn: u8,
    pub contention_window: u16,
    pub timeout: u16,
    pub interface: MacInterface,
    pub scheduler_priority: u8,
    pub pti: u8,
    pub pti_count: u16,
    pub hardware_key_selector: u8,
}

impl HeAmpduTxConfig {
    pub const fn new(
        rate: HeRate,
        bss_color: u8,
        aggregate_length: u16,
        subframes: u8,
        ampdu_density: HtAmpduDensity,
    ) -> Option<Self> {
        Self::new_with_txop(
            rate,
            bss_color,
            aggregate_length,
            subframes,
            ampdu_density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Construct an HE20 SU A-MPDU under the complete vendor duration gate.
    ///
    /// SOURCE: ROM rev0 `he_max_apep_length`, complete
    /// `libpp[trc.o]::rx11AXRate2AMPDULimit_update`, and complete
    /// `libpp[pp_he.o]::ppCheckTxHEAMPDUlength`. The final
    /// assembled APEP must satisfy the rate/GI-specific limit selected by the
    /// negotiated EDCA TXOP; the peer's independent A-MPDU exponent is
    /// enforced by the pinned DMA owner before this constructor is reached.
    ///
    /// SOURCE\[HIL_OPEN_HE_RATE_APEP_GATE_2026_07_30]: live MCS9
    /// 2xLTF/1.6-us runs qualified the resulting 31-MPDU ordinary and 30-MPDU
    /// hardware-HE-Control frontiers with complete BlockAck.
    pub const fn new_with_txop(
        rate: HeRate,
        bss_color: u8,
        aggregate_length: u16,
        subframes: u8,
        ampdu_density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Option<Self> {
        let Some(maximum_apep_bytes) = rate.checked_maximum_apep_bytes(txop_limit) else {
            return None;
        };
        if bss_color > 0x3f
            || aggregate_length == 0
            || (aggregate_length as u32) > maximum_apep_bytes
            || subframes == 0
            || subframes > 32
        {
            return None;
        }
        Some(Self {
            control: TxControlFrame::UNPROTECTED,
            rate,
            ampdu_density,
            txop_limit,
            trigger_based: None,
            bss_color,
            spatial_reuse: 0,
            // SOURCE: complete `libpp[pp_he.o]::ppCalDeliNum`
            // writes he_get_min_subframe_len_static's result to descriptor
            // offset +0x28. Complete `libpp[hal_mac_tx.o]::mac_tx_set_hesig` later copies that halfword into all three
            // queue PROTECTION spacing lanes. Keeping rate, density and this
            // derived value in one constructor prevents a metadata/MMIO
            // mismatch like the fixed-0x31 failure qualified by
            // HIL_OPEN_HE20_MCS9_EMPTY_DELIMITERS_2026_07_29.
            protection_spacing: rate.minimum_ampdu_subframe_bytes(ampdu_density),
            aggregate_length,
            subframes,
            data_power_primary: 0,
            data_power_alternate: 0,
            aifsn: 2,
            contention_window: 0,
            timeout: TxLifetimeClass::AmpduContainer.fresh_queue_timeout(),
            interface: MacInterface::Station,
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            hardware_key_selector: 0,
        })
    }

    pub const fn rate(self) -> HeRate {
        self.rate
    }

    pub const fn ampdu_density(self) -> HtAmpduDensity {
        self.ampdu_density
    }

    pub const fn txop_limit(self) -> HeEdcaTxopLimit {
        self.txop_limit
    }

    pub const fn protection_spacing(self) -> u16 {
        self.protection_spacing
    }

    pub const fn with_trigger_based(mut self, trigger_based: HeTriggerBasedTxConfig) -> Self {
        self.trigger_based = Some(trigger_based);
        self
    }

    pub const fn trigger_based(self) -> Option<HeTriggerBasedTxConfig> {
        self.trigger_based
    }

    pub(crate) const fn valid(self) -> bool {
        let apep_valid = match self.rate.checked_maximum_apep_bytes(self.txop_limit) {
            Some(limit) => (self.aggregate_length as u32) <= limit,
            None => false,
        };
        self.bss_color <= 0x3f
            && self.spatial_reuse <= 0x0f
            && self.protection_spacing <= 0x03ff
            && self.aggregate_length != 0
            && apep_valid
            && self.subframes != 0
            && self.subframes <= 32
            && self.aifsn <= 0x0f
            && self.contention_window <= 0x03ff
            && self.timeout <= 0x0fff
            && self.hardware_key_selector <= 0x3f
            && self.scheduler_priority <= 0x0f
            && self.pti <= 0x0f
            && self.pti_count <= 0x0fff
    }

    pub(crate) const fn pac_parameters(self) -> MacHeTxParameters {
        MacHeTxParameters {
            control: self.control.pac(TxPhyRate::He(self.rate)),
            rate: self.rate.pac_rate(),
            format: MacHeTxFormat::Ampdu,
            apep_length: self.aggregate_length,
            descriptor_count: self.subframes,
            bss_color: self.bss_color,
            spatial_reuse: self.spatial_reuse,
            software_he_control: None,
            data_power_primary: self.data_power_primary,
            data_power_alternate: self.data_power_alternate,
            protection_spacing: self.protection_spacing,
            timeout: self.timeout,
            scheduler_priority: self.scheduler_priority,
            packet_priority: self.pti,
            priority_count: self.pti_count,
            aifsn: self.aifsn,
            contention_window: self.contention_window,
            interface: self.interface,
            hardware_key_selector: self.hardware_key_selector,
        }
    }
}

/// One negotiated aggregate queue vector, independent of HT versus HE format.
///
/// The descriptor/BlockAck owner needs the same three operations for both
/// formats: inspect the selected PHY rate and key slot, then replace the
/// aggregate geometry after retaining only the MPDUs missing from a BlockAck.
/// Keeping that operation here avoids format-specific retry mutation in every
/// upper runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmpduTxConfig {
    Ht(HtAmpduTxConfig),
    He(HeAmpduTxConfig),
}

impl AmpduTxConfig {
    pub const fn rate(self) -> TxPhyRate {
        match self {
            Self::Ht(config) => TxPhyRate::Ht(config.rate),
            Self::He(config) => TxPhyRate::He(config.rate()),
        }
    }

    pub const fn hardware_key_selector(self) -> u8 {
        match self {
            Self::Ht(config) => config.hardware_key_selector,
            Self::He(config) => config.hardware_key_selector,
        }
    }

    /// Replace only the fields that change when a retained aggregate is
    /// published again: its geometry, the next backoff and the protection
    /// selected for the new length.
    pub fn update_retained_retry(
        &mut self,
        aggregate_length: u16,
        subframes: u8,
        contention_window: u16,
        control: TxControlFrame,
    ) {
        match self {
            Self::Ht(config) => {
                config.aggregate_length = aggregate_length;
                config.subframes = subframes;
                config.contention_window = contention_window;
                config.control = control;
            }
            Self::He(config) => {
                config.aggregate_length = aggregate_length;
                config.subframes = subframes;
                config.contention_window = contention_window;
                config.control = control;
            }
        }
    }

    /// PSDU length presented to protection selection.
    pub const fn aggregate_length(self) -> u16 {
        match self {
            Self::Ht(config) => config.aggregate_length,
            Self::He(config) => config.aggregate_length,
        }
    }

    pub const fn control(self) -> TxControlFrame {
        match self {
            Self::Ht(config) => config.control,
            Self::He(config) => config.control,
        }
    }
}

impl LegacyTxConfig {
    /// Conservative 1-Mbit/s management-frame profile used by the first
    /// hardware run.
    ///
    /// The timeout is the exact q0 image observed while the pinned vendor
    /// driver submitted an open-authentication MPDU on ESP32-S31 rev0. The
    /// no-backoff base image is `TX_Q0_CONFIG = 0x1200_03ff`
    /// (`scheduler=1`, `timeout=0x3ff`, `AIFSN=2`, `CW=0`) and
    /// `TX_Q0_PTI = 0x0011_1110` (`count=1`, four packet-PTI lanes at 1).
    /// SOURCE: a live station authentication run on hardware plus complete
    /// `libpp.a[hal_mac_tx.o,hal_mac.o,hal_coex.o]` and
    /// `libcoexist.a[coexist_core.o]`.
    pub const fn management_1m(signal: u16) -> Self {
        Self {
            control: TxControlFrame::UNPROTECTED,
            rate: LegacyRate::Dsss1MLong,
            signal,
            data_power: 8,
            aifsn: 2,
            contention_window: 0,
            timeout: TxLifetimeClass::DirectMpdu.fresh_queue_timeout(),
            interface: MacInterface::Station,
            // Complete libpp.a[hal_mac.o,hal_coex.o] selects the unsigned
            // minimum of packet PTI 1 and coexistence event-one PTI 5.
            scheduler_priority: 1,
            pti: 1,
            pti_count: 1,
            response: MacLegacyTxResponse::Ack,
            hardware_key_selector: 0,
        }
    }

    /// Builds the direct q0 profile from the owned MPDU length.
    ///
    /// Hardware appends the four-byte FCS. For example, a 30-byte open
    /// authentication MPDU requires PLCP1 `0x22`. S31 hardware proved that
    /// replaying the vendor-context value `0x00b6` instead produces TX status
    /// four and no authentication response.
    pub const fn management_1m_from_mpdu_length(mpdu_length: u16) -> Option<Self> {
        let Some(signal) = mpdu_length.checked_add(LEGACY_FCS_LENGTH) else {
            return None;
        };
        if signal > 0x0fff {
            return None;
        }
        Some(Self::management_1m(signal))
    }
}

/// MAC policy owner for one lower, permanently located TX DMA allocation.
///
/// Descriptor and buffer storage live in the audited chip-DMA leaf. This
/// upper owner retains only the queue, generation and completion policy and
/// cannot manufacture a DMA address or final queue-start capability.
#[cfg_attr(
    not(feature = "test-support"),
    doc = r#"
Production builds cannot synthesize a DMA owner, on either host or chip:

```compile_fail,E0599
# #![cfg_attr(target_arch = "riscv32", no_std)]
# #![cfg_attr(target_arch = "riscv32", no_main)]
# #[cfg(target_arch = "riscv32")]
# #[panic_handler]
# fn panic(_: &core::panic::PanicInfo) -> ! {
#     loop {}
# }
use oer_esp32s31_ieee80211_mac::tx::TxSlot;
let slot = TxSlot::<512>::new_model();
```

The production constructor consumes a pinned DMA allocation:

```no_run
# #![cfg_attr(target_arch = "riscv32", no_std)]
# #![cfg_attr(target_arch = "riscv32", no_main)]
# #[cfg(target_arch = "riscv32")]
# #[panic_handler]
# fn panic(_: &core::panic::PanicInfo) -> ! {
#     loop {}
# }
use oer_esp32s31_hal::ieee80211::dma::tx_storage::PinnedTxDmaStorage;
use oer_esp32s31_ieee80211_mac::tx::TxSlot;
fn slot(dma: PinnedTxDmaStorage<512>) -> TxSlot<512> {
    TxSlot::from_dma(dma)
}
```
"#
)]
pub struct TxSlot<const BUFFER_SIZE: usize> {
    dma: PinnedTxDmaStorage<BUFFER_SIZE>,
    generation_cursor: u32,
    work: oer_ieee80211_softmac::MacTxWork,
    active: TxCookie,
    queue: LegacyTxQueue,
}

impl<const BUFFER_SIZE: usize> TxSlot<BUFFER_SIZE> {
    /// Attach MAC policy to a statically pinned lower DMA allocation.
    pub fn from_dma(dma: PinnedTxDmaStorage<BUFFER_SIZE>) -> Self {
        Self {
            dma,
            generation_cursor: 0,
            work: oer_ieee80211_softmac::MacTxWork::new(),
            active: TxCookie(0),
            queue: LegacyTxQueue::Voice,
        }
    }

    /// Construct a native model with no asynchronous DMA actor.
    ///
    /// The `test-support` feature enables this allocation-backed constructor
    /// on either architecture. The allocation models permanently retained
    /// target SRAM; production builds have no model constructors. An invalid
    /// buffer extent returns the DMA binding error.
    ///
    /// ```
    /// use oer_esp32s31_ieee80211_mac::tx::TxSlot;
    /// let slot = TxSlot::<512>::new_model()?;
    /// # Ok::<(), oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorageError>(())
    /// ```
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_model() -> Result<Self, TxDmaStorageError> {
        let storage = Box::leak(Box::new(TxDmaStorage::new()));
        let dma = TxDmaStorage::pin::<oer_esp32s31_hal::ieee80211::dma::space::HostDma>(
            oer_esp32s31_hal::dma_static::Placed::host(storage),
        )?;
        Ok(Self::from_dma(dma))
    }

    /// Receipt remains in pinned CPU metadata across retry detach/abort. Read
    /// at the terminal edge before a new ordinary exchange resets it.
    pub const fn work(&self) -> oer_ieee80211_softmac::MacTxWork {
        self.work
    }

    pub fn reset_work(self: Pin<&mut Self>) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.state() != TxSlotState::Free {
            return Err(TxError::Busy);
        }
        slot.work = oer_ieee80211_softmac::MacTxWork::new();
        Ok(())
    }

    pub fn state(&self) -> TxSlotState {
        self.dma.state()
    }

    /// Borrow the complete source buffer while software exclusively owns it.
    pub fn buffer_mut(self: Pin<&mut Self>) -> Result<&mut [u8; BUFFER_SIZE], TxError> {
        self.get_mut()
            .dma
            .buffer_mut()
            .map_err(map_dma_storage_error)
    }

    /// Current descriptor ownership word for bounded diagnostics.
    pub fn descriptor_word0(&self) -> oer_esp32s31_hal::layouts::WifiTxDmaDescriptorControl {
        self.dma.descriptor_word0()
    }

    /// Stable descriptor address retained by the lower DMA owner.
    pub fn descriptor_address(self: Pin<&Self>) -> u32 {
        self.get_ref().dma.binding().descriptor_address()
    }

    /// Stable source-buffer address retained by the lower DMA owner.
    pub fn buffer_address(self: Pin<&Self>) -> u32 {
        self.get_ref().dma.binding().buffer_address()
    }

    pub fn reserve(
        self: Pin<&mut Self>,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<TxCookie, TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Free {
            return Err(TxError::Busy);
        }
        let generation = slot
            .generation_cursor
            .checked_add(1)
            .ok_or(TxError::ResetRequired)?;
        slot.dma
            .reserve(buffer_capacity, transfer_length)
            .map_err(map_dma_storage_error)?;
        slot.generation_cursor = generation;
        slot.active = TxCookie(generation);
        slot.queue = LegacyTxQueue::Voice;
        Ok(slot.active)
    }

    /// Cancel a descriptor that has not been published to an ordinary queue.
    ///
    /// Queue preparation can fail after the descriptor image is built but
    /// before [`submit_legacy`](Self::submit_legacy) or
    /// [`submit_ht`](Self::submit_ht) transfers ownership to hardware. This
    /// edge makes that pre-publication failure recoverable without inventing
    /// a hardware detach transaction.
    pub fn cancel_reservation(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        slot.dma
            .cancel_reservation()
            .map_err(map_dma_storage_error)?;
        slot.active = TxCookie(0);
        Ok(())
    }

    /// Programs and starts one legacy q0 attempt.
    ///
    /// Pinning keeps both the private descriptor and its enclosed source
    /// buffer stable across the complete hardware-ownership interval.
    pub fn submit_legacy_q0<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        self.submit_legacy(hardware, cookie, LegacyTxQueue::Voice, config)
    }

    /// Programs and starts one legacy attempt on an ordinary EDCA queue.
    pub fn submit_legacy<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: LegacyTxConfig,
    ) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let index = queue.index();
        let publication = slot.dma.publication().map_err(map_dma_storage_error)?;
        let program = MacLegacyTxProgram::new(
            &publication,
            MacLegacyTxParameters {
                control: config.control.pac(TxPhyRate::Legacy(config.rate)),
                rate: config.rate.pac_rate(),
                signal: config.signal,
                data_power: config.data_power,
                response: config.response,
                hardware_key_selector: config.hardware_key_selector,
                interface: config.interface,
                aifsn: config.aifsn,
                contention_window: config.contention_window,
                timeout: config.timeout,
                scheduler_priority: config.scheduler_priority,
                packet_priority: config.pti,
                priority_count: config.pti_count,
            },
        )
        .ok_or(TxError::Invalid)?;
        if !hardware.prepare_bound_legacy_tx(&publication, index, program) {
            return Err(TxError::QueueActive);
        }

        slot.queue = queue;
        publication.commit(|start| {
            hardware.start_bound_legacy_tx(start, index);
        });
        slot.work.record_publication(
            config.signal,
            1,
            NonZeroU32::new(config.rate.nominal_kbps()),
            TxPhyRate::Legacy(config.rate).ppdu_timing(),
            Some(TxContention {
                aifsn: config.aifsn,
                backoff_slots: config.contention_window,
            }),
        );
        Ok(())
    }

    /// Programs and starts one non-aggregate HT MPDU.
    ///
    /// The caller must also have configured the PHY channel engine for
    /// `config.rate.channel_width` during association. This method publishes
    /// the matching HT-SIG/PLCP CBW bits, while the channel engine still owns
    /// secondary-channel placement.
    pub fn submit_ht<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HtTxConfig,
    ) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let index = queue.index();
        let publication = slot.dma.publication().map_err(map_dma_storage_error)?;
        let program =
            MacHtTxProgram::new(&publication, config.pac_parameters()).ok_or(TxError::Invalid)?;
        if !hardware.prepare_bound_ht_tx(&publication, index, program) {
            return Err(TxError::QueueActive);
        }

        slot.queue = queue;
        publication.commit(|start| {
            hardware.start_bound_ht_tx(start, index);
        });
        slot.work.record_publication(
            config.length,
            1,
            NonZeroU32::new(config.rate.nominal_kbps()),
            TxPhyRate::Ht(config.rate).ppdu_timing(),
            Some(TxContention {
                aifsn: config.aifsn,
                backoff_slots: config.contention_window,
            }),
        );
        Ok(())
    }

    /// Programs and starts one HE20 SU S-MPDU on an ordinary EDCA queue.
    ///
    /// The source descriptor retains the encoded MPDU (and any
    /// hardware-generated CCMP MIC/FCS space). The HE queue vector supplies
    /// the S-MPDU delimiter/APEP geometry; this path deliberately does not
    /// borrow the multi-descriptor A-MPDU owner or its BlockAck completion
    /// contract.
    pub fn submit_he_smpdu<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeSmpduTxConfig,
    ) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Reserved || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let index = queue.index();
        let publication = slot.dma.publication().map_err(map_dma_storage_error)?;
        if !config.valid() {
            return Err(TxError::Invalid);
        }
        let program =
            MacHeTxProgram::new(&publication, config.pac_parameters()).ok_or(TxError::Invalid)?;
        if !hardware.prepare_bound_he_tx(&publication, index, program) {
            return Err(TxError::QueueActive);
        }

        slot.queue = queue;
        publication.commit(|start| hardware.start_bound_he_tx(start, index));
        // HE S-MPDU has a four-byte delimiter and a hardware-generated FCS.
        slot.work.record_publication(
            config.apep_length(),
            1,
            None,
            None,
            Some(TxContention {
                aifsn: config.aifsn,
                backoff_slots: config.contention_window,
            }),
        );
        Ok(())
    }

    /// Decodes and acknowledges one q0 completion. Storage stays retained in
    /// `Completed`; it is not reusable until `detach_completed` closes q0.
    pub fn acknowledge_q0_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<TxCompletion>>, TxError> {
        self.acknowledge_completion(hardware)
    }

    /// Decodes and acknowledges the completion for the queue retained by the
    /// hardware-owned slot.
    pub fn acknowledge_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<TxCompletion>>, TxError> {
        let slot = self.get_mut();
        let index = slot.queue.hardware_index();
        let Some(registers) = hardware.take_tx_completion(index) else {
            return Ok(None);
        };

        if slot.dma.state() != TxSlotState::HardwareOwned {
            slot.dma.quarantine();
            return Err(TxError::Stale);
        }
        match decode_or_quarantine(slot.active, registers, || slot.dma.quarantine())
            .map_err(TxError::Completion)?
        {
            CompletionObservation::Observed(completion) => {
                slot.dma.mark_completed().map_err(map_dma_storage_error)?;
                Ok(Some(CompletionObservation::Observed(completion)))
            }
            CompletionObservation::NoEffect => Ok(Some(CompletionObservation::NoEffect)),
        }
    }

    /// Decode and acknowledge the completion of a BlockAckReq together with
    /// the BlockAck it solicited, sampled before the completion edge is
    /// acknowledged.
    pub fn acknowledge_block_ack_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<ampdu::HtAmpduTxCompletion>>, TxError> {
        let slot = self.get_mut();
        let index = slot.queue.hardware_index();
        let Some(registers) = hardware.take_block_ack_completion(index) else {
            return Ok(None);
        };

        if slot.dma.state() != TxSlotState::HardwareOwned {
            slot.dma.quarantine();
            return Err(TxError::Stale);
        }
        match decode_or_quarantine(slot.active, registers.tx(), || slot.dma.quarantine())
            .map_err(TxError::Completion)?
        {
            CompletionObservation::Observed(tx) => {
                slot.dma.mark_completed().map_err(map_dma_storage_error)?;
                Ok(Some(CompletionObservation::Observed(
                    ampdu::HtAmpduTxCompletion {
                        tx,
                        block_ack: ampdu::HtBlockAckObservation::new(
                            registers.block_ack_control(),
                            oer_ieee80211_mac::sequence::SequenceNumber::from_low_bits(
                                registers.block_ack_starting_sequence(),
                            ),
                            registers.block_ack_bitmap(),
                        ),
                        block_ack_received: registers.block_ack_received(),
                    },
                )))
            }
            CompletionObservation::NoEffect => Ok(Some(CompletionObservation::NoEffect)),
        }
    }

    /// Starts the recovered two-phase abort for this queue's TX-timeout edge.
    ///
    /// `SOURCE[PROMOTED_LMAC_TX]` forces CCA to three before its
    /// fixed 16-us settling interval. `Ok(false)` means that this queue has no
    /// timeout edge and leaves all registers untouched.
    pub fn begin_timeout_abort<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::HardwareOwned || cookie != slot.active {
            return Err(TxError::Stale);
        }
        if !hardware.begin_tx_timeout_abort(slot.queue.index()) {
            return Ok(false);
        }
        Ok(true)
    }

    /// Permanently quarantine a hardware-owned slot until the radio resets.
    ///
    /// An executor deadline can expire without the qualified hardware timeout
    /// bit becoming visible. Safe code must not free or reuse the DMA storage
    /// in that state. The caller returns control to the unique radio lifecycle
    /// owner, which performs a full reset before reconstructing TX storage.
    pub fn require_reset(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::HardwareOwned || cookie != slot.active {
            return Err(TxError::Stale);
        }
        slot.dma.require_reset().map_err(map_dma_storage_error)?;
        Ok(())
    }

    /// Disable and release one collision-owned ordinary queue.
    pub fn abort_collision<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::HardwareOwned || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let descriptor_head = slot.dma.binding().descriptor_address();
        match hardware.with_tx_queue_detached(
            slot.queue.index(),
            descriptor_head,
            MacTxDetachReason::Collision,
            |detached| slot.dma.release_aborted(detached),
        ) {
            MacTxDetachOutcome::NoEvent => Ok(false),
            MacTxDetachOutcome::Detached(Ok(())) => {
                slot.active = TxCookie(0);
                Ok(true)
            }
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                slot.dma.quarantine();
                Err(TxError::DetachFailed)
            }
        }
    }

    /// Finishes a timed-out queue abort after at least 16 us of settling.
    ///
    /// The caller owns the one timer edge between this method and
    /// [`begin_timeout_abort`](Self::begin_timeout_abort). The register order
    /// matches the reviewed vendor path: invalidate, release forced CCA,
    /// disable a queue that was still valid, then clear its timeout bit.
    pub fn finish_timeout_abort<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::HardwareOwned || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let descriptor_head = slot.dma.binding().descriptor_address();
        match hardware.with_tx_queue_detached(
            slot.queue.index(),
            descriptor_head,
            MacTxDetachReason::Timeout,
            |detached| slot.dma.release_aborted(detached),
        ) {
            MacTxDetachOutcome::NoEvent => Err(TxError::TimeoutNotPending),
            MacTxDetachOutcome::Detached(Ok(())) => {
                slot.active = TxCookie(0);
                Ok(())
            }
            MacTxDetachOutcome::Failed | MacTxDetachOutcome::Detached(Err(_)) => {
                slot.dma.quarantine();
                Err(TxError::DetachFailed)
            }
        }
    }

    /// Makes the completed static slot reusable after disabling q0 and exact
    /// readback. This is normal single-attempt turnover, not a global DMA
    /// release oracle for freeing the backing allocation during teardown.
    pub fn detach_completed<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<(), TxError> {
        let slot = self.get_mut();
        if slot.dma.state() != TxSlotState::Completed || cookie != slot.active {
            return Err(TxError::Stale);
        }
        let descriptor_head = slot.dma.binding().descriptor_address();
        match hardware.with_tx_queue_detached(
            slot.queue.index(),
            descriptor_head,
            MacTxDetachReason::Completed,
            |detached| slot.dma.release_completed(detached),
        ) {
            MacTxDetachOutcome::Detached(Ok(())) => {
                slot.active = TxCookie(0);
                Ok(())
            }
            MacTxDetachOutcome::NoEvent
            | MacTxDetachOutcome::Failed
            | MacTxDetachOutcome::Detached(Err(_)) => {
                slot.dma.quarantine();
                Err(TxError::DetachFailed)
            }
        }
    }
}

fn map_dma_storage_error(error: TxDmaStorageError) -> TxError {
    match error {
        TxDmaStorageError::Address | TxDmaStorageError::InvalidLength => TxError::Invalid,
        TxDmaStorageError::Busy => TxError::Busy,
        TxDmaStorageError::State => TxError::Stale,
    }
}

#[cfg(test)]
mod completion_tests;
#[cfg(test)]
pub(crate) mod test_support;

pub mod ampdu;
pub mod metadata;
pub mod policy;
pub mod protection;
pub mod runtime;
