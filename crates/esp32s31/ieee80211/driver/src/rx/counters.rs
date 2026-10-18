//! Production counters of one Wi-Fi receive epoch, for the W1 log.
//!
//! The interrupt line (P3), the radio executor (P2) and the application's
//! frame receiver (P1, the prefix counts) each update their own counters
//! with single Relaxed atomic operations; a reader takes a [`WifiRxReport`]
//! at any time. Counts wrap. The reload evidence is the ring's own
//! (saturating, its maximum stops at the attempt limit), copied after every
//! service pass; the teardown durations are written once, by the teardown.
//! The exhaustion snapshot is the runner's, written when a lease returns to
//! a full staging pool ([`WifiRxExhaustion`]).

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211_dma::rx_ring::RxReloadRepairEvidence;
use oer_esp32s31_ieee80211_mac::rx::RxError;
use oer_time::Duration;

use super::transaction::Discard;
use crate::datapath::DatapathRxProgress;

/// Counters beside the receive arena. Its zero bytes are zero counts.
#[derive(bytemuck::Zeroable, Default)]
pub struct WifiRxCounters {
    mac_irqs: AtomicU32,
    spurious_mac_irqs: AtomicU32,
    auxiliary_mac_irqs: AtomicU32,
    unhandled_mac_irqs: AtomicU32,
    power_irqs: AtomicU32,
    spurious_power_irqs: AtomicU32,
    doorbell_unmasks: AtomicU32,
    ignored_unmasks: AtomicU32,
    units: AtomicU32,
    bytes: AtomicU32,
    chained_drops: AtomicU32,
    oversized_drops: AtomicU32,
    empty_drops: AtomicU32,
    too_long_drops: AtomicU32,
    overload_drops: AtomicU32,
    admission_blocks: AtomicU32,
    stage_capacity_blocks: AtomicU32,
    service_passes: AtomicU32,
    service_errors: AtomicU32,
    capacity_drains: AtomicU32,
    continuations: ContinuationCounters,
    reload: ReloadCounters,
    teardown: TeardownCounters,
    prefix: PrefixCounters,
    exhaustion: ExhaustionCounters,
}

/// The runner's continuations: how each was taken and why.
#[derive(bytemuck::Zeroable, Default)]
struct ContinuationCounters {
    immediate: AtomicU32,
    timed: AtomicU32,
    longest_streak: AtomicU32,
    drained: AtomicU32,
    upper_layer_blocked_but_droppable: AtomicU32,
    probe_pending: AtomicU32,
    recycled_append_pending: AtomicU32,
    budget_exhausted: AtomicU32,
    network_backpressured: AtomicU32,
    protocol_blocked_by_tx: AtomicU32,
}

/// One reading of the runner's continuations.
///
/// A drain continues (reruns its service pass with RX still masked) for
/// the [`DatapathRxProgress`] the pass returned; `drained` and
/// `upper_layer_blocked_but_droppable` count only the passes that left a
/// released buffer to rearm.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxContinuationReport {
    /// Continuations taken by an immediate self-wake.
    pub immediate: u32,
    /// Continuations taken after a timer wait.
    pub timed: u32,
    /// The most continuations one drain took before it unmasked, blocked on
    /// staging capacity or stopped (a maximum, not a count).
    pub longest_streak: u32,
    pub drained: u32,
    pub upper_layer_blocked_but_droppable: u32,
    pub probe_pending: u32,
    pub recycled_append_pending: u32,
    pub budget_exhausted: u32,
    pub network_backpressured: u32,
    pub protocol_blocked_by_tx: u32,
}

