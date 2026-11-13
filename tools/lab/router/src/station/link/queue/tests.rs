use oer_tools_lab_client::protocol::station_link::{
    StationLink,
    queue::{Counts, Kind, Observed, Scope, Unit},
};

use super::*;
use crate::station::link::parse_link;

const HEADER: &str = "tid ac backlog-bytes backlog-packets new-flows drops marks overlimit \
                      collisions tx-bytes tx-packets flags";

const RADIO: &str = "access name value\nR fq_flows_cnt 4096\nR fq_backlog 20\n\
                     R fq_overlimit 7\nR fq_overmemory 9\nR fq_collisions 18\n\
                     R fq_memory_usage 46080\nRW fq_memory_limit 4194304\n\
                     RW fq_limit 8192\nRW fq_quantum 300\n";

/// The router's answer for one read: the station's `tx packets` at
/// `station_ms`, its association time, and the two `aqm` files at
/// `queues_ms` (each empty for an unreadable file).
fn answer(
    station_ms: &str,
    associated: Option<&str>,
    tx_packets: u64,
    queues_ms: &str,
    station_aqm: &str,
    radio_aqm: &str,
) -> String {
    let associated = associated
        .map(|at| format!("\tassociated at [boottime]:\t{at}\n"))
        .unwrap_or_default();
    let clocked = |file: &str| {
        if file.is_empty() {
            String::new()
        } else {
            format!("clock: {queues_ms}\n{file}")
        }
    };
    format!(
        "[station]\n\trx bytes:\t1\n\trx packets:\t1\n\ttx bytes:\t1\n\
         \ttx packets:\t{tx_packets}\n\ttx retries:\t0\n\ttx failed:\t0\n{associated}\
         [station-clock]\n{station_ms}\n[channel]\n[survey]\n\
         [station-aqm]\n{}[radio-aqm]\n{}[radio]\n",
        clocked(station_aqm),
        clocked(radio_aqm),
    )
}

/// A station `aqm` file of TID 0 and TID 7 rows.
fn station(tid0: &str, tid7: &str) -> String {
    format!("{HEADER}\n0 2 {tid0} 0x6(RUN AMPDU NO-AMSDU)\n7 0 {tid7} 0x0(RUN)\n")
}

fn read(text: &str) -> StationLink {
    parse_link(text).unwrap()
}

#[test]
fn the_station_file_reads_every_column_by_its_header_with_its_unit_scope_and_time() {
    let file = station("1500 1 3 4 0 2 0 9000 6 ", "0 0 2 0 0 0 0 322 2");
    let link = read(&answer("100.00", Some("50.000s"), 10, "100.02", &file, ""));
    let queues = link.station_queues.unwrap();
    assert_eq!(queues.raw.header, HEADER);
    assert_eq!(queues.raw.observed_at_ms, 100_020);
    assert_eq!(
        queues.raw.lines,
        [
            "0 2 1500 1 3 4 0 2 0 9000 6  0x6(RUN AMPDU NO-AMSDU)",
            "7 0 0 0 2 0 0 0 0 322 2 0x0(RUN)"
        ]
    );
    assert!(queues.unknown_columns.is_empty() && queues.missing_columns.is_empty());
    let tid0 = &queues.tids[0];
    assert_eq!((tid0.tid, tid0.ac), (0, Some(2)));
    assert_eq!(tid0.flags.as_deref(), Some("0x6(RUN AMPDU NO-AMSDU)"));
    let scope = Scope::StationTid { tid: 0 };
    let observed = |value, counts| Some(Observed::new(value, counts, scope, 100_020));
    assert_eq!(tid0.backlog_bytes, observed(1500, Counts::QueuedBytes));
    assert_eq!(tid0.backlog_packets, observed(1, Counts::QueuedPackets));
    assert_eq!(tid0.new_flows, observed(3, Counts::FlowActivations));
    assert_eq!(tid0.drops, observed(4, Counts::CodelDrops));
    assert_eq!(tid0.marks, observed(0, Counts::EcnMarks));
    assert_eq!(tid0.overlimit, observed(2, Counts::OverlimitDropOperations));
    assert_eq!(tid0.collisions, observed(0, Counts::CollidedEnqueues));
    assert_eq!(tid0.tx_bytes, observed(9000, Counts::DequeuedBytes));
    assert_eq!(tid0.tx_packets, observed(6, Counts::DequeuedPackets));
    let overlimit = tid0.overlimit.unwrap();
    assert_eq!(
        (overlimit.unit, overlimit.kind),
        (Unit::Operation, Kind::Counter),
        "drop operations, not packets"
    );
    let backlog = tid0.backlog_packets.unwrap();
    assert_eq!((backlog.unit, backlog.kind), (Unit::Packet, Kind::Level));
    assert_eq!(tid0.tx_packets.unwrap().unit, Unit::Packet);
    assert_eq!(
        queues.tids[1].tx_packets.unwrap().scope,
        Scope::StationTid { tid: 7 }
    );
    assert_eq!(link.tid0_aqm_drops, Some(4));
}

