//! Owned EDCA contention state for the four ordinary TX queues.
//!
//! The contention window of each queue is the portable
//! `oer_ieee80211_softmac::EdcaContention`; its cold values are the
//! Espressif LMAC defaults of `oer_espressif_ieee80211_policy::lmac`. This
//! module adds what the S31 queue imposes: a ten-bit slot field and the
//! validated AIFSN, ACM and TXOP policy of each queue.

use oer_espressif_ieee80211_policy::lmac;
use oer_ieee80211_mac::extensions::wmm::{WmmAcParameters, WmmParameterSet};
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_ieee80211_softmac::EdcaContention;

use crate::tx::LegacyTxQueue;

/// The S31 ordinary-TX queue stores the selected backoff as a ten-bit slot
/// count, so an exponent above ten cannot be represented without truncation.
pub const MAX_HARDWARE_ECW_EXPONENT: u8 = 10;

/// Invalid contention parameters that cannot be represented by the S31 queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdcaParametersError {
    AifsnOutOfRange(u8),
    MinimumExponentOutOfRange(u8),
    MaximumExponentOutOfRange(u8),
    InvertedExponentRange { minimum: u8, maximum: u8 },
}

/// One validated AIFS/contention-window policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaContentionParameters {
    aifsn: u8,
    minimum_exponent: u8,
    maximum_exponent: u8,
}

/// Complete per-AC policy retained from a negotiated WMM parameter record.
///
/// Contention is independently representable by the ordinary hardware queue.
/// ACM and TXOP remain protocol policy: callers must not infer that installing
/// AIFSN/CW also granted admission or hardware medium ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaAccessPolicy {
    contention: EdcaContentionParameters,
    admission_control_mandatory: bool,
    txop_limit_units_32_us: u16,
}

impl EdcaAccessPolicy {
    pub const fn new(
        contention: EdcaContentionParameters,
        admission_control_mandatory: bool,
        txop_limit_units_32_us: u16,
    ) -> Self {
        Self {
            contention,
            admission_control_mandatory,
            txop_limit_units_32_us,
        }
    }

    pub const fn from_wmm(parameters: WmmAcParameters) -> Result<Self, EdcaParametersError> {
        let contention = match EdcaContentionParameters::from_wmm(parameters) {
            Ok(contention) => contention,
            Err(error) => return Err(error),
        };
        Ok(Self::new(
            contention,
            parameters.admission_control_mandatory,
            parameters.txop_limit_units_32_us,
        ))
    }

    pub const fn contention(self) -> EdcaContentionParameters {
        self.contention
    }

    pub const fn admission_control_mandatory(self) -> bool {
        self.admission_control_mandatory
    }

    pub const fn txop_limit_units_32_us(self) -> u16 {
        self.txop_limit_units_32_us
    }
}

impl EdcaContentionParameters {
    pub const fn new(
        aifsn: u8,
        minimum_exponent: u8,
        maximum_exponent: u8,
    ) -> Result<Self, EdcaParametersError> {
        if aifsn > 0x0f {
            return Err(EdcaParametersError::AifsnOutOfRange(aifsn));
        }
        if minimum_exponent > MAX_HARDWARE_ECW_EXPONENT {
            return Err(EdcaParametersError::MinimumExponentOutOfRange(
                minimum_exponent,
            ));
        }
        if maximum_exponent > MAX_HARDWARE_ECW_EXPONENT {
            return Err(EdcaParametersError::MaximumExponentOutOfRange(
                maximum_exponent,
            ));
        }
        if minimum_exponent > maximum_exponent {
            return Err(EdcaParametersError::InvertedExponentRange {
                minimum: minimum_exponent,
                maximum: maximum_exponent,
            });
        }
        Ok(Self {
            aifsn,
            minimum_exponent,
            maximum_exponent,
        })
    }

    /// Convert a parsed WMM AC Parameter Record without retaining its
    /// independent admission-control or TXOP policy.
    pub const fn from_wmm(parameters: WmmAcParameters) -> Result<Self, EdcaParametersError> {
        Self::new(parameters.aifsn, parameters.ecw_min, parameters.ecw_max)
    }

    pub const fn aifsn(self) -> u8 {
        self.aifsn
    }

    pub const fn minimum_exponent(self) -> u8 {
        self.minimum_exponent
    }

    pub const fn maximum_exponent(self) -> u8 {
        self.maximum_exponent
    }

    /// The LMAC's cold contention of a queue: complete
    /// `libpp[lmac.o]::lmacInit, libpp[lmac.o]::lmacInitAc`, kept as the Espressif
    /// family's policy data.
    const fn vendor_default(queue: LegacyTxQueue) -> Self {
        let defaults = lmac::default_contention(match queue {
            LegacyTxQueue::Voice => WmmAccessCategory::Voice,
            LegacyTxQueue::Video => WmmAccessCategory::Video,
            LegacyTxQueue::BestEffort => WmmAccessCategory::BestEffort,
            LegacyTxQueue::Background => WmmAccessCategory::Background,
        });
        Self {
            aifsn: defaults.aifsn,
            minimum_exponent: defaults.ecw_min,
            maximum_exponent: defaults.ecw_max,
        }
    }
}

