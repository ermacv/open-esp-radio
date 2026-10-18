#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc station phase enums carry concrete affine owners"
)]
#![expect(
    clippy::manual_async_fn,
    reason = "station composer implementations preserve explicit borrowed Future contracts"
)]
#![expect(
    clippy::type_complexity,
    reason = "phase transitions expose the complete static owner graph without dynamic erasure"
)]

//! Complete phase owner and dispatch shared by the production station composer.
//!
//! The outer STA lifecycle must move one coherent resource graph through the
//! initial join, connected/disconnected scan and reconnected join frontiers.
//! Keeping runtime, target and security beside that phase prevents an example
//! or HIL adapter from rebuilding them independently between attempts.

use core::{future::Future, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_sta::attempt::{StaAttemptSecurity, StaAttemptStation, StaIdentity};

use oer_ieee80211_mac::scan::ScanRecord;

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_sta::{
    request::StationDiscovery,
    station::{
        StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffReason,
        StaFailureDisposition, StaFault, StaLifecycleStage,
    },
};

use super::{StationAttemptRunner, StationCommand, StationCommandReceiver};

/// Hardware/network frontier owned by one outer station lifecycle phase.
///
/// `InitialScan` is the only state without a selected AP. `InitialJoin` is the
/// only selected-peer state that may carry the cold register owner.
/// `RunningScan` contains the complete disconnected epoch returned by a clean
/// connected teardown. `Reconnected` may only be constructed from the exact
/// epoch returned by that running scan.
pub enum StationServicePhase<H, S, R, N, D, E, C> {
    InitialScan {
        hardware: H,
        receive: S,
        network: N,
        identity: StaIdentity,
    },
    InitialJoin {
        hardware: H,
        receive: R,
        network: N,
        station: StaAttemptStation,
    },
    RunningScan {
        disconnected: D,
        station: StaAttemptStation,
    },
    Reconnected {
        epoch: E,
        network: N,
        station: StaAttemptStation,
    },
    Connected {
        connected: C,
    },
}

/// Value-only identity of the currently owned station phase.
///
/// This may be copied into diagnostics without exposing any phase owner or
/// turning driver state into a string protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationServicePhaseKind {
    InitialScan,
    InitialJoin,
    RunningScan,
    Reconnected,
    Connected,
}

/// Complete movable owner for the board-independent station lifecycle.
///
/// Runtime resources and WPA2/sequence state never live beside a phase as
/// reconstructible copies. Candidate identity belongs to the phase which can
/// legally use it. Every normal and failed attempt returns this exact value at
/// the hardware frontier it reached.
pub struct StationServiceOwner<'security, R, P> {
    pub phase: P,
    pub runtime: R,
    pub security: StaAttemptSecurity<'security>,
}

impl<'security, R, P> StationServiceOwner<'security, R, P> {
    pub const fn new(runtime: R, phase: P, security: StaAttemptSecurity<'security>) -> Self {
        Self {
            phase,
            runtime,
            security,
        }
    }

    pub fn into_parts(self) -> (R, P, StaAttemptSecurity<'security>) {
        (self.runtime, self.phase, self.security)
    }
}

/// Exact owner set entering cold candidate selection.
pub struct StationInitialScanPhase<'security, R, H, S, N> {
    runtime: R,
    hardware: H,
    receive: S,
    network: N,
    identity: StaIdentity,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, H, S, N> StationInitialScanPhase<'security, R, H, S, N> {
    fn new(
        runtime: R,
        hardware: H,
        receive: S,
        network: N,
        identity: StaIdentity,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            hardware,
            receive,
            network,
            identity,
            security,
        }
    }

    pub fn into_parts(self) -> (R, H, S, N, StaIdentity, StaAttemptSecurity<'security>) {
        (
            self.runtime,
            self.hardware,
            self.receive,
            self.network,
            self.identity,
            self.security,
        )
    }
}

/// Exact owner set entering the first join transaction.
pub struct StationInitialJoinPhase<'security, R, H, X, N> {
    runtime: R,
    hardware: H,
    receive: X,
    network: N,
    station: StaAttemptStation,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, H, X, N> StationInitialJoinPhase<'security, R, H, X, N> {
    fn new(
        runtime: R,
        hardware: H,
        receive: X,
        network: N,
        station: StaAttemptStation,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            hardware,
            receive,
            network,
            station,
            security,
        }
    }

    pub fn into_parts(self) -> (R, H, X, N, StaAttemptStation, StaAttemptSecurity<'security>) {
        (
            self.runtime,
            self.hardware,
            self.receive,
            self.network,
            self.station,
            self.security,
        )
    }
}

