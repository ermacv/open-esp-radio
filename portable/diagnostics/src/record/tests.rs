use super::*;
use std::{vec, vec::Vec};

fn encoded(record: &Record<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode(record, |part| {
        bytes.extend_from_slice(part);
        Ok::<_, ()>(())
    })
    .unwrap();
    bytes
}

#[test]
fn raw_record_layout_keeps_the_existing_tags_and_field_order() {
    let panic = Record::PreviousPanic(PreviousPanic {
        hart: 1,
        in_interrupt: true,
        interrupted_pc: Some(0x500123ab),
        line: 0x01020304,
        column: 0x0a0b0c0d,
        file: "p.rs",
        message: Some("é"),
    });
    let bytes = [
        1, 1, 7, 4, 3, 2, 1, 13, 12, 11, 10, 0xab, 0x23, 0x01, 0x50, 4, 2, b'p', b'.', b'r', b's',
        0xc3, 0xa9,
    ];
    assert_eq!(encoded(&panic), bytes);
    assert_eq!(decode(&bytes), Ok(panic));

    let event = Record::Event {
        kind: Kind::new(Domain::Platform, 1),
        words: [0x01020304, 0xaabbccdd],
    };
    let bytes = [2, 1, 1, 4, 3, 2, 1, 0xdd, 0xcc, 0xbb, 0xaa];
    assert_eq!(encoded(&event), bytes);
    assert_eq!(decode(&bytes), Ok(event));
}

#[test]
fn raw_panic_round_trip_is_independent_of_any_frame() {
    for message in [None, Some(""), Some("паника\n\r\0")] {
        for pc in [None, Some(0), Some(u32::MAX)] {
            let record = Record::PreviousPanic(PreviousPanic {
                hart: 1,
                in_interrupt: pc.is_some(),
                interrupted_pc: pc,
                line: u32::MAX,
                column: 1234,
                file: "目录/паника.rs",
                message,
            });
            let bytes = encoded(&record);
            assert_eq!(decode(&bytes), Ok(record));
            for end in 0..bytes.len() {
                assert!(decode(&bytes[..end]).is_err(), "cut {end}");
            }
        }
    }
    let file = "f".repeat(48);
    let message = "m".repeat(64);
    let record = Record::PreviousPanic(PreviousPanic {
        hart: 0,
        in_interrupt: false,
        interrupted_pc: None,
        line: 0,
        column: 0,
        file: &file,
        message: Some(&message),
    });
    let bytes = encoded(&record);
    assert_eq!(bytes.len(), 1 + 16 + 48 + 64);
    assert!(bytes.len() <= MAX_RECORD_BYTES);
    assert_eq!(decode(&bytes), Ok(record));
}

#[test]
fn raw_events_round_trip_with_their_trace_identity_and_both_words() {
    for domain in [
        Domain::Platform,
        Domain::Phy,
        Domain::Ieee80211,
        Domain::Bluetooth,
        Domain::Ieee802154,
    ] {
        for event in [0, 1, u8::MAX] {
            for words in [[0, 0], [u32::MAX, 0], [0, u32::MAX]] {
                let record = Record::Event {
                    kind: Kind::new(domain, event),
                    words,
                };
                let bytes = encoded(&record);
                assert_eq!(decode(&bytes), Ok(record));
                for end in 0..bytes.len() {
                    assert!(decode(&bytes[..end]).is_err());
                }
            }
        }
    }
}

#[test]
fn raw_unknown_invalid_and_trailing_fields_are_handled_errors() {
    assert_eq!(decode(&[]), Err(DecodeError::Truncated));
    assert_eq!(decode(&[99]), Err(DecodeError::UnknownRecord));
    assert_eq!(decode(&[2, 1, 0]), Err(DecodeError::UnknownRecord));
    let good = encoded(&Record::PreviousPanic(PreviousPanic {
        hart: 0,
        in_interrupt: false,
        interrupted_pc: None,
        line: 1,
        column: 2,
        file: "p",
        message: Some("m"),
    }));
    for (index, value) in [
        (2, 0x80),
        (2, 0),
        (11, 1),
        (15, 49),
        (16, 65),
        (17, 0xff),
        (18, 0xff),
    ] {
        let mut bad = good.clone();
        bad[index] = value;
        assert!(decode(&bad).is_err(), "field {index} value {value}");
    }
    let mut trailing = good;
    trailing.push(0);
    assert_eq!(decode(&trailing), Err(DecodeError::Payload));
    for length in 1..=MAX_RECORD_BYTES + 1 {
        for byte in 0..=255 {
            let mut bytes = vec![byte; length];
            bytes[0] = 1;
            let _ = decode(&bytes);
        }
    }
}

