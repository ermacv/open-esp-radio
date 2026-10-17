extern crate std;

use super::*;
use oer_time::{Duration, Instant, RadioDuration, RadioInstant};

#[test]
fn the_monotonic_clock_counts_microseconds_of_the_image_clock() {
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.monotonic,
        Some(ClockRelation::IDENTICAL)
    );
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.resolution,
        Duration::from_micros(1)
    );
}

/// A test port's domain.
enum Port {}

/// A second domain, for a relation that does not involve monotonic time.
enum Other {}

/// Generation `jumps` of the radio-clock relation opened in `epoch`.
fn generation(epoch: u32, jumps: u32) -> Generation<RadioInstant<Port>, Instant> {
    let mut source = GenerationSource::new(epoch);
    for _ in 0..jumps {
        source.jump();
    }
    source.current()
}

fn stamp(micros: u64, jumps: u32) -> RadioStamp<Port> {
    RadioStamp {
        at: RadioInstant::from_micros(micros),
        generation: generation(0, jumps),
    }
}

fn sample(radio: u64, monotonic: u64, uncertainty: u64, jumps: u32) -> ClockSample<Port> {
    ClockSample {
        source: RadioInstant::from_micros(radio),
        target: Instant::from_micros(monotonic),
        uncertainty: Duration::from_micros(uncertainty),
        generation: generation(0, jumps),
    }
}

#[test]
fn a_generation_source_starts_in_its_epoch_and_moves_at_each_break() {
    let mut source = GenerationSource::<RadioInstant<Port>, Instant>::new(7);
    let first = source.current();
    assert_eq!((first.epoch(), first.jump()), (7, 0));
    let second = source.jump();
    assert_ne!(first, second);
    assert_eq!((second.epoch(), second.jump()), (7, 1));
    assert_eq!(source.current(), second);
    // Two owners in different epochs never share a generation.
    assert_ne!(
        GenerationSource::<RadioInstant<Port>, Instant>::new(8).current(),
        first
    );
}

#[test]
fn an_identical_relation_converts_exactly_both_ways_and_ignores_the_sample() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::IDENTICAL;
    let sample = sample(1, 9, 5, 2);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1_234),
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
            uncertainty: RadioDuration::ZERO,
        })
    );
}

#[test]
fn an_affine_relation_refuses_a_coordinate_of_another_generation() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(1);
    let sample = sample(1_000, 9_000, 2, 4);
    assert_eq!(sample.stamp(), stamp(1_000, 4));
    let at = RadioInstant::from_micros(1_500);
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
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(20);
    let sample = sample(1_000_000, 5_000_000, 3, 7);
    let current = generation(0, 7);
    // One second after the sample: 20 ppm of it is 20 us.
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(2_000_000),
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
        relation.project(RadioInstant::<Port>::from_micros(999_999), current, &sample),
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
            uncertainty: RadioDuration::from_micros(23),
        })
    );
}

#[test]
fn a_projection_outside_either_epoch_is_refused_never_clamped() {
    let relation = ClockRelation::<RadioInstant<Port>, Instant>::affine(20);
    let early = sample(10, 5, 0, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(0),
            generation(0, 0),
            &early
        ),
        Err(ProjectionError::OutOfRange)
    );
    let late = sample(0, u64::MAX, 0, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1),
            generation(0, 0),
            &late
        ),
        Err(ProjectionError::OutOfRange)
    );
    // An uncertainty past the range is refused the same way.
    let wide = sample(0, 0, u64::MAX, 0);
    assert_eq!(
        relation.project(
            RadioInstant::<Port>::from_micros(1_000_000),
            generation(0, 0),
            &wide
        ),
        Err(ProjectionError::OutOfRange)
    );
}

#[test]
fn one_projection_serves_a_relation_between_two_radio_domains() {
    let relation = ClockRelation::<RadioInstant<Other>, RadioInstant<Port>>::affine(200);
    let mut source = GenerationSource::<RadioInstant<Other>, RadioInstant<Port>>::new(3);
    let sample = RelationSample {
        source: RadioInstant::<Other>::from_micros(1_000_000),
        target: RadioInstant::<Port>::from_micros(10),
        uncertainty: RadioDuration::from_micros(1),
        generation: source.current(),
    };
    let current = source.current();
    assert_eq!(
        relation.project(RadioInstant::from_micros(1_100_000), current, &sample),
        Ok(Projected {
            at: RadioInstant::from_micros(100_010),
            uncertainty: RadioDuration::from_micros(21),
        })
    );
    // After a jump, the sample of the earlier generation is stale.
    let jumped = source.jump();
    assert_eq!(
        relation.project(RadioInstant::from_micros(1_100_000), jumped, &sample),
        Err(ProjectionError::StaleSample)
    );
}

#[test]
fn a_radio_span_converts_only_through_a_relation() {
    let span = RadioDuration::<Port>::from_micros(1_000_000);
    assert_eq!(
        ClockRelation::<RadioInstant<Port>, Instant>::IDENTICAL.project_span(span),
        Ok(Projected {
            at: Duration::from_micros(1_000_000),
            uncertainty: Duration::ZERO,
        })
    );
    let affine = ClockRelation::<RadioInstant<Port>, Instant>::affine(30);
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
            .project_span(RadioDuration::from_micros(1))
            .map(|projected| projected.uncertainty),
        Ok(Duration::from_micros(1))
    );
}