/// Exact owner set entering a disconnected candidate-refresh transaction.
pub struct StationRunningScanPhase<'security, R, D> {
    runtime: R,
    disconnected: D,
    station: StaAttemptStation,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, D> StationRunningScanPhase<'security, R, D> {
    fn new(
        runtime: R,
        disconnected: D,
        station: StaAttemptStation,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            disconnected,
            station,
            security,
        }
    }

    pub fn into_parts(self) -> (R, D, StaAttemptStation, StaAttemptSecurity<'security>) {
        (self.runtime, self.disconnected, self.station, self.security)
    }
}

/// Exact owner set entering a join transaction after a completed rescan.
pub struct StationReconnectedPhase<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    station: StaAttemptStation,
    security: StaAttemptSecurity<'security>,
}

/// Exact owner set entering one connected data-plane epoch.
///
/// Join and connected service are deliberately separate async transactions:
/// a join future returns this frontier, then the outer lifecycle polls a new
/// future. This prevents both protocol state machines and their owner-return
/// temporaries from occupying one live CPU stack frame.
pub struct StationConnectedPhase<'security, R, C> {
    runtime: R,
    connected: C,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, C> StationConnectedPhase<'security, R, C> {
    fn new(runtime: R, connected: C, security: StaAttemptSecurity<'security>) -> Self {
        Self {
            runtime,
            connected,
            security,
        }
    }

    pub fn into_parts(self) -> (R, C, StaAttemptSecurity<'security>) {
        (self.runtime, self.connected, self.security)
    }
}

impl<'security, R, E, N> StationReconnectedPhase<'security, R, E, N> {
    fn new(
        runtime: R,
        epoch: E,
        network: N,
        station: StaAttemptStation,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            epoch,
            network,
            station,
            security,
        }
    }

    pub fn into_parts(self) -> (R, E, N, StaAttemptStation, StaAttemptSecurity<'security>) {
        (
            self.runtime,
            self.epoch,
            self.network,
            self.station,
            self.security,
        )
    }
}

/// Finite result of cold candidate selection.
pub enum StationInitialScanExit<'security, R, H, X, N, O, E, F = core::convert::Infallible> {
    JoinReady(StationInitialJoinPhase<'security, R, H, X, N>),
    Complete(StaAttemptOutcome<O, E, F>),
}

impl<'security, R, H, X, N, O, E, F> StationInitialScanExit<'security, R, H, X, N, O, E, F> {
    pub fn join_ready(
        runtime: R,
        hardware: H,
        receive: X,
        network: N,
        station: StaAttemptStation,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self::JoinReady(StationInitialJoinPhase::new(
            runtime, hardware, receive, network, station, security,
        ))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, E, F>) -> Self {
        Self::Complete(outcome)
    }
}

/// Finite result of a disconnected candidate-refresh transaction.
///
/// A successful scan returns the exact next join frontier instead of invoking
/// Authentication/Association itself. The common engine consumes that value
/// and remains the only component which dispatches the reconnected phase.
pub enum StationRunningScanExit<'security, R, E, N, O, X, F = core::convert::Infallible> {
    JoinReady(StationReconnectedPhase<'security, R, E, N>),
    Complete(StaAttemptOutcome<O, X, F>),
}

/// Finite result of either initial or reconnected join.
///
/// Success returns a connected frontier instead of entering the data-plane
/// loop from the join future. Every other result is already a complete outer
/// lifecycle outcome.
pub enum StationJoinExit<'security, R, C, O, E, F = core::convert::Infallible> {
    ConnectedReady(StationConnectedPhase<'security, R, C>),
    Complete(StaAttemptOutcome<O, E, F>),
}

impl<'security, R, C, O, E, F> StationJoinExit<'security, R, C, O, E, F> {
    pub fn connected_ready(
        runtime: R,
        connected: C,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self::ConnectedReady(StationConnectedPhase::new(runtime, connected, security))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, E, F>) -> Self {
        Self::Complete(outcome)
    }
}

