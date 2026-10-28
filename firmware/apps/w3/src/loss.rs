//! Passive observations at the bridge boundaries. Means are computed on the host.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub queue_samples: u32,
    pub queue_sum: u64,
    pub queue_max: u32,
    pub publisher_blocked: u32,
    pub published: u32,
    pub echo_requests: u32,
    pub echo_replies: u32,
}
impl Counters {
    pub const fn new() -> Self {
        Self {
            queue_samples: 0,
            queue_sum: 0,
            queue_max: 0,
            publisher_blocked: 0,
            published: 0,
            echo_requests: 0,
            echo_replies: 0,
        }
    }
    pub fn sample_queue(&mut self, queued: u32) {
        self.queue_samples = self.queue_samples.saturating_add(1);
        self.queue_sum = self.queue_sum.saturating_add(u64::from(queued));
        self.queue_max = self.queue_max.max(queued);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn samples_include_empty_turns_and_saturate() {
        let mut counts = Counters::new();
        for queued in [0, 8, 4] {
            counts.sample_queue(queued);
        }
        assert_eq!(
            (counts.queue_samples, counts.queue_sum, counts.queue_max),
            (3, 12, 8)
        );
        counts.queue_samples = u32::MAX;
        counts.queue_sum = u64::MAX;
        counts.sample_queue(8);
        assert_eq!(
            (counts.queue_samples, counts.queue_sum),
            (u32::MAX, u64::MAX)
        );
    }
}
