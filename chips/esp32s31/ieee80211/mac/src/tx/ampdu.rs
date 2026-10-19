//! ESP32-S31 A-MPDU formatting, lifecycle and retained-DMA composition.
//!
//! Generic IEEE 802.11 BlockAck negotiation lives in the portable protocol
//! crate. The [`block_ack`] adapter retains only chip policy and completion
//! geometry. This facade preserves the existing public API while the
//! formatter itself owns no timer, executor or management-frame policy.

#![forbid(unsafe_code)]

use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_softmac::MacTxWork;

use core::{marker::PhantomPinned, pin::Pin};

use oer_esp32s31_hal::ieee80211::dma::tx_ampdu_storage::AmpduDmaStorageError;
use oer_esp32s31_hal::types::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTriggerTxQueueSnapshot,
};
use pin_project::pin_project;

pub mod block_ack;
mod capacity;
mod commit;
mod hardware;
mod length;
mod lifecycle;
#[cfg(any(test, feature = "test-support"))]
mod model;
mod owner;
mod request;
mod retry;
mod submission;

pub use block_ack::{
    ADDBA_ACTION_BODY_LEN, ADDBA_REQUEST_ACTION, ADDBA_RESPONSE_ACTION, AddbaRequest,
    BLOCK_ACK_CATEGORY, BlockAckAction, DELBA_ACTION, HtBlockAckObservation, OperationalTxBlockAck,
    TX_AMPDU_SLOT_CAPACITY, TX_BLOCK_ACK_MAX_WINDOW, TxAmpduBatch, TxAmpduBatchError,
    TxAmpduBatchStep, TxAmpduCompletion, TxAmpduDisposition, TxAmpduMpdu, TxAmpduSlot,
    TxBlockAckAlarm, TxBlockAckBitmap, TxBlockAckConfig, TxBlockAckDialogToken, TxBlockAckError,
    TxBlockAckResponse, TxBlockAckSession, parse_block_ack_action,
};
pub use hardware::HtAmpduHardware;
pub use length::{HtAmpduLength, HtAmpduLengthAccumulator, HtAmpduLengthError};
#[cfg(any(test, feature = "test-support"))]
pub use model::{
    BasicHtAmpduAssemblyError, BasicHtAmpduAssemblyInput, BasicHtAmpduAssemblyOutput,
    BasicHtAmpduCompletionInput, BasicHtAmpduCompletionOutput, basic_ht_ampdu_assembly,
    basic_ht_ampdu_completion,
};
#[cfg(test)]
pub(crate) use model::{TX_BUFFER_END_BIT, TX_DESCRIPTOR_HE_BIT};
pub use oer_esp32s31_hal::ieee80211::dma::tx_ampdu_storage::RetainedAmpduDmaStorage;
pub use owner::{HtAmpduTxResources, RetainedDmaAmpduTx};
pub use request::{
    AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
};

use crate::tx::runtime::{AmpduRetryDecision, AmpduRetryError};
use crate::tx::{LegacyTxQueue, TxCompletion, TxCookie, TxDmaState};
pub const TX_AMPDU_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: u16 = 4;
const TX_AMPDU_DEFAULT_MAX_BYTES: u16 = 0x1fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxError {
    /// A storage transition requiring software-owned idle state observed a
    /// different lifecycle state.
    NotFree(TxDmaState),
    /// A commit attempted to advance beyond the statically bounded slot set.
    SlotCapacity,
    Stale,
    Completion(crate::tx::CompletionFault),
    FrameIndexOutOfRange {
        index: u8,
        count: u8,
    },
    InvalidRetryMask {
        mask: u32,
        count: u8,
    },
    RetainedFrameTooShort {
        index: u8,
        length: u16,
    },
    /// Stored descriptor metadata did not resolve to any currently retained
    /// stable backing or to the storage's own fixed DMA buffer.
    BackingUnavailable {
        index: u8,
    },
    SlotCountOverflow {
        count: usize,
    },
    InvalidStorageGeometry {
        slots: usize,
    },
    TxProgramUnavailable {
        format: HtAmpduTxFormat,
    },
    InvalidTriggerReservation {
        queue: u8,
        subframes: u8,
    },
    FrameTooLong,
    TooFewFrames,
    AggregateFull,
    Length(HtAmpduLengthError),
    AggregateConfigurationMismatch,
    QueueActive,
    TriggerMsduLengthUnavailable,
    TriggerBased(MacHeTbProgramError),
    DetachFailed,
    TimeoutNotPending,
    ResetRequired,
    /// The lower descriptor/backing owner rejected a lifecycle or range edge.
    DmaStorage(AmpduDmaStorageError),
}

