//! Allocation-free candidate-scan values and port.
//!
//! The `StaCandidateScanService` of `oer-ieee80211-sta-service` drives the
//! [`StaCandidateScanBackend`] port through one finite channel plan. Its
//! `StaScanBackend` implements that port with one fixed channel-visit
//! transaction over the primitive [`StaScanPort`] declared here: switch,
//! start receiving, the optional active probe, a bounded dwell, stop and
//! prepare the next channel.

use core::future::Future;

/// One channel visit within a finite scan plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaScanChannelContext<C> {
    pub channel: C,
    pub index: u16,
    pub total_channels: u16,
}

impl<C> StaScanChannelContext<C> {
    pub fn is_last(&self) -> bool {
        self.index.checked_add(1) == Some(self.total_channels)
    }
}

/// Result of scan preparation or one channel visit.
///
/// Every edge returns the exact owner consumed by the backend. A failed dwell
/// therefore cannot strand PAC, RX-DMA or observation-table ownership inside
/// an executor future.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanStepOutcome<O, E> {
    Completed { owner: O },
    Stopped { owner: O },
    Failed { owner: O, error: E },
}

/// Final candidate decision after all planned channel visits complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanSelectionOutcome<O, C, E> {
    Selected { owner: O, candidate: C },
    NoCandidate { owner: O },
    Stopped { owner: O },
    Failed { owner: O, error: E },
}

/// Finite chip/runtime adapter used by the `StaCandidateScanService` of
/// `oer-ieee80211-sta-service`.
///
/// `begin_scan` owns observation reset and any hardware preparation which must
/// happen exactly once per scan attempt. `scan_channel` owns one channel
/// switch, RX/probe publication and bounded dwell. Candidate selection remains
/// a separate edge so an adapter cannot report a retained `ScanRecord` as a
/// refreshed candidate without actually completing the channel plan.
pub trait StaCandidateScanBackend {
    type Owner;
    type Channel: Copy;
    type Candidate;
    type Error;

    fn begin_scan(
        &mut self,
        owner: Self::Owner,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_;

    fn scan_channel(
        &mut self,
        owner: Self::Owner,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = StaScanStepOutcome<Self::Owner, Self::Error>> + '_;

    fn select_candidate(
        &mut self,
        owner: Self::Owner,
    ) -> StaScanSelectionOutcome<Self::Owner, Self::Candidate, Self::Error>;
}

/// Result of the optional active-probe edge.
///
/// Probe failure is deliberately not necessarily a scan failure. A concrete
/// port must close any failed TX publication before returning
/// [`PassiveFallback`](Self::PassiveFallback); the bounded receive dwell then
/// continues as a passive scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActiveProbeOutcome {
    Transmitted,
    PassiveFallback,
}

/// Exact mandatory transaction edge which failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanError<E> {
    Begin(E),
    ChannelSwitch(E),
    ReceiveStart(E),
    ActiveProbe(E),
    ReceiveObserve(E),
    DwellWait(E),
    ReceiveStop(E),
    PrepareNextRing(E),
    CandidateSelection(E),
}

impl<E> StaScanError<E> {
    /// The port's error, whichever step it ended.
    pub const fn error(&self) -> &E {
        match self {
            Self::Begin(error)
            | Self::ChannelSwitch(error)
            | Self::ReceiveStart(error)
            | Self::ActiveProbe(error)
            | Self::ReceiveObserve(error)
            | Self::DwellWait(error)
            | Self::ReceiveStop(error)
            | Self::PrepareNextRing(error)
            | Self::CandidateSelection(error) => error,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanConfigError {
    ZeroDwellTicks,
}

/// Bounded executor-neutral policy for one channel visit.
///
/// A tick has no duration at this layer. The integration port maps it to its
/// clock and executor, so the transaction is also usable by a non-Embassy
/// runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaScanConfig {
    dwell_ticks: u16,
}

impl StaScanConfig {
    pub const fn new(dwell_ticks: u16) -> Result<Self, StaScanConfigError> {
        if dwell_ticks == 0 {
            Err(StaScanConfigError::ZeroDwellTicks)
        } else {
            Ok(Self { dwell_ticks })
        }
    }

    pub const fn dwell_ticks(self) -> u16 {
        self.dwell_ticks
    }
}

/// Primitive operations retained by a concrete cold or running scan owner.
///
/// `start_receive` must either establish a live RX epoch or leave the walker
/// stopped on error. `observe_receive` performs one finite drain/recycle pass;
/// it must not wait. `stop_receive` must confirm that DMA released descriptor
/// ownership before returning success. `prepare_next_ring` runs only after
/// that confirmation and must leave a stopped ring ready for the next channel.
pub trait StaScanPort {
    type Channel: Copy;
    type Candidate;
    type Error;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    /// Switch to the channel and return how many dwell ticks to spend on
    /// it. The port may shorten or lengthen `requested_dwell_ticks`, as the
    /// vendor scan does while another radio shares the air.
    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
        requested_dwell_ticks: u16,
    ) -> impl Future<Output = Result<u16, Self::Error>> + '_;

    fn start_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<ActiveProbeOutcome, Self::Error>> + '_;

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_;

    fn prepare_next_ring(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error>;

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaScanPlanError {
    Empty,
    TooManyChannels,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaScanProgress {
    pub channels_planned: u16,
    pub channels_started: u16,
    pub channels_completed: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCandidateScanExit<O, C, E> {
    Selected {
        owner: O,
        candidate: C,
        progress: StaScanProgress,
    },
    NoCandidate {
        owner: O,
        progress: StaScanProgress,
    },
    Stopped {
        owner: O,
        progress: StaScanProgress,
    },
    Failed {
        owner: O,
        error: E,
        progress: StaScanProgress,
    },
    InvalidPlan {
        owner: O,
        error: StaScanPlanError,
        progress: StaScanProgress,
    },
}