impl WifiRxContinuationReport {
    /// The continuations taken for `reason`; a pass blocked on staging
    /// capacity never continues.
    pub const fn continued(&self, reason: DatapathRxProgress) -> u32 {
        match reason {
            DatapathRxProgress::Drained => self.drained,
            DatapathRxProgress::UpperLayerBlockedButDroppable => {
                self.upper_layer_blocked_but_droppable
            }
            DatapathRxProgress::ProbePending => self.probe_pending,
            DatapathRxProgress::RecycledAppendPending => self.recycled_append_pending,
            DatapathRxProgress::BudgetExhausted => self.budget_exhausted,
            DatapathRxProgress::NetworkBackpressured => self.network_backpressured,
            DatapathRxProgress::ProtocolBlockedByTx => self.protocol_blocked_by_tx,
            DatapathRxProgress::StageCapacityBlocked => 0,
        }
    }
}

impl ContinuationCounters {
    const fn new() -> Self {
        Self {
            immediate: AtomicU32::new(0),
            timed: AtomicU32::new(0),
            longest_streak: AtomicU32::new(0),
            drained: AtomicU32::new(0),
            upper_layer_blocked_but_droppable: AtomicU32::new(0),
            probe_pending: AtomicU32::new(0),
            recycled_append_pending: AtomicU32::new(0),
            budget_exhausted: AtomicU32::new(0),
            network_backpressured: AtomicU32::new(0),
            protocol_blocked_by_tx: AtomicU32::new(0),
        }
    }

    const fn reason(&self, reason: DatapathRxProgress) -> Option<&AtomicU32> {
        match reason {
            DatapathRxProgress::Drained => Some(&self.drained),
            DatapathRxProgress::UpperLayerBlockedButDroppable => {
                Some(&self.upper_layer_blocked_but_droppable)
            }
            DatapathRxProgress::ProbePending => Some(&self.probe_pending),
            DatapathRxProgress::RecycledAppendPending => Some(&self.recycled_append_pending),
            DatapathRxProgress::BudgetExhausted => Some(&self.budget_exhausted),
            DatapathRxProgress::NetworkBackpressured => Some(&self.network_backpressured),
            DatapathRxProgress::ProtocolBlockedByTx => Some(&self.protocol_blocked_by_tx),
            DatapathRxProgress::StageCapacityBlocked => None,
        }
    }

    fn report(&self) -> WifiRxContinuationReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxContinuationReport {
            immediate: load(&self.immediate),
            timed: load(&self.timed),
            longest_streak: load(&self.longest_streak),
            drained: load(&self.drained),
            upper_layer_blocked_but_droppable: load(&self.upper_layer_blocked_but_droppable),
            probe_pending: load(&self.probe_pending),
            recycled_append_pending: load(&self.recycled_append_pending),
            budget_exhausted: load(&self.budget_exhausted),
            network_backpressured: load(&self.network_backpressured),
            protocol_blocked_by_tx: load(&self.protocol_blocked_by_tx),
        }
    }
}

/// A copy of the ring's [`RxReloadRepairEvidence`]; optional values are
/// stored as `value + 1`, so zero means `None`.
#[derive(bytemuck::Zeroable, Default)]
struct ReloadCounters {
    max_reload_attempts: AtomicU32,
    observations: AtomicU32,
    unknown_upper_with_zero_address: AtomicU32,
    base_repairs: AtomicU32,
    last_next_low: AtomicU32,
    last_last_low_plus_one: AtomicU32,
    last_repair_head_plus_one: AtomicU32,
    exhausted_republications: AtomicU32,
    republications_accepted: AtomicU32,
}

const fn encode_optional(value: Option<u32>) -> u32 {
    match value {
        Some(value) => value.saturating_add(1),
        None => 0,
    }
}

const fn decode_optional(value: u32) -> Option<u32> {
    value.checked_sub(1)
}

impl ReloadCounters {
    const fn new() -> Self {
        Self {
            max_reload_attempts: AtomicU32::new(0),
            observations: AtomicU32::new(0),
            unknown_upper_with_zero_address: AtomicU32::new(0),
            base_repairs: AtomicU32::new(0),
            last_next_low: AtomicU32::new(0),
            last_last_low_plus_one: AtomicU32::new(0),
            last_repair_head_plus_one: AtomicU32::new(0),
            exhausted_republications: AtomicU32::new(0),
            republications_accepted: AtomicU32::new(0),
        }
    }

