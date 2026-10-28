//! The header every HIL conformance trace starts with.
//!
//! A port's conformance crate owns its bounds as a type (`struct Bounds`
//! with named fields and `const CEILINGS: Bounds`) and needs no HIL
//! dependency. The HIL agent sends `(case, header, trace)`, where the
//! header is a [`TraceHeader<Bounds>`]: the backend's actual bounds and
//! the [`ClockDomain`] of the trace's times. The host checks the headers of
//! one run with [`check_run`]. The module is allocation-free, so the
//! firmware uses it as it is, and each type derives the HIL wire schema.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The clock a trace's times are on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Schema, Serialize)]
#[non_exhaustive]
pub enum Clock {
    /// The SoC system timer's monotonic microseconds.
    Systimer,
    /// The host model's virtual monotonic microseconds.
    Model,
}

/// The clock of a trace and its generation. A clock starts a new
/// generation when it restarts, at a boot or a port's restart (
/// decision 6), so times of different generations are never compared
/// directly.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Schema, Serialize)]
pub struct ClockDomain {
    pub clock: Clock,
    pub generation: u32,
}

/// What every conformance trace starts with: the backend's actual bounds,
/// as the port's own type, and the clock of the trace's times.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Schema, Serialize)]
pub struct TraceHeader<B> {
    pub bounds: B,
    pub clock: ClockDomain,
}

/// Whether a case restarts the port, and so may start a new clock
/// generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restart {
    No,
    Yes,
}

/// How the headers of one run disagree. `case` counts the run's cases
/// from zero.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunHeaderError {
    /// The run has no trace at all.
    NoTrace,
    /// A case reports other bounds than the run's first.
    Bounds { case: usize },
    /// A case's times are on another clock.
    Clock { case: usize },
    /// A case that declares no restart is on another generation.
    Generation { case: usize },
    /// A restarting case went back to an older generation.
    GenerationBack { case: usize },
    /// An input iterator has more cases than a case index can represent.
    CaseIndexOverflow,
}

/// Checks the headers of one run's cases, in order: one set of bounds and
/// one clock throughout; the generation stays the same, and may only grow
/// at a case that declares a restart.
pub fn check_run<'a, B: PartialEq + 'a>(
    cases: impl IntoIterator<Item = (&'a TraceHeader<B>, Restart)>,
) -> Result<(), RunHeaderError> {
    check_run_indices(cases, 1..=usize::MAX)
}

fn check_run_indices<'a, B: PartialEq + 'a>(
    cases: impl IntoIterator<Item = (&'a TraceHeader<B>, Restart)>,
    mut indices: core::ops::RangeInclusive<usize>,
) -> Result<(), RunHeaderError> {
    let mut cases = cases.into_iter();
    let Some((first, _)) = cases.next() else {
        return Err(RunHeaderError::NoTrace);
    };
    let mut generation = first.clock.generation;
    for (header, restart) in cases {
        let case = indices.next().ok_or(RunHeaderError::CaseIndexOverflow)?;
        if header.bounds != first.bounds {
            return Err(RunHeaderError::Bounds { case });
        }
        if header.clock.clock != first.clock.clock {
            return Err(RunHeaderError::Clock { case });
        }
        match restart {
            Restart::No if header.clock.generation != generation => {
                return Err(RunHeaderError::Generation { case });
            }
            Restart::Yes if header.clock.generation < generation => {
                return Err(RunHeaderError::GenerationBack { case });
            }
            _ => generation = header.clock.generation,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(bound: u64, clock: Clock, generation: u32) -> TraceHeader<u64> {
        TraceHeader {
            bounds: bound,
            clock: ClockDomain { clock, generation },
        }
    }

    #[test]
    fn an_exhausted_case_index_is_an_error_and_the_last_index_is_valid() {
        let a = header(800, Clock::Model, 3);
        assert_eq!(
            check_run_indices(
                [(&a, Restart::No), (&a, Restart::No)],
                usize::MAX..=usize::MAX
            ),
            Ok(())
        );
        assert_eq!(
            check_run_indices(
                [(&a, Restart::No), (&a, Restart::No), (&a, Restart::No)],
                usize::MAX..=usize::MAX
            ),
            Err(RunHeaderError::CaseIndexOverflow)
        );
        let different = header(900, Clock::Model, 3);
        assert_eq!(
            check_run_indices(
                [(&a, Restart::No), (&different, Restart::No)],
                usize::MAX..=usize::MAX
            ),
            Err(RunHeaderError::Bounds { case: usize::MAX })
        );
    }

    #[test]
    fn a_run_has_one_bound_set_one_clock_and_one_generation() {
        let a = header(800, Clock::Systimer, 3);
        assert_eq!(check_run([(&a, Restart::No), (&a, Restart::No)]), Ok(()));
        assert_eq!(
            check_run([
                (&a, Restart::No),
                (&header(900, Clock::Systimer, 3), Restart::No)
            ]),
            Err(RunHeaderError::Bounds { case: 1 })
        );
        assert_eq!(
            check_run([
                (&a, Restart::No),
                (&header(800, Clock::Model, 3), Restart::No)
            ]),
            Err(RunHeaderError::Clock { case: 1 })
        );
        assert_eq!(
            check_run([
                (&a, Restart::No),
                (&header(800, Clock::Systimer, 4), Restart::No)
            ]),
            Err(RunHeaderError::Generation { case: 1 })
        );
    }

    #[test]
    fn a_restarting_case_may_start_a_new_generation_but_not_an_older_one() {
        let a = header(800, Clock::Systimer, 3);
        let restarted = header(800, Clock::Systimer, 4);
        assert_eq!(
            check_run([
                (&a, Restart::No),
                (&restarted, Restart::Yes),
                (&restarted, Restart::No)
            ]),
            Ok(())
        );
        assert_eq!(check_run([(&a, Restart::No), (&a, Restart::Yes)]), Ok(()));
        assert_eq!(
            check_run([(&restarted, Restart::No), (&a, Restart::Yes)]),
            Err(RunHeaderError::GenerationBack { case: 1 })
        );
        assert_eq!(check_run::<u64>([]), Err(RunHeaderError::NoTrace));
    }

    #[test]
    fn a_header_round_trips_through_postcard() {
        let a = header(800, Clock::Systimer, 3);
        let mut buffer = [0; 16];
        let bytes = postcard::to_slice(&a, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<TraceHeader<u64>>(bytes).unwrap(), a);
    }

    /// The header's schema is the protocol's: its bounds' own schema, then
    /// the clock domain.
    #[test]
    fn the_wire_schema_names_the_header_and_its_parts() {
        use postcard_schema::schema::DataModelType;
        let schema = TraceHeader::<u64>::SCHEMA;
        assert_eq!(schema.name, "TraceHeader");
        let DataModelType::Struct(fields) = schema.ty else {
            panic!("a struct")
        };
        let names: [&str; 2] = [fields[0].name, fields[1].name];
        assert_eq!(names, ["bounds", "clock"]);
        assert_eq!(fields[0].ty, u64::SCHEMA);
        assert_eq!(fields[1].ty, ClockDomain::SCHEMA);
    }
}