/// The kind of the test's permits.
struct ModelCaps;

/// A first-in, first-out pool of the test, over the permits a mint made.
struct FifoPool<'id> {
    free: core::cell::RefCell<std::vec::Vec<Permit<'id, ModelCaps>>>,
    /// The tickets of the waiting takers, in arrival order.
    line: core::cell::RefCell<std::collections::VecDeque<u32>>,
    next: core::cell::Cell<u32>,
}

impl<'id> PermitPool<'id, ModelCaps> for FifoPool<'id> {
    fn take<'a>(
        &'a self,
        _: &'a mut Seat<'id, ModelCaps>,
    ) -> impl Future<Output = Permit<'id, ModelCaps>> + 'a {
        let mut place = Place {
            line: &self.line,
            ticket: None,
        };
        core::future::poll_fn(move |_| {
            // The whole place moves into the future, so dropping the future
            // drops it; a closure that named only its field would copy that.
            let place = &mut place;
            let mine = *place.ticket.get_or_insert_with(|| {
                let issued = self.next.get();
                self.next.set(issued + 1);
                self.line.borrow_mut().push_back(issued);
                issued
            });
            if self.line.borrow().front() != Some(&mine) {
                return core::task::Poll::Pending;
            }
            match self.free.borrow_mut().pop() {
                Some(permit) => {
                    self.line.borrow_mut().pop_front();
                    place.ticket = None;
                    core::task::Poll::Ready(permit)
                }
                None => core::task::Poll::Pending,
            }
        })
    }

    fn release(&self, permit: Permit<'id, ModelCaps>) {
        self.free.borrow_mut().push(permit);
    }
}

/// A taker's place in a [`FifoPool`]'s line, given up when its future
/// drops before it is served.
struct Place<'p> {
    line: &'p core::cell::RefCell<std::collections::VecDeque<u32>>,
    ticket: Option<u32>,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            self.line.borrow_mut().retain(|waiting| *waiting != ticket);
        }
    }
}

#[test]
fn a_fifo_permit_pool_meets_the_conformance_rule() {
    let PermitTokens {
        supply,
        line,
        mut seats,
    } = Mint::<Permits<ModelCaps, 2, 3>>::new().into_pools();
    let [held, shared] = supply.into_permits();
    assert_eq!(line.places(), 3);
    let pool = FifoPool {
        free: core::cell::RefCell::new(std::vec![shared]),
        line: core::cell::RefCell::default(),
        next: core::cell::Cell::new(0),
    };
    crate::conformance::permits_serve_takers_in_arrival_order(&pool, &mut seats, [held]);
    // The rule handed every permit back and left every seat with its taker.
    assert_eq!(pool.free.borrow().len(), 2);
    assert_eq!(seats.map(|seat| seat.index()), [0, 1, 2]);
}

#[test]
fn a_mint_numbers_a_lines_seats_and_sizes_its_line() {
    let PermitTokens { line, seats, .. } = Mint::<Permits<ModelCaps, 0, 4>>::new().into_pools();
    assert_eq!(line.places(), 4);
    assert_eq!(seats.map(|seat| seat.index()), [0, 1, 2, 3]);
}

#[test]
fn a_declared_layout_issues_each_pools_capacity_without_stored_mint_state() {
    enum Keys {}
    enum Work {}
    type Layout = (Slots<Keys, 256>, Permits<Work, 2, 3>);
    assert_eq!(core::mem::size_of::<Mint<'_, Layout>>(), 0);
    assert_eq!(core::mem::size_of::<PoolBuilder<'_>>(), 0);
    assert_eq!(core::mem::size_of::<Pools<'_, Layout>>(), 261);
    let (keys, work) = Mint::<Layout>::new().into_pools();
    assert_eq!(keys.len(), 256);
    assert_eq!(keys[0].index(), 0);
    assert_eq!(keys[255].index(), 255);
    assert_eq!(
        work.supply.into_permits().map(|permit| permit.index()),
        [0, 1]
    );
    assert_eq!(work.seats.map(|seat| seat.index()), [0, 1, 2]);
    assert_eq!(work.line.places(), 3);
}

#[test]
fn nested_same_kind_paths_add_no_runtime_storage() {
    type Layout = (Slots<(), 1>, (Slots<(), 1>,));
    let ([first], ([second],)) = Mint::<Layout>::new().into_pools();
    assert_eq!(first.index(), 0);
    assert_eq!(second.index(), 0);
    assert_eq!(core::mem::size_of::<Pools<'_, Layout>>(), 2);
    assert_eq!(core::mem::size_of::<Pools<'_, ()>>(), 0);
    let () = Mint::<()>::new().into_pools();
}

#[test]
fn all_256_permit_and_waiter_indices_are_representable() {
    let tokens = Mint::<Permits<(), 256, 256>>::new().into_pools();
    assert_eq!(tokens.line.places(), 256);
    assert_eq!(tokens.supply.into_permits()[255].index(), 255);
    assert_eq!(tokens.seats[255].index(), 255);
    assert_eq!(
        core::mem::size_of::<Pools<'_, Permits<(), 256, 256>>>(),
        512
    );
}
