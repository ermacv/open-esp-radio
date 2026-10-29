//! Bounded diagnostic readiness distribution; no extra hardware reads.
use oer_diagnostics::Note;
const LIMITS: [u64; 17] = [
    1, 2, 3, 4, 6, 8, 10, 15, 20, 30, 50, 100, 200, 500, 1000, 10_000, 100_000,
];
#[derive(Default)]
pub struct Tails {
    bins: [[u32; 6]; 3],
    largest: [u64; 8],
    total: u32,
    expired: u32,
    max_observations: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationError {
    CounterOverflow,
    InvalidBin,
}

impl Tails {
    pub fn observe(
        &mut self,
        elapsed: u64,
        observations: u16,
        ready: bool,
    ) -> Result<(), ObservationError> {
        self.total = self
            .total
            .checked_add(1)
            .ok_or(ObservationError::CounterOverflow)?;
        self.expired = self
            .expired
            .checked_add(u32::from(!ready))
            .ok_or(ObservationError::CounterOverflow)?;
        self.max_observations = self.max_observations.max(observations);
        let bin = LIMITS
            .iter()
            .position(|limit| elapsed <= *limit)
            .unwrap_or(LIMITS.len());
        let count = self
            .bins
            .iter_mut()
            .flatten()
            .nth(bin)
            .ok_or(ObservationError::InvalidBin)?;
        *count = count
            .checked_add(1)
            .ok_or(ObservationError::CounterOverflow)?;
        let [_, _, _, _, _, _, _, last] = &mut self.largest;
        if elapsed > *last {
            *last = elapsed;
            self.largest.sort_unstable_by(|a, b| b.cmp(a));
        }
        Ok(())
    }

    /// One note per six bins, per two largest waits and the totals.
    #[inline(never)]
    pub async fn report(&self, tick: u32, output: &mut impl super::notes::Notes) {
        for (first, &[b0, b1, b2, b3, b4, b5]) in [0, 6, 12].into_iter().zip(&self.bins) {
            output
                .note(&Note::sar_tail_bins(tick, first, b0, b1, b2, b3, b4, b5))
                .await;
        }
        let [l0, l1, l2, l3, l4, l5, l6, l7] = self.largest;
        for (first, [l0, l1]) in [(0, [l0, l1]), (2, [l2, l3]), (4, [l4, l5]), (6, [l6, l7])] {
            output
                .note(&Note::sar_tail_largest(tick, first, l0, l1))
                .await;
        }
        output
            .note(&Note::sar_tail_totals(
                tick,
                self.total,
                self.expired,
                self.max_observations.into(),
            ))
            .await;
    }
}

#[cfg(test)]
mod tests;
