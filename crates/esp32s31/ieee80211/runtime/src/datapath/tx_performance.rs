// Counters and snapshots are read only by telemetry images.
#![cfg_attr(not(feature = "tx-phase-telemetry"), allow(dead_code))]

//! Diagnostic-only accounting for ESP32-S31 general-memory to SRAM TX promotion.
//!
//! These counters observe the physical admission boundary owned by Core0.
//! Call sites use them unconditionally. Without `tx-phase-telemetry`, samples
//! are zero and every record is an empty inline function, so production
//! builds read no `mcycle`/`minstret` and retain no counter storage.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPerformanceSample {
    pub cycles: u32,
    pub instructions: u32,
}

impl TxPerformanceSample {
    #[inline(always)]
    pub fn read() -> Self {
        #[cfg(feature = "tx-phase-telemetry")]
        {
            Self {
                cycles: cycle_count(),
                instructions: instruction_count(),
            }
        }
        #[cfg(not(feature = "tx-phase-telemetry"))]
        {
            Self::default()
        }
    }

    #[inline(always)]
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            cycles: self.cycles.wrapping_sub(earlier.cycles),
            instructions: self.instructions.wrapping_sub(earlier.instructions),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxPerformanceSnapshot {
    pub radio_returns: u32,
    pub promotion_attempts: u32,
    pub promotion_successes: u32,
    pub promotion_no_credit: u32,
    pub promotion_bytes: u32,
    pub promotion_cycles: u32,
    pub promotion_instructions: u32,
    pub promotion_credit_cycles: u32,
    pub promotion_credit_instructions: u32,
    pub promotion_destination_claim_cycles: u32,
    pub promotion_destination_claim_instructions: u32,
    pub promotion_copy_cycles: u32,
    pub promotion_copy_instructions: u32,
    pub promotion_publication_cycles: u32,
    pub promotion_publication_instructions: u32,
    pub promotion_source_release_cycles: u32,
    pub promotion_source_release_instructions: u32,
    pub promotion_radio_claim_cycles: u32,
    pub promotion_radio_claim_instructions: u32,
}

impl TxPerformanceSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            radio_returns: self.radio_returns.wrapping_sub(earlier.radio_returns),
            promotion_attempts: self
                .promotion_attempts
                .wrapping_sub(earlier.promotion_attempts),
            promotion_successes: self
                .promotion_successes
                .wrapping_sub(earlier.promotion_successes),
            promotion_no_credit: self
                .promotion_no_credit
                .wrapping_sub(earlier.promotion_no_credit),
            promotion_bytes: self.promotion_bytes.wrapping_sub(earlier.promotion_bytes),
            promotion_cycles: self.promotion_cycles.wrapping_sub(earlier.promotion_cycles),
            promotion_instructions: self
                .promotion_instructions
                .wrapping_sub(earlier.promotion_instructions),
            promotion_credit_cycles: self
                .promotion_credit_cycles
                .wrapping_sub(earlier.promotion_credit_cycles),
            promotion_credit_instructions: self
                .promotion_credit_instructions
                .wrapping_sub(earlier.promotion_credit_instructions),
            promotion_destination_claim_cycles: self
                .promotion_destination_claim_cycles
                .wrapping_sub(earlier.promotion_destination_claim_cycles),
            promotion_destination_claim_instructions: self
                .promotion_destination_claim_instructions
                .wrapping_sub(earlier.promotion_destination_claim_instructions),
            promotion_copy_cycles: self
                .promotion_copy_cycles
                .wrapping_sub(earlier.promotion_copy_cycles),
            promotion_copy_instructions: self
                .promotion_copy_instructions
                .wrapping_sub(earlier.promotion_copy_instructions),
            promotion_publication_cycles: self
                .promotion_publication_cycles
                .wrapping_sub(earlier.promotion_publication_cycles),
            promotion_publication_instructions: self
                .promotion_publication_instructions
                .wrapping_sub(earlier.promotion_publication_instructions),
            promotion_source_release_cycles: self
                .promotion_source_release_cycles
                .wrapping_sub(earlier.promotion_source_release_cycles),
            promotion_source_release_instructions: self
                .promotion_source_release_instructions
                .wrapping_sub(earlier.promotion_source_release_instructions),
            promotion_radio_claim_cycles: self
                .promotion_radio_claim_cycles
                .wrapping_sub(earlier.promotion_radio_claim_cycles),
            promotion_radio_claim_instructions: self
                .promotion_radio_claim_instructions
                .wrapping_sub(earlier.promotion_radio_claim_instructions),
        }
    }

    pub fn promotion_unattributed_cycles(self) -> u32 {
        self.promotion_cycles.wrapping_sub(
            self.promotion_credit_cycles
                .wrapping_add(self.promotion_destination_claim_cycles)
                .wrapping_add(self.promotion_copy_cycles)
                .wrapping_add(self.promotion_publication_cycles)
                .wrapping_add(self.promotion_source_release_cycles)
                .wrapping_add(self.promotion_radio_claim_cycles),
        )
    }

    pub fn promotion_unattributed_instructions(self) -> u32 {
        self.promotion_instructions.wrapping_sub(
            self.promotion_credit_instructions
                .wrapping_add(self.promotion_destination_claim_instructions)
                .wrapping_add(self.promotion_copy_instructions)
                .wrapping_add(self.promotion_publication_instructions)
                .wrapping_add(self.promotion_source_release_instructions)
                .wrapping_add(self.promotion_radio_claim_instructions),
        )
    }
}

