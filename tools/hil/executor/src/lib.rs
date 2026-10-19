//! Execution of typed HIL scenarios.
//!
//! A scenario is an [`oer_hil_model::Spec`]; executing it is [`Run<X>`]
//! for an [`Executor`]: [`Model`] around a [`ModelDriver`], or [`Stand`]
//! around a [`StandDriver`] under the [`StandSession`] of a granted stand
//! lease, which the stand executor holds. A conformance scenario is
//! generic over a HIL trait of reports (`(header, trace)` for a case),
//! which two HIL drivers implement: the model agent around the port's
//! harness, which adds the trace header, and the stand's proxy, which asks
//! the target to exercise a case and decodes the `(case, header, trace)` it
//! reports. Both run the same scenario code, and the same pure verdict
//! judges both. The port's conformance crate itself has no HIL dependency.
//!
//! [`execute`] admits a scenario only on an executor whose kind runs its
//! purpose ([`RunsOn`]): the model runs conformance only. An interop
//! scenario on the model does not compile:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::*;
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer { const CONTRACT: ContractName = name!(ContractName, "toy::Peer"); }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   type Finding = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { chip: name!(ChipId, "esp32s31"), composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), p: &StandProfile) -> Result<Estimate, WaitError> { Estimate::from_waits(&[], &[], 0, p) }
//! #   fn verdict((): &(), (): &()) -> Verdict<()> { Verdict::Pass }
//! }
//! impl<X: Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! struct ModelPeer;
//! impl ModelDriver for ModelPeer {}
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Model::new(ModelPeer));
//! ```
//!
//! The same scenario on the stand compiles:
//!
//! ```
//! # use oer_hil_executor::*;
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer { const CONTRACT: ContractName = name!(ContractName, "toy::Peer"); }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   type Finding = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { chip: name!(ChipId, "esp32s31"), composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), p: &StandProfile) -> Result<Estimate, WaitError> { Estimate::from_waits(&[], &[], 0, p) }
//! #   fn verdict((): &(), (): &()) -> Verdict<()> { Verdict::Pass }
//! }
//! impl<X: Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! struct Hostapd;
//! impl StandDriver for Hostapd {}
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Stand::open(StandSession::for_tests(), Hostapd));
//! ```
//!
//! A driver runs as the stand only under the session of a granted lease,
//! which only the arbiter's grant makes; a session cannot be forged:
//!
//! ```compile_fail,E0451
//! # use oer_hil_executor::*;
//! struct ModelPort;
//! impl StandDriver for ModelPort {}
//! let forged = StandSession { _lease: core::marker::PhantomData };
//! let _ = Stand::open(forged, ModelPort);
//! ```
//!
//! Nor can any other executor claim the stand's kind, since only [`Model`]
//! and [`Stand`] are executors:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::*;
//! struct ModelPort;
//! impl ModelDriver for ModelPort {}
//! struct Fake(ModelPort);
//! impl Executor for Fake {
//!     type Kind = OnStand;
//!     type Driver = ModelPort;
//!     fn driver(&mut self) -> &mut ModelPort { &mut self.0 }
//! }
//! ```
//!
//! A driver opened with a session runs as the stand:
//!
//! ```
//! # use oer_hil_executor::*;
//! struct Proxy;
//! impl StandDriver for Proxy {}
//! let _ = Stand::open(StandSession::for_tests(), Proxy);
//! ```
//!
//! `StandSession::for_tests` exists only under the `test-support`
//! feature, which only dev-dependencies enable.
//!
//! The [`Recorder`] keeps what a run measured, a name being a count or a
//! measurement, never both. [`plan`] refuses what the stand lacks, checks a
//! tier against its budget before anything is built or leased, groups it
//! into image segments, adds each instance's boot, derives each segment's
//! lease limit and labels every provisional stand span it used.
//!
//! Its `clippy.toml`, a copy of the HIL scenario crates' group, forbids
//! duration literals and `thread::sleep`: waits come from
//! [`oer_hil_model::Wait`].
#![forbid(unsafe_code)]

mod execute;
mod executor;
pub mod plan;
mod recorder;

pub use execute::{Cause, Failed, Infrastructure, Outcome, Run, execute};
pub use executor::{
    Executed, Executor, ExecutorKind, Model, ModelDriver, OnModel, OnStand, RunsOn, Stand,
    StandDriver, StandSession,
};
pub use recorder::{RecordError, Recorder};