impl<'security, R, E, N, O, X, F> StationRunningScanExit<'security, R, E, N, O, X, F> {
    pub fn join_ready(
        runtime: R,
        epoch: E,
        network: N,
        station: StaAttemptStation,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self::JoinReady(StationReconnectedPhase::new(
            runtime, epoch, network, station, security,
        ))
    }

    pub const fn complete(outcome: StaAttemptOutcome<O, X, F>) -> Self {
        Self::Complete(outcome)
    }
}

/// Value-only result after a disconnected scan has returned every owner.
///
/// Scan execution and diagnostics remain target policy, but the transition
/// into the reconnected join frontier is shared by every production consumer.
pub enum StationRunningScanCompletion<E> {
    Selected(ScanRecord),
    Stopped,
    Failed {
        disposition: StaFailureDisposition,
        error: E,
    },
}

/// Complete candidate refresh without allowing a composition root to invent
/// a different outer station transition.
///
/// `prepare_reconnect` consumes the exact disconnected epoch only after a real
/// candidate was selected. All other exits reconstruct the same running-scan
/// owner; no PAC, RX or network capability can be synthesized here.
pub fn complete_esp32s31_station_running_scan<'security, R, D, E, N, O, X, F>(
    runtime: R,
    disconnected: D,
    mut station: StaAttemptStation,
    security: StaAttemptSecurity<'security>,
    completion: StationRunningScanCompletion<X>,
    prepare_reconnect: impl FnOnce(D) -> (N, E),
    restore_owner: impl FnOnce(R, D, StaAttemptStation, StaAttemptSecurity) -> O,
) -> StationRunningScanExit<'security, R, E, N, O, X, F> {
    match completion {
        StationRunningScanCompletion::Selected(candidate) => {
            station.access_point = candidate;
            let (network, epoch) = prepare_reconnect(disconnected);
            StationRunningScanExit::join_ready(runtime, epoch, network, station, security)
        }
        StationRunningScanCompletion::Stopped => {
            StationRunningScanExit::complete(StaAttemptOutcome::Stopped {
                owner: restore_owner(runtime, disconnected, station, security),
            })
        }
        StationRunningScanCompletion::Failed { disposition, error } => {
            StationRunningScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, disconnected, station, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    error,
                ),
            })
        }
    }
}

/// Hardware-owning phase transactions required by the common station engine.
///
/// Implementations provide board/network bindings only. Observability is a
/// separate engine concern, so HIL telemetry cannot alter which hardware
/// transaction is selected. A port must return the exact owner frontier it
/// received after every finite transaction.
pub trait StationEnginePort<'security, M: RawMutex> {
    type Runtime;
    type InitialHardware;
    type InitialScanRx;
    type RxFrontier;
    type Network;
    type Disconnected;
    type Reconnected;
    type Connected;
    type Error;
    /// Exact non-reusable owner returned when a phase cannot prove the normal
    /// station frontier reusable. This is data, not a request to reset.
    type Fault: StaFault;
    /// The image's monotonic clock, on which station backoff waits.
    type Timer: oer_time::Timer;

    fn timer(&self) -> &Self::Timer;

    fn run_initial_scan<'a>(
        &'a mut self,
        phase: StationInitialScanPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::InitialScanRx,
            Self::Network,
        >,
        discovery: StationDiscovery,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StationInitialScanExit<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
            StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_initial_join<'a>(
        &'a mut self,
        phase: StationInitialJoinPhase<
            'security,
            Self::Runtime,
            Self::InitialHardware,
            Self::RxFrontier,
            Self::Network,
        >,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_running_scan<'a>(
        &'a mut self,
        phase: StationRunningScanPhase<'security, Self::Runtime, Self::Disconnected>,
        discovery: StationDiscovery,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StationRunningScanExit<
            'security,
            Self::Runtime,
            Self::Reconnected,
            Self::Network,
            StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_reconnected<'a>(
        &'a mut self,
        phase: StationReconnectedPhase<'security, Self::Runtime, Self::Reconnected, Self::Network>,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StationJoinExit<
            'security,
            Self::Runtime,
            Self::Connected,
            StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn run_connected<'a>(
        &'a mut self,
        phase: StationConnectedPhase<'security, Self::Runtime, Self::Connected>,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<
        Output = StaAttemptOutcome<
            StationEngineOwner<'security, M, Self>,
            Self::Error,
            Self::Fault,
        >,
    > + 'a
    where
        Self: Sized + 'a,
        'security: 'a;

    fn candidate_refresh_contract_error(&mut self) -> Self::Error;
}