pub struct TxPerformanceCounters {
    radio_returns: AtomicU32,
    promotion_attempts: AtomicU32,
    promotion_successes: AtomicU32,
    promotion_no_credit: AtomicU32,
    promotion_bytes: AtomicU32,
    promotion_cycles: AtomicU32,
    promotion_instructions: AtomicU32,
    promotion_credit_cycles: AtomicU32,
    promotion_credit_instructions: AtomicU32,
    promotion_destination_claim_cycles: AtomicU32,
    promotion_destination_claim_instructions: AtomicU32,
    promotion_copy_cycles: AtomicU32,
    promotion_copy_instructions: AtomicU32,
    promotion_publication_cycles: AtomicU32,
    promotion_publication_instructions: AtomicU32,
    promotion_source_release_cycles: AtomicU32,
    promotion_source_release_instructions: AtomicU32,
    promotion_radio_claim_cycles: AtomicU32,
    promotion_radio_claim_instructions: AtomicU32,
}

impl TxPerformanceCounters {
    pub const fn new() -> Self {
        Self {
            radio_returns: AtomicU32::new(0),
            promotion_attempts: AtomicU32::new(0),
            promotion_successes: AtomicU32::new(0),
            promotion_no_credit: AtomicU32::new(0),
            promotion_bytes: AtomicU32::new(0),
            promotion_cycles: AtomicU32::new(0),
            promotion_instructions: AtomicU32::new(0),
            promotion_credit_cycles: AtomicU32::new(0),
            promotion_credit_instructions: AtomicU32::new(0),
            promotion_destination_claim_cycles: AtomicU32::new(0),
            promotion_destination_claim_instructions: AtomicU32::new(0),
            promotion_copy_cycles: AtomicU32::new(0),
            promotion_copy_instructions: AtomicU32::new(0),
            promotion_publication_cycles: AtomicU32::new(0),
            promotion_publication_instructions: AtomicU32::new(0),
            promotion_source_release_cycles: AtomicU32::new(0),
            promotion_source_release_instructions: AtomicU32::new(0),
            promotion_radio_claim_cycles: AtomicU32::new(0),
            promotion_radio_claim_instructions: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    pub(crate) fn record_radio_return(&self) {
        #[cfg(feature = "tx-phase-telemetry")]
        self.radio_returns.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn record_promotion_no_credit(
        &self,
        started: TxPerformanceSample,
        ended: TxPerformanceSample,
    ) {
        #[cfg(not(feature = "tx-phase-telemetry"))]
        let _ = (started, ended);
        #[cfg(feature = "tx-phase-telemetry")]
        self.record_promotion_no_credit_enabled(started, ended);
    }

    #[cfg(feature = "tx-phase-telemetry")]
    #[inline(always)]
    fn record_promotion_no_credit_enabled(
        &self,
        started: TxPerformanceSample,
        ended: TxPerformanceSample,
    ) {
        let total = ended.wrapping_delta_since(started);
        self.promotion_attempts.fetch_add(1, Ordering::Relaxed);
        self.promotion_no_credit.fetch_add(1, Ordering::Relaxed);
        self.promotion_cycles
            .fetch_add(total.cycles, Ordering::Relaxed);
        self.promotion_instructions
            .fetch_add(total.instructions, Ordering::Relaxed);
        self.promotion_credit_cycles
            .fetch_add(total.cycles, Ordering::Relaxed);
        self.promotion_credit_instructions
            .fetch_add(total.instructions, Ordering::Relaxed);
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn record_promotion(
        &self,
        bytes: usize,
        started: TxPerformanceSample,
        credit_acquired: TxPerformanceSample,
        destination_claimed: TxPerformanceSample,
        copy: TxPerformanceSample,
        publication_started: TxPerformanceSample,
        published: TxPerformanceSample,
        source_released: TxPerformanceSample,
        radio_claimed: TxPerformanceSample,
    ) {
        #[cfg(not(feature = "tx-phase-telemetry"))]
        let _ = (
            bytes,
            started,
            credit_acquired,
            destination_claimed,
            copy,
            publication_started,
            published,
            source_released,
            radio_claimed,
        );
        #[cfg(feature = "tx-phase-telemetry")]
        self.record_promotion_enabled(
            bytes,
            started,
            credit_acquired,
            destination_claimed,
            copy,
            publication_started,
            published,
            source_released,
            radio_claimed,
        );
    }

    #[cfg(feature = "tx-phase-telemetry")]
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn record_promotion_enabled(
        &self,
        bytes: usize,
        started: TxPerformanceSample,
        credit_acquired: TxPerformanceSample,
        destination_claimed: TxPerformanceSample,
        copy: TxPerformanceSample,
        publication_started: TxPerformanceSample,
        published: TxPerformanceSample,
        source_released: TxPerformanceSample,
        radio_claimed: TxPerformanceSample,
    ) {
        let total = radio_claimed.wrapping_delta_since(started);
        let credit = credit_acquired.wrapping_delta_since(started);
        let destination_claim = destination_claimed.wrapping_delta_since(credit_acquired);
        let publication_total = published.wrapping_delta_since(publication_started);
        let publication = TxPerformanceSample {
            cycles: publication_total.cycles.wrapping_sub(copy.cycles),
            instructions: publication_total
                .instructions
                .wrapping_sub(copy.instructions),
        };
        let source_release = source_released.wrapping_delta_since(published);
        let radio_claim = radio_claimed.wrapping_delta_since(source_released);

        self.promotion_attempts.fetch_add(1, Ordering::Relaxed);
        self.promotion_successes.fetch_add(1, Ordering::Relaxed);
        self.promotion_bytes
            .fetch_add(u32::try_from(bytes).unwrap_or(u32::MAX), Ordering::Relaxed);
        for (sample, cycles, instructions) in [
            (total, &self.promotion_cycles, &self.promotion_instructions),
            (
                credit,
                &self.promotion_credit_cycles,
                &self.promotion_credit_instructions,
            ),
            (
                destination_claim,
                &self.promotion_destination_claim_cycles,
                &self.promotion_destination_claim_instructions,
            ),
            (
                copy,
                &self.promotion_copy_cycles,
                &self.promotion_copy_instructions,
            ),
            (
                publication,
                &self.promotion_publication_cycles,
                &self.promotion_publication_instructions,
            ),
            (
                source_release,
                &self.promotion_source_release_cycles,
                &self.promotion_source_release_instructions,
            ),
            (
                radio_claim,
                &self.promotion_radio_claim_cycles,
                &self.promotion_radio_claim_instructions,
            ),
        ] {
            cycles.fetch_add(sample.cycles, Ordering::Relaxed);
            instructions.fetch_add(sample.instructions, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> TxPerformanceSnapshot {
        TxPerformanceSnapshot {
            radio_returns: self.radio_returns.load(Ordering::Relaxed),
            promotion_attempts: self.promotion_attempts.load(Ordering::Relaxed),
            promotion_successes: self.promotion_successes.load(Ordering::Relaxed),
            promotion_no_credit: self.promotion_no_credit.load(Ordering::Relaxed),
            promotion_bytes: self.promotion_bytes.load(Ordering::Relaxed),
            promotion_cycles: self.promotion_cycles.load(Ordering::Relaxed),
            promotion_instructions: self.promotion_instructions.load(Ordering::Relaxed),
            promotion_credit_cycles: self.promotion_credit_cycles.load(Ordering::Relaxed),
            promotion_credit_instructions: self
                .promotion_credit_instructions
                .load(Ordering::Relaxed),
            promotion_destination_claim_cycles: self
                .promotion_destination_claim_cycles
                .load(Ordering::Relaxed),
            promotion_destination_claim_instructions: self
                .promotion_destination_claim_instructions
                .load(Ordering::Relaxed),
            promotion_copy_cycles: self.promotion_copy_cycles.load(Ordering::Relaxed),
            promotion_copy_instructions: self.promotion_copy_instructions.load(Ordering::Relaxed),
            promotion_publication_cycles: self.promotion_publication_cycles.load(Ordering::Relaxed),
            promotion_publication_instructions: self
                .promotion_publication_instructions
                .load(Ordering::Relaxed),
            promotion_source_release_cycles: self
                .promotion_source_release_cycles
                .load(Ordering::Relaxed),
            promotion_source_release_instructions: self
                .promotion_source_release_instructions
                .load(Ordering::Relaxed),
            promotion_radio_claim_cycles: self.promotion_radio_claim_cycles.load(Ordering::Relaxed),
            promotion_radio_claim_instructions: self
                .promotion_radio_claim_instructions
                .load(Ordering::Relaxed),
        }
    }
}

impl Default for TxPerformanceCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static TX_PERFORMANCE: TxPerformanceCounters = TxPerformanceCounters::new();

#[cfg(all(feature = "tx-phase-telemetry", target_arch = "riscv32"))]
#[inline(always)]
fn cycle_count() -> u32 {
    riscv::register::mcycle::read() as u32
}

#[cfg(all(feature = "tx-phase-telemetry", not(target_arch = "riscv32")))]
#[inline(always)]
fn cycle_count() -> u32 {
    0
}

#[cfg(all(feature = "tx-phase-telemetry", target_arch = "riscv32"))]
#[inline(always)]
fn instruction_count() -> u32 {
    riscv::register::minstret::read() as u32
}

#[cfg(all(feature = "tx-phase-telemetry", not(target_arch = "riscv32")))]
#[inline(always)]
fn instruction_count() -> u32 {
    0
}

#[cfg(all(test, feature = "tx-phase-telemetry"))]
mod tests;
