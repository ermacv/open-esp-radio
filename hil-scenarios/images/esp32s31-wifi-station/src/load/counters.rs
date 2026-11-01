//! The window's counters: the boot hart's (the radio's and the station's)
//! cycles and retired instructions, the L1 cache's access counts by bus,
//! what the network bridge did with the station's frames, what the IP
//! stack took, and the receive path's counts: the S31 driver's and the
//! station service's receive decisions (the facade's `StationRx` events),
//! and the link's rates: the data MPDUs addressed to the station by their
//! PPDU's rate and A-MPDU containment (`RxData`), and the station's single
//! data transmissions by their last rate and end (`TxData`).
//!
//! The bridge reports each frame through `oer_trace::observe!`, so only an
//! `oer_trace` build counts them; hart 1's network task is their one writer
//! and the load server, in the same task, their reader. The station's
//! receive decisions and the link's rates reach the image's observer from
//! the station service, on the boot hart, and count there; the driver's counters are the
//! radio's, on the boot hart too. The boot hart's relay, which owns the
//! cache's counters, reads the boot hart's counters, the cache's and the
//! receive path's there at the window's ends and sends them to the server;
//! the stack's are read on hart 1. The cache's are cleared just before the
//! window's start. Bridge, cache and receive counts wrap at 2^32 (a cache
//! bus reports its overflow); the stack's saturate there.

use oer_services_load::{
    CacheCounts, CacheGeometry, Counters, DataBus, DriverRx, InstructionBus, Link, RxCounts,
    StationRx, TxAmpdu, TxCounts,
};

/// What the network bridge did with one frame from the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build reports the bridge's frames")
)]
pub(crate) enum Bridged {
    /// The bridge took it from the station; a refusal below follows when
    /// the stack does not take it.
    Received,
    /// The stack's receive pool had no packet for it.
    RefusedPool,
    /// The stack's receive queue had no room for it.
    RefusedQueue,
    /// Its length or a down link refused it.
    RefusedOther,
    /// The station lent it in place and the bridge copied it after all.
    Copied,
}

#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build reports the bridge's frames")
)]
impl Bridged {
    const ALL: [Bridged; 5] = [
        Bridged::Received,
        Bridged::RefusedPool,
        Bridged::RefusedQueue,
        Bridged::RefusedOther,
        Bridged::Copied,
    ];

    #[cfg(any(oer_trace, test))]
    fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|outcome| *outcome as u32 == raw)
    }
}

/// One frame's outcome at the network bridge.
#[cfg(any(oer_trace, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BridgeFrame {
    pub outcome: Bridged,
}

#[cfg(any(oer_trace, test))]
impl oer_trace::Event for BridgeFrame {
    const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Platform, 42);
    // The executors' activity channel, beside the parts' polls.
    const CHANNEL: oer_trace::Channel = const {
        match oer_trace::Channel::new(oer_trace::Domain::Platform, 7) {
            Ok(channel) => channel,
            Err(_) => panic!("the activity channel is in the platform range"),
        }
    };
    fn encode(&self) -> [u32; 2] {
        [self.outcome as u32, 0]
    }
    fn decode([outcome, reserved]: [u32; 2]) -> Option<Self> {
        if reserved != 0 {
            return None;
        }
        Some(Self {
            outcome: Bridged::from_raw(outcome)?,
        })
    }
}

/// The bridge's counts by outcome.
#[cfg(target_arch = "riscv32")]
static BRIDGE: [core::sync::atomic::AtomicU32; Bridged::ALL.len()] =
    [const { core::sync::atomic::AtomicU32::new(0) }; Bridged::ALL.len()];

/// Count `frame`; only hart 1's network task calls it.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe(frame: BridgeFrame) {
    use core::sync::atomic::Ordering;
    if let Some(count) = BRIDGE.get(usize::from(frame.outcome as u8)) {
        count.store(
            count.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }
}