/// Side-effect-only observations emitted by the common station engine.
///
/// An observer never receives mutable access to the hardware port or an owned
/// phase frontier. It may record lifecycle progress, but cannot select or
/// bypass a station transition.
pub trait StationEngineObserver<'security, M, P>
where
    M: RawMutex,
    P: StationEnginePort<'security, M>,
{
    fn attempt_started(&mut self, _context: StaAttemptContext, _phase: StationServicePhaseKind) {}

    fn attempt_finished<'a>(
        &'a mut self,
        _context: StaAttemptContext,
        _outcome: &'a StaAttemptOutcome<StationEngineOwner<'security, M, P>, P::Error, P::Fault>,
    ) -> impl Future<Output = ()> + 'a
    where
        Self: Sized + 'a,
        P: 'a,
        'security: 'a,
    {
        async {}
    }

    fn command_deferred(&mut self, _command: StationCommand, _accepted: bool) {}

    fn backoff_started(&mut self, _delay_millis: u32, _reason: StaBackoffReason) {}
}

/// Zero-cost observer for compositions that do not request lifecycle
/// instrumentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopStationEngineObserver;

impl<'security, M, P> StationEngineObserver<'security, M, P> for NoopStationEngineObserver
where
    M: RawMutex,
    P: StationEnginePort<'security, M>,
{
}

pub type StationEngineOwner<'security, M, E> = StationServiceOwner<
    'security,
    <E as StationEnginePort<'security, M>>::Runtime,
    StationServicePhase<
        <E as StationEnginePort<'security, M>>::InitialHardware,
        <E as StationEnginePort<'security, M>>::InitialScanRx,
        <E as StationEnginePort<'security, M>>::RxFrontier,
        <E as StationEnginePort<'security, M>>::Network,
        <E as StationEnginePort<'security, M>>::Disconnected,
        <E as StationEnginePort<'security, M>>::Reconnected,
        <E as StationEnginePort<'security, M>>::Connected,
    >,
>;

/// Common outer STA engine shared by firmware and HIL.
///
/// Scan/join/connected transactions remain finite port operations supplied
/// by `E`; phase selection, the refresh precondition and lifecycle callbacks
/// are not reimplemented by each consumer.
pub struct StationEngine<'security, P, O = NoopStationEngineObserver> {
    port: P,
    discovery: StationDiscovery,
    observer: O,
    _security: PhantomData<&'security mut ()>,
}

impl<P> StationEngine<'_, P, NoopStationEngineObserver> {
    pub const fn new(port: P, discovery: StationDiscovery) -> Self {
        Self {
            port,
            discovery,
            observer: NoopStationEngineObserver,
            _security: PhantomData,
        }
    }
}

impl<P, O> StationEngine<'_, P, O> {
    pub const fn port(&self) -> &P {
        &self.port
    }

    pub const fn with_observer(port: P, discovery: StationDiscovery, observer: O) -> Self {
        Self {
            port,
            discovery,
            observer,
            _security: PhantomData,
        }
    }

    pub fn into_port(self) -> P {
        self.port
    }

    pub fn into_parts(self) -> (P, StationDiscovery, O) {
        (self.port, self.discovery, self.observer)
    }
}

