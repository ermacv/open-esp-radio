//! AP adapter for the shared retained-DMA HT A-MPDU mechanism.
//!
//! The MAC crate owns descriptor assembly, BlockAck sampling and retained
//! retry compaction. This module supplies only AP peer identity, pairwise key
//! authority and interface-specific publication configuration.

use crate::{
    engine::{ApAggregateBinding, ApAggregateFrame},
    tx::{ApTx, ApTxError},
};
use oer_ieee80211_mac::{
    block_ack::encode_block_ack_request, qos::WmmAccessCategory, sequence::SequenceNumber,
};

use oer_memory::StableDmaBacking;

use oer_esp32s31_hal::types::MacInterface;

use oer_esp32s31_ieee80211::ampdu_tx::{
    AmpduTxRoleAdapter, HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError,
};

use oer_esp32s31_ieee80211_mac::tx::{
    HtRate, LegacyTxQueue, TxCookie,
    ampdu::{
        AmpduFrameLayout, AmpduFrameSize, AmpduRepublication, HtAmpduFrameRequest, HtAmpduHardware,
        HtAmpduTxError, HtAmpduTxResources, HtBlockAckObservation, RetainedAmpduDmaStorage,
        RetainedAmpduRetryCompletionError, RetainedDmaAmpduTx, TX_AMPDU_METADATA_SIZE,
    },
    runtime::{
        AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
        VENDOR_AMPDU_MSDU_LIFETIME_MICROS,
    },
};

use oer_ieee80211_ap::ApAssociationIdentity;

use oer_ieee80211_softmac::MacTxWork;

use oer_esp32s31_ieee80211::tx::WifiTxProgress;

mod budget;

pub use budget::ApAmpduBudget;

/// The TX Block Ack agreement one client aggregate was built under.
///
/// Its retries stay aggregated only while this exact agreement is
/// operational; a DELBA, a renegotiation or a new association ends it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAggregateAgreement {
    pub association: ApAssociationIdentity,
    pub block_ack_generation: u32,
}

/// AP peer decision captured before consuming a network lease into an
/// aggregate transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAggregateAdmission {
    binding: ApAggregateBinding,
    association: ApAssociationIdentity,
    rate: HtRate,
    block_ack_window: u16,
    block_ack_generation: u32,
    amsdu: bool,
}

impl ApAggregateAdmission {
    pub(crate) const fn new(
        binding: ApAggregateBinding,
        association: ApAssociationIdentity,
        rate: HtRate,
        block_ack_window: u16,
        block_ack_generation: u32,
        amsdu: bool,
    ) -> Self {
        Self {
            binding,
            association,
            rate,
            block_ack_window,
            block_ack_generation,
            amsdu,
        }
    }

    pub const fn agreement(self) -> ApAggregateAgreement {
        ApAggregateAgreement {
            association: self.association,
            block_ack_generation: self.block_ack_generation,
        }
    }

    pub const fn peer(self) -> [u8; 6] {
        self.binding.peer()
    }

    pub const fn association(self) -> ApAssociationIdentity {
        self.association
    }

    pub const fn binding(self) -> ApAggregateBinding {
        self.binding
    }

    pub const fn rate(self) -> HtRate {
        self.rate
    }

    /// Whether the exact operational TID-0 agreement echoed A-MSDU support.
    pub const fn amsdu(self) -> bool {
        self.amsdu
    }

    pub fn accepts_ethernet(self, ethernet: &[u8]) -> bool {
        ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
            == Some(self.peer())
    }

