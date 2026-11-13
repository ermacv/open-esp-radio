//! The library's observations (`oer::observe`) in the load
//! service's vocabulary: the executors' CPU, the parts' polls, the PHY
//! tracking ticks, the link's and the station's counts, the boot hart's
//! counters and the sampler's pages, each the difference of a window's two
//! readings where the service reports one.

use oer::observe::addba;
use oer::observe::cpu::ExecutorSample;
use oer::observe::link;
use oer::observe::parts::{PARTS, PartReading};
use oer::observe::tracking::TrackingSample;
use oer_hil_services_load::{
    Addba, AddbaOutcome, AddbaRequest, CacheCounts, CacheGeometry, Counters, DataBus, DriverRx,
    ExecutorLoad, HartLoad, InstructionBus, Link, Load, MAX_PAGE_SAMPLES, MAX_PARTS, Mac,
    PartWindow, Parts, Positions, Rates, RxCounts, RxLink, Sample, Samples, StallBoundary,
    StationRx, Tracking, TxAmpdu, TxCounts, TxLink,
};
use oer_portable_trace::profile::Thinning;

/// The CPU report at image microsecond `now_us` of every executor's
/// sample: a hart's idle time for each executor that saw idle, the work of
/// each that saw work. Only differences between two reports count.
pub(crate) fn load(now_us: u64, samples: &[ExecutorSample]) -> Load {
    let mut load = Load {
        now_us,
        ..Load::default()
    };
    for sample in samples {
        if let Some(idle_us) = sample.idle_us {
            match load.harts.iter_mut().find(|hart| hart.hart == sample.hart) {
                Some(hart) => {
                    hart.idle_us = hart.idle_us.saturating_add(idle_us);
                    hart.wakes = hart.wakes.wrapping_add(sample.wakes);
                }
                // The library's harts fit the report's: a full one only
                // loses the hart.
                None => {
                    let _room = load.harts.push(HartLoad {
                        hart: sample.hart,
                        idle_us,
                        wakes: sample.wakes,
                    });
                }
            }
        }
        if let Some(work_us) = sample.work_us {
            // The library's executors fit the report's, likewise.
            let _room = load.executors.push(ExecutorLoad {
                executor: sample.executor,
                hart: sample.hart,
                level: sample.level,
                work_us,
                rounds: sample.rounds,
            });
        }
    }
    load
}

/// Every named part's window, in the order of its number.
pub(crate) type Windows = heapless::Vec<PartWindow, PARTS>;

/// Every part named at both ends of one window, between the readings
/// `start` and `end`; the longest polls are `end`'s.
pub(crate) fn parts(start: &[PartReading; PARTS], end: &[PartReading; PARTS]) -> Windows {
    let mut windows = Windows::new();
    for (start, end) in start.iter().zip(end) {
        let Some(name) = end.name.filter(|_| start.name.is_some()) else {
            continue;
        };
        let (start, end) = (start.sample, end.sample);
        // There is room for every part; a name past the wire's length is
        // cut to none, which the host shows as such.
        let _room = windows.push(PartWindow {
            name: heapless::String::try_from(name).unwrap_or_default(),
            polls: end.polls.wrapping_sub(start.polls),
            poll_us: end.poll_us.wrapping_sub(start.poll_us),
            ended_poll_us: end.ended_us.wrapping_sub(start.ended_us),
            longest_poll_us: end.longest_us,
        });
    }
    windows
}

/// The windows from the part numbered `from`, as many as one answer holds.
pub(crate) fn page(windows: &Windows, from: u8) -> Parts {
    let mut parts = Parts::default();
    for window in windows.iter().skip(usize::from(from)).take(MAX_PARTS) {
        // `take` keeps the page within its room: no push is refused.
        let _room = parts.parts.push(window.clone());
    }
    parts
}

/// The tracking ticks between the window's ends `start` and `end`; the
/// extremes are `end`'s, of the window's epoch.
pub(crate) fn tracking(start: &TrackingSample, end: &TrackingSample) -> Tracking {
    let count = |at: fn(&TrackingSample) -> u32| at(end).wrapping_sub(at(start));
    Tracking {
        available: true,
        ticks: count(|sample| sample.ticks),
        not_due: count(|sample| sample.not_due),
        unavailable: count(|sample| sample.unavailable),
        inhibited: count(|sample| sample.inhibited),
        rfpll_corrected: count(|sample| sample.rfpll_corrected),
        wifi_i2c_changed: count(|sample| sample.wifi_i2c_changed),
        wifi_tx_power: count(|sample| sample.wifi_tx_power),
        bluetooth_ieee802154_tx_power: count(|sample| sample.bluetooth_ieee802154_tx_power),
        calibration_common: count(|sample| sample.calibration_common),
        calibration_transmit: count(|sample| sample.calibration_transmit),
        temperature_last: end.temperature_last,
        temperature_min: end.temperature_min,
        temperature_max: end.temperature_max,
        tick_us: count(|sample| sample.tick_us),
        longest_tick_us: end.longest_tick_us,
    }
}

