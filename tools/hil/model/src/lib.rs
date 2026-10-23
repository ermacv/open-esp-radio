//! The HIL scenario model: what a scenario is, independently of
//! how a run executes it.
//!
//! A scenario is a Rust type implementing [`Spec`]: its stable
//! [`ScenarioId`], its one purpose as a marker type ([`Conformance`],
//! [`Interop`], [`Measurement`], [`Investigation`]), the contract it
//! exercises ([`Subject`]), typed parameters, criteria, trace and
//! findings, the [`Resources`] and [`TargetCapabilities`] it needs, an
//! [`Estimate`] derived from its waits, and a pure [`Spec::verdict`] over a
//! recorded trace. Because the verdict is a pure function of the trace and
//! the criteria, a reader can recompute it without a new run; this crate is
//! a `formats` package so that it may.
//!
//! A contract's bounds are its port conformance crate's own type, with a
//! `CEILINGS` constant; a conformance trace starts with the
//! `oer-hil-trace-header` header that carries the backend's actual bounds.
//!
//! Time is never a literal here. A [`Wait`] is a contract ceiling, a typed
//! protocol interval ([`ProtocolQuantity`]) or a span measured on the
//! stand ([`StandProfile`]); arithmetic on it is checked ([`Overflow`]),
//! and a stand span used before its evidence suffices carries a
//! [`Provisional`] label.
#![forbid(unsafe_code)]

pub mod catalog;
pub mod image_class;
pub mod schema;

mod arithmetic;
mod capabilities;
mod digest;
mod estimate;
mod finite;
mod measurement;
mod name;
mod profile;
mod purpose;
pub mod repetitions;
mod resources;
mod spec;
mod tier;
mod verdict;
mod wait;

pub use arithmetic::{Overflow, sum};
pub use capabilities::{Composition, ImageConflict, TargetCapabilities};
pub use digest::{InvalidDigest, Sha256, canonical_json};
pub use estimate::Estimate;
pub use finite::NonFinite;
pub use measurement::{MeasuredValue, Unit};
pub use name::{
    CapabilityKey, ChipId, CompositionName, ContractName, FixtureName, Grammar, InstanceName,
    InvalidName, MeasurementName, Operation, ScenarioId, ScenarioInstance, SpanName,
};
pub use profile::{
    CONFIDENCE, InvalidRunId, MeasuredSpan, ProfileError, ProfileErrors, QUANTILE, RawStandProfile,
    RunId, StandProfile, Support,
};
pub use purpose::{Conformance, Interop, Investigation, Measurement, OpenQuestion, PurposeKind};
pub use resources::{
    BoardCapability, BoardNeed, BoardOffer, BoardRole, LpClockSource, Resources, StandOffer, Unmet,
};
pub use spec::{IdentityError, Instance, Spec, Subject};
pub use tier::Tier;
pub use verdict::{Findings, NoFinding, Verdict};
pub use wait::{
    ConnectionInterval, Factor, InvalidFactor, OutOfRange, ProtocolQuantity, Provisional, Resolved,
    Symbols, TimeUnits, Wait, WaitError,
};

pub mod health;
