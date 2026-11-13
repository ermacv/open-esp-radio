use super::*;

/// One TID row's values, in the station file's column order:
/// backlog-bytes, backlog-packets, new-flows, drops, marks, overlimit,
/// collisions, tx-bytes, tx-packets; `None` for a column the file lacks.
type Row = [Option<u64>; 9];

const COLUMNS: [Counts; 9] = [
    Counts::QueuedBytes,
    Counts::QueuedPackets,
    Counts::FlowActivations,
    Counts::CodelDrops,
    Counts::EcnMarks,
    Counts::OverlimitDropOperations,
    Counts::CollidedEnqueues,
    Counts::DequeuedBytes,
    Counts::DequeuedPackets,
];

/// The radio's values of these tests, by name.
const RADIO: [(&str, Counts, u64); 5] = [
    ("fq_backlog", Counts::RadioQueuedPackets, 20),
    ("fq_overlimit", Counts::RadioOvermemoryDropOperations, 7),
    ("fq_overmemory", Counts::RadioDropOperations, 9),
    ("fq_collisions", Counts::RadioCollidedEnqueues, 18),
    ("fq_limit", Counts::RadioPacketLimit, 8192),
];

fn all(values: [u64; 9]) -> Row {
    values.map(Some)
}

fn tid(tid: u8, row: Row, at_ms: u64) -> TidQueue {
    let scope = Scope::StationTid { tid };
    let value = |at: usize| {
        row.get(at)
            .copied()
            .flatten()
            .zip(COLUMNS.get(at))
            .map(|(value, counts)| Observed::new(value, *counts, scope, at_ms))
    };
    TidQueue {
        tid,
        ac: None,
        backlog_bytes: value(0),
        backlog_packets: value(1),
        new_flows: value(2),
        drops: value(3),
        marks: value(4),
        overlimit: value(5),
        collisions: value(6),
        tx_bytes: value(7),
        tx_packets: value(8),
        flags: None,
        unknown: BTreeMap::new(),
    }
}

fn raw(at_ms: u64) -> Raw {
    Raw {
        observed_at_ms: at_ms,
        header: String::new(),
        lines: Vec::new(),
    }
}

/// One read: the station's `tx packets` at `station_ms`, its association
/// time, its TID 0 and TID 7 rows read at `queues_ms` (none without a
/// station file) and the radio's values with `radio` overriding some
/// (none without a radio file).
fn read(
    station_ms: u64,
    associated: Option<u64>,
    tx_packets: u64,
    queues_ms: u64,
    rows: Option<(Row, Row)>,
    radio: Option<&[(&str, u64)]>,
) -> StationLink {
    let station_queues = rows.map(|(tid0, tid7)| StationQueues {
        raw: raw(queues_ms),
        unknown_columns: Vec::new(),
        missing_columns: Vec::new(),
        cut_row: None,
        tids: vec![tid(0, tid0, queues_ms), tid(7, tid7, queues_ms)],
    });
    let radio_queues = radio.map(|overrides| RadioQueues {
        raw: raw(queues_ms),
        unknown_columns: Vec::new(),
        unknown_names: Vec::new(),
        missing_names: Vec::new(),
        cut_names: Vec::new(),
        values: RADIO
            .iter()
            .map(|(name, counts, value)| {
                let value = overrides
                    .iter()
                    .find(|(other, _)| other == name)
                    .map_or(*value, |(_, value)| *value);
                (
                    (*name).to_owned(),
                    RadioEntry {
                        access: Some("R".to_owned()),
                        value: Some(Observed::new(value, *counts, Scope::Radio, queues_ms)),
                    },
                )
            })
            .collect(),
    });
    StationLink {
        observed_at_ms: station_ms,
        associated_at_boottime_ms: associated,
        rx_bytes: 1,
        rx_packets: 1,
        tx_bytes: 1,
        tx_packets,
        tx_retries: 0,
        tx_failed: 0,
        rx_drop_misc: None,
        rx_duration_us: None,
        tx_duration_us: None,
        signal_avg_dbm: None,
        rx_bitrate: None,
        tx_bitrate: None,
        channel: None,
        frequency_mhz: None,
        width_mhz: None,
        survey: None,
        tid0_aqm_drops: None,
        station_queues,
        radio_queues,
        radio: BTreeMap::new(),
    }
}

fn delta(change: Change) -> i64 {
    match change {
        Change::Valid(delta) => delta.change,
        other => panic!("not valid: {other:?}"),
    }
}

const IDLE: [u64; 9] = [0; 9];