fn rates(rates: &link::Rates) -> Rates {
    Rates {
        ht20_long_gi: rates.ht20_long_gi,
        ht20_short_gi: rates.ht20_short_gi,
        ht40_long_gi: rates.ht40_long_gi,
        ht40_short_gi: rates.ht40_short_gi,
        ht_other: rates.ht_other,
        non_ht: rates.non_ht,
        he: rates.he,
        unknown: rates.unknown,
    }
}

/// The link's counts as the wire names them.
pub(crate) fn link(link: &link::Link) -> Link {
    Link {
        rx: RxLink {
            mpdus: rates(&link.rx.mpdus),
            in_ampdu: link.rx.in_ampdu,
            single: link.rx.single,
            containment_unknown: link.rx.containment_unknown,
        },
        tx: TxLink {
            mpdus: rates(&link.tx.mpdus),
            transmissions: link.tx.transmissions,
            delivered: link.tx.delivered,
            ack_timeouts: link.tx.ack_timeouts,
            other_ends: link.tx.other_ends,
            unsent: link.tx.unsent,
            missing: link.tx.missing,
        },
    }
}

/// The transmit agreements' counts as the wire names them: its size
/// histogram wraps at 2^16.
pub(crate) fn tx_ampdu(ampdu: &link::TxAmpdu) -> TxAmpdu {
    let [one, two, three_four, five_eight, nine_sixteen, rest] =
        ampdu.by_size.map(|count| count.to_le_bytes());
    let bin = |[low, high, ..]: [u8; 4]| u16::from_le_bytes([low, high]);
    TxAmpdu {
        aggregates: ampdu.aggregates,
        refused: ampdu.refused,
        subframes: ampdu.subframes,
        subframes_1: bin(one),
        subframes_2: bin(two),
        subframes_3_4: bin(three_four),
        subframes_5_8: bin(five_eight),
        subframes_9_16: bin(nine_sixteen),
        subframes_17_32: bin(rest),
        acknowledged: ampdu.acknowledged,
        retried: ampdu.retried,
        dropped: ampdu.dropped,
        block_ack_requests: ampdu.block_ack_requests,
    }
}

/// The ADDBA Requests the station took, as the wire names them.
pub(crate) fn addba(reading: &addba::Addba) -> Addba {
    let request = |request: &addba::Request| AddbaRequest {
        requested: request.requested,
        outcome: match request.outcome {
            addba::Outcome::Accepted { window } => AddbaOutcome::Accepted { window },
            addba::Outcome::Repeated { window } => AddbaOutcome::Repeated { window },
            addba::Outcome::Refused { status } => AddbaOutcome::Refused { status },
            addba::Outcome::Unanswered => AddbaOutcome::Unanswered,
        },
    };
    Addba {
        last: reading
            .last
            .each_ref()
            .map(|last| last.as_ref().map(request)),
        accepted: reading.accepted,
        repeated: reading.repeated,
        refused: reading.refused,
        unanswered: reading.unanswered,
    }
}

/// What one end of a window read: the boot hart's counters, the frames
/// the network took, the cache's, the receive path's,
/// the transmit agreements' and the link's counts, and the ADDBA Requests
/// the station took since boot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub cycles: u64,
    pub instructions: u64,
    pub stack_received: u32,
    pub cache: CacheCounts,
    pub cache_geometry: CacheGeometry,
    pub rx: RxCounts,
    pub tx: TxCounts,
    pub link: Link,
    pub mac: Mac,
    pub positions: Positions,
    pub addba: Addba,
}

fn instruction_window(start: &InstructionBus, end: &InstructionBus) -> InstructionBus {
    InstructionBus {
        hits: end.hits.wrapping_sub(start.hits),
        misses: end.misses.wrapping_sub(start.misses),
        conflicts: end.conflicts.wrapping_sub(start.conflicts),
        next_level_reads: end.next_level_reads.wrapping_sub(start.next_level_reads),
        overflowed: start.overflowed || end.overflowed,
    }
}

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