/// The station's receive decisions by kind, and the latest agreement's
/// window.
#[cfg(target_arch = "riscv32")]
static STATION: [core::sync::atomic::AtomicU32; STATION_KINDS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; STATION_KINDS];
#[cfg(target_arch = "riscv32")]
static WINDOW: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(target_arch = "riscv32")]
const STATION_KINDS: usize = oer_esp32s31_wifi::observation::StationRxKind::ALL.len();

/// Count one receive decision; only the station service, on the boot
/// hart, calls it, and only the boot hart's relay reads the counts.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe_station(decision: oer_esp32s31_wifi::observation::StationRx) {
    use core::sync::atomic::Ordering;
    use oer_esp32s31_wifi::observation::StationRxKind;
    let bump = |count: &core::sync::atomic::AtomicU32, by: u32| {
        count.store(
            count.load(Ordering::Relaxed).wrapping_add(by),
            Ordering::Relaxed,
        );
    };
    if decision.kind == StationRxKind::AgreementStarted {
        bump(&STARTED, 1);
        WINDOW.store(decision.count, Ordering::Relaxed);
    } else if let Some(count) = STATION.get(usize::from(decision.kind as u8)) {
        bump(count, decision.count);
    }
}

/// The station's receive decisions now, by kind, as the wire names them;
/// the started-agreement kind counts agreements, its window aside.
#[cfg(target_arch = "riscv32")]
fn station_rx() -> StationRx {
    use core::sync::atomic::Ordering;
    use oer_esp32s31_wifi::observation::StationRxKind as Kind;
    let at = |kind: Kind| {
        STATION
            .get(usize::from(kind as u8))
            .map_or(0, |count| count.load(Ordering::Relaxed))
    };
    StationRx {
        port_events_lost: at(Kind::PortEventsLost),
        dropped_header: at(Kind::DroppedHeader),
        dropped_frame_kind: at(Kind::DroppedFrameKind),
        dropped_address: at(Kind::DroppedAddress),
        dropped_fragmented: at(Kind::DroppedFragmented),
        dropped_protection: at(Kind::DroppedProtection),
        dropped_integrity: at(Kind::DroppedIntegrity),
        dropped_replay: at(Kind::DroppedReplay),
        dropped_payload: at(Kind::DroppedPayload),
        dropped_not_connected: at(Kind::DroppedNotConnected),
        dropped_security: at(Kind::DroppedSecurity),
        dropped_draining: at(Kind::DroppedDraining),
        refused_too_long: at(Kind::RefusedTooLong),
        refused_queue_full: at(Kind::RefusedQueueFull),
        refused_no_network: at(Kind::RefusedNoNetwork),
        copied_aggregated: at(Kind::CopiedAggregated),
        copied_kept: at(Kind::CopiedKept),
        mpdus_in_ampdu: at(Kind::MpduInAmpdu),
        mpdus_single: at(Kind::MpduSingle),
        mpdus_unknown: at(Kind::MpduUnknown),
        released_in_order: at(Kind::ReleasedInOrder),
        released_by_timeout: at(Kind::ReleasedByTimeout),
        released_by_bar: at(Kind::ReleasedByBar),
        released_by_stop: at(Kind::ReleasedByStop),
        missing_by_timeout: at(Kind::MissingByTimeout),
        missing_other: at(Kind::MissingOther),
        duplicates: at(Kind::Duplicate),
        rejected: at(Kind::Rejected),
        agreements_started: STARTED.load(Ordering::Relaxed),
        agreements_stopped: at(Kind::AgreementStopped),
        agreement_window: u16::try_from(WINDOW.load(Ordering::Relaxed)).unwrap_or(u16::MAX),
        rsn_inputs_dropped: at(Kind::RsnInputDropped),
    }
}

/// The station's transmit agreements' settlements, as the wire names
/// them; only the station service, on the boot hart, writes them.
#[cfg(target_arch = "riscv32")]
static TX_AMPDU: [core::sync::atomic::AtomicU32; TX_AMPDU_COUNTS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; TX_AMPDU_COUNTS];
/// Aggregates, refused, subframes, the six size bins, acknowledged,
/// retried, dropped and BlockAckReqs.
#[cfg(target_arch = "riscv32")]
const TX_AMPDU_COUNTS: usize = 13;

