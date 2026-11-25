extern crate std;

use std::{
    collections::hash_map::DefaultHasher,
    format,
    hash::{Hash, Hasher},
};

use super::{
    ClockRelation, Generation, GenerationSource, Projected, ProjectionError, RelationSample, Stamp,
};
use crate::{DomainDuration, DomainInstant, Duration, Instant, Monotonic};

fn hash<T: Hash>(value: T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// A test port's domain.
enum Port {}

/// A second domain, for a relation that does not involve monotonic time.
enum Other {}

/// Generation `jumps` of the radio-clock relation opened in `epoch`.
fn generation(epoch: u32, jumps: u32) -> Generation<Port, Monotonic> {
    let mut source = GenerationSource::new(epoch);
    for _ in 0..jumps {
        source.jump();
    }
    source.current()
}

fn stamp(micros: u64, jumps: u32) -> Stamp<Port, Monotonic> {
    Stamp {
        at: DomainInstant::from_micros(micros),
        generation: generation(0, jumps),
    }
}

fn sample(
    radio: u64,
    monotonic: u64,
    uncertainty: u64,
    jumps: u32,
) -> RelationSample<Port, Monotonic> {
    RelationSample {
        source: DomainInstant::from_micros(radio),
        target: Instant::from_micros(monotonic),
        uncertainty: Duration::from_micros(uncertainty),
        generation: generation(0, jumps),
    }
}

#[test]
fn a_generation_source_starts_in_its_epoch_and_moves_at_each_break() {
    let mut source = GenerationSource::<Port, Monotonic>::new(7);
    let first = source.current();
    assert_eq!((first.epoch(), first.jump()), (7, 0));
    let second = source.jump();
    assert_ne!(first, second);
    assert_eq!((second.epoch(), second.jump()), (7, 1));
    assert_eq!(source.current(), second);
    // Two owners in different epochs never share a generation.
    assert_ne!(GenerationSource::<Port, Monotonic>::new(8).current(), first);
}

#[test]
fn an_identical_relation_converts_exactly_both_ways_and_ignores_the_sample() {
    let relation = ClockRelation::<Port, Monotonic>::IDENTICAL;
    let sample = sample(1, 9, 5, 2);
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(1_234),
            generation(0, 0),
            &sample
        ),
        Ok(Projected {
            at: Instant::from_micros(1_234),
            uncertainty: Duration::ZERO,
        })
    );
    assert_eq!(
        relation.project_back(Instant::from_micros(1_234), &sample),
        Ok(Projected {
            at: stamp(1_234, 2),
            uncertainty: DomainDuration::ZERO,
        })
    );
}

#[test]
fn an_affine_relation_refuses_a_coordinate_of_another_generation() {
    let relation = ClockRelation::<Port, Monotonic>::affine(1);
    let sample = sample(1_000, 9_000, 2, 4);
    assert_eq!(sample.stamp(), stamp(1_000, 4));
    let at = DomainInstant::from_micros(1_500);
    assert_eq!(
        relation.project(at, generation(0, 3), &sample),
        Err(ProjectionError::StaleSample)
    );
    assert_eq!(
        relation.project(at, generation(0, 4), &sample),
        Ok(Projected {
            at: Instant::from_micros(9_500),
            uncertainty: Duration::from_micros(3),
        })
    );
    // A projection back carries the sample's generation.
    assert_eq!(
        relation
            .project_back(Instant::from_micros(9_500), &sample)
            .map(|projected| projected.at),
        Ok(stamp(1_500, 4))
    );
}

#[test]
fn an_affine_relation_projects_from_its_sample_with_a_drift_bound() {
    let relation = ClockRelation::<Port, Monotonic>::affine(20);
    let sample = sample(1_000_000, 5_000_000, 3, 7);
    let current = generation(0, 7);
    // One second after the sample: 20 ppm of it is 20 us.
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(2_000_000),
            current,
            &sample
        ),
        Ok(Projected {
            at: Instant::from_micros(6_000_000),
            uncertainty: Duration::from_micros(23),
        })
    );
    // Before the sample, the distance counts the same way.
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(999_999),
            current,
            &sample
        ),
        Ok(Projected {
            at: Instant::from_micros(4_999_999),
            uncertainty: Duration::from_micros(4),
        })
    );
    // Back into the radio domain, the uncertainty is a radio span.
    assert_eq!(
        relation.project_back(Instant::from_micros(6_000_000), &sample),
        Ok(Projected {
            at: stamp(2_000_000, 7),
            uncertainty: DomainDuration::from_micros(23),
        })
    );
}

