//! Finite execution transaction for one composed connected station epoch.
//!
//! Composition roots may choose network services, task placement, observers
//! and fault decorators, but they must not choose a different shutdown order.
//! This module keeps the live runner opaque across its terminal edge, maps the
//! exit while its service observations are still available, then proves IRQ
//! quiescence before returning any reusable owner.

use core::future::Future;

use crate::{
    datapath::{
        DatapathRunner,
        irq::{InterruptEpoch, MacInterruptEpochDrain},
        services::SingleRoleServices,
    },
    roles::station::teardown::{
        ConnectedStaControlTeardown, ConnectedStaGroupSecurity, ConnectedStaRxPark,
        ConnectedStaTeardownFailure, ConnectedStaTeardownSuccess, ConnectedStaTxTeardown,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_mac::{crypto::CcmpKeyHardware, irq::MacInterruptRoute};

use oer_esp32s31_ieee80211_sta::hardware::control::ConnectedControlHardware;

use oer_ieee80211_runtime::await_stack_boundary;

use super::{
    ConnectedEpochQuiesceFailure, ConnectedEpochQuiesced, ConnectedEpochRunnerOwner,
    ConnectedStationExit, StationCommandReceiver, quiesce_esp32s31_connected_epoch,
    run_esp32s31_connected_station_epoch,
};

/// Runner interface consumed by the common connected execution transaction.
///
/// The trait hides network geometry from the transaction without weakening
/// ownership: its only production implementation is the concrete
/// [`DatapathRunner`], and it inherits the consuming shutdown interface.
#[doc(hidden)]
pub trait ConnectedStationRunner<M: RawMutex>: ConnectedEpochRunnerOwner {
    type Error;

    fn run_station_epoch<'a>(
        &'a mut self,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<Output = ConnectedStationExit<Self::Error>> + 'a;

    /// Revoke station RX admission after the finite runner exits but before
    /// its interrupt route is quiesced.
    ///
    /// This is a logical role boundary. It must not begin the outer-MAC
    /// channel-stop transaction: vendor `ic_mac_deinit` pairs that request
    /// synchronously with `ic_mac_init` around an actual PHY retune.
    fn close_station_rx_admission(&mut self);

    /// Observe natural MAC activity drain after station admission is closed.
    /// IRQ and DMA ownership remain live until the last accepted RX/TX work
    /// reaches the idle frontier.
    fn station_rx_frontend_quiescent(&mut self) -> bool;

    /// The clock the drain waits run on.
    type Clock: oer_time::Timer;

    fn clock(&self) -> &Self::Clock;
}

/// Service graph operation required at the connected RX ingress frontier.
#[doc(hidden)]
pub trait ConnectedStationIngress {
    fn close_station_rx_admission(&mut self);
    fn station_rx_frontend_quiescent(&mut self) -> bool;
}

impl<H, R, X, C> ConnectedStationIngress for SingleRoleServices<H, R, X, C>
where
    H: ConnectedControlHardware,
{
    fn close_station_rx_admission(&mut self) {
        self.hardware_mut().disable_station_receive_policy();
    }

    fn station_rx_frontend_quiescent(&mut self) -> bool {
        self.hardware_mut().mac_runtime_active_state() == 0
    }
}

impl<'irq, RM, CM, N, B, RX, C> ConnectedStationRunner<CM> for DatapathRunner<'irq, RM, N, B, RX, C>
where
    C: oer_time::Timer,
    RM: RawMutex,
    CM: RawMutex,
    N: crate::datapath::network::DatapathNetwork,
    B: crate::datapath::DatapathServices<
            N::TxFrame,
            N::PhysicalTxFrame,
            Exit = oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason,
        > + ConnectedStationIngress,
    RX: crate::datapath::network::DatapathNetworkRxSet,
{
    type Error = B::Error;
    type Clock = C;

    fn clock(&self) -> &C {
        DatapathRunner::clock(self)
    }

    fn run_station_epoch<'a>(
        &'a mut self,
        control: &'a mut StationCommandReceiver<'_, CM>,
    ) -> impl Future<Output = ConnectedStationExit<Self::Error>> + 'a {
        run_esp32s31_connected_station_epoch(self, control)
    }

    fn close_station_rx_admission(&mut self) {
        self.services_mut().close_station_rx_admission();
    }

    fn station_rx_frontend_quiescent(&mut self) -> bool {
        self.services_mut().station_rx_frontend_quiescent()
    }
}