#[test]
fn unknown_columns_stay_verbatim_and_missing_ones_are_unavailable_never_zero() {
    // Another kernel's order, a column this parser does not know and no
    // `marks` or `overlimit`.
    let file = "tx-packets tid drops future-count ac flags\n\
                27 0 5 99 2 0x0(RUN)\n";
    let link = read(&answer("1.00", Some("0.500s"), 10, "1.00", file, ""));
    let queues = link.station_queues.unwrap();
    assert_eq!(queues.unknown_columns, ["future-count"]);
    assert_eq!(
        queues.missing_columns,
        [
            "backlog-bytes",
            "backlog-packets",
            "new-flows",
            "marks",
            "overlimit",
            "collisions",
            "tx-bytes"
        ]
    );
    let tid0 = &queues.tids[0];
    assert_eq!(tid0.tx_packets.map(|value| value.value), Some(27));
    assert_eq!(tid0.drops.map(|value| value.value), Some(5));
    assert_eq!(
        (tid0.overlimit, tid0.marks, tid0.backlog_packets),
        (None, None, None)
    );
    assert_eq!(
        tid0.unknown.get("future-count").map(String::as_str),
        Some("99")
    );
    assert_eq!(queues.raw.lines, ["27 0 5 99 2 0x0(RUN)"]);
}

#[test]
fn the_radio_file_names_its_swapped_drop_counters_by_what_they_hold() {
    let radio = format!("{RADIO}R fq_future 3\n");
    let link = read(&answer("1.00", None, 1, "1.01", "", &radio));
    assert_eq!(link.station_queues, None, "no station file");
    let queues = link.radio_queues.unwrap();
    assert_eq!(queues.raw.header, "access name value");
    assert_eq!(queues.raw.lines.len(), 10);
    let value = |name: &str| queues.values.get(name).unwrap().value.unwrap();
    // `aqm_read` prints `fq->overmemory` as `fq_overlimit` and
    // `fq->overlimit` as `fq_overmemory`.
    assert_eq!(
        value("fq_overlimit").counts,
        Counts::RadioOvermemoryDropOperations
    );
    assert_eq!(value("fq_overmemory").counts, Counts::RadioDropOperations);
    assert_eq!(value("fq_overmemory").unit, Unit::Operation);
    assert_eq!(value("fq_backlog").unit, Unit::Packet);
    assert_eq!(value("fq_memory_usage").unit, Unit::Byte);
    assert_eq!(
        (value("fq_limit").kind, value("fq_limit").scope),
        (Kind::Setting, Scope::Radio)
    );
    assert_eq!(value("fq_flows_cnt").observed_at_ms, 1010);
    assert_eq!(
        queues.values.get("fq_limit").unwrap().access.as_deref(),
        Some("RW")
    );
    assert_eq!(queues.unknown_names, ["fq_future"]);
    assert_eq!(queues.values.get("fq_future").unwrap().value, None);
    assert!(queues.missing_names.is_empty());
    let short = read(&answer(
        "1.00",
        None,
        1,
        "1.01",
        "",
        "name access value\nfq_backlog R 4\n",
    ));
    let short = short.radio_queues.unwrap();
    assert_eq!(
        short.values.get("fq_backlog").unwrap().value.unwrap().value,
        4
    );
    assert_eq!(short.missing_names.len(), 8);
}