#[test]
fn the_balance_takes_the_whole_tid_0_and_never_adds_overlimit_operations() {
    let start = read(
        10_000,
        Some(5000),
        1000,
        10_020,
        Some((
            all([1500, 1, 3, 4, 0, 2, 0, 9000, 6]),
            all([0, 0, 2, 0, 0, 0, 0, 322, 2]),
        )),
        Some(&[]),
    );
    // 500 more entered: TID 0 dequeued 400, CoDel dropped 30 and its
    // backlog grew by 20; TID 7 dequeued 1; 3 more overlimit operations.
    let end = read(
        16_000,
        Some(5000),
        1500,
        16_030,
        Some((
            all([31_500, 21, 9, 34, 1, 5, 0, 609_000, 406]),
            all([0, 0, 3, 0, 0, 0, 0, 483, 3]),
        )),
        Some(&[("fq_overmemory", 12)]),
    );
    let queues = window(&start, &end);
    assert_eq!(queues.association, Association::Same);
    let tid0 = queues.tids.get(&0).unwrap();
    assert_eq!(delta(tid0.tx_packets), 400);
    assert_eq!(delta(tid0.drops), 30);
    assert_eq!(delta(tid0.backlog_packets), 20);
    assert_eq!(delta(tid0.overlimit), 3);
    let Change::Valid(overlimit) = tid0.overlimit else {
        panic!("{:?}", tid0.overlimit);
    };
    assert_eq!(
        (overlimit.unit, overlimit.from_ms, overlimit.to_ms),
        (Unit::Operation, 10_020, 16_030)
    );
    assert_eq!(delta(queues.balance.entry), 500);
    // 500 − (400 + 30 + 20) − 1 (TID 7): the 3 operations stay out.
    assert_eq!(queues.balance.remainder, Remainder::Packets(49));
    assert_eq!(queues.balance.skew_ms, Some(30));
    let Change::Valid(radio) = *queues.radio.get("fq_overmemory").unwrap() else {
        panic!("the radio's counter");
    };
    assert_eq!((radio.change, radio.scope), (3, Scope::Radio));
    // A draining backlog adds to what the queue accounts.
    let drained = read(
        16_000,
        Some(5000),
        1500,
        16_030,
        Some((
            all([0, 0, 9, 34, 1, 5, 0, 609_000, 426]),
            all([0, 0, 3, 0, 0, 0, 0, 483, 3]),
        )),
        Some(&[]),
    );
    let queues = window(&start, &drained);
    assert_eq!(delta(queues.tids.get(&0).unwrap().backlog_packets), -1);
    assert_eq!(queues.balance.remainder, Remainder::Packets(50));
    let json = serde_json::to_value(&queues).unwrap();
    assert_eq!(serde_json::from_value::<QueueWindow>(json).unwrap(), queues);
}

#[test]
fn a_counter_that_went_back_invalidates_its_change_and_leaves_the_balance_open() {
    let rows = |tx| Some((all([0, 0, 3, 4, 0, 0, 0, 9000, tx]), all(IDLE)));
    let start = read(1000, Some(500), 100, 1000, rows(60), Some(&[]));
    let end = read(
        2000,
        Some(500),
        200,
        2000,
        rows(5),
        Some(&[("fq_collisions", 2)]),
    );
    let queues = window(&start, &end);
    assert_eq!(queues.association, Association::Same);
    let tid0 = queues.tids.get(&0).unwrap();
    assert_eq!(tid0.tx_packets, Change::Invalid(Invalid::WentBack));
    assert_eq!(delta(tid0.drops), 0, "the others stay valid");
    assert!(matches!(queues.balance.remainder, Remainder::Open(_)));
    assert_eq!(
        *queues.radio.get("fq_collisions").unwrap(),
        Change::Invalid(Invalid::WentBack)
    );
    // A level may fall.
    let fell = read(
        2000,
        Some(500),
        200,
        2000,
        rows(60),
        Some(&[("fq_backlog", 5)]),
    );
    assert_eq!(
        delta(*window(&start, &fell).radio.get("fq_backlog").unwrap()),
        -15
    );
}

#[test]
fn a_new_association_invalidates_the_station_s_changes_but_not_the_radio_s() {
    let start = read(
        1000,
        Some(500),
        100,
        1000,
        Some((all([0, 0, 3, 4, 0, 0, 0, 9000, 60]), all(IDLE))),
        Some(&[]),
    );
    let later = Some((all([0, 0, 9, 8, 0, 0, 0, 19_000, 90]), all(IDLE)));
    let rejoined = read(
        9000,
        Some(8500),
        900,
        9000,
        later,
        Some(&[("fq_overmemory", 10)]),
    );
    let queues = window(&start, &rejoined);
    assert_eq!(queues.association, Association::Changed);
    let tid0 = queues.tids.get(&0).unwrap();
    for change in [
        tid0.tx_packets,
        tid0.drops,
        tid0.overlimit,
        tid0.backlog_packets,
    ] {
        assert_eq!(change, Change::Invalid(Invalid::Reassociated));
    }
    assert_eq!(queues.balance.entry, Change::Invalid(Invalid::Reassociated));
    assert!(matches!(queues.balance.remainder, Remainder::Open(_)));
    assert_eq!(delta(*queues.radio.get("fq_overmemory").unwrap()), 1);
    let unconfirmed = read(9000, None, 900, 9000, later, Some(&[]));
    let queues = window(&start, &unconfirmed);
    assert_eq!(queues.association, Association::Unconfirmed);
    assert_eq!(
        queues.tids.get(&0).unwrap().tx_packets,
        Change::Invalid(Invalid::AssociationUnconfirmed)
    );
}