/// The histogram bin of an aggregate of `members`: 1, 2, 3 to 4, 5 to 8,
/// 9 to 16, 17 to 32 (and more).
#[cfg(any(all(target_arch = "riscv32", oer_trace), test))]
const fn size_bin(members: u8) -> usize {
    match members {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        _ => 5,
    }
}

/// Count one settlement or BlockAckReq; only the station service, on the
/// boot hart, calls it, and only the boot hart's relay reads the counts.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe_station_tx(settled: oer_esp32s31_wifi::observation::StationTxAmpdu) {
    use core::sync::atomic::Ordering;
    let bump = |index: usize, by: u32| {
        if let Some(count) = TX_AMPDU.get(index) {
            count.store(
                count.load(Ordering::Relaxed).wrapping_add(by),
                Ordering::Relaxed,
            );
        }
    };
    if settled.block_ack_request {
        bump(12, 1);
        return;
    }
    bump(0, 1);
    bump(1, u32::from(settled.refused));
    bump(2, u32::from(settled.members));
    bump(3_usize.saturating_add(size_bin(settled.members)), 1);
    bump(9, u32::from(settled.acknowledged));
    bump(10, u32::from(settled.retried));
    bump(11, u32::from(settled.dropped));
}

/// The transmit agreements' counts now, as the wire names them.
#[cfg(target_arch = "riscv32")]
fn tx_ampdu() -> TxAmpdu {
    use core::sync::atomic::Ordering;
    let at = |index: usize| {
        TX_AMPDU
            .get(index)
            .map_or(0, |count| count.load(Ordering::Relaxed))
    };
    // The histogram wraps at 2^16 on the wire.
    let bin = |index: usize| at(index) as u16;
    TxAmpdu {
        aggregates: at(0),
        refused: at(1),
        subframes: at(2),
        subframes_1: bin(3),
        subframes_2: bin(4),
        subframes_3_4: bin(5),
        subframes_5_8: bin(6),
        subframes_9_16: bin(7),
        subframes_17_32: bin(8),
        acknowledged: at(9),
        retried: at(10),
        dropped: at(11),
        block_ack_requests: at(12),
    }
}

/// Receive agreements started: a started agreement's count is its window.
#[cfg(target_arch = "riscv32")]
static STARTED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// One rate histogram's buckets: HT MCS 0 to 7 by width and guard
/// interval (20 MHz long, 20 MHz short, 40 MHz long, 40 MHz short), then
/// the other HT MCSs, non-HT, HE and no rate.
#[cfg(any(target_arch = "riscv32", test))]
const RATE_BUCKETS: usize = 4 * 8 + 4;
#[cfg(any(target_arch = "riscv32", test))]
const HT_OTHER: usize = 32;
#[cfg(any(target_arch = "riscv32", test))]
const NON_HT: usize = 33;
#[cfg(any(target_arch = "riscv32", test))]
const HE: usize = 34;
#[cfg(any(target_arch = "riscv32", test))]
const UNKNOWN_RATE: usize = 35;

/// The bucket of an HT rate of `mcs`, `width40` wide, with the short guard
/// interval or the long one.
#[cfg(any(target_arch = "riscv32", test))]
const fn ht_bucket(mcs: u8, width40: bool, short_gi: bool) -> usize {
    if mcs > 7 {
        return HT_OTHER;
    }
    (width40 as usize * 2 + short_gi as usize) * 8 + mcs as usize
}

/// The wire's histogram of `buckets`.
#[cfg(any(target_arch = "riscv32", test))]
fn rates(buckets: &[u32; RATE_BUCKETS]) -> oer_services_load::Rates {
    let at = |bucket: usize| buckets.get(bucket).copied().unwrap_or(0);
    let mcs =
        |width40, short_gi| core::array::from_fn(|mcs| at(ht_bucket(mcs as u8, width40, short_gi)));
    oer_services_load::Rates {
        ht20_long_gi: mcs(false, false),
        ht20_short_gi: mcs(false, true),
        ht40_long_gi: mcs(true, false),
        ht40_short_gi: mcs(true, true),
        ht_other: at(HT_OTHER),
        non_ht: at(NON_HT),
        he: at(HE),
        unknown: at(UNKNOWN_RATE),
    }
}

