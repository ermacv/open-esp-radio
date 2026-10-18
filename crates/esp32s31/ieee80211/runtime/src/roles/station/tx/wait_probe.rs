//! Bounded observation deadlines for the explicitly intrusive TX wait image.

pub(super) struct Schedule {
    published: oer_time::Instant,
    next: usize,
    armed: bool,
}

impl Default for Schedule {
    /// A schedule with no publication to observe.
    fn default() -> Self {
        Self {
            published: oer_time::Instant::EPOCH,
            next: 0,
            armed: false,
        }
    }
}

impl Schedule {
    const OFFSETS: [u64; 4] = [5_000, 10_000, 20_000, 40_000];

    pub(super) fn new(published: oer_time::Instant) -> Self {
        Self {
            published,
            next: 0,
            armed: true,
        }
    }

    pub(super) fn deadline(&self) -> Option<oer_time::Instant> {
        self.armed.then_some(())?;
        self.published.checked_add(oer_time::Duration::from_micros(
            *Self::OFFSETS.get(self.next)?,
        ))
    }

    /// Return publication age and wake lateness. Missed observation points
    /// are skipped rather than causing a burst of immediately ready polls.
    pub(super) fn sample(&mut self, now: oer_time::Instant) -> Option<(u64, u64)> {
        let deadline = self.deadline()?;
        if now < deadline {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.published).as_micros();
        while Self::OFFSETS
            .get(self.next)
            .is_some_and(|offset| *offset <= elapsed)
        {
            self.next += 1;
        }
        Some((elapsed, now.saturating_duration_since(deadline).as_micros()))
    }
}

#[cfg(test)]
mod tests {
    use super::Schedule;

    #[test]
    fn observations_are_bounded_and_early_wakes_do_not_consume_them() {
        let mut schedule = Schedule::new(oer_time::Instant::from_micros(100));
        assert_eq!(schedule.sample(oer_time::Instant::from_micros(5_099)), None);
        assert_eq!(
            schedule.deadline(),
            Some(oer_time::Instant::from_micros(5_100))
        );
        for offset in [5_000, 10_000, 20_000, 40_000] {
            assert_eq!(
                schedule.sample(oer_time::Instant::from_micros(100 + offset)),
                Some((offset, 0))
            );
        }
        assert_eq!(schedule.deadline(), None);
        assert_eq!(
            schedule.sample(oer_time::Instant::from_micros(100_000)),
            None
        );
    }

    #[test]
    fn late_wake_records_lateness_without_immediate_repolling() {
        let mut schedule = Schedule::new(oer_time::Instant::from_micros(100));
        assert_eq!(
            schedule.sample(oer_time::Instant::from_micros(21_100)),
            Some((21_000, 16_000))
        );
        assert_eq!(
            schedule.deadline(),
            Some(oer_time::Instant::from_micros(40_100))
        );
        assert_eq!(
            schedule.sample(oer_time::Instant::from_micros(21_100)),
            None
        );
        assert_eq!(Schedule::default().deadline(), None);
        assert_eq!(
            Schedule::new(oer_time::Instant::from_micros(u64::MAX)).deadline(),
            None
        );
    }
}
