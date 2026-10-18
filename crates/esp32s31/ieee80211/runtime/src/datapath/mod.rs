//! Role-neutral Embassy DATAPATH loop for one running ESP32-S31 Wi-Fi owner.
//!
//! DATAPATH owns IRQ/RX/TX/network arbitration. AP and STA semantics enter only
//! through [`DatapathServices`]; the scheduler never imports either role crate.

use core::future::{Future, pending, ready};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::datapath::irq::EmbassyMacIrqRuntime;
use crate::datapath::network::{DatapathNetwork, DatapathNetworkRxSet};
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;
use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};
use embassy_sync::blocking_mutex::raw::RawMutex;
pub use oer_esp32s31_ieee80211::datapath::{
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathRxWorkCounters,
    DatapathStopProgress,
};
pub use oer_esp32s31_ieee80211::tx::{WifiTxProgress, WifiTxWake};
use oer_network_interface::{LinkState, NetworkInterfaceId};
use oer_time::{Duration, Instant, Timer};

pub mod irq;
pub mod network;
#[cfg(feature = "owned-network")]
pub mod owned;
pub mod rx;
pub mod services;
pub(crate) mod software_tx_queue;
mod sram_tx;
pub mod stop;
pub mod tx;
pub(crate) mod tx_performance;

pub use oer_ieee80211_datapath::MaterializationOwnershipSnapshot;
pub use oer_ieee80211_datapath::{MaterializedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame};
pub use sram_tx::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer, PinnedTxPool, PinnedTxResources,
};
#[cfg(feature = "tx-phase-telemetry")]
pub use tx_performance::{
    TX_PERFORMANCE, TxPerformanceCounters, TxPerformanceSample, TxPerformanceSnapshot,
};

/// Aggregation demand of the next network batch of one logical interface.
///
/// `target` is the number of MPDUs one batch can carry for the destination it
/// would serve: its negotiated Block Ack window, or one for a peer without an
/// operational agreement and for group traffic. `ready` counts the owners
/// already visible for that destination.
///
/// The scheduler never delays air time for a demand. Like the Linux TXQ
/// drivers, a batch is formed from what is queued when the medium can take
/// it; accumulation happens while the preceding PPDU is on air. The demand
/// only bounds how many new owners justify waking the scheduler to extend a
/// prepared successor during an active exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBatchDemand {
    pub target: usize,
    pub ready: usize,
}

impl TxBatchDemand {
    /// Demand without a destination worth aggregating for.
    pub const fn single(ready: usize) -> Self {
        Self { target: 1, ready }
    }

    pub const fn complete(self) -> bool {
        self.ready >= self.target
    }
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
static RECYCLED_RX_PROBE_DELAY_MICROS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "core0-rx-coarse-telemetry")]
static ADAPTIVE_RECYCLED_RX_PROBE: AtomicBool = AtomicBool::new(false);

/// Configure a same-image delayed continuation for a masked recycled-only RX
/// frontier. Zero preserves the production immediate software repost.
///
/// The delay applies only after the first IRQ-originated service has finished;
/// it never delays an idle-to-active RX transition. Hardware terminal,
/// completion-frontier and budget continuations remain immediate.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn configure_recycled_rx_probe_delay_for_diagnostics(delay_micros: u32) {
    RECYCLED_RX_PROBE_DELAY_MICROS.store(delay_micros, Ordering::Relaxed);
}

/// Select the same-image adaptive continuation policy for coarse HIL.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn configure_adaptive_recycled_rx_probe_for_diagnostics(enabled: bool) {
    ADAPTIVE_RECYCLED_RX_PROBE.store(enabled, Ordering::Relaxed);
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
fn recycled_rx_probe_delay_for_diagnostics() -> Option<Duration> {
    let micros = RECYCLED_RX_PROBE_DELAY_MICROS.load(Ordering::Relaxed);
    (micros != 0).then(|| Duration::from_micros(u64::from(micros)))
}