#[test]
fn a_file_its_header_does_not_describe_refuses_the_observation() {
    let good = station("0 0 0 0 0 0 0 0 0", "0 0 0 0 0 0 0 0 0");
    for station_aqm in [
        good.replace("0x6(RUN AMPDU NO-AMSDU)", "0x6(RUN AMPDU"),
        good.replace(" 0x0(RUN)", ""),
        good.replace("\n7 0 ", "\n0 0 "),
        good.replace("\n7 0 0", "\n7 0 x"),
        good.replace("tid ac", "ac"),
        good.replace("tid ac", "tid tid"),
        good.replace("\n7 0 ", "\n300 0 "),
        format!("{good}clock: 2.00\n{good}"),
    ] {
        let text = answer("1.00", None, 1, "1.00", &station_aqm, "");
        assert!(parse_link(&text).is_err(), "{station_aqm}");
    }
    for radio_aqm in [
        RADIO.replace("RW fq_limit 8192", "fq_limit 8192"),
        RADIO.replace("fq_backlog 20", "fq_backlog many"),
        RADIO.replace("access name value", "access key value"),
        format!("{RADIO}R fq_backlog 1\n"),
    ] {
        let text = answer("1.00", None, 1, "1.00", "", &radio_aqm);
        assert!(parse_link(&text).is_err(), "{radio_aqm}");
    }
    let unclocked = answer("1.00", None, 1, "1.00", &good, "").replace("clock: 1.00\n", "");
    assert!(parse_link(&unclocked).is_err());
    let bad_clock = answer("1.00", None, 1, "1.5", &good, "");
    assert!(parse_link(&bad_clock).is_err());
}

#[test]
fn the_clock_reads_hundredths_of_seconds() {
    assert_eq!(parse_clock("508749.69").unwrap(), 508_749_690);
    assert_eq!(parse_clock("0.05").unwrap(), 50);
    for refused in ["", "12", "12.5", "12.500", "x.10", "1.x0"] {
        assert!(parse_clock(refused).is_err(), "{refused}");
    }
}

#[test]
fn two_parsed_reads_make_the_client_s_window() {
    use oer_tools_lab_client::protocol::station_link::queue::{Change, Remainder, window};
    let start = read(&answer(
        "10.00",
        Some("5.000s"),
        1000,
        "10.02",
        &station("1500 1 3 4 0 2 0 9000 6", "0 0 2 0 0 0 0 322 2"),
        RADIO,
    ));
    let end = read(&answer(
        "16.00",
        Some("5.000s"),
        1500,
        "16.03",
        &station("31500 21 9 34 1 5 0 609000 406", "0 0 3 0 0 0 0 483 3"),
        &RADIO.replace("fq_overmemory 9", "fq_overmemory 12"),
    ));
    let queues = window(&start, &end);
    assert_eq!(queues.balance.remainder, Remainder::Packets(49));
    assert!(matches!(
        queues.radio.get("fq_overmemory"),
        Some(Change::Valid(delta)) if delta.change == 3
    ));
}

/// The phy's `aqm` text of the T3 run's first read (`evidence/001-t3`):
/// exactly the kernel's 199 characters, its `fq_quantum 300` cut to `30`.
const RADIO_CUT: &str = "access name value\nR fq_flows_cnt 4096\nR fq_backlog 0\n\
                         R fq_overlimit 1963106\nR fq_overmemory 1963106\n\
                         R fq_collisions 18\nR fq_memory_usage 0\n\
                         RW fq_memory_limit 4194304\nRW fq_limit 8192\nRW fq_quantum 30";

/// The radio's queues of one read whose radio file is `radio`, as the
/// router's script prints it: the file, then the newline it appends.
fn radio_read(radio: &str) -> RadioQueues {
    let file = format!("{radio}\n");
    read(&answer("1.00", Some("0.500s"), 10, "1.00", "", &file))
        .radio_queues
        .unwrap()
}

/// Every complete row's value, by name.
fn values(queues: &RadioQueues) -> Vec<(&str, u64)> {
    queues
        .values
        .iter()
        .filter_map(|(name, entry)| Some((name.as_str(), entry.value?.value)))
        .collect()
}