    fn record(&self, evidence: RxReloadRepairEvidence) {
        let RxReloadRepairEvidence {
            max_reload_attempts,
            observations,
            unknown_upper_with_zero_address,
            base_repairs,
            last_next_low,
            last_last_low,
            last_repair_head,
            exhausted_republications,
            republications_accepted,
        } = evidence;
        let store = |counter: &AtomicU32, value| counter.store(value, Ordering::Relaxed);
        store(&self.max_reload_attempts, max_reload_attempts);
        store(&self.observations, observations);
        store(
            &self.unknown_upper_with_zero_address,
            unknown_upper_with_zero_address,
        );
        store(&self.base_repairs, base_repairs);
        store(&self.last_next_low, last_next_low);
        store(&self.last_last_low_plus_one, encode_optional(last_last_low));
        store(
            &self.last_repair_head_plus_one,
            encode_optional(last_repair_head),
        );
        store(&self.exhausted_republications, exhausted_republications);
        store(&self.republications_accepted, republications_accepted);
    }

    fn report(&self) -> RxReloadRepairEvidence {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        RxReloadRepairEvidence {
            max_reload_attempts: load(&self.max_reload_attempts),
            observations: load(&self.observations),
            unknown_upper_with_zero_address: load(&self.unknown_upper_with_zero_address),
            base_repairs: load(&self.base_repairs),
            last_next_low: load(&self.last_next_low),
            last_last_low: decode_optional(load(&self.last_last_low_plus_one)),
            last_repair_head: decode_optional(load(&self.last_repair_head_plus_one)),
            exhausted_republications: load(&self.exhausted_republications),
            republications_accepted: load(&self.republications_accepted),
        }
    }
}

/// The teardown's measured waits.
#[derive(bytemuck::Zeroable, Default)]
struct TeardownCounters {
    mask_confirm_us: AtomicU32,
    drain_us: AtomicU32,
    drain_polls: AtomicU32,
}

/// One reading of the teardown's measured waits; zero until the teardown
/// wrote them. Microseconds saturate at `u32::MAX`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxTeardownReport {
    /// From the mask request to the line's masked fact, or to the
    /// [`MASK_CONFIRM_TIMEOUT`](super::receive::MASK_CONFIRM_TIMEOUT).
    pub mask_confirm_us: u32,
    /// From the first drain check to the last one; zero when the mask was
    /// never confirmed, as the drain then never runs.
    pub drain_us: u32,
    /// [`DRAIN_POLL`](super::receive::DRAIN_POLL) waits between the drain
    /// checks.
    pub drain_polls: u32,
}

fn saturating_micros(duration: Duration) -> u32 {
    u32::try_from(duration.as_micros()).unwrap_or(u32::MAX)
}

impl TeardownCounters {
    const fn new() -> Self {
        Self {
            mask_confirm_us: AtomicU32::new(0),
            drain_us: AtomicU32::new(0),
            drain_polls: AtomicU32::new(0),
        }
    }

    fn report(&self) -> WifiRxTeardownReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxTeardownReport {
            mask_confirm_us: load(&self.mask_confirm_us),
            drain_us: load(&self.drain_us),
            drain_polls: load(&self.drain_polls),
        }
    }
}

/// The application's counts of the RX-control prefix decode.
#[derive(bytemuck::Zeroable, Default)]
struct PrefixCounters {
    invalid: AtomicU32,
    chain: AtomicU32,
    bounds: AtomicU32,
    rx_failure: AtomicU32,
    mic_failure: AtomicU32,
    quarantined: AtomicU32,
    output_small: AtomicU32,
    metadata: AtomicU32,
    ignored: AtomicU32,
    unsupported: AtomicU32,
    strict_rxend_refusable: AtomicU32,
    strict_dump_refusable: AtomicU32,
}