/// Bound a masked post-append probe by the packet-rate implied by the last
/// physical batch. Small-frame streams use 512 us against the 96-entry
/// physical ring. A completed long frame immediately proves that its byte
/// rate bounds packet pressure well below the same reserve, so it uses 1024
/// us without a 128/256/512-us training ramp. The ramp created a rate cliff:
/// at 30 Mbit/s each short probe observed an empty frontier and unmasked RX
/// before the next packet, while 40 Mbit/s happened to reach the 1024-us
/// state. A recycle-only confirmation retains the class established earlier
/// in the same masked drain epoch.
fn adaptive_recycled_rx_probe_delay(
    work: DatapathRxWorkCounters,
    current_level: u8,
) -> (Duration, u8) {
    let average_bytes = work
        .staged_bytes
        .checked_div(work.completed_units)
        .unwrap_or(0);
    if work.completed_units == 0 {
        let micros = match current_level {
            1 => 128,
            2 => 256,
            3 => 512,
            4 => 1_024,
            _ => 64,
        };
        return (Duration::from_micros(micros), current_level.min(4));
    }
    if average_bytes < 1_024 {
        return (Duration::from_micros(512), 3);
    }
    (Duration::from_micros(1_024), 4)
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
fn adaptive_recycled_rx_probe_enabled() -> bool {
    ADAPTIVE_RECYCLED_RX_PROBE.load(Ordering::Relaxed)
}

#[cfg(not(feature = "core0-rx-coarse-telemetry"))]
fn adaptive_recycled_rx_probe_enabled() -> bool {
    true
}

const fn tx_lookahead_allowed(
    requested: bool,
    interfaces: DatapathInterfaceScope,
    origin: DatapathTxOrigin,
) -> bool {
    requested
        && matches!(interfaces, DatapathInterfaceScope::Single(_))
        && matches!(origin, DatapathTxOrigin::Network)
}

/// Scheduler-owned limit for one role-specific protocol RX turn.
///
/// `None` means no network TX is waiting and the role may use its own bounded
/// batch. `Some` carries the exact remaining frame credit before DATAPATH owes the
/// queued TX side another transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatapathRxServiceContext {
    pub maximum_protocol_frames: Option<usize>,
}

/// Terminal, non-error outcome of one role-neutral radio event loop.
///
/// Keeping this distinct from `()` prevents an outer station owner from
/// confusing a proved link loss with a runner that completed without a
/// lifecycle transition. The caller may use this edge to tear down the
/// network stack, release the connected epoch and start reassociation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathRunnerExit<E> {
    /// Role policy reached its terminal edge.
    Role(E),
    /// The outer lifecycle requested a finite stop. The runner waited
    /// for any active TX transaction to release hardware, published link-down
    /// and returned the same owners as a disconnect without claiming peer
    /// reachability had failed.
    Stopped,
}

/// Logical interfaces scheduled by one physical DATAPATH owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathInterfaceScope {
    Single(NetworkInterfaceId),
    Pair {
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    },
}

/// Semantic owner of the currently active physical TX transaction.
///
/// Network completion returns directly to data arbitration. Control completion
/// re-arms the control machine because one terminal management edge may expose
/// another finite protocol transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatapathTxOrigin {
    Network,
    Control,
}

impl DatapathInterfaceScope {
    pub fn pair(first: NetworkInterfaceId, second: NetworkInterfaceId) -> Self {
        assert_ne!(first, second, "DATAPATH pair requires distinct interfaces");
        Self::Pair { first, second }
    }

    pub const fn primary(self) -> NetworkInterfaceId {
        match self {
            Self::Single(interface)
            | Self::Pair {
                first: interface, ..
            } => interface,
        }
    }

    pub const fn contains(self, interface: NetworkInterfaceId) -> bool {
        match self {
            Self::Single(owned) => owned.value() == interface.value(),
            Self::Pair { first, second } => {
                first.value() == interface.value() || second.value() == interface.value()
            }
        }
    }

    pub const fn is_pair(self) -> bool {
        matches!(self, Self::Pair { .. })
    }
}

/// Finite chip-specific and role-specific operations used by [`DatapathRunner`].
///
/// An implementation normally owns the live RX descriptor ring, staging
/// storage, staging publisher, TX descriptor state and a short-lived PAC
/// facade such as `CooperativeRadioHardware`. `service_rx` must snapshot and drain one
/// durable RX frontier into bounded descriptor-backed upper ownership. A separate
/// protocol consumer retains duplicate/protocol history.
///
/// Every method must finish after a bounded number of hardware observations.
/// A method may await a timer edge needed by a typed transaction, but it must
/// release every mutable PAC borrow before that edge. RX-before-TX arbitration
/// and the lifetime of a pinned network lease belong to [`DatapathRunner::run`].
pub trait DatapathServices<SoftwareFrame, PhysicalFrame>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: MaterializedTxFrame,
{
    type Error: 'static;
    type Exit: 'static;