const COMPLETE: [(&str, u64); 8] = [
    ("fq_backlog", 0),
    ("fq_collisions", 18),
    ("fq_flows_cnt", 4096),
    ("fq_limit", 8192),
    ("fq_memory_limit", 4194304),
    ("fq_memory_usage", 0),
    ("fq_overlimit", 1963106),
    ("fq_overmemory", 1963106),
];

#[test]
fn a_radio_file_cut_at_the_kernels_buffer_keeps_every_complete_value_and_names_the_cut_one() {
    assert_eq!(RADIO_CUT.len(), RADIO_TEXT_MAX);
    let queues = radio_read(RADIO_CUT);
    assert_eq!(values(&queues), COMPLETE);
    assert_eq!(queues.cut_names, ["fq_quantum"]);
    assert!(queues.missing_names.is_empty());
    // `30` parses, yet it is a cut `300`: no value of it.
    assert!(!queues.values.contains_key("fq_quantum"));
    assert_eq!(queues.raw.lines.last().unwrap(), "RW fq_quantum 30");
}

#[test]
fn a_radio_file_cut_inside_a_name_still_gives_its_complete_values() {
    // Larger counters: the same file ends inside the last row's name.
    let text = RADIO_CUT
        .replace("fq_overlimit 1963106", "fq_overlimit 196310600")
        .replace("fq_overmemory 1963106", "fq_overmemory 1963106000")
        .replace("fq_collisions 18", "fq_collisions 18000");
    let text = &text[..RADIO_TEXT_MAX];
    assert!(text.ends_with("RW fq_qu"));
    let queues = radio_read(text);
    let mut expected = COMPLETE;
    expected[1].1 = 18_000;
    expected[6].1 = 196_310_600;
    expected[7].1 = 1_963_106_000;
    assert_eq!(values(&queues), expected);
    assert_eq!(queues.cut_names, ["fq_quantum"]);
    assert!(queues.missing_names.is_empty());
}

#[test]
fn a_radio_row_that_does_not_match_its_header_before_the_last_is_refused() {
    let text = RADIO_CUT
        .replace("R fq_collisions 18", "R fq_colli 18 x")
        .replace("fq_memory_usage 0", "fq_memory_usage 0000");
    assert_eq!(text.len(), RADIO_TEXT_MAX);
    let file = format!("{text}\n");
    assert!(parse_link(&answer("1.00", Some("0.500s"), 10, "1.00", "", &file)).is_err());
}

#[test]
fn a_shorter_whole_radio_file_reports_nothing_cut() {
    let queues = radio_read(RADIO.trim_end());
    assert!(queues.cut_names.is_empty() && queues.missing_names.is_empty());
    assert_eq!(
        queues.values["fq_quantum"].value.map(|value| value.value),
        Some(300)
    );
}

#[test]
fn a_station_file_cut_at_the_kernels_buffer_keeps_its_whole_rows() {
    // Seventeen TID rows at full width, the last of them cut by the buffer.
    let row = |tid: u8| {
        format!(
            "{tid} 2 4294967295 4294967295 4294967295 4294967295 4294967295 \
             4294967295 4294967295 4294967295 4294967295 0xf(STOP AMPDU NO-AMSDU DIRTY)"
        )
    };
    let whole = (0..17).map(row).collect::<Vec<_>>().join("\n");
    let text = format!("{HEADER}\n{whole}");
    let text = &text[..STATION_TEXT_MAX];
    let link = read(&answer(
        "1.00",
        Some("0.500s"),
        10,
        "1.00",
        &format!("{text}\n"),
        "",
    ));
    let queues = link.station_queues.unwrap();
    let cut = queues.cut_row.unwrap();
    assert!(
        row(16).starts_with(&cut) && cut != row(16),
        "the last row, cut"
    );
    assert_eq!(queues.tids.len(), 16, "every whole row");
    assert_eq!(queues.tids[15].tx_packets.unwrap().value, 4_294_967_295);
    // A whole file of the same rows but the last is not cut.
    let shorter = (0..16).map(row).collect::<Vec<_>>().join("\n");
    let link = read(&answer(
        "1.00",
        Some("0.500s"),
        10,
        "1.00",
        &format!("{HEADER}\n{shorter}\n"),
        "",
    ));
    assert!(link.station_queues.unwrap().cut_row.is_none());
}
