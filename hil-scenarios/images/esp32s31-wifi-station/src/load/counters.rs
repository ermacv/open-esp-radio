//! The window's counters: the boot hart's (the radio's and the station's)
//! cycles and retired instructions, the L1 cache's access counts by bus,
//! what the network bridge did with the station's frames, what the IP
//! stack took, and the receive path's counts: the S31 driver's and the
//! station service's receive decisions (the facade's `StationRx` events).
//!
//! The bridge reports each frame through `oer_trace::observe!`, so only an
//! `oer_trace` build counts them; hart 1's network task is their one writer
//! and the load server, in the same task, their reader. The station's
//! receive decisions reach the image's observer from the station service,
//! on the boot hart, and count there; the driver's counters are the
//! radio's, on the boot hart too. The boot hart's relay, which owns the
//! cache's counters, reads the boot hart's counters, the cache's and the
//! receive path's there at the window's ends and sends them to the server;
//! the stack's are read on hart 1. The cache's are cleared just before the
//! window's start. Bridge, cache and receive counts wrap at 2^32 (a cache
//! bus reports its overflow); the stack's saturate there.

use oer_services_load::{
    CacheCounts, CacheGeometry, Counters, DataBus, DriverRx, InstructionBus, RxCounts, StationRx,
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

/// Receive agreements started: a started agreement's count is its window.
#[cfg(target_arch = "riscv32")]
static STARTED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
}

/// The cache's counters of every bus.
#[cfg(target_arch = "riscv32")]
pub(crate) type Cache = oer_esp32s31_hal::peripherals::cache_counters::CacheCounters;

/// What the boot hart's relay reads there for the load server: the boot
/// hart's cycles and retired instructions, the cache's counts and
/// geometry, and the receive path's counts, which only the boot hart
/// writes.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Hart {
    pub cycles: u64,
    pub instructions: u64,
    pub cache: CacheCounts,
    pub cache_geometry: CacheGeometry,
    pub rx: RxCounts,
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
    fn a_bridge_frame_round_trips_and_refuses_unknown_words() {
        for outcome in Bridged::ALL {
            let frame = BridgeFrame { outcome };
            assert_eq!(BridgeFrame::decode(frame.encode()), Some(frame));
        }
        assert_eq!(BridgeFrame::decode([5, 0]), None);
        assert_eq!(BridgeFrame::decode([0, 1]), None);
    }
}