    pub fn bind_policy(
        self,
        hardware_key_selector: u8,
        arena_capacity: usize,
    ) -> Result<HtAmpduTxRolePolicy, HtAmpduTxRolePolicyError> {
        HtAmpduTxRolePolicy::new(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector,
            },
            self.rate,
            self.block_ack_window,
            u8::try_from(arena_capacity).unwrap_or(u8::MAX),
            arena_capacity,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAmpduError {
    Busy,
    Idle,
    PeerChanged,
    KeyChanged,
    SequenceDiscontinuity,
    TooFewFrames,
    Geometry,
    HardwareDidNotDetach,
    DeadlineOverflow,
    RadioClockUnavailable,
    ConflictingInterruptEvents(u32),
    CompletionInterruptWithoutState,
    NothingToUnaggregate,
    Ordinary(ApTxError),
    Hardware(HtAmpduTxError),
    Retry(AmpduRetryError),
    RolePolicy(HtAmpduTxRolePolicyError),
}

impl From<HtAmpduTxError> for ApAmpduError {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Hardware(error)
    }
}

impl From<AmpduRetryError> for ApAmpduError {
    fn from(error: AmpduRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<RetainedAmpduRetryCompletionError> for ApAmpduError {
    fn from(error: RetainedAmpduRetryCompletionError) -> Self {
        match error {
            RetainedAmpduRetryCompletionError::Hardware(error) => Self::Hardware(error),
            RetainedAmpduRetryCompletionError::Retry(error) => Self::Retry(error),
        }
    }
}

impl From<ApTxError> for ApAmpduError {
    fn from(error: ApTxError) -> Self {
        Self::Ordinary(error)
    }
}

impl From<HtAmpduTxRolePolicyError> for ApAmpduError {
    fn from(error: HtAmpduTxRolePolicyError) -> Self {
        Self::RolePolicy(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPreparedAmpdu {
    pub peer: [u8; 6],
    pub rate: HtRate,
    pub first_sequence: SequenceNumber,
    pub subframes: u8,
    pub aggregate_length: u16,
    pub hardware_key_selector: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAmpduCompletion {
    pub tx_status: u8,
    pub block_ack_received: bool,
    pub block_ack_control: u8,
    pub first_sequence: SequenceNumber,
    pub starting_sequence: SequenceNumber,
    pub subframes: u8,
    /// Original aggregate positions absent from this completion.
    pub missing_original_indices: u32,
    /// Signed SNR of the response that completed this publication; see
    /// [`TxCompletion::ack_snr_sample`](oer_esp32s31_ieee80211_mac::tx::TxCompletion::ack_snr_sample).
    pub block_ack_snr_db: Option<i8>,
    pub acknowledged: u8,
    pub aggregate_attempts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAmpduProgress {
    Pending,
    Republished(ApAmpduCompletion),
    /// Hardware/BlockAck processing is terminal, while the detached DMA
    /// backing remains retained until the caller performs the explicit
    /// release edge.
    CompletionReady(ApAmpduCompletion),
    /// The aggregate's agreement ended. Its missing MPDUs leave the retained
    /// storage one ordinary transmission at a time through
    /// [`ApAmpduTx::start_next_unaggregated`].
    Unaggregate(ApAmpduCompletion),
    /// The protection exchange failed at every attempt. The aggregate stays
    /// retained while the ordinary owner sends a BlockAckReq for its head;
    /// [`ApAmpduTx::observe_block_ack_request`] resorts it by the answer.
    RequestingBlockAck(ApAmpduCompletion),
}

/// TID of every AP aggregate: the AP negotiates its TX agreement on TID 0.
const AP_AGGREGATE_TID: u8 = 0;

/// A completed aggregate whose DMA storage stays retained for its retries.
struct RetainedAggregate<const SLOTS: usize> {
    cookie: TxCookie,
    rate: HtRate,
    hardware_key_selector: u8,
    agreement: ApAggregateAgreement,
    retry: AmpduRetryState<SLOTS>,
}

enum ApAmpduState<const SLOTS: usize> {
    Idle,
    Building {
        cookie: TxCookie,
        peer: [u8; 6],
        rate: HtRate,
        first_sequence: SequenceNumber,
        next_sequence: SequenceNumber,
        hardware_key_selector: u8,
        agreement: ApAggregateAgreement,
    },
    Hardware {
        cookie: TxCookie,
        rate: HtRate,
        hardware_key_selector: u8,
        agreement: ApAggregateAgreement,
        retry: AmpduRetryState<SLOTS>,
    },
    Completed {
        cookie: TxCookie,
    },
    RequestingBlockAck {
        cookie: TxCookie,
        rate: HtRate,
        hardware_key_selector: u8,
        agreement: ApAggregateAgreement,
        retry: AmpduRetryState<SLOTS>,
    },
    /// Retained aggregate subframe indices not yet copied out as ordinary
    /// MPDUs; the storage is released with the copy of the last one.
    Unaggregating {
        cookie: TxCookie,
        hardware_key_selector: u8,
        remaining: u32,
    },
}

/// One AP publication owner over the same retained-DMA mechanism used by STA.
pub struct ApAmpduTx<'storage, B: 'storage, const SLOTS: usize, const BUFFER_SIZE: usize> {
    inner: RetainedDmaAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>,
    state: ApAmpduState<SLOTS>,
}

impl<'storage, B: StableDmaBacking + 'storage, const SLOTS: usize, const BUFFER_SIZE: usize>
    ApAmpduTx<'storage, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        resources: HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
        retention: &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        maximum_aggregate_bytes: u16,
    ) -> Result<Self, ApAmpduError> {
        if SLOTS < 2 || SLOTS > 32 {
            return Err(ApAmpduError::TooFewFrames);
        }
        let mut inner = RetainedDmaAmpduTx::new(resources, retention);
        inner.configure_max_aggregate_bytes(maximum_aggregate_bytes)?;
        Ok(Self {
            inner,
            state: ApAmpduState::Idle,
        })
    }

    /// Published work retained through terminal release until the next begin.
    pub fn work(&self) -> MacTxWork {
        self.inner.work()
    }

    pub fn begin(
        &mut self,
        peer: [u8; 6],
        rate: HtRate,
        first_sequence: SequenceNumber,
        hardware_key_selector: u8,
        agreement: ApAggregateAgreement,
    ) -> Result<(), ApAmpduError> {
        if !matches!(self.state, ApAmpduState::Idle) {
            return Err(ApAmpduError::Busy);
        }
        let cookie = self.inner.begin()?;
        self.state = ApAmpduState::Building {
            cookie,
            peer,
            rate,
            first_sequence,
            next_sequence: first_sequence,
            hardware_key_selector,
            agreement,
        };
        Ok(())
    }

    /// Agreement of the aggregate hardware currently owns.
    pub fn published_agreement(&self) -> Option<ApAggregateAgreement> {
        match self.state {
            ApAmpduState::Hardware { agreement, .. }
            | ApAmpduState::RequestingBlockAck { agreement, .. } => Some(agreement),
            _ => None,
        }
    }

    /// Whether missing MPDUs of an ended aggregate still wait to be sent.
    pub fn is_unaggregating(&self) -> bool {
        matches!(self.state, ApAmpduState::Unaggregating { .. })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.state, ApAmpduState::Idle)
    }

    pub fn push(
        &mut self,
        peer: [u8; 6],
        backing: B,
        frame: ApAggregateFrame,
    ) -> Result<(), ApAmpduError> {
        let ApAmpduState::Building {
            cookie,
            peer: expected_peer,
            rate,
            next_sequence,
            hardware_key_selector,
            ..
        } = &mut self.state
        else {
            return Err(ApAmpduError::Idle);
        };
        if peer != *expected_peer {
            return Err(ApAmpduError::PeerChanged);
        }
        if frame.hardware_key_selector != *hardware_key_selector {
            return Err(ApAmpduError::KeyChanged);
        }
        if frame.sequence_number != *next_sequence {
            return Err(ApAmpduError::SequenceDiscontinuity);
        }
        let dma_offset = frame
            .encoded
            .offset
            .checked_sub(TX_AMPDU_METADATA_SIZE)
            .ok_or(ApAmpduError::Geometry)?;
        let layout = AmpduFrameLayout::new(
            dma_offset,
            AmpduFrameSize::new(
                frame.encoded.length,
                oer_esp32s31_ieee80211::ordinary_tx::TX_CCMP_MIC_SIZE as u8,
            ),
        )
        .ok_or(ApAmpduError::Geometry)?;
        self.inner
            .commit_ht(*cookie, backing, HtAmpduFrameRequest::new(layout, 0, *rate))?;
        *next_sequence = next_sequence.next();
        Ok(())
    }

    pub fn prepared(&self) -> Result<ApPreparedAmpdu, ApAmpduError> {
        let ApAmpduState::Building {
            cookie,
            peer,
            rate,
            first_sequence,
            hardware_key_selector,
            ..
        } = self.state
        else {
            return Err(ApAmpduError::Idle);
        };
        let aggregate = self.inner.prepared_aggregate(cookie)?;
        if aggregate.subframes < 2 {
            return Err(ApAmpduError::TooFewFrames);
        }
        Ok(ApPreparedAmpdu {
            peer,
            rate,
            first_sequence,
            subframes: aggregate.subframes,
            aggregate_length: aggregate.bytes,
            hardware_key_selector,
        })
    }

    pub fn publish<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H: HtAmpduHardware>(
        &mut self,
        ordinary: &mut ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<ApPreparedAmpdu, ApAmpduError>
    where
        P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
        T: oer_time::Timer,
    {
        let prepared = self.prepared()?;
        let ApAmpduState::Building {
            cookie, agreement, ..
        } = self.state
        else {
            return Err(ApAmpduError::Idle);
        };
        let retry = AmpduRetryState::new(
            prepared.first_sequence,
            prepared.subframes,
            AmpduRetryPolicy {
                lifetime: oer_time::RadioDuration::from_micros(
                    VENDOR_AMPDU_MSDU_LIFETIME_MICROS as u64,
                ),
                retain_single_mpdu: true,
            },
            radio_stamp,
        )?;
        let config = ordinary
            .ht_ampdu_config(
                prepared.rate,
                prepared.aggregate_length,
                prepared.subframes,
                prepared.hardware_key_selector,
            )
            .ok_or(ApAmpduError::Geometry)?;
        self.inner
            .submit(hardware, cookie, LegacyTxQueue::BestEffort, config)?;
        self.state = ApAmpduState::Hardware {
            cookie,
            rate: prepared.rate,
            hardware_key_selector: prepared.hardware_key_selector,
            agreement,
            retry,
        };
        Ok(prepared)
    }

    /// Process one hardware observation without conflating an absent
    /// completion with a retained retry publication.
    ///
    /// `block_ack_operational` states whether the agreement of
    /// [`Self::published_agreement`] is still operational. Once it ended, the
    /// missing MPDUs leave the aggregate as ordinary frames, as the vendor's
    /// `ppResortTxAMPDU` moves them to the ordinary queue when
    /// `trc_isTxAmpduOperational` is false.
    pub fn service_completion<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H: HtAmpduHardware>(
        &mut self,
        ordinary: &mut ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
        block_ack_operational: bool,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<ApAmpduProgress, ApAmpduError>
    where
        P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
        T: oer_time::Timer,
    {
        if let ApAmpduState::Hardware { retry, .. } = &self.state {
            retry.validate_stamp(radio_stamp)?;
        }
        let ApAmpduState::Hardware {
            cookie,
            rate,
            hardware_key_selector,
            agreement,
            mut retry,
        } = core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(ApAmpduError::Idle);
        };
        let Some(observed) = self.inner.observe_retry_completion(
            hardware,
            cookie,
            &mut retry,
            radio_stamp,
            block_ack_operational,
        )?
        else {
            self.state = ApAmpduState::Hardware {
                cookie,
                rate,
                hardware_key_selector,
                agreement,
                retry,
            };
            return Ok(ApAmpduProgress::Pending);
        };
        let completion = observed.completion;
        let current_subframes = observed.subframes;
        let current_first_sequence = observed.first_sequence;
        let decision = observed.decision;
        let observation = ApAmpduCompletion {
            tx_status: completion.tx.status(),
            block_ack_received: completion.block_ack_received,
            block_ack_control: completion.block_ack.control,
            first_sequence: current_first_sequence,
            starting_sequence: completion.block_ack.block_ack.starting_sequence,
            subframes: current_subframes,
            missing_original_indices: retry.missing_original_indices(),
            block_ack_snr_db: completion.tx.ack_snr_sample(),
            acknowledged: retry.acknowledged(),
            aggregate_attempts: retry.aggregate_attempts(),
        };
        self.apply_decision(
            ordinary,
            hardware,
            RetainedAggregate {
                cookie,
                rate,
                hardware_key_selector,
                agreement,
                retry,
            },
            decision,
            observation,
        )
    }

    /// Validate the radio generation before servicing this aggregate's
    /// ordinary BlockAckReq. A failure preserves both owners and the answer
    /// still held by hardware. Use the same stamp when observing that answer.
    pub fn validate_block_ack_request_stamp(
        &self,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<(), ApAmpduError> {
        let ApAmpduState::RequestingBlockAck { retry, .. } = &self.state else {
            return Err(ApAmpduError::Idle);
        };
        retry
            .validate_stamp(radio_stamp)
            .map_err(ApAmpduError::Retry)
    }

    /// Resort the retained aggregate by the answer to its BlockAckReq: the
    /// BlockAck received, or `None` when the request exhausted its retries.
    pub fn observe_block_ack_request<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H>(
        &mut self,
        ordinary: &mut ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
        block_ack: Option<HtBlockAckObservation>,
        block_ack_operational: bool,
        radio_stamp: oer_ieee80211_lower_mac::Ieee80211Stamp,
    ) -> Result<ApAmpduProgress, ApAmpduError>
    where
        P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
        T: oer_time::Timer,
        H: HtAmpduHardware,
    {
        self.validate_block_ack_request_stamp(radio_stamp)?;
        let ApAmpduState::RequestingBlockAck {
            cookie,
            rate,
            hardware_key_selector,
            agreement,
            mut retry,
        } = core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(ApAmpduError::Idle);
        };
        let first_sequence = retry.current_first_sequence();
        let subframes = retry.current_subframes();
        let decision =
            retry.observe_block_ack_request(block_ack, radio_stamp, block_ack_operational)?;
        let observation = ApAmpduCompletion {
            tx_status: 0,
            block_ack_received: block_ack.is_some(),
            block_ack_control: block_ack.map_or(0, |block_ack| block_ack.control),
            first_sequence,
            starting_sequence: block_ack.map_or(first_sequence, |block_ack| {
                block_ack.block_ack.starting_sequence
            }),
            subframes,
            missing_original_indices: retry.missing_original_indices(),
            block_ack_snr_db: None,
            acknowledged: retry.acknowledged(),
            aggregate_attempts: retry.aggregate_attempts(),
        };
        self.apply_decision(
            ordinary,
            hardware,
            RetainedAggregate {
                cookie,
                rate,
                hardware_key_selector,
                agreement,
                retry,
            },
            decision,
            observation,
        )
    }

    /// Act on one retry decision for the retained aggregate.
    fn apply_decision<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H>(
        &mut self,
        ordinary: &mut ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
        aggregate: RetainedAggregate<SLOTS>,
        decision: AmpduRetryDecision,
        observation: ApAmpduCompletion,
    ) -> Result<ApAmpduProgress, ApAmpduError>
    where
        P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
        T: oer_time::Timer,
        H: HtAmpduHardware,
    {
        let RetainedAggregate {
            cookie,
            rate,
            hardware_key_selector,
            agreement,
            retry,
        } = aggregate;
        let republication = match decision {
            AmpduRetryDecision::RetainAggregate { retry_mask } => {
                Some((retry_mask, AmpduRepublication::Retransmission))
            }
            AmpduRetryDecision::RepublishUnchanged { retry_mask } => {
                Some((retry_mask, AmpduRepublication::AfterProtectionFailure))
            }
            AmpduRetryDecision::RequestBlockAck {
                starting_sequence, ..
            } => {
                ordinary.reset_aggregate_contention();
                let (receiver, transmitter) = {
                    let (head, _) = self.inner.completed_frame(cookie, 0)?;
                    (
                        head[4..10].try_into().expect("an MPDU carries Address 1"),
                        head[10..16].try_into().expect("an MPDU carries Address 2"),
                    )
                };
                ordinary.start_block_ack_request(
                    hardware,
                    &encode_block_ack_request(
                        receiver,
                        transmitter,
                        AP_AGGREGATE_TID,
                        starting_sequence,
                    ),
                    WmmAccessCategory::BestEffort,
                )?;
                self.state = ApAmpduState::RequestingBlockAck {
                    cookie,
                    rate,
                    hardware_key_selector,
                    agreement,
                    retry,
                };
                return Ok(ApAmpduProgress::RequestingBlockAck(observation));
            }
            AmpduRetryDecision::Unaggregate { retry_mask } => {
                ordinary.reset_aggregate_contention();
                self.state = ApAmpduState::Unaggregating {
                    cookie,
                    hardware_key_selector,
                    remaining: retry_mask,
                };
                return Ok(ApAmpduProgress::Unaggregate(observation));
            }
            AmpduRetryDecision::Finish { .. } | AmpduRetryDecision::FinishTriggerFlow => None,
        };
        if let Some((retry_mask, republication)) = republication {
            let aggregate = self
                .inner
                .retain_for_ampdu_retry(cookie, retry_mask, republication)?;
            ordinary.record_aggregate_retry_failure();
            let refreshed = ordinary
                .ht_ampdu_config(
                    rate,
                    aggregate.bytes,
                    aggregate.subframes,
                    hardware_key_selector,
                )
                .ok_or(ApAmpduError::Geometry)?;
            self.inner
                .submit(hardware, cookie, LegacyTxQueue::BestEffort, refreshed)?;
            self.state = ApAmpduState::Hardware {
                cookie,
                rate,
                hardware_key_selector,
                agreement,
                retry,
            };
            return Ok(ApAmpduProgress::Republished(observation));
        }
        if decision.missing() == 0 {
            ordinary.record_aggregate_success();
        } else {
            ordinary.reset_aggregate_contention();
        }
        self.state = ApAmpduState::Completed { cookie };
        Ok(ApAmpduProgress::CompletionReady(observation))
    }

    /// Send the next missing MPDU of an ended aggregate as an ordinary frame.
    ///
    /// The MPDU keeps its sequence number and CCMP PN and gains the Retry
    /// bit. The copy of the last one releases the retained aggregate, after
    /// which this owner is idle while the ordinary transmission completes.
    pub fn start_next_unaggregated<P, E, T, const ORDINARY_BUFFER_SIZE: usize, H>(
        &mut self,
        ordinary: &mut ApTx<'_, P, E, T, ORDINARY_BUFFER_SIZE>,
        hardware: &mut H,
    ) -> Result<WifiTxProgress, ApAmpduError>
    where
        P: oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerProfile,
        E: oer_esp32s31_ieee80211::ordinary_tx::WifiTxEntropy,
        T: oer_time::Timer,
        H: oer_esp32s31_ieee80211_mac::tx::TxHardware,
    {
        let ApAmpduState::Unaggregating {
            cookie,
            hardware_key_selector,
            remaining,
        } = self.state
        else {
            return Err(ApAmpduError::NothingToUnaggregate);
        };
        if remaining == 0 {
            return Err(ApAmpduError::NothingToUnaggregate);
        }
        let index = remaining.trailing_zeros() as u8;
        let remaining = remaining & (remaining - 1);
        let progress = {
            let (encoded, hardware_mic_length) = self.inner.completed_frame(cookie, index)?;
            ordinary.start_aggregate_mpdu_retry(
                hardware,
                encoded,
                usize::from(hardware_mic_length),
                hardware_key_selector,
            )?
        };
        if remaining == 0 {
            self.inner.release_completed(cookie)?;
            self.state = ApAmpduState::Idle;
        } else {
            self.state = ApAmpduState::Unaggregating {
                cookie,
                hardware_key_selector,
                remaining,
            };
        }
        Ok(progress)
    }

    /// Release the exact detached terminal batch after the caller has
    /// finished completion classification and observation.
    pub fn release_completed(&mut self) -> Result<(), ApAmpduError> {
        let ApAmpduState::Completed { cookie } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(ApAmpduError::Idle);
        };
        self.inner.release_completed(cookie)?;
        Ok(())
    }

    pub fn cancel_build(&mut self) -> Result<(), ApAmpduError> {
        let ApAmpduState::Building { cookie, .. } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(ApAmpduError::Idle);
        };
        self.inner.cancel(cookie)?;
        Ok(())
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<bool, ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } = self.state else {
            return Err(ApAmpduError::Idle);
        };
        Ok(self.inner.begin_timeout_abort(hardware, cookie)?)
    }

    /// Finish a timeout abort after the caller-owned hardware settle delay.
    pub fn finish_timeout_abort<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } =
            core::mem::replace(&mut self.state, ApAmpduState::Idle)
        else {
            return Err(ApAmpduError::Idle);
        };
        self.inner.finish_timeout_abort(hardware, cookie)?;
        Ok(())
    }

    pub fn abort_collision<H: HtAmpduHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<bool, ApAmpduError> {
        let ApAmpduState::Hardware { cookie, .. } = self.state else {
            return Err(ApAmpduError::Idle);
        };
        if !self.inner.abort_collision(hardware, cookie)? {
            return Ok(false);
        }
        self.state = ApAmpduState::Idle;
        Ok(true)
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(
        self,
    ) -> Result<
        (
            HtAmpduTxResources<'storage, SLOTS, BUFFER_SIZE>,
            &'storage mut RetainedAmpduDmaStorage<B, SLOTS>,
        ),
        Self,
    > {
        if !matches!(self.state, ApAmpduState::Idle) {
            return Err(self);
        }
        let Self { inner, state: _ } = self;
        inner.try_into_resources().map_err(|inner| Self {
            inner,
            state: ApAmpduState::Idle,
        })
    }
}