    /// Drain one snapshotted RX-success frontier into independent ownership.
    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a;

    /// Whether the role exposes a DMA-only RX producer that is safe while a
    /// TX transaction owns its protocol/control domain.
    fn can_service_rx_during_tx(&self) -> bool {
        true
    }

    /// Service RX work proven safe while an active TX owns its transaction.
    ///
    /// The default preserves roles whose ordinary RX service is already safe.
    /// A combined role must override this method and admit only its DMA
    /// producer plus protocol work that cannot execute control or hardware
    /// actions until the TX terminal edge.
    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.service_rx(
            network_rx,
            DatapathRxServiceContext {
                maximum_protocol_frames: None,
            },
        )
    }

    /// Whether role-owned software RX work is ready without a fresh hardware
    /// interrupt. This covers finite reorder deadlines and retained batches;
    /// it must not report speculative hardware work.
    fn has_rx_work(&self) -> bool {
        false
    }

    /// Monotonic number of protocol frames completed by this role.
    ///
    /// DATAPATH uses the delta across one service call for frame-based RX/TX
    /// fairness. DMA-only staging must not advance this counter: it releases
    /// hardware ownership but has not consumed a protocol scheduling turn.
    fn serviced_rx_frames(&self) -> u64 {
        0
    }

    /// Monotonic physical units and bytes completed by the RX DMA owner.
    fn rx_work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters::default()
    }

    /// Apply at most one owned control event or publish one control frame.
    ///
    /// The runner invokes this only while no network TX transaction owns the
    /// shared descriptor. `More` returns through the scheduler before a new
    /// network lease can be claimed; `TxPending` enters the same IRQ/deadline
    /// completion loop as ordinary data.
    fn service_control<'a>(
        &'a mut self,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a
