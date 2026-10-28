extern crate std;

use std::boxed::Box;
use std::vec::Vec;

use super::{Retained, Trace};
use oer_trace::{Domain, Kind, Record, oldest_first};

const STEP: Kind = Kind::new(Domain::Phy, 1);
const LOSS: Kind = Kind::new(Domain::Ieee80211, 7);

fn trace<const E: usize, const S: usize, const W: usize>() -> (&'static Retained<E, S, W>, Trace) {
    let retained: &'static Retained<E, S, W> = Box::leak(Box::new(Retained::new()));
    (retained, Trace::new(retained))
}

/// The test binary's clock; records these tests write carry explicit stamps.
#[cfg(oer_trace)]
fn test_clock() -> u64 {
    0x1_2345_6789
}
#[cfg(oer_trace)]
crate::trace_clock!(test_clock);

#[cfg(oer_trace)]
#[test]
fn record_timestamps_use_the_low_word_of_the_linked_image_clock() {
    let (_, trace) = trace::<1, 0, 0>();
    assert_eq!(trace.now_us(), 0x2345_6789);
}

fn record(trace: &Trace, step: u32) -> bool {
    trace.record(STEP, [step, 0], step * 10)
}

fn ordered(trace: &Trace) -> Vec<u32> {
    let mut records: Vec<Record> = trace.records().collect();
    oldest_first(&mut records);
    records.iter().map(|record| record.words[0]).collect()
}

#[test]
fn a_wrapped_ring_keeps_the_newest_entries_in_write_order() {
    let (_, trace) = trace::<4, 0, 0>();
    trace.restart();
    for step in 0..10 {
        record(&trace, step);
    }
    assert_eq!(ordered(&trace), [6, 7, 8, 9]);
}

#[test]
fn write_order_survives_the_sequence_tag_wrapping() {
    let (_, trace) = trace::<8, 0, 0>();
    trace.restart();
    for step in 0..0xffff + 5 {
        record(&trace, step);
    }
    let expected: Vec<u32> = (0xffff - 3..0xffff + 5).collect();
    assert_eq!(ordered(&trace), expected);
}

#[test]
fn an_entry_whose_commit_word_is_missing_is_dropped() {
    let (_, trace) = trace::<4, 0, 0>();
    trace.restart();
    for step in 0..3 {
        record(&trace, step);
    }
    trace.tear_entry(1);
    assert_eq!(ordered(&trace), [0, 2]);
}

#[test]
fn a_freeze_keeps_its_post_trigger_window_and_names_the_trigger() {
    let (_, trace) = trace::<16, 0, 0>();
    trace.restart();
    record(&trace, 0);
    record(&trace, 1);
    assert!(!trace.freeze(LOSS, 2));
    assert!(!trace.freeze(STEP, 0), "only the first freeze counts");
    assert!(!record(&trace, 2));
    assert!(record(&trace, 3), "the last window entry stops recording");
    assert!(trace.is_frozen());
    assert!(!record(&trace, 4));
    assert_eq!(ordered(&trace), [0, 1, 2, 3]);
    let trigger = trace.trigger().unwrap();
    assert_eq!(trigger.kind(), Some(LOSS));
    let first_after = trace.records().find(|r| r.tag == trigger.tag).unwrap();
    assert_eq!(first_after.words[0], 2);
}

#[test]
fn an_immediate_freeze_stops_the_next_entry() {
    let (_, trace) = trace::<4, 0, 0>();
    trace.restart();
    record(&trace, 0);
    assert!(trace.freeze(LOSS, 0));
    assert!(!record(&trace, 1));
    assert_eq!(ordered(&trace), [0]);
}

#[test]
fn the_next_boot_holds_what_the_previous_one_left_until_restarted() {
    let (retained, before) = trace::<4, 1, 4>();
    before.restart();
    record(&before, 0);
    record(&before, 1);
    before.freeze(LOSS, 0);
    before.capture(STEP, 5, |words| words.push(9));

    let after = Trace::new(retained);
    let previous = after.hold().previous.unwrap();
    assert_eq!(previous.entries, 2);
    assert_eq!(previous.snapshots, 1);
    assert_eq!(previous.trigger.unwrap().kind(), Some(LOSS));
    after.capture(STEP, 6, |words| words.push(1));
    assert_eq!(after.snapshots().count(), 1, "a held trace keeps its slots");

    after.restart();
    assert_eq!(after.records().count(), 0);
    assert_eq!(after.snapshots().count(), 0);
    assert_eq!(after.trigger(), None);
}

#[test]
fn storage_never_written_by_a_trace_reports_nothing() {
    let (_, trace) = trace::<4, 1, 4>();
    assert_eq!(trace.hold().previous, None);
}