/// Observation-only wrapper around the DATAPATH runner future.
///
/// Implementations may count polls or measure residence time, but cannot
/// replace the future's result or gain access to the runner owners. Generic
/// dispatch keeps the ordinary firmware path free of a vtable and permits the
/// compiler to erase [`NoopConnectedRunObserver`] completely.
pub trait ConnectedRunObserver {
    fn observe<'a, F>(&'a mut self, future: F) -> impl Future<Output = F::Output> + 'a
    where
        F: Future + 'a;
}

/// Production observer which adds no work to the DATAPATH runner.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopConnectedRunObserver;

impl ConnectedRunObserver for NoopConnectedRunObserver {
    async fn observe<'a, F>(&'a mut self, future: F) -> F::Output
    where
        F: Future + 'a,
    {
        future.await
    }
}

/// A classified finite runner exit paired with its proved quiescent owners.
pub struct ConnectedEpochStopped<X, I, N, S> {
    pub exit: X,
    pub quiesced: ConnectedEpochQuiesced<I, N, S>,
}

/// Incomplete hardware stop retaining the already-classified runner exit and
/// every owner needed to continue the same quiescence transaction.
pub struct ConnectedRunQuiesceFailure<X, I, C, E> {
    pub exit: X,
    pub error: crate::datapath::irq::MacInterruptEpochQuiesceError<E>,
    interrupt: I,
    runner: C,
}

impl<'runtime, X, R, IM, C>
    ConnectedRunQuiesceFailure<X, InterruptEpoch<'runtime, R, IM>, C, R::Error>
where
    R: MacInterruptRoute,
    IM: RawMutex,
    C: ConnectedEpochRunnerOwner,
{
    /// Retry only the unfinished IRQ shutdown edge. The DATAPATH runner
    /// is never polled a second time and its previously classified exit is
    /// preserved across every failed attempt.
    #[allow(
        clippy::type_complexity,
        reason = "retry returns the exact stopped owners or the complete retry frontier"
    )]
    pub fn retry_quiesce(
        self,
        platform: &R::Platform,
    ) -> Result<
        ConnectedEpochStopped<X, InterruptEpoch<'runtime, R, IM>, C::Network, C::Services>,
        Self,
    > {
        match quiesce_esp32s31_connected_epoch(self.interrupt, platform, self.runner) {
            Ok(quiesced) => Ok(ConnectedEpochStopped {
                exit: self.exit,
                quiesced,
            }),
            Err(ConnectedEpochQuiesceFailure::Interrupt {
                error,
                interrupt,
                runner,
            }) => Err(Self {
                exit: self.exit,
                error,
                interrupt,
                runner,
            }),
        }
    }
}

impl<X, I, N, S> ConnectedEpochStopped<X, I, N, S> {
    /// Remove an observation/fault decorator without releasing any other
    /// stopped owner or separating the classified runner exit.
    pub fn map_services<U>(self, map: impl FnOnce(S) -> U) -> ConnectedEpochStopped<X, I, N, U> {
        ConnectedEpochStopped {
            exit: self.exit,
            quiesced: self.quiesced.map_services(map),
        }
    }
}

/// Successful completion of the complete run, quiesce and driver teardown
/// transaction.
pub struct ConnectedEpochCompleted<X, I, N, D> {
    pub exit: X,
    pub interrupt: I,
    pub interrupt_drain: MacInterruptEpochDrain,
    pub network: N,
    pub driver: D,
}

/// Owner-preserving quarantined frontier after asynchronous quiescence
/// succeeded but control/RX/TX/key teardown did not.
pub struct ConnectedServiceTeardownFailure<X, I, N, E> {
    pub exit: X,
    pub interrupt: I,
    pub interrupt_drain: MacInterruptEpochDrain,
    pub network: N,
    pub error: E,
}

impl<X, I, N, H, R, A, C> ConnectedEpochStopped<X, I, N, SingleRoleServices<H, R, A, C>>
where
    H: CcmpKeyHardware,
    C: ConnectedStaControlTeardown<H, A>,
    R: ConnectedStaRxPark<H>,
    A: ConnectedStaTxTeardown,
{
    /// Complete the mandatory control/RX/TX/key teardown while retaining the
    /// classified runner exit and all reusable owners on success or failure.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_teardown(
        self,
        group_security: ConnectedStaGroupSecurity,
    ) -> Result<
        ConnectedEpochCompleted<
            X,
            I,
            N,
            ConnectedStaTeardownSuccess<
                H,
                R::Parked,
                A::Resources,
                A::Aggregate,
                C::Report,
                C::Released,
            >,
        >,
        ConnectedServiceTeardownFailure<
            X,
            I,
            N,
            ConnectedStaTeardownFailure<H, R, R::Parked, A, C, C::Error, R::Error>,
        >,
    > {
        let Self { exit, quiesced } = self;
        match quiesced.try_teardown(group_security) {
            Ok(teardown) => Ok(ConnectedEpochCompleted {
                exit,
                interrupt: teardown.interrupt,
                interrupt_drain: teardown.interrupt_drain,
                network: teardown.network,
                driver: teardown.driver,
            }),
            Err(failure) => Err(ConnectedServiceTeardownFailure {
                exit,
                interrupt: failure.interrupt,
                interrupt_drain: failure.interrupt_drain,
                network: failure.network,
                error: failure.error,
            }),
        }
    }
}