/// One reading of the prefix decode counts: refusals by their
/// [`RxError`], and admitted frames a strict check would have refused.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxPrefixReport {
    pub invalid: u32,
    pub chain: u32,
    pub bounds: u32,
    /// A nonzero RX state other than the two below.
    pub rx_failure: u32,
    /// RX state `0xf5`.
    pub mic_failure: u32,
    /// RX state `0xc6`.
    pub quarantined: u32,
    pub output_small: u32,
    pub metadata: u32,
    pub ignored: u32,
    pub unsupported: u32,
    /// Admitted frames with a nonzero RXEND state.
    pub strict_rxend_refusable: u32,
    /// Admitted frames whose dump length is not `sig_len` plus the FCS.
    pub strict_dump_refusable: u32,
}

impl WifiRxPrefixReport {
    /// The refusals counted for `reason`.
    pub const fn refused(&self, reason: RxError) -> u32 {
        match reason {
            RxError::Invalid => self.invalid,
            RxError::Chain => self.chain,
            RxError::Bounds => self.bounds,
            RxError::RxFailure => self.rx_failure,
            RxError::MicFailure => self.mic_failure,
            RxError::Quarantined => self.quarantined,
            RxError::OutputSmall => self.output_small,
            RxError::Metadata => self.metadata,
            RxError::Ignored => self.ignored,
            RxError::Unsupported => self.unsupported,
        }
    }
}

impl PrefixCounters {
    const fn new() -> Self {
        Self {
            invalid: AtomicU32::new(0),
            chain: AtomicU32::new(0),
            bounds: AtomicU32::new(0),
            rx_failure: AtomicU32::new(0),
            mic_failure: AtomicU32::new(0),
            quarantined: AtomicU32::new(0),
            output_small: AtomicU32::new(0),
            metadata: AtomicU32::new(0),
            ignored: AtomicU32::new(0),
            unsupported: AtomicU32::new(0),
            strict_rxend_refusable: AtomicU32::new(0),
            strict_dump_refusable: AtomicU32::new(0),
        }
    }

    fn report(&self) -> WifiRxPrefixReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxPrefixReport {
            invalid: load(&self.invalid),
            chain: load(&self.chain),
            bounds: load(&self.bounds),
            rx_failure: load(&self.rx_failure),
            mic_failure: load(&self.mic_failure),
            quarantined: load(&self.quarantined),
            output_small: load(&self.output_small),
            metadata: load(&self.metadata),
            ignored: load(&self.ignored),
            unsupported: load(&self.unsupported),
            strict_rxend_refusable: load(&self.strict_rxend_refusable),
            strict_dump_refusable: load(&self.strict_dump_refusable),
        }
    }

    const fn refused(&self, reason: RxError) -> &AtomicU32 {
        match reason {
            RxError::Invalid => &self.invalid,
            RxError::Chain => &self.chain,
            RxError::Bounds => &self.bounds,
            RxError::RxFailure => &self.rx_failure,
            RxError::MicFailure => &self.mic_failure,
            RxError::Quarantined => &self.quarantined,
            RxError::OutputSmall => &self.output_small,
            RxError::Metadata => &self.metadata,
            RxError::Ignored => &self.ignored,
            RxError::Unsupported => &self.unsupported,
        }
    }
}