#[test]
fn raw_encoder_propagates_sink_errors_without_a_transport() {
    let mut writes = 0;
    let error = encode(
        &Record::Event {
            kind: BootSmokePassed::KIND,
            words: [0, 0],
        },
        |_| {
            writes += 1;
            if writes == 2 {
                Err("bounded payload is full")
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(error, Err(EncodeError::Sink("bounded payload is full")));
    assert_eq!(writes, 2);
}

#[test]
fn oversize_retained_text_is_refused_before_the_sink_is_called() {
    let file = "é".repeat(24) + "x";
    let message = "é".repeat(32) + "x";
    for (file, message, expected) in [
        (
            file.as_str(),
            None,
            PanicTextError::FileTooLong { bytes: 49 },
        ),
        (
            "valid.rs",
            Some(message.as_str()),
            PanicTextError::MessageTooLong { bytes: 65 },
        ),
    ] {
        let record = Record::PreviousPanic(PreviousPanic {
            hart: 0,
            in_interrupt: false,
            interrupted_pc: None,
            line: 1,
            column: 1,
            file,
            message,
        });
        let mut writes = 0;
        assert_eq!(
            encode(&record, |_| {
                writes += 1;
                Ok::<_, ()>(())
            }),
            Err(EncodeError::Input(expected))
        );
        assert_eq!(writes, 0);
    }
}

#[test]
fn a_secondary_ready_event_keeps_its_hart_and_stack_pointer() {
    let ready = SecondaryReady {
        hart: 1,
        sp: 0x5012_3450,
    };
    let words = ready.encode();
    assert_eq!(SecondaryReady::decode(words), Some(ready));
    assert_ne!(SecondaryReady::KIND, BootSmokePassed::KIND);
    let record = Record::Event {
        kind: SecondaryReady::KIND,
        words,
    };
    assert_eq!(decode(&encoded(&record)), Ok(record));
}

#[test]
fn a_hart_exchange_event_keeps_its_hart_request_and_answer() {
    let exchange = crate::HartExchange {
        hart: 1,
        request: 0xbeef,
        answer: 0xdead_0001,
    };
    let words = exchange.encode();
    assert_eq!(crate::HartExchange::decode(words), Some(exchange));
    assert_ne!(crate::HartExchange::KIND, SecondaryReady::KIND);
    assert_ne!(crate::HartExchange::KIND, BootSmokePassed::KIND);
    let record = Record::Event {
        kind: crate::HartExchange::KIND,
        words,
    };
    assert_eq!(decode(&encoded(&record)), Ok(record));
}

#[test]
fn hart_aes_and_liveness_events_keep_their_words_and_refuse_others() {
    for event in [
        crate::HartAes {
            hart: 1,
            matched: true,
        },
        crate::HartAes {
            hart: 1,
            matched: false,
        },
    ] {
        assert_eq!(crate::HartAes::decode(event.encode()), Some(event));
    }
    assert_eq!(crate::HartAes::decode([1, 2]), None);
    assert_eq!(crate::HartAes::decode([0x1_0000, 1]), None);
    let liveness = crate::HartLiveness {
        hart: 1,
        rounds: u32::MAX,
    };
    assert_eq!(
        crate::HartLiveness::decode(liveness.encode()),
        Some(liveness)
    );
    let kinds = [
        BootSmokePassed::KIND,
        SecondaryReady::KIND,
        crate::HartExchange::KIND,
        crate::HartAes::KIND,
        crate::HartLiveness::KIND,
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert!(!kinds[index + 1..].contains(kind), "{kind:?}");
    }
    let record = Record::Event {
        kind: crate::HartLiveness::KIND,
        words: liveness.encode(),
    };
    assert_eq!(decode(&encoded(&record)), Ok(record));
}
