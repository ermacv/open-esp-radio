//! The counter owner over host memory from the cache block's base: the
//! words a start writes and the images it leaves last, the counts and
//! overflow bits a snapshot reads and the geometry's one-hot decoding,
//! checked through the owner's and the part's own accessors.

use std::{vec, vec::Vec};

use super::*;

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; instances::cache::SIZE / 4]
}

/// How many words differ between `before` and `after`.
fn changed(before: &[u32], after: &[u32]) -> usize {
    before.iter().zip(after).filter(|(a, b)| a != b).count()
}

const NONE: Buses = Buses {
    ibus0: false,
    ibus1: false,
    dbus0: false,
    dbus1: false,
};

#[test]
fn a_start_enables_exactly_the_selected_buses() {
    for fill in [0, u32::MAX] {
        let selected = Buses {
            ibus0: true,
            dbus0: true,
            ..NONE
        };
        let mut counters = memory(fill);
        let mut geometry = memory(fill);
        let mut owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
        owner.start(selected);
        assert_eq!(owner.enabled(), selected);
        owner.start(Buses::ALL);
        assert_eq!(owner.enabled(), Buses::ALL);
        owner.start(NONE);
        assert_eq!(owner.enabled(), NONE);
    }
}

#[test]
fn a_start_writes_only_the_trace_the_enable_word_and_the_overflow_clear() {
    let mut counters = memory(0);
    let mut geometry = memory(0x5a5a_5a5a);
    let (counters_before, geometry_before) = (counters.clone(), geometry.clone());
    let mut owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    owner.start(Buses::ALL);
    drop(owner);
    assert_eq!(
        changed(&counters_before, &counters),
        3,
        "the counts and the raw overflow word stay as read"
    );
    assert_eq!(geometry, geometry_before, "a start writes no geometry word");
}

#[test]
fn a_start_turns_the_counters_trace_on() {
    let mut counters = memory(0);
    let mut geometry = memory(0);
    let mut owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    assert!(!owner.tracing());
    owner.start(Buses {
        dbus1: true,
        ..NONE
    });
    assert!(owner.tracing());
}

/// The block after the writes a start must leave last: the trace on, the
/// overflow clear of `buses`, and the enable word written from the value
/// read with every clear lane zero.
fn expected_after_start(fill: u32, buses: Buses) -> Vec<u32> {
    let mut words = memory(fill);
    let mut part = CacheAccessCounters::host(&mut words).expect("host memory");
    part.modify_trace_ena(|w| {
        w.set_l1_cache_trace_ena(true);
    });
    part.trigger_l1_cache_acs_cnt_int_clr(|t| {
        if buses.ibus0 {
            t.l1_ibus0_ovf_int_clr();
        }
        if buses.ibus1 {
            t.l1_ibus1_ovf_int_clr();
        }
        if buses.dbus0 {
            t.l1_dbus0_ovf_int_clr();
        }
        if buses.dbus1 {
            t.l1_dbus1_ovf_int_clr();
        }
    });
    part.modify_l1_cache_acs_cnt_ctrl(|w| {
        w.set_l1_ibus0_cnt_ena(buses.ibus0)
            .set_l1_ibus1_cnt_ena(buses.ibus1)
            .set_l1_dbus0_cnt_ena(buses.dbus0)
            .set_l1_dbus1_cnt_ena(buses.dbus1);
    });
    drop(part);
    words
}

#[test]
fn a_start_ends_with_an_enable_write_that_sets_no_clear_lane() {
    let selections = [
        Buses::ALL,
        Buses {
            ibus0: true,
            dbus0: true,
            ..NONE
        },
        Buses {
            ibus1: true,
            dbus1: true,
            ..NONE
        },
    ];
    for fill in [0, u32::MAX, 0xa5a5_a5a5] {
        for buses in selections {
            let mut counters = memory(fill);
            let mut geometry = memory(0);
            let mut owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
            owner.start(buses);
            drop(owner);
            assert_eq!(
                counters,
                expected_after_start(fill, buses),
                "fill {fill:#x}, {buses:?}: the counters run under an enable word \
                 without a clear strobe"
            );
        }
    }
}