/// The walker and the ring as the runner read them when a lease returned
/// after a drain had left every staging slot claimed or every buffer
/// detached: the registers before the capacity drain's first service pass,
/// in the state of the full pool (one lease already back). Report-only: no
/// field takes part in descriptor ownership.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxExhaustion {
    /// How long the pool stayed full, from the first drain that left it so
    /// to the return, in µs (saturating).
    pub blocked_us: u32,
    /// `RX_NEXT_DESCRIPTOR`: the low address and whether its unassigned
    /// upper field read nonzero.
    pub next_low: u32,
    pub next_upper: bool,
    /// `RX_LAST_DESCRIPTOR`, low address.
    pub last_low: u32,
    /// `RX_DESCRIPTOR_BASE`, where the platform reads it.
    pub base: Option<u32>,
    /// `RX_CONTROL`: the walker enable and the reload doorbell.
    pub walker_enabled: bool,
    pub reload_doorbell: bool,
    /// The hardware starvation counter, where the platform has one.
    pub buffer_full: Option<u16>,
    /// The ring's view: its descriptor base, the recycle frontier and the
    /// accepted tail (descriptor indices), descriptors returned to software
    /// in this generation, newly completed descriptors at the frontier, the
    /// accepted list left from NEXT (`None` when NEXT lies outside the
    /// ring) and whether a reload it requested is still pending.
    pub ring_base: u32,
    pub recycle_start: u32,
    pub accepted_tail: u32,
    pub observed: u32,
    pub completed: u32,
    pub remaining: Option<u32>,
    pub reload_pending: bool,
    /// Buffers released for rearm, buffers detached as staged leases, and
    /// claimed staging slots.
    pub released: u32,
    pub detached: u32,
    pub staged: u32,
}

const NEXT_UPPER: u32 = 1 << 0;
const BASE_READ: u32 = 1 << 1;
const WALKER_ENABLED: u32 = 1 << 2;
const RELOAD_DOORBELL: u32 = 1 << 3;
const BUFFER_FULL_READ: u32 = 1 << 4;
const REMAINING_KNOWN: u32 = 1 << 5;
const RELOAD_PENDING: u32 = 1 << 6;

/// The exhaustion snapshot of the longest full-pool stretch, and how many
/// such stretches a returned lease ended.
#[derive(bytemuck::Zeroable, Default)]
struct ExhaustionCounters {
    snapshots: AtomicU32,
    blocked_us: AtomicU32,
    flags: AtomicU32,
    next_low: AtomicU32,
    last_low: AtomicU32,
    base: AtomicU32,
    buffer_full: AtomicU32,
    ring_base: AtomicU32,
    recycle_start: AtomicU32,
    accepted_tail: AtomicU32,
    observed: AtomicU32,
    completed: AtomicU32,
    remaining: AtomicU32,
    released: AtomicU32,
    detached: AtomicU32,
    staged: AtomicU32,
}

/// One reading of the exhaustion snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxExhaustionReport {
    /// Full-pool stretches a returned lease ended.
    pub snapshots: u32,
    /// The snapshot of the longest of them; `None` before the first.
    pub longest: Option<WifiRxExhaustion>,
}

impl ExhaustionCounters {
    const fn new() -> Self {
        Self {
            snapshots: AtomicU32::new(0),
            blocked_us: AtomicU32::new(0),
            flags: AtomicU32::new(0),
            next_low: AtomicU32::new(0),
            last_low: AtomicU32::new(0),
            base: AtomicU32::new(0),
            buffer_full: AtomicU32::new(0),
            ring_base: AtomicU32::new(0),
            recycle_start: AtomicU32::new(0),
            accepted_tail: AtomicU32::new(0),
            observed: AtomicU32::new(0),
            completed: AtomicU32::new(0),
            remaining: AtomicU32::new(0),
            released: AtomicU32::new(0),
            detached: AtomicU32::new(0),
            staged: AtomicU32::new(0),
        }
    }

