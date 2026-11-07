//! The Wi-Fi MAC local time on a 64-bit timeline.
//!
//! The MAC local-time counter (`WIFI_MAC_LOCAL_TIME`) counts microseconds in
//! 32 bits and wraps every 2³² µs, about 71.6 minutes; receive timestamps
//! are readings of the same counter (`rx::decode_rx_local_timestamp`).
//! Observation on hardware established its unit and that it runs at the rate
//! of the system timer.

/// The 32-bit MAC local time widened to a 64-bit, never-decreasing timeline.
///
/// The timeline remembers its last fresh reading. A raw value is placed at
/// the timeline value nearest that reading, so a reading or a receive stamp
/// lands exactly while less than 2³¹ µs (about 35.8 minutes) lies between
/// it and the last reading: the owner reads the counter at least that
/// often.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacTimeline {
    /// The last fresh raw reading and its place on the timeline.
    last: Option<(u32, u64)>,
}

impl MacTimeline {
    /// A timeline that has not read the counter yet.
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Place the fresh counter reading `raw` and remember it. The timeline
    /// never runs backwards: a reading behind the last one returns the last
    /// place and is not remembered.
    pub fn advance(&mut self, raw: u32) -> u64 {
        let at = match (self.last, self.place(raw)) {
            (None, _) => u64::from(raw),
            (Some((_, last)), Some(at)) if at >= last => at,
            (Some((_, last)), _) => return last,
        };
        self.last = Some((raw, at));
        at
    }

    /// The timeline value of the raw value `raw` (a receive stamp), within
    /// 2³¹ µs before or after the last reading; `None` before the first
    /// reading or before the timeline's start.
    pub const fn place(&self, raw: u32) -> Option<u64> {
        let Some((_, last)) = self.last else {
            return None;
        };
        self.place_near(raw, last)
    }

    /// The timeline value of the raw value `raw` within 2³¹ µs before or
    /// after the timeline value `near`, such as a monotonic projection of
    /// the counter; `None` before the first reading or before the
    /// timeline's start.
    pub const fn place_near(&self, raw: u32, near: u64) -> Option<u64> {
        let Some((last_raw, last)) = self.last else {
            return None;
        };
        // The raw value `near` stands for on the timeline's mapping.
        let near_raw = last_raw.wrapping_add(near.wrapping_sub(last) as u32);
        let distance = raw.wrapping_sub(near_raw) as i32;
        near.checked_add_signed(distance as i64)
    }

    /// Remember the counter reading `raw` at the timeline value `at`, as
    /// placed by its owner ([`Self::place_near`]) or after the counter's
    /// relation to the timeline broke. The timeline still never runs
    /// backwards: `at` below the last place is raised to it.
    pub fn settle(&mut self, raw: u32, at: u64) -> u64 {
        let at = match self.last {
            Some((_, last)) => at.max(last),
            None => at,
        };
        self.last = Some((raw, at));
        at
    }
}

#[cfg(test)]
mod tests {
    use super::MacTimeline;

    #[test]
    fn the_first_reading_starts_the_timeline_at_the_counter() {
        let mut timeline = MacTimeline::new();
        assert_eq!(timeline.place(5), None);
        assert_eq!(timeline.advance(1_000), 1_000);
        assert_eq!(timeline.advance(1_500), 1_500);
    }

    #[test]
    fn readings_carry_across_the_32_bit_wrap() {
        let mut timeline = MacTimeline::new();
        timeline.advance(u32::MAX - 9);
        assert_eq!(timeline.advance(5), (1 << 32) + 5);
        assert_eq!(timeline.advance(1 << 31), (1 << 32) + (1 << 31));
        assert_eq!(timeline.advance(3 << 30), (1 << 32) + (3 << 30));
        assert_eq!(timeline.advance(10), (2 << 32) + 10);
    }

    #[test]
    fn a_stamp_lands_before_or_after_the_last_reading() {
        let mut timeline = MacTimeline::new();
        let last = timeline.advance(u32::MAX - 1_000);
        // A frame stamped just before the reading, and one after the wrap.
        assert_eq!(timeline.place(u32::MAX - 1_400), Some(last - 400));
        assert_eq!(timeline.place(300), Some((1 << 32) + 300));
        // Nothing lies before the timeline's start.
        let mut fresh = MacTimeline::new();
        fresh.advance(5);
        assert_eq!(fresh.place(u32::MAX), None);
    }

    #[test]
    fn a_reading_far_from_the_last_lands_near_its_projection() {
        let mut timeline = MacTimeline::new();
        timeline.advance(100);
        // Three wraps and 500 us later, as a monotonic projection tells.
        let near = 100 + (3 << 32) + 480;
        assert_eq!(timeline.place_near(600, near), Some(100 + (3 << 32) + 500));
        assert_eq!(timeline.place(600), Some(600));
    }

    #[test]
    fn the_timeline_never_runs_backwards() {
        let mut timeline = MacTimeline::new();
        timeline.advance(2_000);
        assert_eq!(timeline.advance(1_990), 2_000);
        assert_eq!(timeline.advance(2_010), 2_010);
    }

    #[test]
    fn a_settle_continues_from_the_given_place() {
        let mut timeline = MacTimeline::new();
        timeline.advance(50_000);
        // The counter restarted near zero; the timeline goes on at 80 000.
        assert_eq!(timeline.settle(7, 80_000), 80_000);
        assert_eq!(timeline.advance(1_007), 81_000);
        assert_eq!(timeline.place(3), Some(79_996));
        // Settling never moves the timeline back.
        assert_eq!(timeline.settle(2_000, 60_000), 81_000);
    }
}