/// The received data MPDUs by rate and by containment (unknown, single,
/// in an A-MPDU); the station's single data transmissions by rate, by end
/// (delivered, ACK timeout, other, unsent) and their transmissions. The
/// station service, on the boot hart, is their one writer.
#[cfg(target_arch = "riscv32")]
static RX_RATES: [core::sync::atomic::AtomicU32; RATE_BUCKETS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; RATE_BUCKETS];
#[cfg(target_arch = "riscv32")]
static RX_CONTAINMENT: [core::sync::atomic::AtomicU32; 3] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 3];
#[cfg(target_arch = "riscv32")]
static TX_RATES: [core::sync::atomic::AtomicU32; RATE_BUCKETS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; RATE_BUCKETS];
#[cfg(target_arch = "riscv32")]
static TX_ENDS: [core::sync::atomic::AtomicU32; 4] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 4];
#[cfg(target_arch = "riscv32")]
static TX_TRANSMISSIONS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Add `by` to `count`; only its one writer calls it.
#[cfg(all(target_arch = "riscv32", oer_trace))]
fn bump(count: Option<&core::sync::atomic::AtomicU32>, by: u32) {
    use core::sync::atomic::Ordering;
    if let Some(count) = count {
        count.store(
            count.load(Ordering::Relaxed).wrapping_add(by),
            Ordering::Relaxed,
        );
    }
}

/// The bucket of `rate`.
#[cfg(all(target_arch = "riscv32", oer_trace))]
const fn rate_bucket(rate: oer_esp32s31_wifi::observation::RateClass) -> usize {
    use oer_esp32s31_wifi::observation::RateClass;
    match rate {
        RateClass::Ht {
            mcs,
            width40,
            short_gi,
        } => ht_bucket(mcs, width40, short_gi),
        RateClass::NonHt => NON_HT,
        RateClass::He => HE,
        RateClass::Unknown => UNKNOWN_RATE,
    }
}

/// Count a received data MPDU; only the station service, on the boot hart,
/// calls it.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe_rx_data(data: oer_esp32s31_wifi::observation::RxData) {
    bump(RX_RATES.get(rate_bucket(data.rate)), 1);
    bump(RX_CONTAINMENT.get(usize::from(data.containment as u8)), 1);
}

/// Count a single data transmission's end; only the station service, on
/// the boot hart, calls it.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe_tx_data(data: oer_esp32s31_wifi::observation::TxData) {
    use oer_esp32s31_wifi::observation::TxEnd;
    bump(TX_RATES.get(rate_bucket(data.rate)), 1);
    let end = match data.end {
        TxEnd::Delivered => 0,
        TxEnd::AckTimeout => 1,
        TxEnd::Other => 2,
        TxEnd::Unsent => 3,
    };
    bump(TX_ENDS.get(end), 1);
    bump(Some(&TX_TRANSMISSIONS), u32::from(data.transmissions));
}

/// The link's counts now, as the wire names them.
#[cfg(target_arch = "riscv32")]
fn link() -> Link {
    use core::sync::atomic::{AtomicU32, Ordering};
    let read = |counts: &[AtomicU32; RATE_BUCKETS]| {
        counts.each_ref().map(|count| count.load(Ordering::Relaxed))
    };
    let [unknown, single, in_ampdu] = RX_CONTAINMENT
        .each_ref()
        .map(|count| count.load(Ordering::Relaxed));
    let [delivered, ack_timeouts, other_ends, unsent] = TX_ENDS
        .each_ref()
        .map(|count| count.load(Ordering::Relaxed));
    Link {
        rx: oer_services_load::RxLink {
            mpdus: rates(&read(&RX_RATES)),
            in_ampdu,
            single,
            containment_unknown: unknown,
        },
        tx: oer_services_load::TxLink {
            mpdus: rates(&read(&TX_RATES)),
            transmissions: TX_TRANSMISSIONS.load(Ordering::Relaxed),
            delivered,
            ack_timeouts,
            other_ends,
            unsent,
        },
    }
}

