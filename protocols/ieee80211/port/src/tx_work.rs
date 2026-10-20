//! Submitted work, kept separate from delivery and measured airtime.

use core::num::NonZeroU32;

/// Cumulative work published for one exchange, including selective retries.
///
/// These are software publication facts, not proof that every byte went on air.
/// A publication aborted before transmission still contributes. Programmed
/// contention is not measured waiting time; ACK, protection exchanges and
/// hardware-internal retries are not described here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacTxWork {
    pub publications: u32,
    /// Submitted PSDU bytes, including aggregate delimiters and padding.
    pub psdu_bytes: u32,
    /// Submitted MPDUs, counting each republication of a retained MPDU.
    pub mpdus: u32,
    /// Sum of ceil(PSDU bits / nominal PHY bitrate), in microseconds.
    ///
    /// This deliberately is not PPDU duration or measured airtime: it excludes
    /// PHY preambles, symbol rounding, service/tail bits and MAC overhead.
    /// Rates may themselves be rounded. Unmodelled publications contribute
    /// only to `unestimated_publications`, never an invented duration.
    pub nominal_data_micros: u32,
    pub unestimated_publications: u32,
    /// Sum of modelled PPDU durations, including PHY overhead and symbol rounding.
    /// Aborted publications still count; this is not measured airtime.
    pub ppdu_micros: u32,
    pub unestimated_ppdus: u32,
    /// Sum of programmed AIFSN values, excluding the SIFS component of AIFS.
    pub aifs_slots: u32,
    /// Sum of selected backoff counts, not CW maxima or exponent values.
    pub backoff_slots: u32,
    /// Publications with no programmed-contention evidence.
    pub unreported_contention: u32,
    /// At least one counter saturated; totals are then lower bounds.
    pub saturated: bool,
}

impl MacTxWork {
    pub const fn new() -> Self {
        Self {
            publications: 0,
            psdu_bytes: 0,
            mpdus: 0,
            nominal_data_micros: 0,
            unestimated_publications: 0,
            ppdu_micros: 0,
            unestimated_ppdus: 0,
            aifs_slots: 0,
            backoff_slots: 0,
            unreported_contention: 0,
            saturated: false,
        }
    }

    /// Complete an explicitly uniform per-publication overhead model: PPDU
    /// work plus the caller's assumed response/protection cost on every attempt.
    /// Use only if that same overhead model applies to all publications in this
    /// receipt. This is an estimate, not measured airtime or completion residence.
    /// Unknown PPDU work, saturated counters, overflow and an empty receipt
    /// cannot become a zero-cost scheduling settlement.
    pub fn estimated_exchange_micros(&self, overhead_per_publication: u32) -> Option<NonZeroU32> {
        if self.saturated || self.unestimated_ppdus != 0 || self.publications == 0 {
            return None;
        }
        let overhead = self.publications.checked_mul(overhead_per_publication)?;
        NonZeroU32::new(self.ppdu_micros.checked_add(overhead)?)
    }

    /// Add the work of another exchange that belongs to this one, such as
    /// the ordinary retries of MPDUs taken out of an aggregate.
    pub fn absorb(&mut self, other: MacTxWork) {
        let mut add = |counter: &mut u32, value: u32| {
            let (sum, overflow) = counter.overflowing_add(value);
            *counter = if overflow { u32::MAX } else { sum };
            self.saturated |= overflow;
        };
        add(&mut self.publications, other.publications);
        add(&mut self.psdu_bytes, other.psdu_bytes);
        add(&mut self.mpdus, other.mpdus);
        add(&mut self.nominal_data_micros, other.nominal_data_micros);
        add(
            &mut self.unestimated_publications,
            other.unestimated_publications,
        );
        add(&mut self.ppdu_micros, other.ppdu_micros);
        add(&mut self.unestimated_ppdus, other.unestimated_ppdus);
        add(&mut self.aifs_slots, other.aifs_slots);
        add(&mut self.backoff_slots, other.backoff_slots);
        add(&mut self.unreported_contention, other.unreported_contention);
        self.saturated |= other.saturated;
    }

    /// Record only after hardware publication commits. Preparation failure
    /// must not call this method. Use the geometry of this attempt, not the
    /// original aggregate, after selective retry compaction.
    pub fn record(&mut self, psdu_bytes: u16, mpdus: u8, nominal_kbps: Option<NonZeroU32>) {
        self.record_publication(psdu_bytes, mpdus, nominal_kbps, None, None);
    }

    pub fn record_publication(
        &mut self,
        psdu_bytes: u16,
        mpdus: u8,
        nominal_kbps: Option<NonZeroU32>,
        timing: Option<crate::tx_cost::PpduTiming>,
        contention: Option<crate::tx_cost::TxContention>,
    ) {
        let mut add = |counter: &mut u32, value: u32| {
            let (sum, overflow) = counter.overflowing_add(value);
            *counter = if overflow { u32::MAX } else { sum };
            self.saturated |= overflow;
        };
        match contention {
            Some(contention) => {
                add(&mut self.aifs_slots, u32::from(contention.aifsn));
                add(&mut self.backoff_slots, u32::from(contention.backoff_slots));
            }
            None => add(&mut self.unreported_contention, 1),
        }
        match timing {
            Some(timing) => add(&mut self.ppdu_micros, timing.duration_micros(psdu_bytes)),
            None => add(&mut self.unestimated_ppdus, 1),
        }
        add(&mut self.publications, 1);
        add(&mut self.psdu_bytes, u32::from(psdu_bytes));
        add(&mut self.mpdus, u32::from(mpdus));
        if let Some(rate) = nominal_kbps {
            let micros = (u32::from(psdu_bytes) * 8_000).div_ceil(rate.get());
            add(&mut self.nominal_data_micros, micros);
        } else {
            add(&mut self.unestimated_publications, 1);
        }
    }
}

#[cfg(test)]
mod tests;