where {
        ready(Ok(DatapathControlProgress::Idle))
    }

    /// Whether control work is ready at this transaction boundary.
    ///
    /// Implementations must make this an O(1) observation. Timed roles retain
    /// one absolute deadline; mailbox-driven roles inspect their published
    /// readiness state. The conservative default preserves custom services
    /// which have not specialised their control scheduler.
    fn control_ready(&self, _now: oer_time::Instant) -> bool {
        true
    }

    fn control_required_before_network_tx(&self) -> bool {
        false
    }

    /// Whether control lets the scheduler publish a network frame now.
    fn control_admits_network_tx(&self) -> bool {
        true
    }

    fn control_required_before_stop(&self) -> bool {
        false
    }

    /// VIF that owns a role-generated live TX transaction.
    ///
    /// Standalone runners infer their sole interface. A paired runner requires
    /// the combined services owner to report this identity before DATAPATH enters
    /// the shared IRQ/deadline completion loop.
    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// Whether role work published a live TX transaction inside the current
    /// service call.
    ///
    /// A paired owner reports this through [`Self::active_tx_interface`]. A
    /// single-interface role may leave the identity implicit, but must still
    /// expose the ownership edge so the runner adopts the transaction before
    /// observing stop or any other idle-boundary work.
    fn has_active_tx(&self) -> bool {
        self.active_tx_interface().is_some()
    }

    /// VIF retained by a software-owned standby aggregate.
    fn prepared_tx_interface(&self) -> Option<NetworkInterfaceId> {
        None
    }

    /// Advance role shutdown by one finite transition at an idle TX boundary.
    fn service_stop(&mut self) -> Result<DatapathStopProgress, Self::Error> {
        Ok(DatapathStopProgress::Stopped)
    }

    /// Wake the outer scheduler for a control timer or independently
    /// published control event. Backends without such a source never wake it.
    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a {
        pending()
    }

    /// Transfer one network-owned frame into the MAC/DMA transaction.
    ///
    /// The services owner receives ownership rather than a temporary borrow. An
    /// ordinary copy-based transmitter may release the lease immediately;
    /// a referenced A-MPDU owner may retain it, claim further ready leases
    /// from `network`, and return all of them only after BlockAck/detach.
    fn start_tx<'a, I>(
        &'a mut self,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a;

    /// Number of network leases claimed by the most recent successful
    /// [`Self::start_tx`] call.
    ///
    /// Aggregate encoders may synchronously drain more ready leases after the
    /// first frame is transferred by the runner. Fairness must charge every
    /// claimed lease, not merely the explicit first argument.
    fn last_started_tx_frame_count(&self) -> usize {
        1
    }

    /// Wait for the executor deadline of the active TX transaction.
    ///
    /// This future is created only after [`Self::start_tx`] returned
    /// [`WifiTxProgress::Pending`]. A retry may replace the active deadline;
    /// the runner creates a fresh future after every service operation.
    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_;

    /// Inspect, complete, abort or restart the active TX transaction.
    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    fn has_prepared_tx(&self) -> bool {
        false
    }

    /// Aggregation demand of the next network batch of `interface`; see
    /// [`TxBatchDemand`]. Non-aggregate services keep the single-frame
    /// default.
    fn tx_batch_demand<I>(&self, _interface: NetworkInterfaceId, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        TxBatchDemand::single(self.prepared_tx_frame_count() + network.queue_len())
    }

    /// MPDUs already retained in the software-owned standby arena.
    fn prepared_tx_frame_count(&self) -> usize {
        0
    }

    /// A retained software batch may still be waiting for an asynchronous
    /// materializer. It remains owned/prepared but cannot yet enter hardware.
    fn prepared_tx_start_ready(&self) -> bool {
        self.has_prepared_tx()
    }

    /// This role chooses owners directly in `advance_prepared_tx` when the
    /// source exposes destination queues. The runner must not push an
    /// unrelated owner into an already selected aggregate afterward.
    fn pulls_tx_queues(&self) -> bool {
        false
    }

    /// Queue whose publications can extend the current prepared transmission.
    /// `None` allows the role to select its next destination.
    fn prepared_tx_destination(&self) -> Option<[u8; 6]> {
        None
    }

    /// Advance a retained software-only preparation edge without claiming a
    /// new network frame. Ordinary synchronous roles need no such edge.
    fn advance_prepared_tx<I>(&mut self, _network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(())
    }

    /// Retain one diagnostic phase boundary for a later typed publication
    /// observation. This method and all its call sites are absent from an
    /// ordinary build.
    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_tx_scheduler_phase(
        &mut self,
        _phase: PreparedTxSchedulerPhase,
        _at_micros: u64,
    ) {
    }

    fn start_prepared_tx<I>(&mut self, _network: &I) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(WifiTxProgress::Complete)
    }

    fn cancel_prepared_tx<I>(&mut self, _network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(())
    }

    fn can_prepare_tx(&self) -> bool {
        false
    }

    fn prepare_tx<'a, I>(
        &'a mut self,
        _frame: SoftwareFrame,
        _network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        ready(Ok(()))
    }
}

/// Single Embassy owner for RX DMA, control, network TX and MAC IRQ order.
pub struct DatapathRunner<'irq, M: RawMutex, N, B, R, K> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    /// Monotonic clock of every scheduling deadline this owner keeps.
    clock: K,
    network: N,
    interfaces: DatapathInterfaceScope,
    network_rx: R,
    services: B,
    active_tx_interface: Option<NetworkInterfaceId>,
    active_tx_origin: Option<DatapathTxOrigin>,
    prepared_tx_interface: Option<NetworkInterfaceId>,
    prepared_tx_completion: crate::diagnostics::core0_rx_performance::Core0PreparedTxMark,
    control_ready_latched: bool,
    rx_progress: DatapathRxProgress,
    recycled_rx_probe_deadline: Option<Instant>,
    recycled_rx_probe_coalescing_level: u8,
    /// Deficit round robin between the RX frontier and network TX.
    fairness: fairness::RxTxFairness,
    /// Relative network frames admitted for each VIF while both members of a
    /// pair are backlogged. Selection follows the smaller total, so unequal
    /// aggregate sizes do not turn transaction round-robin into airtime-sized
    /// starvation. The counters reset whenever only one VIF is runnable.
    pair_tx_served_frames: [u64; 2],
}

pub mod execution;
mod fairness;
mod owner;
pub mod paired;
mod scheduler;
mod service;

#[cfg(all(test, feature = "owned-network"))]
mod owned_tests;
