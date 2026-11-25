//! One relation between two clock domains, and the one projection that
//! carries time across it.
//!
//! A [`ClockRelation`] states how the two clocks
//! advance, a [`RelationSample`] pairs one reading of each in a
//! [`Generation`] of the relation, and [`ClockRelation::project`] and
//! [`ClockRelation::project_back`] carry a coordinate across with its
//! uncertainty as a span of the target domain ([`Projected`]).

use core::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use crate::{DomainDuration, DomainInstant, Duration};

/// How the clocks of two domains advance relative to each other.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RelationRate {
    /// The two clocks are one: a coordinate of either domain names the same
    /// time as the other domain's coordinate with the same microseconds. A
    /// projection needs no sample.
    Identical,
    /// The source clock advances at the target's rate within `drift_ppm`
    /// parts per million, from an offset only a [`RelationSample`] gives. A
    /// caller that reads both clocks together projects between them over a
    /// bounded span, widened by the drift.
    Affine {
        /// The largest rate difference, in parts per million.
        drift_ppm: u32,
    },
}

/// The relation of the clock of the source domain `A` to the clock of the
/// target domain `B`: its [`RelationRate`], for that pair of domains only.
///
/// A relation of another pair does not project this pair's samples:
///
/// ```compile_fail,E0308
/// use oer_portable_time::{
///     ClockRelation, DomainInstant, Duration, GenerationSource, Instant, RelationSample,
/// };
///
/// enum Radio {}
/// enum Tsf {}
/// let tsf = ClockRelation::<Tsf, Radio>::affine(200);
/// let sample = RelationSample {
///     source: DomainInstant::<Radio>::from_micros(0),
///     target: Instant::from_micros(0),
///     uncertainty: Duration::ZERO,
///     generation: GenerationSource::new(0).current(),
/// };
/// let _ = tsf.project_back(Instant::from_micros(1), &sample);
/// ```
pub struct ClockRelation<A, B> {
    rate: RelationRate,
    domains: PhantomData<fn() -> (A, B)>,
}

impl<A, B> ClockRelation<A, B> {
    /// The two clocks are one ([`RelationRate::Identical`]).
    pub const IDENTICAL: Self = Self::new(RelationRate::Identical);

    /// The source advances at the target's rate within `drift_ppm`
    /// ([`RelationRate::Affine`]).
    pub const fn affine(drift_ppm: u32) -> Self {
        Self::new(RelationRate::Affine { drift_ppm })
    }

    const fn new(rate: RelationRate) -> Self {
        Self {
            rate,
            domains: PhantomData,
        }
    }

    /// How the two clocks advance.
    pub const fn rate(self) -> RelationRate {
        self.rate
    }
}

impl<A, B> Clone for ClockRelation<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, B> Copy for ClockRelation<A, B> {}

impl<A, B> PartialEq for ClockRelation<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.rate == other.rate
    }
}

impl<A, B> Eq for ClockRelation<A, B> {}

impl<A, B> Hash for ClockRelation<A, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rate.hash(state);
    }
}

impl<A, B> fmt::Debug for ClockRelation<A, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ClockRelation")
            .field(&self.rate)
            .finish()
    }
}

/// One stretch of the relation between the domains `A` and `B`: it changes
/// whenever the relation breaks (a clock restarts, sleeps or jumps), so a
/// coordinate converts only with a sample of its own generation.
///
/// A generation has no public constructor: only the relation's owner makes
/// one, through its [`GenerationSource`]. A generation of another relation
/// does not compare with it:
///
/// ```compile_fail,E0308
/// use oer_portable_time::GenerationSource;
///
/// enum Radio {}
/// enum Tsf {}
/// let radio = GenerationSource::<Radio, oer_portable_time::Monotonic>::new(1).current();
/// let tsf = GenerationSource::<Tsf, Radio>::new(1).current();
/// let _ = radio == tsf;
/// ```
pub struct Generation<A, B> {
    epoch: u32,
    jump: u32,
    relation: PhantomData<fn() -> (A, B)>,
}

impl<A, B> Generation<A, B> {
    /// The epoch the relation's owner took when it was created.
    pub const fn epoch(self) -> u32 {
        self.epoch
    }

    /// The breaks of the relation since its owner was created.
    pub const fn jump(self) -> u32 {
        self.jump
    }
}

impl<A, B> Clone for Generation<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, B> Copy for Generation<A, B> {}

impl<A, B> PartialEq for Generation<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch && self.jump == other.jump
    }
}

impl<A, B> Eq for Generation<A, B> {}

impl<A, B> Hash for Generation<A, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.epoch.hash(state);
        self.jump.hash(state);
    }
}

