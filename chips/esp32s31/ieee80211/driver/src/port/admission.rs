//! BufferAdmission starts on P1; only P2 supplies a radio coordinate.

use super::*;
use radio::{ClockRelation, GenerationSource, RelationSample};

#[derive(Clone, Copy)]
pub(super) enum BufferAdmission {
    Vacant,
    Queued(Instant),
    Projected {
        monotonic: Instant,
        radio: Ieee80211Instant,
    },
}

impl BufferAdmission {
    pub(super) fn submit(&mut self, now: Instant) {
        if matches!(self, Self::Vacant) {
            *self = Self::Queued(now);
        }
    }

    pub(super) fn monotonic(self) -> Option<Instant> {
        match self {
            Self::Vacant => None,
            Self::Queued(at) | Self::Projected { monotonic: at, .. } => Some(at),
        }
    }

    pub(super) fn radio(self) -> Option<Ieee80211Instant> {
        match self {
            Self::Projected { radio, .. } => Some(radio),
            Self::Vacant | Self::Queued(_) => None,
        }
    }

    pub(super) fn project(
        &mut self,
        monotonic: Instant,
        radio: Ieee80211Instant,
    ) -> Result<(), Fault> {
        let Self::Queued(at) = *self else {
            return Ok(());
        };
        let sample = RelationSample {
            source: monotonic,
            target: radio,
            uncertainty: oer_time::RadioDuration::ZERO,
            generation: GenerationSource::new(0).current(),
        };
        let projected = ClockRelation::affine(0)
            .project(at, sample.generation, &sample)
            .map_err(|_| Fault::Clock)?;
        *self = Self::Projected {
            monotonic: at,
            radio: projected.at,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_time_virtual::VirtualClock;

    #[test]
    fn dequeue_projection_and_retry_keep_the_submit_age_across_offset_changes() {
        let clock = VirtualClock::<1>::new();
        let submitted = clock.now();
        let mut admission = BufferAdmission::Vacant;
        admission.submit(submitted);
        clock
            .advance(Duration::from_micros(50))
            .expect("virtual time");
        // The P2 sample has an offset different from the one at submit.
        let radio = Ieee80211Instant::from_micros(clock.now().as_micros() + 700);
        admission.project(clock.now(), radio).expect("projection");
        assert_eq!(admission.monotonic(), Some(submitted));
        assert_eq!(
            admission.radio(),
            Some(Ieee80211Instant::from_micros(submitted.as_micros() + 700))
        );
        assert_eq!(
            clock
                .now()
                .saturating_duration_since(admission.monotonic().expect("admitted")),
            Duration::from_micros(50)
        );
        admission.submit(clock.now());
        admission
            .project(clock.now(), Ieee80211Instant::from_micros(1))
            .expect("retry preserves the first projection");
        assert_eq!(admission.monotonic(), Some(submitted));
        assert_eq!(
            admission.radio(),
            Some(Ieee80211Instant::from_micros(submitted.as_micros() + 700))
        );
    }
}
