//! Bounded diagnostic readiness distribution; no extra hardware reads.
use oer_diagnostics::Note;
const LIMITS: [u64; 17] = [
    1, 2, 3, 4, 6, 8, 10, 15, 20, 30, 50, 100, 200, 500, 1000, 10_000, 100_000,
];
#[derive(Default)]
pub struct Tails {
    bins: [u32; 18],
    largest: [u64; 8],
    total: u32,
    expired: u32,
    max_observations: u16,
}
impl Tails {
    pub fn observe(&mut self, elapsed: u64, observations: u16, ready: bool) {
        self.total += 1;
        self.expired += u32::from(!ready);
        self.max_observations = self.max_observations.max(observations);
        let bin = LIMITS
            .iter()
            .position(|limit| elapsed <= *limit)
            .unwrap_or(LIMITS.len());
        self.bins[bin] += 1;
        if elapsed > self.largest[7] {
            self.largest[7] = elapsed;
            self.largest.sort_unstable_by(|a, b| b.cmp(a));
        }
    }

    /// One note per six bins, per two largest waits and the totals.
    #[inline(never)]
    pub async fn report(&self, tick: u32, output: &mut impl super::Notes) {
        for (index, bins) in self.bins.chunks_exact(6).enumerate() {
            let [b0, b1, b2, b3, b4, b5] = *bins else {
                continue;
            };
            output
                .note(&Note::sar_tail_bins(
                    tick,
                    index as u32 * 6,
                    b0,
                    b1,
                    b2,
                    b3,
                    b4,
                    b5,
                ))
                .await;
        }
        for (index, largest) in self.largest.chunks_exact(2).enumerate() {
            let [l0, l1] = *largest else {
                continue;
            };
            output
                .note(&Note::sar_tail_largest(tick, index as u32 * 2, l0, l1))
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