/// The driver's receive counters now, as the wire names them; unavailable
/// before the receive path was bound or without a trace build.
#[cfg(target_arch = "riscv32")]
fn driver_rx() -> DriverRx {
    #[cfg(oer_trace)]
    if let Some(report) = oer_esp32s31_wifi::rx_counters() {
        let prefix = report.prefix;
        return DriverRx {
            available: true,
            mac_irqs: report.mac_irqs,
            units: report.units,
            bytes: report.bytes,
            overload_drops: report.overload_drops,
            admission_blocks: report.admission_blocks,
            stage_capacity_blocks: report.stage_capacity_blocks,
            chained_drops: report.chained_drops,
            oversized_drops: report.oversized_drops,
            empty_drops: report.empty_drops,
            too_long_drops: report.too_long_drops,
            service_passes: report.service_passes,
            service_errors: report.service_errors,
            capacity_drains: report.capacity_drains,
            continued_immediately: report.continuations.immediate,
            continued_after_wait: report.continuations.timed,
            rx_failures: prefix.rx_failure,
            mic_failures: prefix.mic_failure,
            quarantined: prefix.quarantined,
            prefix_other: prefix
                .invalid
                .wrapping_add(prefix.chain)
                .wrapping_add(prefix.bounds)
                .wrapping_add(prefix.output_small)
                .wrapping_add(prefix.metadata)
                .wrapping_add(prefix.ignored)
                .wrapping_add(prefix.unsupported),
        };
    }
    DriverRx::default()
}

/// What one end of the window read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub cycles: u64,
    pub instructions: u64,
    pub bridge: [u32; Bridged::ALL.len()],
    pub stack_received: u32,
    pub cache: CacheCounts,
    pub cache_geometry: CacheGeometry,
    pub rx: RxCounts,
    pub tx: TxCounts,
    pub link: Link,
}

/// The cache's counters of every bus.
#[cfg(target_arch = "riscv32")]
pub(crate) type Cache = oer_esp32s31_hal::peripherals::cache_counters::CacheCounters;

/// What the boot hart's relay reads there for the load server: the boot
/// hart's cycles and retired instructions, the cache's counts and
/// geometry, and the receive path's and the link's counts, which only the
/// boot hart writes.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Hart {
    pub cycles: u64,
    pub instructions: u64,
    pub cache: CacheCounts,
    pub cache_geometry: CacheGeometry,
    pub rx: RxCounts,
    pub tx: TxCounts,
    pub link: Link,
}

#[cfg(target_arch = "riscv32")]
impl Hart {
    /// The counters of the hart that calls it and `cache`'s; with `start`,
    /// clear and run every bus's cache counters first: the window's start.
    pub(crate) fn now(cache: &mut Cache, start: bool) -> Self {
        if start {
            cache.start(oer_esp32s31_hal::peripherals::cache_counters::Buses::ALL);
        }
        let cycles = oer_riscv::cycle64();
        let instructions = oer_riscv::instret64();
        let counts = cache.snapshot();
        Self {
            cycles,
            instructions,
            cache: CacheCounts {
                ibus0: instruction(counts.ibus0),
                ibus1: instruction(counts.ibus1),
                dbus0: data(counts.dbus0),
                dbus1: data(counts.dbus1),
            },
            cache_geometry: geometry(cache.geometry()),
            rx: RxCounts {
                driver: driver_rx(),
                station: station_rx(),
            },
            tx: TxCounts { ampdu: tx_ampdu() },
            link: link(),
        }
    }
}

/// The counters now: the boot hart's `hart` with the cache's and the
/// receive path's, the bridge's, and the stack's received frames
/// `stack_received`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn snapshot(hart: Hart, stack_received: u32) -> Snapshot {
    use core::sync::atomic::Ordering;
    Snapshot {
        cycles: hart.cycles,
        instructions: hart.instructions,
        bridge: BRIDGE.each_ref().map(|count| count.load(Ordering::Relaxed)),
        stack_received,
        cache: hart.cache,
        cache_geometry: hart.cache_geometry,
        rx: hart.rx,
        tx: hart.tx,
        link: hart.link,
    }
}