/// Runtime contention state for one access category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaBackoffState {
    policy: EdcaAccessPolicy,
    contention: EdcaContention,
}

impl EdcaBackoffState {
    pub const fn new(parameters: EdcaContentionParameters) -> Self {
        Self {
            policy: EdcaAccessPolicy::new(parameters, false, 0),
            contention: EdcaContention::new(
                parameters.minimum_exponent,
                parameters.maximum_exponent,
            ),
        }
    }

    pub const fn parameters(self) -> EdcaContentionParameters {
        self.policy.contention
    }

    pub const fn access_policy(self) -> EdcaAccessPolicy {
        self.policy
    }

    pub const fn current_exponent(self) -> u8 {
        self.contention.cw_exponent()
    }

    /// Install a new parameter set while retaining a still-valid current CW.
    pub fn reconfigure(&mut self, parameters: EdcaContentionParameters) {
        // SOURCE: complete `libpp[lmac.o]::lmacSetAcParam`.
        // It replaces AIFSN/min/max, clamps current down to a lower new max,
        // clamps it up to a higher new min, and otherwise retains it: the
        // portable `EdcaContention::reconfigure`.
        self.policy.contention = parameters;
        self.contention
            .reconfigure(parameters.minimum_exponent, parameters.maximum_exponent);
    }

    /// Install contention, ACM and TXOP as one already validated AC policy.
    pub fn reconfigure_access_policy(&mut self, policy: EdcaAccessPolicy) {
        self.reconfigure(policy.contention);
        self.policy = policy;
    }

    /// Select the hardware slot count from caller-supplied entropy.
    pub const fn select_slot(self, entropy: u32) -> u16 {
        // SOURCE: complete `libpp[lmac.o]::lmacTxFrame`
        // +0x10e..0x12c calls `libpp[hal_mac.o]::hal_random`, masks it with
        // `(1 << current_exponent) - 1`, stores the u16 result at AC+0x06,
        // and passes it to `hal_mac_tx_config_edca`: the portable draw
        // `EdcaContention::draw` over that entropy word.
        (entropy & self.contention.cw() as u32) as u16
    }

    /// Advance the current CW after an attempt that will be retried.
    pub fn record_retry_failure(&mut self) {
        // SOURCE: complete `libpp[lmac.o]::lmacProcessLongRetryFail,libpp[lmac.o]::lmacProcessShortRetryFail`. Both raise
        // AC+0x08 by one while it is below the active maximum.
        self.contention.record_failure();
    }

    /// Reset contention after a successful frame exchange.
    pub fn record_success(&mut self) {
        // SOURCE: complete `libpp[lmac.o]::lmacProcessLongFrameSuccess,libpp[lmac.o]::lmacProcessShortFrameSuccess`. Both
        // copy AC+0x09 (ECWmin) to AC+0x08 (current exponent).
        self.contention.reset();
    }

    /// Reset a terminal exchange before a new MSDU starts.
    pub fn reset_terminal_exchange(&mut self) {
        // SOURCE: the retry-limit branches in complete
        // `libpp[lmac.o]::lmacProcessLongRetryFail,libpp[lmac.o]::lmacProcessShortRetryFail` restore the
        // active minimum before discarding or completing the exchange.
        self.contention.reset();
    }
}

/// Owned contention state for all four ordinary hardware queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdcaQueues {
    queues: [EdcaBackoffState; 4],
}

impl EdcaQueues {
    pub const fn vendor_defaults() -> Self {
        Self {
            queues: [
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Voice,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Video,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::BestEffort,
                )),
                EdcaBackoffState::new(EdcaContentionParameters::vendor_default(
                    LegacyTxQueue::Background,
                )),
            ],
        }
    }

    pub const fn queue(&self, queue: LegacyTxQueue) -> &EdcaBackoffState {
        &self.queues[queue as usize]
    }

    pub const fn access_policy(&self, queue: LegacyTxQueue) -> EdcaAccessPolicy {
        self.queue(queue).access_policy()
    }

    pub fn select_slot(&self, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.queue(queue).select_slot(entropy)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].record_retry_failure();
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].record_success();
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.queues[queue as usize].reset_terminal_exchange();
    }

    /// Atomically validate and install all four WMM AC records.
    pub fn configure_from_wmm(
        &mut self,
        parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        let voice =
            EdcaAccessPolicy::from_wmm(parameters.access_category(WmmAccessCategory::Voice))?;
        let video =
            EdcaAccessPolicy::from_wmm(parameters.access_category(WmmAccessCategory::Video))?;
        let best_effort =
            EdcaAccessPolicy::from_wmm(parameters.access_category(WmmAccessCategory::BestEffort))?;
        let background =
            EdcaAccessPolicy::from_wmm(parameters.access_category(WmmAccessCategory::Background))?;

        self.queues[LegacyTxQueue::Voice as usize].reconfigure_access_policy(voice);
        self.queues[LegacyTxQueue::Video as usize].reconfigure_access_policy(video);
        self.queues[LegacyTxQueue::BestEffort as usize].reconfigure_access_policy(best_effort);
        self.queues[LegacyTxQueue::Background as usize].reconfigure_access_policy(background);
        Ok(())
    }
}

impl Default for EdcaQueues {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}

#[cfg(test)]
mod tests;