/// Run one connected station owner and close every asynchronous publication
/// frontier in the mandatory order.
///
/// `classify` executes immediately after the runner returns and before IRQ or
/// task shutdown. It may derive copy-only policy/qualification evidence from
/// `runner`, but cannot move any of its owners. A quiesce failure returns the
/// complete runner through [`ConnectedEpochQuiesceFailure`] and never
/// exposes a partial teardown result.
#[allow(clippy::too_many_arguments)]
pub async fn run_and_quiesce_esp32s31_connected_epoch<'runtime, R, IM, CM, C, O, X>(
    interrupt: InterruptEpoch<'runtime, R, IM>,
    platform: &R::Platform,
    mut runner: C,
    control: &mut StationCommandReceiver<'_, CM>,
    observer: &mut O,
    classify: impl FnOnce(ConnectedStationExit<C::Error>, &C) -> X,
) -> Result<
    ConnectedEpochStopped<X, InterruptEpoch<'runtime, R, IM>, C::Network, C::Services>,
    ConnectedRunQuiesceFailure<X, InterruptEpoch<'runtime, R, IM>, C, R::Error>,
>
where
    R: MacInterruptRoute,
    IM: RawMutex,
    CM: RawMutex,
    C: ConnectedStationRunner<CM>,
    O: ConnectedRunObserver,
{
    // The runner's batching/select future is retained inside this transaction
    // for ownership, but its poll frame is isolated from the quiesce frame.
    // Without this boundary fat LTO combines both unrelated call chains into
    // one 10-KiB CPU stack frame.
    let raw_exit = await_stack_boundary!(observer.observe(runner.run_station_epoch(control)));
    quiesce_completed_esp32s31_connected_epoch(interrupt, platform, runner, raw_exit, classify)
        .await
}

/// Quiesce a connected runner whose finite datapath future has already
/// returned.
///
/// Keeping this terminal transaction separate permits a composition root to
/// execute the long-lived datapath in its own executor task without moving
/// IRQ teardown or reusable owners across that task boundary. The runner is
/// returned only after admission, the MAC frontend and the interrupt route
/// have reached the same ordered stop frontier as the inline transaction.
#[allow(clippy::too_many_arguments)]
pub async fn quiesce_completed_esp32s31_connected_epoch<'runtime, R, IM, CM, C, X>(
    interrupt: InterruptEpoch<'runtime, R, IM>,
    platform: &R::Platform,
    mut runner: C,
    raw_exit: ConnectedStationExit<C::Error>,
    classify: impl FnOnce(ConnectedStationExit<C::Error>, &C) -> X,
) -> Result<
    ConnectedEpochStopped<X, InterruptEpoch<'runtime, R, IM>, C::Network, C::Services>,
    ConnectedRunQuiesceFailure<X, InterruptEpoch<'runtime, R, IM>, C, R::Error>,
>
where
    R: MacInterruptRoute,
    IM: RawMutex,
    CM: RawMutex,
    C: ConnectedStationRunner<CM>,
{
    let exit = classify(raw_exit, &runner);
    runner.close_station_rx_admission();
    // Let already-accepted RX/TX work leave the MAC pipeline before closing
    // its ordinary interrupt route. The physical descriptor walker remains
    // live, but this logical role handoff deliberately does not assert the
    // channel-stop request: that request is legal only inside the paired
    // stop/retune/restart transaction owned by a real channel switch.
    crate::time::wait_for(runner.clock(), crate::time::MAC_STOP_SETTLE).await;
    while !runner.station_rx_frontend_quiescent() {
        crate::time::wait_for(runner.clock(), crate::time::MAC_STOP_POLL).await;
    }
    match quiesce_esp32s31_connected_epoch(interrupt, platform, runner) {
        Ok(quiesced) => Ok(ConnectedEpochStopped { exit, quiesced }),
        Err(ConnectedEpochQuiesceFailure::Interrupt {
            error,
            interrupt,
            runner,
        }) => Err(ConnectedRunQuiesceFailure {
            exit,
            error,
            interrupt,
            runner,
        }),
    }
}

#[cfg(test)]
mod tests;
