extern crate std;

use super::header::{Header, LENGTH};
use super::*;

#[test]
fn a_header_reads_back_what_was_written_and_refuses_other_payloads() {
    let header = Header {
        run: 0x0102_0304,
        sequence: u64::MAX - 1,
        sent_us: 123_456_789,
        echo_request: true,
        echo_reply: false,
    };
    let mut payload = [0xa5; 1400];
    header.write(&mut payload).unwrap();
    assert_eq!(Header::read(&payload), Some(header));
    assert_eq!(&payload[..4], b"OERL");
    assert_eq!(payload[LENGTH], 0xa5, "the filler stays");
    assert!(Header::read(&payload[..LENGTH - 1]).is_none());
    assert!(header.write(&mut [0; LENGTH - 1]).is_none());
    let mut foreign = payload;
    foreign[0] = b'X';
    assert!(Header::read(&foreign).is_none());
    let mut unknown_flag = payload;
    unknown_flag[4] = 4;
    assert!(Header::read(&unknown_flag).is_none());
}

#[test]
fn the_largest_reports_fit_one_response() {
    let load = Load {
        now_us: u64::MAX,
        harts: (0..MAX_HARTS as u8)
            .map(|hart| HartLoad {
                hart,
                idle_us: u64::MAX,
            })
            .collect(),
        executors: (0..MAX_EXECUTORS as u8)
            .map(|executor| ExecutorLoad {
                executor,
                hart: 1,
                level: 3,
                work_us: u64::MAX,
            })
            .collect(),
    };
    let mut buffer = [0; 512];
    assert!(postcard::to_slice(&load, &mut buffer).unwrap().len() <= 256);
    let report = Collected::Report(Report {
        received: Received {
            datagrams: u64::MAX,
            highest: Some(u64::MAX),
            first_us: Some(u64::MAX),
            last_us: Some(u64::MAX),
            ..Received::default()
        },
        sent: Sent {
            datagrams: u64::MAX,
            first_us: Some(u64::MAX),
            last_us: Some(u64::MAX),
            ..Sent::default()
        },
    });
    assert!(postcard::to_slice(&report, &mut buffer).unwrap().len() <= 256);
}

#[test]
fn the_service_names_three_endpoints_and_their_answers_by_distinct_keys() {
    let keys = <Request as oer_hil_agent::base::Service>::KEYS;
    assert_eq!(keys.len(), 6);
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(distinct.len(), keys.len());
}