    /// Count `snapshot`; keep it when its block is the longest so far.
    fn record(&self, snapshot: WifiRxExhaustion) {
        let previous = self.snapshots.fetch_add(1, Ordering::Relaxed);
        if previous != 0 && snapshot.blocked_us < self.blocked_us.load(Ordering::Relaxed) {
            return;
        }
        let flag = |set: bool, bit: u32| if set { bit } else { 0 };
        let flags = flag(snapshot.next_upper, NEXT_UPPER)
            | flag(snapshot.base.is_some(), BASE_READ)
            | flag(snapshot.walker_enabled, WALKER_ENABLED)
            | flag(snapshot.reload_doorbell, RELOAD_DOORBELL)
            | flag(snapshot.buffer_full.is_some(), BUFFER_FULL_READ)
            | flag(snapshot.remaining.is_some(), REMAINING_KNOWN)
            | flag(snapshot.reload_pending, RELOAD_PENDING);
        let store = |counter: &AtomicU32, value: u32| counter.store(value, Ordering::Relaxed);
        store(&self.blocked_us, snapshot.blocked_us);
        store(&self.flags, flags);
        store(&self.next_low, snapshot.next_low);
        store(&self.last_low, snapshot.last_low);
        store(&self.base, snapshot.base.unwrap_or(0));
        store(
            &self.buffer_full,
            u32::from(snapshot.buffer_full.unwrap_or(0)),
        );
        store(&self.ring_base, snapshot.ring_base);
        store(&self.recycle_start, snapshot.recycle_start);
        store(&self.accepted_tail, snapshot.accepted_tail);
        store(&self.observed, snapshot.observed);
        store(&self.completed, snapshot.completed);
        store(&self.remaining, snapshot.remaining.unwrap_or(0));
        store(&self.released, snapshot.released);
        store(&self.detached, snapshot.detached);
        store(&self.staged, snapshot.staged);
    }

    fn report(&self) -> WifiRxExhaustionReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        let snapshots = load(&self.snapshots);
        let flags = load(&self.flags);
        let set = |bit: u32| flags & bit != 0;
        let longest = (snapshots != 0).then(|| WifiRxExhaustion {
            blocked_us: load(&self.blocked_us),
            next_low: load(&self.next_low),
            next_upper: set(NEXT_UPPER),
            last_low: load(&self.last_low),
            base: set(BASE_READ).then(|| load(&self.base)),
            walker_enabled: set(WALKER_ENABLED),
            reload_doorbell: set(RELOAD_DOORBELL),
            buffer_full: set(BUFFER_FULL_READ)
                .then(|| u16::try_from(load(&self.buffer_full)).unwrap_or(u16::MAX)),
            ring_base: load(&self.ring_base),
            recycle_start: load(&self.recycle_start),
            accepted_tail: load(&self.accepted_tail),
            observed: load(&self.observed),
            completed: load(&self.completed),
            remaining: set(REMAINING_KNOWN).then(|| load(&self.remaining)),
            reload_pending: set(RELOAD_PENDING),
            released: load(&self.released),
            detached: load(&self.detached),
            staged: load(&self.staged),
        });
        WifiRxExhaustionReport { snapshots, longest }
    }
}