impl<A, B> fmt::Debug for Generation<A, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Generation")
            .field("epoch", &self.epoch)
            .field("jump", &self.jump)
            .finish()
    }
}

/// The owner of a relation's generations: it starts in an epoch the owner
/// takes from a source shared by every owner of one radio start, so two
/// owners never share a generation, and moves to the next generation at
/// each break of the relation. It is neither `Copy` nor `Clone`: a copy
/// would hand out the same generation for two different breaks.
///
/// ```compile_fail,E0599
/// use oer_portable_time::GenerationSource;
///
/// enum Radio {}
/// let source = GenerationSource::<Radio, oer_portable_time::Monotonic>::new(0);
/// let _ = source.clone();
/// ```
pub struct GenerationSource<A, B> {
    current: Generation<A, B>,
}

impl<A, B> GenerationSource<A, B> {
    /// A source whose first generation opens `epoch`.
    pub const fn new(epoch: u32) -> Self {
        Self {
            current: Generation {
                epoch,
                jump: 0,
                relation: PhantomData,
            },
        }
    }

    /// The current generation.
    pub const fn current(&self) -> Generation<A, B> {
        self.current
    }

    /// Break the relation: the next generation of the same epoch, which no
    /// earlier coordinate belongs to. The counter wraps after 2^32 breaks.
    pub fn jump(&mut self) -> Generation<A, B> {
        self.current.jump = self.current.jump.wrapping_add(1);
        self.current
    }
}

impl<A, B> PartialEq for GenerationSource<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.current == other.current
    }
}

impl<A, B> Eq for GenerationSource<A, B> {}

impl<A, B> fmt::Debug for GenerationSource<A, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerationSource")
            .field("current", &self.current)
            .finish()
    }
}

/// A coordinate of domain `A` with the generation of its relation to `B` it
/// was taken in.
pub struct Stamp<A, B> {
    /// The coordinate.
    pub at: DomainInstant<A>,
    /// The relation `at` belongs to.
    pub generation: Generation<A, B>,
}

impl<A, B> Clone for Stamp<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, B> Copy for Stamp<A, B> {}

impl<A, B> PartialEq for Stamp<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.generation == other.generation
    }
}

impl<A, B> Eq for Stamp<A, B> {}

impl<A, B> fmt::Debug for Stamp<A, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Stamp")
            .field("at", &self.at)
            .field("generation", &self.generation)
            .finish()
    }
}

/// One paired reading of the clocks of the domains `A` (the source) and `B`
/// (the target), taken back to back by the owner of their relation, in one
/// of its generations.
pub struct RelationSample<A, B> {
    /// The source clock's reading.
    pub source: DomainInstant<A>,
    /// The target clock's reading beside it.
    pub target: DomainInstant<B>,
    /// How far apart the two readings may lie, on the target clock.
    pub uncertainty: DomainDuration<B>,
    /// The generation of the relation the sample belongs to.
    pub generation: Generation<A, B>,
}

impl<A, B> Clone for RelationSample<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, B> Copy for RelationSample<A, B> {}

impl<A, B> PartialEq for RelationSample<A, B> {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.target == other.target
            && self.uncertainty == other.uncertainty
            && self.generation == other.generation
    }
}

impl<A, B> Eq for RelationSample<A, B> {}

impl<A, B> fmt::Debug for RelationSample<A, B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelationSample")
            .field("source", &self.source)
            .field("target", &self.target)
            .field("uncertainty", &self.uncertainty)
            .field("generation", &self.generation)
            .finish()
    }
}

impl<A, B> RelationSample<A, B> {
    /// The sample's source reading as a stamp of its generation.
    pub const fn stamp(&self) -> Stamp<A, B> {
        Stamp {
            at: self.source,
            generation: self.generation,
        }
    }
}

/// A converted value and how far the true value may lie from it, as a span
/// `S` of the domain it was converted into, monotonic by default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Projected<T, S = Duration> {
    /// The converted value.
    pub at: T,
    /// How far the true value may lie from `at`, either way.
    pub uncertainty: S,
}

/// Why a coordinate has no counterpart across a relation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProjectionError {
    /// The coordinate and the sample belong to different generations: the
    /// relation broke between them.
    StaleSample,
    /// The projected coordinate or its uncertainty lies outside the
    /// representable range.
    OutOfRange,
}

