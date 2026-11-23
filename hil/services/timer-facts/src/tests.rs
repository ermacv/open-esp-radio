extern crate std;

use oer_protocols_hil_agent::base::Service;
use oer_protocols_hil_protocol::MAX_POSTCARD_BYTES;
use serde::Serialize;

use super::*;

fn encoded_length(message: &impl Serialize) -> usize {
    let mut buffer = [0_u8; 2 * MAX_POSTCARD_BYTES];
    postcard::to_slice(message, &mut buffer)
        .expect("a test message fits twice the frame")
        .len()
}

#[test]
fn the_largest_answer_of_each_endpoint_fits_one_frame() {
    let spread = Spread {
        count: u16::MAX,
        least: i32::MIN,
        most: i32::MAX,
        sum: i64::MIN,
    };
    let ran = Ran::Done(Tally {
        trials: u16::MAX,
        lost: u16::MAX,
        before_strobe: u16::MAX,
        after_strobe: [u16::MAX; 3],
        early: u16::MAX,
        on_time: u16::MAX,
        first: spread,
        at: spread,
    });
    let attempt = Attempt::Made {
        strobe: i32::MIN,
        alarms: u8::MAX,
        first: Some(i32::MIN),
        second: Some(i32::MIN),
    };
    let swept = Swept::Done(core::iter::repeat_n(attempt, MOST_ATTEMPTS).collect());
    let timed = Timed::Made {
        strobe: i32::MIN,
        read: i32::MIN,
        alarms: u8::MAX,
        first: Some(u32::MAX),
    };
    let shot = Shot::Done(core::iter::repeat_n(timed, MOST_ATTEMPTS).collect());
    for (name, length) in [
        (
            "calibrated",
            encoded_length(&Calibrated::Rate {
                ticks: u32::MAX,
                cycles: u32::MAX,
            }),
        ),
        ("ran", encoded_length(&ran)),
        ("swept", encoded_length(&swept)),
        ("shot", encoded_length(&shot)),
        (
            "reached",
            encoded_length(&Reached::AtOnce { latency: u32::MAX }),
        ),
        (
            "looped",
            encoded_length(&Looped::Stopped {
                loads: u32::MAX,
                alarms: u32::MAX,
                missed: u32::MAX,
            }),
        ),
        (
            "words",
            encoded_length(&WordsRead {
                work: [true; 3],
                interrupt: [true; 3],
            }),
        ),
    ] {
        assert!(
            length <= MAX_POSTCARD_BYTES,
            "{name}: {length} bytes, past the frame's {MAX_POSTCARD_BYTES}"
        );
    }
}

#[test]
fn every_request_fits_the_images_payload_bound() {
    for length in [
        encoded_length(&Calibrate),
        encoded_length(&Run {
            case: Case::Behind {
                ticks: u32::MAX,
                over: Standing::Ahead,
            },
            trials: u16::MAX,
        }),
        encoded_length(&Run {
            case: Case::Apart(Apart {
                mixed: Mixed::Ahead,
                order: Order::Early,
            }),
            trials: u16::MAX,
        }),
        encoded_length(&Short {
            quarters: i16::MIN,
            over: Standing::Ahead,
            trials: u8::MAX,
        }),
        encoded_length(&Sweep {
            offset: i32::MIN,
            new: New::Behind,
            trials: u8::MAX,
        }),
        encoded_length(&Far { ahead: u64::MAX }),
        encoded_length(&Concurrent::Stop),
        encoded_length(&Words),
    ] {
        assert!(length <= usize::from(MAXIMUM_PAYLOAD_BYTES));
    }
}

#[test]
fn a_spread_keeps_its_least_greatest_and_sum() {
    let spread = [5, -3, 9, 0].into_iter().fold(Spread::EMPTY, Spread::with);
    assert_eq!(
        spread,
        Spread {
            count: 4,
            least: -3,
            most: 9,
            sum: 11,
        }
    );
    assert_eq!(Spread::EMPTY.count, 0);
}

#[test]
fn the_service_has_seven_endpoints_and_their_answers() {
    assert_eq!(Request::KEYS.len(), 14);
}
