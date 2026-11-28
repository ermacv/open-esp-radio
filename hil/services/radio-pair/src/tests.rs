extern crate std;

use oer_protocols_hil_agent::base::Service;
use oer_protocols_hil_protocol::MAX_POSTCARD_BYTES;
use serde::Serialize;
use std::collections::BTreeSet;

use super::*;

fn encoded_length(message: &impl Serialize) -> usize {
    let mut buffer = [0_u8; 2 * MAX_POSTCARD_BYTES];
    postcard::to_slice(message, &mut buffer)
        .expect("a test message fits twice the frame")
        .len()
}

#[test]
fn the_largest_answer_of_each_endpoint_fits_one_frame() {
    let reading = Reading {
        item: Item::WifiSharedIndexDc,
        index: u8::MAX,
        value: Value::Signed(i32::MIN),
    };
    let page = SnapshotPage::Readings {
        page: u8::MAX,
        pages: u8::MAX,
        readings: core::iter::repeat_n(reading, PAGE_READINGS).collect(),
    };
    let counters = Counters::Counted {
        mac: MacCounters {
            mpdu: u16::MAX,
            fcs_error: u16::MAX,
            abort: u16::MAX,
            abort_fcs_pass: u16::MAX,
            power_drop_error: u16::MAX,
            signal_field: u16::MAX,
            end: u16::MAX,
            data_success: u16::MAX,
            other_unicast: u16::MAX,
            buffer_full: u16::MAX,
            fifo_overflow: u16::MAX,
            last_unmatched_error: u16::MAX,
            ack_interrupt: u16::MAX,
        },
        decode: DecodeCounters {
            brx_agc: u16::MAX,
            brx: u16::MAX,
            nrx: u16::MAX,
            nrx_abort: u16::MAX,
            nrx_agc_exit: u16::MAX,
            nrx_baseband_off: u16::MAX,
            nrx_fdm_watchdog: u16::MAX,
            nrx_restart: u16::MAX,
            nrx_service: u16::MAX,
            nrx_tx_over: u16::MAX,
            nrx_unsupported: u16::MAX,
        },
        hang: HangCounters {
            rx: u8::MAX,
            tx: u8::MAX,
            rx_tx_hang: u32::MAX,
            rx_tx_panic: u32::MAX,
        },
        ring: Some(RingState {
            standing: Standing {
                last: Some(u8::MAX),
                next: NextAt::Ring(u8::MAX),
                tail: u8::MAX,
                tail_word: TailWord::Completed,
                cursor: Cursor::Moved,
            },
            controls: core::iter::repeat_n(u32::MAX, RING_DESCRIPTORS).collect(),
            seats: core::iter::repeat_n(Seat::Done { length: u16::MAX }, RING_DESCRIPTORS)
                .collect(),
        }),
    };
    let ends = CaseEnds {
        tag: u32::MAX,
        acknowledged: [u32::MAX; BEFORE],
        timeouts: [u32::MAX; BEFORE],
        other: u32::MAX,
    };
    let sent = Sent::Sent {
        cases: core::iter::repeat_n(ends, CASES).collect(),
        micros: u32::MAX,
    };
    let counts = Counts {
        sender: u32::MAX,
        singles: u32::MAX,
        chains: u32::MAX,
        mismatched: u32::MAX,
        lengths: u32::MAX,
        foreign: u32::MAX,
        length_anomalies: u32::MAX,
        doorbell_anomalies: u32::MAX,
        patterns: u32::MAX,
        pattern_written: u32::MAX,
    };
    let chain = ChainCase {
        tag: u32::MAX,
        chains: u32::MAX,
        least: u32::MAX,
        most: u32::MAX,
        expected: u32::MAX,
        mismatched: u32::MAX,
    };
    let word = WordSeen {
        position: u8::MAX,
        word: u32::MAX,
        seen: u32::MAX,
    };
    let windowed = Windowed::Counted {
        counts,
        spares: u8::MAX,
        chains: core::iter::repeat_n(chain, CASES).collect(),
        words: core::iter::repeat_n(word, WORDS).collect(),
        words_overflow: u32::MAX,
    };
    let turned = Turned::Turned {
        before: counts,
        after: counts,
        handles: u8::MAX,
        stop_micros: u32::MAX,
        rf: TurnedRf::ClosedAndWoken {
            close_micros: u32::MAX,
            wake_micros: u32::MAX,
        },
        start_micros: u32::MAX,
    };
    let tracked = Turned::Turned {
        before: counts,
        after: counts,
        handles: u8::MAX,
        stop_micros: u32::MAX,
        rf: TurnedRf::Tracked {
            close_micros: u32::MAX,
            wake_micros: u32::MAX,
            track_micros: u32::MAX,
            conversions: Conversions {
                ready: u32::MAX,
                not_ready: u32::MAX,
                earliest_micros: u32::MAX,
                latest_micros: u32::MAX,
                most_observations: u16::MAX,
            },
        },
        start_micros: u32::MAX,
    };
    let wait = Wait {
        count: u32::MAX,
        asked_micros: u32::MAX,
        taken_micros: u32::MAX,
    };
    let up = BroughtUp::Up {
        station: [u8::MAX; 6],
        ap: [u8::MAX; 6],
        register_micros: u32::MAX,
        waits: Waits {
            settle_spun: wait,
            settle_timed: wait,
            completion: wait,
            bus_busy: wait,
            engine_retry: wait,
            yields: wait,
        },
    };
    let hang = HangCounters {
        rx: u8::MAX,
        tx: u8::MAX,
        rx_tx_hang: u32::MAX,
        rx_tx_panic: u32::MAX,
    };
    let failed = Sent::Failed(RoundFailure {
        failure: Failure {
            phase: u32::MAX,
            index: u32::MAX,
        },
        round: u32::MAX,
        tag: u32::MAX,
        micros: u32::MAX,
        bound_millis: u32::MAX,
        queue: QueueState {
            ownership: QueueOwnership::Published,
            raised: [
                QueueEvent::Completion,
                QueueEvent::Timeout,
                QueueEvent::Collision,
            ]
            .into_iter()
            .collect(),
            active_state: u8::MAX,
            hang,
        },
    });
    for (name, length) in [
        ("snapshot page", encoded_length(&page)),
        ("counters", encoded_length(&counters)),
        ("sent", encoded_length(&sent)),
        ("failed rounds", encoded_length(&failed)),
        ("windowed", encoded_length(&windowed)),
        ("turned", encoded_length(&turned)),
        ("tracked", encoded_length(&tracked)),
        ("brought up", encoded_length(&up)),
        (
            "bursted",
            encoded_length(&Bursted::Sent {
                ended: u16::MAX,
                acknowledged: u16::MAX,
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
        encoded_length(&SnapshotCold { page: u8::MAX }),
        encoded_length(&SnapshotUp { page: u8::MAX }),
        encoded_length(&Round { rounds: u32::MAX }),
        encoded_length(&Single { length: u16::MAX }),
        encoded_length(&Expect { sender: [0xff; 6] }),
        encoded_length(&Target {
            receiver: [0xff; 6],
        }),
        encoded_length(&Window {
            millis: u32::MAX,
            record: Record::Chains,
        }),
        encoded_length(&Stall { millis: u32::MAX }),
        encoded_length(&Turn {
            millis: u32::MAX,
            kind: TurnKind::CloseAndWake,
        }),
        encoded_length(&HandOver),
        encoded_length(&Burst {
            to: To::Receiver { length: u16::MAX },
            frames: u16::MAX,
            gap_micros: u32::MAX,
        }),
    ] {
        assert!(length <= usize::from(MAXIMUM_PAYLOAD_BYTES));
    }
}

#[test]
fn both_images_share_the_radio_s_endpoints_and_keep_their_own_apart() {
    let sender: BTreeSet<_> = SenderRequest::KEYS.iter().copied().collect();
    let receiver: BTreeSet<_> = ReceiverRequest::KEYS.iter().copied().collect();
    let shared: BTreeSet<_> = sender.intersection(&receiver).copied().collect();
    // Five endpoints (two snapshots, the bring-up, the counters, the stop)
    // and their four answers: the two snapshots share their page.
    assert_eq!(shared.len(), 9);
    // Each side's own: the sender four endpoints and four answers, the
    // receiver five and five (the hand-over's among them).
    assert_eq!(sender.len(), 9 + 8);
    assert_eq!(receiver.len(), 9 + 10);
}