/// The counts between the window's ends `start` and `end`; the cache's
/// geometry is `end`'s.
pub(crate) fn counters(start: &Snapshot, end: &Snapshot) -> Counters {
    Counters {
        cycles: end.cycles.wrapping_sub(start.cycles),
        instructions: end.instructions.wrapping_sub(start.instructions),
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

/// The MAC's counts between the window's ends `start` and `end`.
pub(crate) fn mac_window(start: &Snapshot, end: &Snapshot) -> Mac {
    Mac::window(&start.mac, &end.mac)
}

/// The answered aggregates between the window's ends `start` and `end`.
pub(crate) fn positions_window(start: &Snapshot, end: &Snapshot) -> Positions {
    Positions::window(&start.positions, &end.positions)
}

/// The link's counts between the window's ends `start` and `end`.
pub(crate) fn link_window(start: &Snapshot, end: &Snapshot) -> Link {
    Link::window(&start.link, &end.link)
}

/// The page of `buffer` from the sample numbered `from`, of a session that
/// reported `ticks` ticks, `stray` ticks without a session, and was `cut`.
pub(crate) fn samples<const N: usize>(
    buffer: &Thinning<N>,
    from: u16,
    ticks: u32,
    stray: u32,
    cut: bool,
) -> Samples {
    let kept = buffer.len();
    let from = usize::from(from).min(kept);
    let to = from.saturating_add(MAX_PAGE_SAMPLES).min(kept);
    let mut samples = heapless::Vec::new();
    for slot in from..to {
        if let Some(sample) = buffer.get(slot) {
            // The range holds at most a page: no push is refused.
            let _room = samples.push(Sample {
                tick: sample.tick(),
                pc: sample.pc(),
                level: sample.level(),
                ra: buffer.return_address(slot),
            });
        }
    }
    Samples {
        period_us: buffer.period_us(),
        ticks,
        stride: buffer.stride(),
        kept: u16::try_from(kept).unwrap_or(u16::MAX),
        stray,
        cut,
        samples,
    }
}

/// The image's time of a radio owner's stamp read `age` microseconds before
/// the image read its own clock at `now_us`: both clocks count the one
/// system timer, so an age carries over from one to the other.
pub(crate) fn stamped(now_us: u64, age: u32) -> u64 {
    now_us.saturating_sub(u64::from(age))
}

/// A boundary that took `count` steps, its last `age` microseconds before
/// `now_us`; without a step it has no time.
pub(crate) fn boundary(now_us: u64, count: u32, age: u32) -> StallBoundary {
    StallBoundary {
        count,
        last_us: (count != 0).then(|| stamped(now_us, age)),
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) use chip::{hart_count, snapshot, stall};

/// The chip's own types: the cache's, the driver's and the station's.
#[cfg(target_arch = "riscv32")]
mod chip {
    use oer::observe::{
        CacheGeometry, CacheLevel, Counters, DataBus, HartClock, HartCounters, InstructionBus,
        MacBank, RxProgress, RxStallReading, StationRxCounts, StationRxKind, WifiRxReport,
    };
    use oer_hil_services_load as wire;

    use super::Snapshot;

    /// The radio owner's stall `reading`, taken with the image's clock at
    /// `now_us`: the snapshot and the MAC's banks as the wire names them.
    pub(crate) fn stall(
        reading: &RxStallReading,
        now_us: u64,
    ) -> (wire::RxSnapshot, wire::StallMac) {
        let at = |stamp: u32| super::stamped(now_us, reading.age(stamp));
        let boundary = |progress: RxProgress| {
            super::boundary(now_us, progress.count, reading.age(progress.at))
        };
        let taken = &reading.snapshot;
        let hardware = taken.hardware.map(|hardware| {
            let (walker, session) = (hardware.walker, hardware.session);
            let descriptors = session.descriptors;
            wire::StallHardware {
                walker: wire::StallWalker {
                    enabled: walker.enabled,
                    reload_doorbell: walker.reload_doorbell,
                    next_low: walker.next_low,
                    next_upper: walker.next_upper,
                    last_low: walker.last_low,
                    base: walker.base,
                    buffer_full: walker.buffer_full,
                    runtime_state: walker.runtime_state,
                },
                ring: hardware.ring.map(|ring| wire::StallRing {
                    base: ring.base,
                    recycle_start: ring.recycle_start,
                    accepted_tail: ring.accepted_tail,
                    observed: ring.observed,
                    completed: ring.completed,
                    remaining: ring.remaining,
                    reload_pending: ring.reload_pending,
                }),
                session: wire::StallSession {
                    descriptors: descriptors.total,
                    armed: descriptors.armed,
                    done: descriptors.done,
                    software: descriptors.software,
                    reserved_owner: descriptors.reserved_owner,
                    reserved_done: descriptors.reserved_done,
                    released: session.released,
                    detached: session.detached,
                    staged: session.staged,
                    masked: session.masked,
                    frozen: session.frozen,
                    line_request_pending: session.line_request_pending,
                    receive_fact_pending: session.receive_fact_pending,
                    mac_irqs: session.mac_irqs,
                    doorbell_unmasks: session.doorbell_unmasks,
                    ignored_unmasks: session.ignored_unmasks,
                    service_passes: session.service_passes,
                },
            }
        });
        let device = taken.device;
        let snapshot = wire::RxSnapshot {
            at_us: at(taken.at),
            dma: boundary(taken.dma),
            driver: boundary(taken.driver),
            device: wire::StallDevice {
                rx_pending: device.rx_pending,
                continuation: device.continuation,
                cut: device.cut,
                pausing: device.pausing,
                paused: device.paused,
                queued: device.queued,
            },
            hardware,
            resume: reading.resume.map(|resume| wire::StallResume {
                checked_us: at(resume.at),
                dma: boundary(resume.dma),
                driver: boundary(resume.driver),
                serviced: resume.serviced,
            }),
        };
        let bank = |bank: &MacBank| wire::Mac::new(bank.counts, bank.frequency_offset);
        let mac = wire::StallMac {
            fresh: taken.mac.as_ref().map(bank),
            periodic: taken.periodic.as_ref().map(|sample| bank(&sample.bank)),
            periodic_us: taken.periodic.map(|sample| at(sample.at)),
        };
        (snapshot, mac)
    }

    /// A hart's `clock` as the wire names it.
    pub(crate) fn hart_count(clock: HartClock) -> wire::HartCount {
        wire::HartCount {
            cycles: clock.cycles,
            instructions: clock.instructions,
        }
    }

    /// One end of a window: the boot hart's `hart` and the library's
    /// `counters`, both read on the boot hart; the IP stack's frames are
    /// hart 1's to count.
    pub(crate) fn snapshot(
        hart: HartCounters,
        counters: Counters,
        addba: &oer::observe::addba::Addba,
    ) -> Snapshot {
        Snapshot {
            cycles: hart.cycles,
            instructions: hart.instructions,
            stack_received: 0,
            cache: wire::CacheCounts {
                ibus0: instruction(hart.cache.ibus0),
                ibus1: instruction(hart.cache.ibus1),
                dbus0: data(hart.cache.dbus0),
                dbus1: data(hart.cache.dbus1),
            },
            cache_geometry: geometry(hart.cache_geometry),
            rx: wire::RxCounts {
                driver: counters.driver.map(driver).unwrap_or_default(),
                station: station(&counters.station_rx),
            },
            tx: wire::TxCounts {
                ampdu: super::tx_ampdu(&counters.ampdu),
            },
            link: super::link(&counters.link),
            mac: wire::Mac::new(counters.mac.0, counters.mac.1),
            positions: wire::Positions {
                sent: counters.positions.sent,
                holes: counters.positions.holes,
                tail_runs: counters.positions.tail_runs,
                interior: counters.positions.interior,
            },
            addba: super::addba(addba),
        }
    }

    fn instruction(bus: InstructionBus) -> wire::InstructionBus {
        wire::InstructionBus {
            hits: bus.hits,
            misses: bus.misses,
            conflicts: bus.conflicts,
            next_level_reads: bus.next_level_reads,
            overflowed: bus.overflowed,
        }
    }

    fn data(bus: DataBus) -> wire::DataBus {
        wire::DataBus {
            hits: bus.hits,
            misses: bus.misses,
            conflicts: bus.conflicts,
            next_level_reads: bus.next_level_reads,
            next_level_writes: bus.next_level_writes,
            overflowed: bus.overflowed,
        }
    }

    fn geometry(geometry: CacheGeometry) -> wire::CacheGeometry {
        let level = |level: CacheLevel, ways: Option<u32>| wire::CacheLevel {
            size_bytes: level.size_bytes,
            line_bytes: level.line_bytes,
            ways,
        };
        wire::CacheGeometry {
            l1_icache: level(geometry.l1_icache, Some(geometry.l1_icache_ways)),
            l1_dcache: level(geometry.l1_dcache, Some(geometry.l1_dcache_ways)),
            l2: level(geometry.l2, None),
            l2_bypassed: geometry.l2_bypassed,
        }
    }

    fn driver(report: WifiRxReport) -> wire::DriverRx {
        let prefix = report.prefix;
        wire::DriverRx {
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
        }
    }

    fn station(counts: &StationRxCounts) -> wire::StationRx {
        use StationRxKind as Kind;
        let at = |kind: Kind| counts.count(kind);
        wire::StationRx {
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
            refused_pressure: at(Kind::RefusedPressure),
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
            agreements_started: counts.agreements_started,
            agreements_stopped: at(Kind::AgreementStopped),
            agreement_window: u16::try_from(counts.agreement_window).unwrap_or(u16::MAX),
            rsn_inputs_dropped: at(Kind::RsnInputDropped),
        }
    }
}

#[cfg(test)]
mod tests;