impl<A, B> ClockRelation<A, B> {
    /// The target coordinate of the source coordinate `at` of `generation`,
    /// projected from `sample`.
    ///
    /// Exact for an [`Identical`](RelationRate::Identical) relation, which
    /// ignores the sample. For an [`Affine`](RelationRate::Affine) one, `generation` must be
    /// the sample's ([`ProjectionError::StaleSample`] otherwise); the
    /// distance from the sample is carried over unchanged, and the
    /// uncertainty is the sample's plus the drift over that distance,
    /// rounded up to whole microseconds.
    pub fn project(
        self,
        at: DomainInstant<A>,
        generation: Generation<A, B>,
        sample: &RelationSample<A, B>,
    ) -> Result<Projected<DomainInstant<B>, DomainDuration<B>>, ProjectionError> {
        match self.rate {
            RelationRate::Identical => Ok(Projected {
                at: DomainInstant::from_micros(at.as_micros()),
                uncertainty: DomainDuration::from_micros(0),
            }),
            RelationRate::Affine { drift_ppm } => {
                if generation != sample.generation {
                    return Err(ProjectionError::StaleSample);
                }
                let (at, distance) = shift(
                    sample.target.as_micros(),
                    sample.source.as_micros(),
                    at.as_micros(),
                )?;
                Ok(Projected {
                    at: DomainInstant::from_micros(at),
                    uncertainty: DomainDuration::from_micros(drift_bound(
                        sample.uncertainty.as_micros(),
                        distance,
                        drift_ppm,
                    )?),
                })
            }
        }
    }

    /// The source coordinate of the target coordinate `at`, in the sample's
    /// generation, projected from `sample`; the inverse of
    /// [`Self::project`], with the same uncertainty as a span of the source
    /// domain. The relation carries its bound across at the nominal rate the
    /// drift widens.
    pub fn project_back(
        self,
        at: DomainInstant<B>,
        sample: &RelationSample<A, B>,
    ) -> Result<Projected<Stamp<A, B>, DomainDuration<A>>, ProjectionError> {
        match self.rate {
            RelationRate::Identical => Ok(Projected {
                at: Stamp {
                    at: DomainInstant::from_micros(at.as_micros()),
                    generation: sample.generation,
                },
                uncertainty: DomainDuration::from_micros(0),
            }),
            RelationRate::Affine { drift_ppm } => {
                let (at, distance) = shift(
                    sample.source.as_micros(),
                    sample.target.as_micros(),
                    at.as_micros(),
                )?;
                Ok(Projected {
                    at: Stamp {
                        at: DomainInstant::from_micros(at),
                        generation: sample.generation,
                    },
                    uncertainty: DomainDuration::from_micros(drift_bound(
                        sample.uncertainty.as_micros(),
                        distance,
                        drift_ppm,
                    )?),
                })
            }
        }
    }

    /// The target span of the source span `span`: the same microseconds,
    /// widened by an affine relation's drift over it, rounded up. A span
    /// carries no epoch offset, so it needs no sample.
    pub fn project_span(
        self,
        span: DomainDuration<A>,
    ) -> Result<Projected<DomainDuration<B>, DomainDuration<B>>, ProjectionError> {
        let micros = span.as_micros();
        let uncertainty = match self.rate {
            RelationRate::Identical => 0,
            RelationRate::Affine { drift_ppm } => drift_bound(0, micros, drift_ppm)?,
        };
        Ok(Projected {
            at: DomainDuration::from_micros(micros),
            uncertainty: DomainDuration::from_micros(uncertainty),
        })
    }
}

/// `target_base` moved by `at - source_base`, and that distance.
fn shift(target_base: u64, source_base: u64, at: u64) -> Result<(u64, u64), ProjectionError> {
    let shifted = if at >= source_base {
        target_base.checked_add(at.abs_diff(source_base))
    } else {
        target_base.checked_sub(source_base.abs_diff(at))
    };
    shifted
        .map(|shifted| (shifted, at.abs_diff(source_base)))
        .ok_or(ProjectionError::OutOfRange)
}

/// `uncertainty` plus `drift_ppm` parts per million of `distance`, rounded
/// up, in microseconds.
fn drift_bound(uncertainty: u64, distance: u64, drift_ppm: u32) -> Result<u64, ProjectionError> {
    let product = u128::from(distance)
        .checked_mul(u128::from(drift_ppm))
        .ok_or(ProjectionError::OutOfRange)?;
    let drift = product
        .checked_div(1_000_000)
        .and_then(|whole| whole.checked_add(u128::from(product % 1_000_000 != 0)))
        .ok_or(ProjectionError::OutOfRange)?;
    u64::try_from(drift)
        .ok()
        .and_then(|drift| uncertainty.checked_add(drift))
        .ok_or(ProjectionError::OutOfRange)
}

#[cfg(test)]
mod tests;