#[cfg(target_arch = "riscv32")]
fn instruction(
    bus: oer_esp32s31_hal::peripherals::cache_counters::InstructionBus,
) -> InstructionBus {
    InstructionBus {
        hits: bus.hits,
        misses: bus.misses,
        conflicts: bus.conflicts,
        next_level_reads: bus.next_level_reads,
        overflowed: bus.overflowed,
    }
}

#[cfg(target_arch = "riscv32")]
fn data(bus: oer_esp32s31_hal::peripherals::cache_counters::DataBus) -> DataBus {
    DataBus {
        hits: bus.hits,
        misses: bus.misses,
        conflicts: bus.conflicts,
        next_level_reads: bus.next_level_reads,
        next_level_writes: bus.next_level_writes,
        overflowed: bus.overflowed,
    }
}

#[cfg(target_arch = "riscv32")]
fn geometry(geometry: oer_esp32s31_hal::peripherals::cache_counters::Geometry) -> CacheGeometry {
    use oer_esp32s31_hal::peripherals::cache_counters::Level;
    use oer_services_load::CacheLevel;
    let level = |level: Level, ways: Option<u32>| CacheLevel {
        size_bytes: level.size_bytes,
        line_bytes: level.line_bytes,
        ways,
    };
    CacheGeometry {
        l1_icache: level(geometry.l1_icache, Some(geometry.l1_icache_ways)),
        l1_dcache: level(geometry.l1_dcache, Some(geometry.l1_dcache_ways)),
        l2: level(geometry.l2, None),
        l2_bypassed: geometry.l2_bypassed,
    }
}

/// The counts of one instruction bus between `start` and `end`.
fn instruction_window(start: &InstructionBus, end: &InstructionBus) -> InstructionBus {
    InstructionBus {
        hits: end.hits.wrapping_sub(start.hits),
        misses: end.misses.wrapping_sub(start.misses),
        conflicts: end.conflicts.wrapping_sub(start.conflicts),
        next_level_reads: end.next_level_reads.wrapping_sub(start.next_level_reads),
        overflowed: start.overflowed || end.overflowed,
    }
}

/// The counts of one data bus between `start` and `end`.
fn data_window(start: &DataBus, end: &DataBus) -> DataBus {
    DataBus {
        hits: end.hits.wrapping_sub(start.hits),
        misses: end.misses.wrapping_sub(start.misses),
        conflicts: end.conflicts.wrapping_sub(start.conflicts),
        next_level_reads: end.next_level_reads.wrapping_sub(start.next_level_reads),
        next_level_writes: end.next_level_writes.wrapping_sub(start.next_level_writes),
        overflowed: start.overflowed || end.overflowed,
    }
}

/// The link's counts between the window's ends `start` and `end`.
pub(crate) fn link_window(start: &Snapshot, end: &Snapshot) -> Link {
    Link::window(&start.link, &end.link)
}