impl From<AmpduDmaStorageError> for HtAmpduTxError {
    fn from(error: AmpduDmaStorageError) -> Self {
        Self::DmaStorage(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduTxFormat {
    HtAmpdu,
    HeAmpdu,
}

/// Why a detached aggregate is published again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRepublication {
    /// BlockAck reported the selected MPDUs missing. They were on air, so
    /// each retained MPDU gains the IEEE 802.11 Retry bit.
    Retransmission,
    /// The RTS/CTS exchange failed before the data PPDU. Every MPDU is
    /// retained unchanged: none was transmitted.
    AfterProtectionFailure,
}

/// Completion of a PPDU that solicited a BlockAck: an A-MPDU or a
/// BlockAckReq.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduTxCompletion {
    pub tx: TxCompletion,
    pub block_ack: HtBlockAckObservation,
    pub block_ack_received: bool,
}

/// One completion observed after synchronizing the hardware, descriptor and
/// retry owners in that order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedAmpduRetryCompletion {
    pub completion: HtAmpduTxCompletion,
    pub first_sequence: SequenceNumber,
    pub subframes: u8,
    pub decision: AmpduRetryDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedAmpduRetryCompletionError {
    Hardware(HtAmpduTxError),
    Retry(AmpduRetryError),
}

impl From<HtAmpduTxError> for RetainedAmpduRetryCompletionError {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Hardware(error)
    }
}

impl From<AmpduRetryError> for RetainedAmpduRetryCompletionError {
    fn from(error: AmpduRetryError) -> Self {
        Self::Retry(error)
    }
}

impl HtAmpduTxCompletion {
    /// Return whether a completed A-MPDU positively acknowledges one MPDU.
    ///
    /// The ordinary TX status does not decide whether the BlockAck bitmap is
    /// usable for a non-trigger A-MPDU. The vendor A-MPDU completion path reads
    /// and applies the bitmap from its ACK-timeout/frame-exchange terminal path
    /// as well as from a status-zero completion. The independent hardware
    /// result bit is the validity edge: when it is clear, the bitmap words may
    /// still contain the preceding successful result and must not suppress a
    /// retry.
    ///
    /// Trigger-flow ACK timeout is a different vendor branch. If its packet
    /// counters do not select `lmacProcessTBSuccess`, it remains a retry even
    /// when the ordinary BlockAck result registers look populated. Only a
    /// status-zero trigger completion may consume that bitmap.
    ///
    /// SOURCE\[HIL_OPEN_HT_AMPDU_PARTIAL_2026_07_29]: live HT40 MCS7 SGI
    /// four-stream TX load produced successful partial BlockAck completions
    /// and status-five completions with stale nonzero bitmap words but a clear
    /// BlockAck-received result.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacEndFrameExchangeSequence`
    /// status-five A-MPDU branch calls `libpp[hal_mac_tx.o]::hal_mac_tx_get_blockack`, publishes the
    /// result through `libpp[pp.o]::ppTxqUpdateBitmap`, and then enters `libpp[pp.o]::ppResortTxAMPDU`.
    pub const fn acknowledges(self, sequence: SequenceNumber) -> bool {
        match self.valid_block_ack() {
            Some(block_ack) => block_ack.acknowledges(sequence),
            None => false,
        }
    }

    /// The BlockAck the completion carries, under the validity rule of
    /// [`Self::acknowledges`]: the hardware result bit is set, and a
    /// Trigger-flow completion counts only with status zero. `None` when the
    /// bitmap words are stale.
    pub const fn valid_block_ack(self) -> Option<block_ack::TxBlockAckBitmap> {
        if (!self.tx.is_trigger_flow() || self.tx.status() == 0) && self.block_ack_received {
            Some(self.block_ack.block_ack)
        } else {
            None
        }
    }
}

/// Pinned A-MPDU protocol state and bounded frame-formatting workspace.
///
/// This type records aggregate layout, completion state and retained frame
/// metadata. It cannot publish a descriptor chain or start hardware by itself:
/// production submission requires [`HtAmpduTxResources`], which couples this
/// state to the lower DMA crate's descriptor/backing owner.
///
/// SOURCE\[HIL_OPEN_HT_AMPDU_DIRECT_2026_07_29]: ESP32-S31 rev0,
/// psram-code-psram-data, open PHY/MAC, HT40 MCS7 SGI. Four observed
/// two-MPDU submissions from this pool each returned a BlockAck bitmap ending
/// in `0x000f`, with no aggregate hardware timeout.
#[pin_project]
pub struct HtAmpduTxStorage<const SLOTS: usize> {
    /// Stable backing allocation selected for each committed MPDU: a
    /// separately pinned lease retained by the safe runtime wrapper.
    buffer_addresses: [usize; SLOTS],
    frame_lengths: [u16; SLOTS],
    hardware_mic_lengths: [u8; SLOTS],
    /// Original MSDU length corresponding to each encoded MPDU.
    ///
    /// Zero means the upper MAC did not supply this vendor-visible value and
    /// therefore the frame cannot be published as Trigger-eligible.
    msdu_lengths: [u16; SLOTS],
    psdu_lengths: [u16; SLOTS],
    /// MAC inserts one four-byte HE-Control field for this MPDU.
    ///
    /// The encoded frame and low fourteen-bit metadata length exclude those
    /// bytes; DMA metadata byte seven bit zero owns the insertion.
    hardware_he_control: [bool; SLOTS],
    empty_delimiters: [u8; SLOTS],
    descriptor_capacities: [u16; SLOTS],
    state: TxDmaState,
    generation_cursor: u32,
    active: TxCookie,
    queue: LegacyTxQueue,
    count: u8,
    prepared_length: u16,
    aggregate_length: u16,
    work: MacTxWork,
    max_aggregate_bytes: u16,
    trigger_reservation: Option<MacHeTbLinkReservation>,
    trigger_publication_snapshot: Option<MacHeTriggerTxQueueSnapshot>,
    detached: bool,
    #[pin]
    _pin: PhantomPinned,
}