/// One reading of [`WifiRxCounters`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxReport {
    /// MAC handler runs.
    pub mac_irqs: u32,
    /// MAC handler runs which saw no enabled cause.
    pub spurious_mac_irqs: u32,
    /// MAC handler runs which acknowledged an auxiliary cause.
    pub auxiliary_mac_irqs: u32,
    /// MAC handler runs which acknowledged a cause without a known meaning.
    pub unhandled_mac_irqs: u32,
    /// WDEVPWR handler runs which acknowledged a cause.
    pub power_irqs: u32,
    /// WDEVPWR handler runs which saw no enabled cause.
    pub spurious_power_irqs: u32,
    /// RX delivery restored through the doorbell.
    pub doorbell_unmasks: u32,
    /// Unmask requests ignored because both banks were masked.
    pub ignored_unmasks: u32,
    /// Units the publisher received.
    pub units: u32,
    /// Bytes of those units.
    pub bytes: u32,
    /// Chained units dropped.
    pub chained_drops: u32,
    /// Chains longer than one descriptor's length field, dropped.
    pub oversized_drops: u32,
    /// Empty units dropped.
    pub empty_drops: u32,
    /// Units longer than a staging lease, dropped.
    pub too_long_drops: u32,
    /// Bulk units dropped for want of an upper credit.
    pub overload_drops: u32,
    /// Units left at the ring head for want of a credit.
    pub admission_blocks: u32,
    /// Service passes which ended blocked on staging capacity.
    pub stage_capacity_blocks: u32,
    /// RX service transactions run.
    pub service_passes: u32,
    /// RX service transactions which failed.
    pub service_errors: u32,
    /// Drains started by a returned-capacity fact (a lease came back), with
    /// or without an RX fact beside it.
    pub capacity_drains: u32,
    /// The runner's continuations.
    pub continuations: WifiRxContinuationReport,
    /// The ring's reload-repair and republication evidence, as of the last
    /// service pass.
    pub reload: RxReloadRepairEvidence,
    /// The teardown's measured waits.
    pub teardown: WifiRxTeardownReport,
    /// The application's RX-control prefix decode.
    pub prefix: WifiRxPrefixReport,
    /// The walker and ring when a lease returned to a full staging pool.
    pub exhaustion: WifiRxExhaustionReport,
}

fn bump(counter: &AtomicU32) {
    counter.fetch_add(1, Ordering::Relaxed);
}

impl WifiRxCounters {
    pub const fn new() -> Self {
        Self {
            mac_irqs: AtomicU32::new(0),
            spurious_mac_irqs: AtomicU32::new(0),
            auxiliary_mac_irqs: AtomicU32::new(0),
            unhandled_mac_irqs: AtomicU32::new(0),
            power_irqs: AtomicU32::new(0),
            spurious_power_irqs: AtomicU32::new(0),
            doorbell_unmasks: AtomicU32::new(0),
            ignored_unmasks: AtomicU32::new(0),
            units: AtomicU32::new(0),
            bytes: AtomicU32::new(0),
            chained_drops: AtomicU32::new(0),
            oversized_drops: AtomicU32::new(0),
            empty_drops: AtomicU32::new(0),
            too_long_drops: AtomicU32::new(0),
            overload_drops: AtomicU32::new(0),
            admission_blocks: AtomicU32::new(0),
            stage_capacity_blocks: AtomicU32::new(0),
            service_passes: AtomicU32::new(0),
            service_errors: AtomicU32::new(0),
            capacity_drains: AtomicU32::new(0),
            continuations: ContinuationCounters::new(),
            reload: ReloadCounters::new(),
            teardown: TeardownCounters::new(),
            prefix: PrefixCounters::new(),
            exhaustion: ExhaustionCounters::new(),
        }
    }