#[test]
fn a_projection_outside_either_epoch_is_refused_never_clamped() {
    let relation = ClockRelation::<Port, Monotonic>::affine(20);
    let early = sample(10, 5, 0, 0);
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(0),
            generation(0, 0),
            &early
        ),
        Err(ProjectionError::OutOfRange)
    );
    let late = sample(0, u64::MAX, 0, 0);
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(1),
            generation(0, 0),
            &late
        ),
        Err(ProjectionError::OutOfRange)
    );
    // An uncertainty past the range is refused the same way.
    let wide = sample(0, 0, u64::MAX, 0);
    assert_eq!(
        relation.project(
            DomainInstant::<Port>::from_micros(1_000_000),
            generation(0, 0),
            &wide
        ),
        Err(ProjectionError::OutOfRange)
    );
}

#[test]
fn one_projection_serves_a_relation_between_two_radio_domains() {
    let relation = ClockRelation::<Other, Port>::affine(200);
    let mut source = GenerationSource::<Other, Port>::new(3);
    let sample = RelationSample {
        source: DomainInstant::<Other>::from_micros(1_000_000),
        target: DomainInstant::<Port>::from_micros(10),
        uncertainty: DomainDuration::from_micros(1),
        generation: source.current(),
    };
    let current = source.current();
    assert_eq!(
        relation.project(DomainInstant::from_micros(1_100_000), current, &sample),
        Ok(Projected {
            at: DomainInstant::from_micros(100_010),
            uncertainty: DomainDuration::from_micros(21),
        })
    );
    // After a jump, the sample of the earlier generation is stale.
    let jumped = source.jump();
    assert_eq!(
        relation.project(DomainInstant::from_micros(1_100_000), jumped, &sample),
        Err(ProjectionError::StaleSample)
    );
}

#[test]
fn a_radio_span_converts_only_through_a_relation() {
    let span = DomainDuration::<Port>::from_micros(1_000_000);
    assert_eq!(
        ClockRelation::<Port, Monotonic>::IDENTICAL.project_span(span),
        Ok(Projected {
            at: Duration::from_micros(1_000_000),
            uncertainty: Duration::ZERO,
        })
    );
    let affine = ClockRelation::<Port, Monotonic>::affine(30);
    // 30 ppm over one second, and a part of a microsecond rounds up.
    assert_eq!(
        affine.project_span(span),
        Ok(Projected {
            at: Duration::from_micros(1_000_000),
            uncertainty: Duration::from_micros(30),
        })
    );
    assert_eq!(
        affine
            .project_span(DomainDuration::from_micros(1))
            .map(|projected| projected.uncertainty),
        Ok(Duration::from_micros(1))
    );
}

#[test]
fn a_relation_is_its_rate() {
    let relation = ClockRelation::<Port, Monotonic>::affine(20);
    assert_eq!(relation, ClockRelation::affine(20));
    for other in [ClockRelation::affine(21), ClockRelation::IDENTICAL] {
        assert_ne!(relation, other);
        assert_ne!(hash(relation), hash(other));
    }
    assert_eq!(
        format!("{relation:?}"),
        "ClockRelation(Affine { drift_ppm: 20 })"
    );
}

#[test]
fn a_generation_and_its_source_are_compared_by_epoch_and_jump() {
    let generation = generation(0, 1);
    assert_ne!(hash(generation), hash(self::generation(0, 2)));
    assert_ne!(hash(generation), hash(self::generation(1, 1)));
    assert_eq!(
        format!("{generation:?}"),
        "Generation { epoch: 0, jump: 1 }"
    );
    let source = GenerationSource::<Port, Monotonic>::new(7);
    assert_eq!(source, GenerationSource::new(7));
    assert_ne!(source, GenerationSource::new(8));
    assert_eq!(
        format!("{source:?}"),
        "GenerationSource { current: Generation { epoch: 7, jump: 0 } }"
    );
}

#[test]
fn a_stamp_and_a_sample_are_compared_by_every_field() {
    assert_ne!(stamp(1, 0), stamp(2, 0));
    assert_ne!(stamp(1, 0), stamp(1, 1));
    assert_eq!(
        format!("{:?}", stamp(1, 0)),
        "Stamp { at: DomainInstant(1), generation: Generation { epoch: 0, jump: 0 } }"
    );
    let whole = sample(1, 2, 3, 4);
    assert_eq!(whole, sample(1, 2, 3, 4));
    for other in [
        sample(9, 2, 3, 4),
        sample(1, 9, 3, 4),
        sample(1, 2, 9, 4),
        sample(1, 2, 3, 9),
    ] {
        assert_ne!(whole, other);
    }
    assert_eq!(
        format!("{whole:?}"),
        "RelationSample { source: DomainInstant(1), target: DomainInstant(2), \
         uncertainty: DomainDuration(3), generation: Generation { epoch: 0, jump: 4 } }"
    );
}