impl<const SLOTS: usize> HtAmpduTxStorage<SLOTS> {
    pub const fn new() -> Self {
        Self {
            buffer_addresses: [0; SLOTS],
            frame_lengths: [0; SLOTS],
            hardware_mic_lengths: [0; SLOTS],
            msdu_lengths: [0; SLOTS],
            psdu_lengths: [0; SLOTS],
            hardware_he_control: [false; SLOTS],
            empty_delimiters: [0; SLOTS],
            descriptor_capacities: [0; SLOTS],
            state: TxDmaState::Free,
            generation_cursor: 0,
            active: TxCookie(0),
            queue: LegacyTxQueue::BestEffort,
            count: 0,
            prepared_length: 0,
            aggregate_length: 0,
            work: MacTxWork::new(),
            // Conservative HT A-MPDU exponent zero until the peer capability
            // is installed by the association owner.
            max_aggregate_bytes: TX_AMPDU_DEFAULT_MAX_BYTES,
            trigger_reservation: None,
            trigger_publication_snapshot: None,
            detached: false,
            _pin: PhantomPinned,
        }
    }

    /// Consume a unique static owner and permanently pin its DMA addresses.
    pub fn pin_static(storage: &'static mut Self) -> Pin<&'static mut Self> {
        Pin::static_mut(storage)
    }

    pub const fn state(&self) -> TxDmaState {
        self.state
    }

    #[cfg(any(test, feature = "test-support", oer_probe))]
    pub const fn frame_count(&self) -> u8 {
        self.count
    }

    /// Work published by the retained DMA owner since the last successful
    /// `begin`. Survives completion, detach and release. HE time is unestimated.
    pub const fn work(&self) -> MacTxWork {
        self.work
    }

    pub const fn aggregate_length(&self) -> u16 {
        self.aggregate_length
    }

    /// Installs the peer-advertised maximum A-MPDU byte length.
    ///
    /// This must be done while software owns an idle pool. The four HT values
    /// are 0x1fff, 0x3fff, 0x7fff and 0xffff for capability exponents 0..=3.
    pub fn configure_max_aggregate_bytes(
        self: Pin<&mut Self>,
        max_aggregate_bytes: u16,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::Free {
            return Err(HtAmpduTxError::NotFree(*storage.state));
        }
        if max_aggregate_bytes == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits));
        }
        *storage.max_aggregate_bytes = max_aggregate_bytes;
        Ok(())
    }

    /// Calculate the exact A-MPDU byte count for the committed software-owned
    /// prefix, before any DMA address is published.
    pub fn prepared_aggregate(&self, cookie: TxCookie) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.state != TxDmaState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        // Both HT and HE can retain a one-subframe A-MPDU. Complete
        // `libpp[pp.o]::ppAssembleAMPDU` starts from the supplied
        // chain head and iterates until its next pointer is null; it has no
        // minimum-two branch.
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        self.calculate_aggregate()
    }

    /// Read the committed metadata-byte-four value while software still owns
    /// the aggregate.
    ///
    /// This bounded observation is intended for formatter validation and
    /// hardware reporting. It does not expose the DMA buffer or permit mutation.
    pub fn prepared_empty_delimiters(
        &self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<u8, HtAmpduTxError> {
        if self.state != TxDmaState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(index);
        if index >= usize::from(self.count) {
            return Err(HtAmpduTxError::FrameIndexOutOfRange {
                index: index as u8,
                count: self.count,
            });
        }
        Ok(self.empty_delimiters[index])
    }

    /// Begin constructing one aggregate in the software-owned pool.
    pub fn begin(self: Pin<&mut Self>) -> Result<TxCookie, HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::Free {
            return Err(HtAmpduTxError::NotFree(*storage.state));
        }
        if !(2..=TX_AMPDU_SLOT_CAPACITY).contains(&SLOTS) {
            return Err(HtAmpduTxError::InvalidStorageGeometry { slots: SLOTS });
        }
        let generation = (*storage.generation_cursor)
            .checked_add(1)
            .ok_or(HtAmpduTxError::ResetRequired)?;
        *storage.generation_cursor = generation;
        *storage.active = TxCookie(generation);
        *storage.count = 0;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.work = MacTxWork::new();
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxDmaState::Reserved;
        Ok(*storage.active)
    }
}

impl<const SLOTS: usize> Default for HtAmpduTxStorage<SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
