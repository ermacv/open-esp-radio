//! Bounded diagnostic readiness distribution; no extra hardware reads.
const LIMITS: [u64; 17] = [
    1, 2, 3, 4, 6, 8, 10, 15, 20, 30, 50, 100, 200, 500, 1000, 10_000, 100_000,
];
#[derive(Default, Debug)]
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
}