#[test]
fn a_start_of_no_bus_from_zero_changes_only_the_trace() {
    let mut counters = memory(0);
    let mut geometry = memory(0);
    let before = counters.clone();
    let mut owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    owner.start(NONE);
    assert!(owner.tracing());
    assert_eq!(owner.enabled(), NONE);
    drop(owner);
    assert_eq!(changed(&before, &counters), 1);
}

#[test]
fn a_snapshot_reads_a_distinct_word_for_every_count() {
    let mut counters: Vec<u32> = (0..instances::cache::SIZE as u32 / 4)
        .map(|word| word.wrapping_mul(0x9e37_79b9) | 1)
        .collect();
    let mut geometry = memory(0);
    let owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    let snapshot = owner.snapshot();
    let mut counts = Vec::new();
    for bus in [snapshot.ibus0, snapshot.ibus1] {
        counts.extend([bus.hits, bus.misses, bus.conflicts, bus.next_level_reads]);
    }
    for bus in [snapshot.dbus0, snapshot.dbus1] {
        counts.extend([
            bus.hits,
            bus.misses,
            bus.conflicts,
            bus.next_level_reads,
            bus.next_level_writes,
        ]);
    }
    let mut distinct = counts.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(counts.len(), 18);
    assert_eq!(distinct.len(), counts.len(), "each count is its own word");
}

#[test]
fn a_snapshot_carries_whole_counts_and_each_bus_s_overflow() {
    let mut counters = memory(u32::MAX);
    let mut geometry = memory(0);
    let owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    let full = owner.snapshot();
    assert_eq!(
        full.ibus0,
        InstructionBus {
            hits: u32::MAX,
            misses: u32::MAX,
            conflicts: u32::MAX,
            next_level_reads: u32::MAX,
            overflowed: true,
        }
    );
    assert_eq!(full.ibus1, full.ibus0);
    assert_eq!(
        full.dbus1,
        DataBus {
            hits: u32::MAX,
            misses: u32::MAX,
            conflicts: u32::MAX,
            next_level_reads: u32::MAX,
            next_level_writes: u32::MAX,
            overflowed: true,
        }
    );
    assert_eq!(full.dbus0, full.dbus1);
    drop(owner);
    let mut counters = memory(0);
    let owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
    assert_eq!(owner.snapshot(), Snapshot::default());
}

#[test]
fn a_one_hot_lane_names_its_size_and_any_other_image_none() {
    let mut lanes = [false; 13];
    assert_eq!(size(lanes), None, "no lane");
    lanes[7] = true;
    assert_eq!(size(lanes), Some(32 * 1024));
    lanes[8] = true;
    assert_eq!(size(lanes), None, "two lanes");
    let mut lanes = [false; 6];
    lanes[3] = true;
    assert_eq!(line(lanes), Some(64));
    lanes[0] = true;
    assert_eq!(line(lanes), None);
    assert_eq!(size([true; 13]), None);
    let mut largest = [false; 13];
    largest[12] = true;
    assert_eq!(size(largest), Some(1024 * 1024));
}

#[test]
fn a_geometry_of_unset_or_crowded_words_names_no_size_and_keeps_the_ways() {
    for fill in [0, u32::MAX] {
        let mut counters = memory(0);
        let mut geometry = memory(fill);
        let owner = CacheCounters::host(&mut counters, &mut geometry).expect("host memory");
        let read = owner.geometry();
        let unknown = Level {
            size_bytes: None,
            line_bytes: None,
        };
        assert_eq!(read.l1_icache, unknown);
        assert_eq!(read.l1_dcache, unknown);
        assert_eq!(read.l2, unknown);
        assert_eq!(read.l2_bypassed, fill != 0);
        assert_eq!(read.l1_icache_ways, cache::L1_ICACHE_WAYS);
        assert_eq!(read.l1_dcache_ways, cache::L1_DCACHE_WAYS);
    }
}
