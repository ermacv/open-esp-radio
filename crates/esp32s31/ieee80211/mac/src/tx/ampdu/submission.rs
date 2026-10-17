//! Fallible, side-effect-free HT/HE submission preparation.

use oer_esp32s31_hal::types::{
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTxParameters,
    MacHtTxParameters,
};

use super::{HtAmpduLength, HtAmpduTxError, HtAmpduTxStorage};
use crate::tx::{HeAmpduTxConfig, HtAmpduTxConfig, LegacyTxQueue, TxCookie, TxSlotState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PreparedHeTrigger {
    pub(super) policy: MacHeTbTidLimit,
    pub(super) reservation: MacHeTbLinkReservation,
    pub(super) tid: MacHeTid,
    pub(super) queued_msdu_bytes: u32,
}

pub(super) struct PreparedHtSubmission {
    pub(super) aggregate: HtAmpduLength,
    pub(super) parameters: MacHtTxParameters,
}

pub(super) struct PreparedHeSubmission {
    pub(super) aggregate: HtAmpduLength,
    pub(super) parameters: MacHeTxParameters,
    pub(super) trigger: Option<PreparedHeTrigger>,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    pub(super) fn prepared_ht_submission(
        &self,
        cookie: TxCookie,
        config: HtAmpduTxConfig,
    ) -> Result<PreparedHtSubmission, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        let aggregate = self.calculate_aggregate()?;
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::AggregateConfigurationMismatch);
        }
        Ok(PreparedHtSubmission {
            aggregate,
            parameters: config.pac_parameters(),
        })
    }

    pub(super) fn prepared_he_submission(
        &self,
        cookie: TxCookie,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<PreparedHeSubmission, HtAmpduTxError> {
        if self.state != TxSlotState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if self.count == 0 {
            return Err(HtAmpduTxError::TooFewFrames);
        }
        let aggregate = self.calculate_aggregate()?;
        if config.aggregate_length != aggregate.bytes || config.subframes != aggregate.subframes {
            return Err(HtAmpduTxError::AggregateConfigurationMismatch);
        }
        if !config.valid() {
            return Err(HtAmpduTxError::TxProgramUnavailable {
                format: super::HtAmpduTxFormat::HeAmpdu,
            });
        }
        let trigger = self.prepared_he_trigger(queue, config)?;
        Ok(PreparedHeSubmission {
            aggregate,
            parameters: config.pac_parameters(),
            trigger,
        })
    }

    pub(super) fn prepared_he_trigger(
        &self,
        queue: LegacyTxQueue,
        config: HeAmpduTxConfig,
    ) -> Result<Option<PreparedHeTrigger>, HtAmpduTxError> {
        let Some(trigger) = config.trigger_based() else {
            return Ok(None);
        };
        let count = self.count;
        let reservation =
            MacHeTbLinkReservation::for_queue(trigger.tid_limit(), queue.index(), count).ok_or(
                HtAmpduTxError::InvalidTriggerReservation {
                    queue: queue.index(),
                    subframes: count,
                },
            )?;
        if self.psdu_lengths[..usize::from(count)]
            .iter()
            .any(|length| *length == 0 || *length > 0x3fff)
        {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::InvalidMpduLength,
            ));
        }
        let mut queued_msdu_bytes = 0_u32;
        for msdu_length in &self.msdu_lengths[..usize::from(count)] {
            if *msdu_length == 0 {
                return Err(HtAmpduTxError::TriggerMsduLengthUnavailable);
            }
            queued_msdu_bytes = queued_msdu_bytes
                .checked_add(u32::from(*msdu_length))
                .ok_or(HtAmpduTxError::TriggerBased(
                    MacHeTbProgramError::QueuedBytesTooLarge,
                ))?;
        }
        if queued_msdu_bytes > 0x000f_ffff {
            return Err(HtAmpduTxError::TriggerBased(
                MacHeTbProgramError::QueuedBytesTooLarge,
            ));
        }
        Ok(Some(PreparedHeTrigger {
            policy: trigger.tid_limit(),
            reservation,
            tid: trigger.tid(),
            queued_msdu_bytes,
        }))
    }
}