/// The counts between the window's ends `start` and `end`.
pub(crate) fn window(start: &Snapshot, end: &Snapshot) -> Counters {
    let bridge = |outcome: Bridged| {
        let at = |snapshot: &Snapshot| {
            snapshot
                .bridge
                .get(usize::from(outcome as u8))
                .copied()
                .unwrap_or(0)
        };
        at(end).wrapping_sub(at(start))
    };
    Counters {
        cycles: end.cycles.wrapping_sub(start.cycles),
        instructions: end.instructions.wrapping_sub(start.instructions),
        bridge_received: bridge(Bridged::Received),
        bridge_refused_pool: bridge(Bridged::RefusedPool),
        bridge_refused_queue: bridge(Bridged::RefusedQueue),
        bridge_refused_other: bridge(Bridged::RefusedOther),
        bridge_copied: bridge(Bridged::Copied),
        stack_received: end.stack_received.saturating_sub(start.stack_received),
        cache: CacheCounts {
            ibus0: instruction_window(&start.cache.ibus0, &end.cache.ibus0),
            ibus1: instruction_window(&start.cache.ibus1, &end.cache.ibus1),
            dbus0: data_window(&start.cache.dbus0, &end.cache.dbus0),
            dbus1: data_window(&start.cache.dbus1, &end.cache.dbus1),
        },
        cache_geometry: end.cache_geometry,
        rx: RxCounts {
            driver: DriverRx::window(&start.rx.driver, &end.rx.driver),
            station: StationRx::window(&start.rx.station, &end.rx.station),
        },
        tx: TxCounts {
            ampdu: TxAmpdu::window(&start.tx.ampdu, &end.tx.ampdu),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_trace::Event as _;

    #[test]
    fn a_window_takes_wrapping_bridge_and_hart_differences() {
        let start = Snapshot {
            cycles: u64::MAX - 1,
            instructions: 10,
            bridge: [u32::MAX, 1, 2, 3, 6],
            stack_received: 5,
            ..Snapshot::default()
        };
        let end = Snapshot {
            cycles: 3,
            instructions: 25,
            bridge: [4, 1, 7, 3, 8],
            stack_received: 9,
            ..Snapshot::default()
        };
        assert_eq!(
            window(&start, &end),
            Counters {
                cycles: 5,
                instructions: 15,
                bridge_received: 5,
                bridge_refused_pool: 0,
                bridge_refused_queue: 5,
                bridge_refused_other: 0,
                bridge_copied: 2,
                stack_received: 4,
                ..Counters::default()
            }
        );
    }

    #[test]
    fn a_window_takes_wrapping_cache_differences_keeps_overflow_and_the_end_s_geometry() {
        let start = Snapshot {
            cache: CacheCounts {
                ibus0: InstructionBus {
                    hits: u32::MAX - 2,
                    misses: 10,
                    conflicts: 1,
                    next_level_reads: 10,
                    overflowed: false,
                },
                dbus0: DataBus {
                    hits: 100,
                    next_level_writes: 4,
                    ..DataBus::default()
                },
                ..CacheCounts::default()
            },
            ..Snapshot::default()
        };
        let geometry = CacheGeometry {
            l1_icache: oer_services_load::CacheLevel {
                size_bytes: Some(32 * 1024),
                line_bytes: Some(64),
                ways: Some(2),
            },
            ..CacheGeometry::default()
        };
        let end = Snapshot {
            cache: CacheCounts {
                ibus0: InstructionBus {
                    hits: 7,
                    misses: 25,
                    conflicts: 1,
                    next_level_reads: 30,
                    overflowed: false,
                },
                dbus0: DataBus {
                    hits: 160,
                    misses: 9,
                    next_level_writes: 6,
                    ..DataBus::default()
                },
                dbus1: DataBus {
                    overflowed: true,
                    ..DataBus::default()
                },
                ..CacheCounts::default()
            },
            cache_geometry: geometry,
            ..Snapshot::default()
        };
        let counters = window(&start, &end);
        assert_eq!(
            counters.cache.ibus0,
            InstructionBus {
                hits: 10,
                misses: 15,
                conflicts: 0,
                next_level_reads: 20,
                overflowed: false,
            }
        );
        assert_eq!(
            counters.cache.dbus0,
            DataBus {
                hits: 60,
                misses: 9,
                next_level_writes: 2,
                ..DataBus::default()
            }
        );
        assert!(counters.cache.dbus1.overflowed);
        assert_eq!(counters.cache.ibus1, InstructionBus::default());
        assert_eq!(counters.cache_geometry, geometry);
        let overflowed_at_start = Snapshot {
            cache: CacheCounts {
                ibus1: InstructionBus {
                    overflowed: true,
                    ..InstructionBus::default()
                },
                ..CacheCounts::default()
            },
            ..Snapshot::default()
        };
        assert!(
            window(&overflowed_at_start, &Snapshot::default())
                .cache
                .ibus1
                .overflowed
        );
    }

    #[test]
    fn a_window_takes_the_receive_path_s_differences() {
        let reading = |units, timeout, window| Snapshot {
            rx: RxCounts {
                driver: DriverRx {
                    available: true,
                    units,
                    ..DriverRx::default()
                },
                station: StationRx {
                    released_by_timeout: timeout,
                    agreement_window: window,
                    ..StationRx::default()
                },
            },
            ..Snapshot::default()
        };
        let rx = window(&reading(10, 1, 16), &reading(25, 4, 16)).rx;
        assert!(rx.driver.available);
        assert_eq!(rx.driver.units, 15);
        assert_eq!(rx.station.released_by_timeout, 3);
        assert_eq!(rx.station.agreement_window, 16);
    }

    #[test]
    fn an_aggregate_counts_in_the_bin_of_its_subframes() {
        let bins: std::vec::Vec<_> = [1, 2, 3, 4, 5, 8, 9, 16, 17, 32]
            .into_iter()
            .map(size_bin)
            .collect();
        assert_eq!(bins, [0, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
    }

    #[test]
    fn a_window_takes_the_transmit_path_s_differences() {
        let reading = |aggregates, bin| Snapshot {
            tx: TxCounts {
                ampdu: TxAmpdu {
                    aggregates,
                    subframes_17_32: bin,
                    ..TxAmpdu::default()
                },
            },
            ..Snapshot::default()
        };
        let tx = window(&reading(10, u16::MAX), &reading(14, 3)).tx;
        assert_eq!((tx.ampdu.aggregates, tx.ampdu.subframes_17_32), (4, 4));
    }

    #[test]
    fn each_rate_counts_in_its_own_bucket_of_the_wire_s_histogram() {
        let mut buckets = [0; RATE_BUCKETS];
        let mut count = |bucket: usize, by: u32| buckets[bucket] += by;
        count(ht_bucket(7, true, true), 5);
        count(ht_bucket(7, true, false), 4);
        count(ht_bucket(0, false, false), 3);
        count(ht_bucket(6, false, true), 2);
        count(ht_bucket(8, true, true), 9);
        count(ht_bucket(31, false, false), 1);
        count(NON_HT, 6);
        count(HE, 7);
        count(UNKNOWN_RATE, 8);
        let rates = rates(&buckets);
        assert_eq!(rates.ht40_short_gi, [0, 0, 0, 0, 0, 0, 0, 5]);
        assert_eq!(rates.ht40_long_gi, [0, 0, 0, 0, 0, 0, 0, 4]);
        assert_eq!(rates.ht20_long_gi, [3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(rates.ht20_short_gi, [0, 0, 0, 0, 0, 0, 2, 0]);
        assert_eq!(
            (rates.ht_other, rates.non_ht, rates.he, rates.unknown),
            (10, 6, 7, 8)
        );
        let distinct: std::collections::BTreeSet<_> = (0..=7u8)
            .flat_map(|mcs| {
                [(false, false), (false, true), (true, false), (true, true)]
                    .map(|(width40, short_gi)| ht_bucket(mcs, width40, short_gi))
            })
            .collect();
        assert_eq!(distinct, (0..HT_OTHER).collect());
    }

    #[test]
    fn a_link_window_is_the_wrapping_difference_of_its_ends() {
        let mut start = Snapshot::default();
        start.link.rx.mpdus.ht40_short_gi[7] = u32::MAX;
        start.link.tx.transmissions = 7;
        let mut end = start;
        end.link.rx.mpdus.ht40_short_gi[7] = 99;
        end.link.tx.transmissions = 20;
        let link = link_window(&start, &end);
        assert_eq!(link.rx.mpdus.ht40_short_gi[7], 100);
        assert_eq!(link.tx.transmissions, 13);
    }

    #[test]
    fn a_bridge_frame_round_trips_and_refuses_unknown_words() {
        for outcome in Bridged::ALL {
            let frame = BridgeFrame { outcome };
            assert_eq!(BridgeFrame::decode(frame.encode()), Some(frame));
        }
        assert_eq!(BridgeFrame::decode([5, 0]), None);
        assert_eq!(BridgeFrame::decode([0, 1]), None);
    }
}
