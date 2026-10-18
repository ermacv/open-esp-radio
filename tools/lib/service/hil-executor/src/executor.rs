//! Executors, the drivers each accepts and which purposes each admits.

use core::marker::PhantomData;

use oer_hil_model::{Conformance, Interop, Investigation, Measurement, OpenQuestion};
use oer_stand_arbiter::Grant;
use serde::Serialize;

/// Which executor ran an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Executed {
    Model,
    Stand,
}

mod sealed {
    pub trait Sealed {}
}

/// The kind of an executor: the host model or the stand.
pub trait ExecutorKind: sealed::Sealed {
    const VALUE: Executed;
}

/// The host model (`ModelAir`, a port's model backend).
pub enum OnModel {}

/// The stand: the target, real peers and the air.
pub enum OnStand {}

impl sealed::Sealed for OnModel {}
impl sealed::Sealed for OnStand {}

impl ExecutorKind for OnModel {
    const VALUE: Executed = Executed::Model;
}

impl ExecutorKind for OnStand {
    const VALUE: Executed = Executed::Stand;
}

/// A purpose an executor kind admits. The model executes conformance
/// only: interop needs real peers, measurements real hardware, and an
/// investigation asks about the stand.
pub trait RunsOn<K: ExecutorKind> {}

impl RunsOn<OnModel> for Conformance {}
impl RunsOn<OnStand> for Conformance {}
impl RunsOn<OnStand> for Interop {}
impl RunsOn<OnStand> for Measurement {}
impl<Q: OpenQuestion> RunsOn<OnStand> for Investigation<Q> {}

/// The proof that a stand executor works under a granted stand lease: it
/// is made only from the arbiter's [`Grant`] and lives no longer than it.
/// The runner opens it; nothing else can mint one.
pub struct StandSession<'g> {
    _lease: PhantomData<&'g Grant>,
}

impl<'g> StandSession<'g> {
    pub fn open(_grant: &'g Grant) -> Self {
        Self {
            _lease: PhantomData,
        }
    }
}

#[cfg(feature = "test-support")]
impl StandSession<'static> {
    /// A session without a lease, for the tests of stand drivers: only the
    /// `test-support` feature, which no runner enables, has it.
    pub fn for_tests() -> Self {
        Self {
            _lease: PhantomData,
        }
    }
}

/// A driver of the host model: a HIL agent around a port's model harness,
/// which adds the trace header. The port's crates never implement it, as
/// they have no HIL dependency.
pub trait ModelDriver {}

/// A driver of the stand, such as the proxy that asks the target to
/// exercise a case. Only [`Stand::open`], which takes the session of a
/// granted lease, runs one as the stand.
pub trait StandDriver {}

/// Something that executes scenarios: its kind, and the driver a scenario
/// uses (for a conformance scenario, the HIL agent around its port's
/// harness, or the stand's proxy). It is sealed:
/// [`Model`] and [`Stand`] are the only executors, so no other type can
/// claim a kind its driver does not prove.
pub trait Executor: sealed::Sealed {
    type Kind: ExecutorKind;
    type Driver;
    fn driver(&mut self) -> &mut Self::Driver;
}

/// The host model executor around a model driver.
pub struct Model<D: ModelDriver>(D);

impl<D: ModelDriver> Model<D> {
    pub const fn new(driver: D) -> Self {
        Self(driver)
    }
}

/// The stand executor: a stand driver under the session of a granted
/// lease, which it holds for as long as it runs.
pub struct Stand<'g, D: StandDriver> {
    _session: StandSession<'g>,
    driver: D,
}

impl<'g, D: StandDriver> Stand<'g, D> {
    pub const fn open(session: StandSession<'g>, driver: D) -> Self {
        Self {
            _session: session,
            driver,
        }
    }
}

impl<D: ModelDriver> sealed::Sealed for Model<D> {}
impl<D: StandDriver> sealed::Sealed for Stand<'_, D> {}

impl<D: ModelDriver> Executor for Model<D> {
    type Kind = OnModel;
    type Driver = D;

    fn driver(&mut self) -> &mut D {
        &mut self.0
    }
}

impl<D: StandDriver> Executor for Stand<'_, D> {
    type Kind = OnStand;
    type Driver = D;

    fn driver(&mut self) -> &mut D {
        &mut self.driver
    }
}