impl<'security, M, P, O> StationAttemptRunner<M> for StationEngine<'security, P, O>
where
    M: RawMutex,
    P: StationEnginePort<'security, M>,
    O: StationEngineObserver<'security, M, P>,
{
    type Owner = StationEngineOwner<'security, M, P>;
    type Error = P::Error;
    type Fault = P::Fault;
    type Timer = P::Timer;

    fn timer(&self) -> &P::Timer {
        self.port.timer()
    }

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
        control: &'a mut StationCommandReceiver<'_, M>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error, Self::Fault>> + 'a {
        async move {
            let phase_kind = match &owner.phase {
                StationServicePhase::InitialScan { .. } => StationServicePhaseKind::InitialScan,
                StationServicePhase::InitialJoin { .. } => StationServicePhaseKind::InitialJoin,
                StationServicePhase::RunningScan { .. } => StationServicePhaseKind::RunningScan,
                StationServicePhase::Reconnected { .. } => StationServicePhaseKind::Reconnected,
                StationServicePhase::Connected { .. } => StationServicePhaseKind::Connected,
            };
            self.observer.attempt_started(context, phase_kind);
            let (runtime, phase, security) = owner.into_parts();
            let outcome = match phase {
                StationServicePhase::InitialScan {
                    hardware,
                    receive,
                    network,
                    identity,
                } => {
                    match await_stack_boundary!(self.port.run_initial_scan(
                        StationInitialScanPhase::new(
                            runtime, hardware, receive, network, identity, security,
                        ),
                        self.discovery,
                        context,
                        control,
                    )) {
                        StationInitialScanExit::JoinReady(phase) => {
                            let (runtime, hardware, receive, network, station, security) =
                                phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: StationServiceOwner::new(
                                    runtime,
                                    StationServicePhase::InitialJoin {
                                        hardware,
                                        receive,
                                        network,
                                        station,
                                    },
                                    security,
                                ),
                            }
                        }
                        StationInitialScanExit::Complete(outcome) => outcome,
                    }
                }
                StationServicePhase::InitialJoin {
                    hardware,
                    receive,
                    network,
                    station,
                } => {
                    match await_stack_boundary!(self.port.run_initial_join(
                        StationInitialJoinPhase::new(
                            runtime, hardware, receive, network, station, security,
                        ),
                        context,
                        control,
                    )) {
                        StationJoinExit::ConnectedReady(phase) => {
                            let (runtime, connected, security) = phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: StationServiceOwner::new(
                                    runtime,
                                    StationServicePhase::Connected { connected },
                                    security,
                                ),
                            }
                        }
                        StationJoinExit::Complete(outcome) => outcome,
                    }
                }
                StationServicePhase::RunningScan {
                    disconnected,
                    station,
                } => {
                    if context.refresh_candidate {
                        match await_stack_boundary!(self.port.run_running_scan(
                            StationRunningScanPhase::new(runtime, disconnected, station, security,),
                            self.discovery,
                            context,
                            control,
                        )) {
                            StationRunningScanExit::JoinReady(phase) => {
                                let (runtime, epoch, network, station, security) =
                                    phase.into_parts();
                                StaAttemptOutcome::Advanced {
                                    owner: StationServiceOwner::new(
                                        runtime,
                                        StationServicePhase::Reconnected {
                                            epoch,
                                            network,
                                            station,
                                        },
                                        security,
                                    ),
                                }
                            }
                            StationRunningScanExit::Complete(outcome) => outcome,
                        }
                    } else {
                        let error = self.port.candidate_refresh_contract_error();
                        StaAttemptOutcome::Failed {
                            owner: StationServiceOwner::new(
                                runtime,
                                StationServicePhase::RunningScan {
                                    disconnected,
                                    station,
                                },
                                security,
                            ),
                            failure: StaAttemptFailure::new(
                                StaLifecycleStage::CandidateSelection,
                                StaFailureDisposition::Terminal,
                                error,
                            ),
                        }
                    }
                }
                StationServicePhase::Reconnected {
                    epoch,
                    network,
                    station,
                } => {
                    match await_stack_boundary!(self.port.run_reconnected(
                        StationReconnectedPhase::new(runtime, epoch, network, station, security,),
                        context,
                        control,
                    )) {
                        StationJoinExit::ConnectedReady(phase) => {
                            let (runtime, connected, security) = phase.into_parts();
                            StaAttemptOutcome::Advanced {
                                owner: StationServiceOwner::new(
                                    runtime,
                                    StationServicePhase::Connected { connected },
                                    security,
                                ),
                            }
                        }
                        StationJoinExit::Complete(outcome) => outcome,
                    }
                }
                StationServicePhase::Connected { connected } => {
                    await_stack_boundary!(self.port.run_connected(
                        StationConnectedPhase::new(runtime, connected, security),
                        context,
                        control,
                    ))
                }
            };
            self.observer.attempt_finished(context, &outcome).await;
            outcome
        }
    }

    fn command_deferred(&mut self, command: StationCommand, accepted: bool) {
        self.observer.command_deferred(command, accepted);
    }

    fn backoff_started(&mut self, delay_millis: u32, reason: StaBackoffReason) {
        self.observer.backoff_started(delay_millis, reason);
    }
}

#[cfg(test)]
mod tests;