    /// Read every counter.
    pub fn report(&self) -> WifiRxReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxReport {
            mac_irqs: load(&self.mac_irqs),
            spurious_mac_irqs: load(&self.spurious_mac_irqs),
            auxiliary_mac_irqs: load(&self.auxiliary_mac_irqs),
            unhandled_mac_irqs: load(&self.unhandled_mac_irqs),
            power_irqs: load(&self.power_irqs),
            spurious_power_irqs: load(&self.spurious_power_irqs),
            doorbell_unmasks: load(&self.doorbell_unmasks),
            ignored_unmasks: load(&self.ignored_unmasks),
            units: load(&self.units),
            bytes: load(&self.bytes),
            chained_drops: load(&self.chained_drops),
            oversized_drops: load(&self.oversized_drops),
            empty_drops: load(&self.empty_drops),
            too_long_drops: load(&self.too_long_drops),
            overload_drops: load(&self.overload_drops),
            admission_blocks: load(&self.admission_blocks),
            stage_capacity_blocks: load(&self.stage_capacity_blocks),
            service_passes: load(&self.service_passes),
            service_errors: load(&self.service_errors),
            capacity_drains: load(&self.capacity_drains),
            continuations: self.continuations.report(),
            reload: self.reload.report(),
            teardown: self.teardown.report(),
            prefix: self.prefix.report(),
            exhaustion: self.exhaustion.report(),
        }
    }

    #[inline(always)]
    pub(crate) fn mac_interrupt(&self, spurious: bool, auxiliary: bool, unhandled: bool) {
        bump(&self.mac_irqs);
        if spurious {
            bump(&self.spurious_mac_irqs);
        }
        if auxiliary {
            bump(&self.auxiliary_mac_irqs);
        }
        if unhandled {
            bump(&self.unhandled_mac_irqs);
        }
    }

    #[inline(always)]
    pub(crate) fn power_interrupt(&self, spurious: bool) {
        bump(if spurious {
            &self.spurious_power_irqs
        } else {
            &self.power_irqs
        });
    }

    #[inline(always)]
    pub(crate) fn doorbell_unmask(&self) {
        bump(&self.doorbell_unmasks);
    }

    #[inline(always)]
    pub(crate) fn ignored_unmask(&self) {
        bump(&self.ignored_unmasks);
    }

    pub(crate) fn received(&self, bytes: usize) {
        bump(&self.units);
        self.bytes
            .fetch_add(u32::try_from(bytes).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    pub(crate) fn discarded(&self, discard: Discard) {
        bump(match discard {
            Discard::Chained => &self.chained_drops,
            Discard::Oversized => &self.oversized_drops,
            Discard::Empty => &self.empty_drops,
            Discard::TooLong => &self.too_long_drops,
            Discard::OverloadBulk => &self.overload_drops,
        });
    }

    pub(crate) fn admission_blocked(&self) {
        bump(&self.admission_blocks);
    }

    pub(crate) fn stage_capacity_block(&self) {
        bump(&self.stage_capacity_blocks);
    }

    pub(crate) fn service_pass(&self) {
        bump(&self.service_passes);
    }

    pub(crate) fn service_error(&self) {
        bump(&self.service_errors);
    }

    pub(crate) fn capacity_drain(&self) {
        bump(&self.capacity_drains);
    }

    /// One continuation for `reason`, the `streak`-th of its drain.
    pub(crate) fn continuation(&self, reason: DatapathRxProgress, streak: u32) {
        if let Some(counter) = self.continuations.reason(reason) {
            bump(counter);
        }
        self.continuations
            .longest_streak
            .fetch_max(streak, Ordering::Relaxed);
    }

    pub(crate) fn immediate_continuation(&self) {
        bump(&self.continuations.immediate);
    }

    pub(crate) fn timed_continuation(&self) {
        bump(&self.continuations.timed);
    }

    pub(crate) fn mask_confirm(&self, waited: Duration) {
        self.teardown
            .mask_confirm_us
            .store(saturating_micros(waited), Ordering::Relaxed);
    }

    pub(crate) fn drain(&self, waited: Duration, polls: u32) {
        self.teardown
            .drain_us
            .store(saturating_micros(waited), Ordering::Relaxed);
        self.teardown.drain_polls.store(polls, Ordering::Relaxed);
    }

    pub(crate) fn prefix_refused(&self, reason: RxError) {
        bump(self.prefix.refused(reason));
    }

    pub(crate) fn strict_refusable(&self, rxend: bool, dump: bool) {
        if rxend {
            bump(&self.prefix.strict_rxend_refusable);
        }
        if dump {
            bump(&self.prefix.strict_dump_refusable);
        }
    }

    /// A lease returned to a full pool; `snapshot` is what the runner read
    /// before its capacity drain.
    pub(crate) fn exhaustion(&self, snapshot: WifiRxExhaustion) {
        self.exhaustion.record(snapshot);
    }

    pub(crate) fn reload_evidence(&self, evidence: RxReloadRepairEvidence) {
        self.reload.record(evidence);
    }
}