#[test]
fn a_snapshot_keeps_what_fits_and_detects_being_overwritten() {
    let (_, trace) = trace::<4, 1, 3>();
    trace.restart();
    trace.capture(STEP, 7, |words| words.extend([1, 2, 3, 4]));
    let snapshot = trace.snapshot(0).unwrap();
    assert_eq!(snapshot.point(), Some(STEP));
    assert_eq!(
        (snapshot.len, snapshot.truncated, snapshot.t_us),
        (3, true, 7)
    );
    let mut page = [0; 2];
    assert_eq!(trace.read_snapshot(&snapshot, 1, &mut page), Some(2));
    assert_eq!(page, [2, 3]);

    trace.capture(LOSS, 8, |words| words.push(5));
    assert_eq!(trace.read_snapshot(&snapshot, 0, &mut page), None);
    assert_eq!(trace.snapshot(0).unwrap().point(), Some(LOSS));
}

#[cfg(oer_trace)]
mod recording {
    use super::super as recorder;
    use super::super::{Retained, Trace};
    use oer_trace::{Channel, Domain, Event, Kind};

    use super::LOSS;

    #[derive(Debug, PartialEq)]
    struct Beacon {
        sequence: u16,
        rssi: i8,
    }

    impl Event for Beacon {
        const KIND: Kind = Kind::new(Domain::Ieee80211, 1);
        const CHANNEL: Channel = const {
            match Channel::new(Domain::Ieee80211, 3) {
                Ok(channel) => channel,
                Err(_) => panic!("invalid trace channel"),
            }
        };

        fn encode(&self) -> [u32; 2] {
            [
                u32::from(self.sequence) | (u32::from(self.rssi as u8) << 16),
                0,
            ]
        }

        fn decode(words: [u32; 2]) -> Option<Self> {
            Some(Self {
                sequence: words[0] as u16,
                rssi: (words[0] >> 16) as u8 as i8,
            })
        }
    }

    #[test]
    fn only_enabled_channels_of_a_started_trace_are_recorded_and_decoded() {
        static RETAINED: Retained<8, 0, 0> = Retained::new();
        static TRACE: Trace = Trace::new(&RETAINED);

        let beacon = Beacon {
            sequence: 12,
            rssi: -40,
        };
        recorder::emit(&beacon);
        assert!(recorder::install(&TRACE).previous.is_none());
        recorder::set_mask(Beacon::CHANNEL.mask());
        assert_eq!(recorder::mask(), 0, "a held trace records nothing");
        recorder::start(Channel::new(Domain::Phy, 0).unwrap().mask());
        recorder::emit(&beacon);
        assert_eq!(TRACE.records().count(), 0);

        recorder::set_mask(Beacon::CHANNEL.mask());
        recorder::emit(&beacon);
        let record = TRACE.records().next().unwrap();
        assert_eq!(record.decode::<Beacon>(), Some(beacon));
        assert_eq!(record.decode::<Other>(), None);

        recorder::freeze(LOSS, 0);
        assert_eq!(recorder::mask(), 0, "a frozen trace disables every channel");

        let high = Channel::new(Domain::Ieee802154, 11).unwrap();
        recorder::start(high.mask());
        assert!(
            recorder::enabled(high),
            "channel 63 uses the high mask word"
        );
        assert!(!recorder::enabled(Beacon::CHANNEL));
        recorder::freeze(LOSS, 0);
        assert_eq!(recorder::mask(), 0);
    }

    #[derive(Debug, PartialEq)]
    struct Other;

    impl Event for Other {
        const KIND: Kind = Kind::new(Domain::Platform, 1);
        const CHANNEL: Channel = const {
            match Channel::new(Domain::Platform, 0) {
                Ok(channel) => channel,
                Err(_) => panic!("invalid trace channel"),
            }
        };

        fn encode(&self) -> [u32; 2] {
            [0; 2]
        }

        fn decode(_: [u32; 2]) -> Option<Self> {
            Some(Self)
        }
    }
}

#[test]
fn snapshot_reads_keep_empty_ranges_and_zero_capacity_behavior() {
    let (_, empty) = trace::<1, 1, 0>();
    empty.restart();
    empty.capture(STEP, 1, |writer| {
        assert_eq!(writer.remaining(), 0);
        writer.push(7);
        assert_eq!(writer.remaining(), 0);
    });
    let snapshot = empty.snapshot(0).unwrap();
    assert_eq!((snapshot.len, snapshot.truncated), (0, true));
    let mut out = [99];
    assert_eq!(
        empty.read_snapshot(&snapshot, usize::MAX, &mut out),
        Some(0)
    );
    assert_eq!(out, [99]);

    let (_, trace) = trace::<1, 1, 2>();
    trace.restart();
    trace.capture(STEP, 2, |writer| {
        assert_eq!(writer.remaining(), 2);
        writer.extend([8, 9]);
        assert_eq!(writer.remaining(), 0);
    });
    let mut snapshot = trace.snapshot(0).unwrap();
    assert_eq!(trace.read_snapshot(&snapshot, 2, &mut out), Some(0));
    snapshot.len = usize::MAX;
    assert_eq!(trace.read_snapshot(&snapshot, 0, &mut out), Some(0));
    snapshot.slot = usize::MAX;
    assert_eq!(trace.read_snapshot(&snapshot, 0, &mut out), None);
    assert_eq!(out, [99]);
}