#[test]
fn a_value_one_read_lacks_is_unavailable_and_leaves_the_balance_open() {
    let start = read(
        1000,
        Some(500),
        100,
        1000,
        Some((all([0, 0, 3, 4, 0, 0, 0, 9000, 60]), all(IDLE))),
        None,
    );
    // The end's file has only `tx-packets` and `drops`.
    let sparse = |tx, drops| {
        let mut row = [None; 9];
        row[3] = Some(drops);
        row[8] = Some(tx);
        row
    };
    let end = read(
        2000,
        Some(500),
        200,
        2000,
        Some((sparse(90, 4), sparse(0, 0))),
        Some(&[]),
    );
    let queues = window(&start, &end);
    let tid0 = queues.tids.get(&0).unwrap();
    assert_eq!(delta(tid0.tx_packets), 30);
    assert_eq!(tid0.backlog_packets, Change::Unavailable);
    assert_eq!(tid0.overlimit, Change::Unavailable);
    assert!(matches!(queues.balance.remainder, Remainder::Open(_)));
    assert!(
        queues
            .radio
            .values()
            .all(|change| *change == Change::Unavailable),
        "the start had no radio file"
    );
    let none = read(1000, Some(500), 100, 1000, None, None);
    let queues = window(&none, &none);
    assert!(queues.tids.is_empty() && queues.radio.is_empty());
    assert_eq!(delta(queues.balance.entry), 0);
    assert!(matches!(queues.balance.remainder, Remainder::Open(_)));
    assert_eq!(queues.balance.skew_ms, None);
}

#[test]
fn a_station_change_across_a_router_clock_that_went_back_is_invalid() {
    // The owner's counterexample: the first read at 100 s with the counter
    // at 100, the second at 5 s with it already at 200, the association
    // time numerically equal.
    let rows = |tx| Some((all([0, 0, 0, 0, 0, 0, 0, 0, tx]), all(IDLE)));
    let start = read(100_000, Some(1000), 100, 100_000, rows(100), None);
    let end = read(5000, Some(1000), 200, 5000, rows(200), None);
    let queues = window(&start, &end);
    assert_eq!(queues.association, Association::Same);
    assert_eq!(
        queues.balance.entry,
        Change::Invalid(Invalid::ClockWentBack)
    );
    let tid0 = queues.tids.get(&0).unwrap();
    for change in [
        tid0.tx_packets,
        tid0.drops,
        tid0.backlog_packets,
        tid0.overlimit,
    ] {
        assert_eq!(change, Change::Invalid(Invalid::ClockWentBack));
    }
    assert_eq!(
        queues.tids.get(&7).unwrap().tx_packets,
        Change::Invalid(Invalid::ClockWentBack)
    );
    assert!(matches!(queues.balance.remainder, Remainder::Open(_)));
    // Equal times stand: the clock steps 10 ms.
    let same_tick = read(100_000, Some(1000), 200, 100_000, rows(200), None);
    let queues = window(&start, &same_tick);
    assert_eq!(delta(queues.balance.entry), 100);
    assert_eq!(delta(queues.tids.get(&0).unwrap().tx_packets), 100);
    assert_eq!(queues.balance.remainder, Remainder::Packets(0));
}

#[test]
fn a_radio_change_across_a_router_clock_that_went_back_is_invalid() {
    // The owner's counterexample on the radio: 100 at 100 s, 200 at 5 s.
    let start = read(
        100_000,
        None,
        1,
        100_000,
        None,
        Some(&[("fq_overmemory", 100)]),
    );
    let end = read(5000, None, 1, 5000, None, Some(&[("fq_overmemory", 200)]));
    let queues = window(&start, &end);
    assert_eq!(
        *queues.radio.get("fq_overmemory").unwrap(),
        Change::Invalid(Invalid::ClockWentBack)
    );
    assert!(
        queues
            .radio
            .values()
            .all(|change| *change == Change::Invalid(Invalid::ClockWentBack)),
        "every radio value"
    );
    // Its own read's clock suffices, though the station's went forward.
    let late = read(
        200_000,
        None,
        1,
        5000,
        None,
        Some(&[("fq_overmemory", 200)]),
    );
    assert_eq!(
        *window(&start, &late).radio.get("fq_overmemory").unwrap(),
        Change::Invalid(Invalid::ClockWentBack)
    );
    let forward = read(
        100_000,
        None,
        1,
        100_000,
        None,
        Some(&[("fq_overmemory", 200)]),
    );
    assert_eq!(
        delta(*window(&start, &forward).radio.get("fq_overmemory").unwrap()),
        100
    );
}
