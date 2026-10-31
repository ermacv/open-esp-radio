use super::*;
use crate::BootSmokePassed;
use oer_trace::{Channel, Domain, Kind};
use std::{format, vec, vec::Vec};

fn encode(panic: &PreviousPanic<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    Console::new(|chunk: &[u8]| {
        bytes.extend_from_slice(chunk);
        Ok::<_, ()>(())
    })
    .previous_panic(panic)
    .unwrap();
    bytes
}

fn line(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_prefix(b"\r\n")
        .unwrap()
        .strip_suffix(b"\r\n")
        .unwrap()
}

fn raw_frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut raw = vec![0, 0, 0, 0, tag];
    raw.extend_from_slice(payload);
    let crc = raw.iter().fold(0xffff, |crc, &byte| checksum(crc, byte));
    raw.extend_from_slice(&crc.to_le_bytes());
    let mut frame = PREFIX.to_vec();
    for byte in raw {
        frame.extend_from_slice(format!("{byte:02x}").as_bytes());
    }
    frame
}

#[test]
fn crc_matches_ccitt_false_check_value() {
    assert_eq!(
        b"123456789"
            .iter()
            .fold(0xffff, |crc, &byte| checksum(crc, byte)),
        0x29b1
    );
}

#[test]
fn previous_panic_round_trip_keeps_every_field_and_message_presence() {
    for message in [None, Some(""), Some("panic: Привет\n\r\0")] {
        for pc in [None, Some(0), Some(u32::MAX)] {
            let panic = PreviousPanic {
                hart: 1,
                in_interrupt: pc.is_some(),
                interrupted_pc: pc,
                line: u32::MAX,
                column: 1234,
                file: "目录/паника.rs",
                message,
            };
            let wire = encode(&panic);
            let mut scratch = [0; MAX_PAYLOAD + 7];
            assert_eq!(
                decode(line(&wire), &mut scratch),
                Ok(Some((0, Record::PreviousPanic(panic))))
            );
        }
    }
    let panic = PreviousPanic {
        hart: 0,
        in_interrupt: false,
        interrupted_pc: None,
        line: 0,
        column: 0,
        file: &"f".repeat(48),
        message: Some(&"m".repeat(64)),
    };
    let wire = encode(&panic);
    assert_eq!(
        decode(line(&wire), &mut [0; MAX_PAYLOAD + 7]),
        Ok(Some((0, Record::PreviousPanic(panic))))
    );
}

#[test]
fn truncation_corruption_unknown_records_and_versions_return_errors() {
    let frame = raw_frame(2, &[2, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
    for end in PREFIX.len()..frame.len() {
        assert!(
            decode(&frame[..end], &mut [0; MAX_PAYLOAD + 7]).is_err(),
            "cut {end}"
        );
    }
    let mut corrupt = frame.clone();
    corrupt[PREFIX.len()] = b'1';
    assert_eq!(
        decode(&corrupt, &mut [0; MAX_PAYLOAD + 7]),
        Err(DecodeError::Checksum)
    );
    corrupt[PREFIX.len()] = b'z';
    assert_eq!(
        decode(&corrupt, &mut [0; MAX_PAYLOAD + 7]),
        Err(DecodeError::Hex)
    );
    assert_eq!(
        decode(&raw_frame(99, &[]), &mut [0; MAX_PAYLOAD + 7]),
        Err(DecodeError::UnknownRecord)
    );
    assert_eq!(
        decode(b"OER-DIAG:02", &mut [0; MAX_PAYLOAD + 7]),
        Err(DecodeError::Version)
    );
    assert_eq!(
        decode(b"ROM boot banner", &mut [0; MAX_PAYLOAD + 7]),
        Ok(None)
    );
}

#[test]
fn malformed_panic_payloads_never_panic_even_with_a_valid_checksum() {
    for length in 0..=MAX_PAYLOAD + 1 {
        for byte in 0..=255 {
            let mut payload = vec![0; length];
            if length > 1 {
                payload[1] = byte;
            }
            if length > 14 {
                payload[14] = byte;
            }
            if length > 15 {
                payload[15] = byte;
            }
            let _ = decode(&raw_frame(1, &payload), &mut [0; MAX_PAYLOAD + 7]);
        }
    }
    let mut payload = vec![0; 17];
    payload[14] = 1;
    payload[16] = 0xff;
    assert_eq!(
        decode(&raw_frame(1, &payload), &mut [0; MAX_PAYLOAD + 7]),
        Err(DecodeError::Payload)
    );
}

#[test]
fn failed_write_consumes_sequence_and_the_next_record_resynchronizes() {
    let mut bytes = Vec::new();
    let mut writes = 0;
    let mut console = Console::new(|chunk: &[u8]| {
        writes += 1;
        if writes == 3 {
            return Err(());
        }
        bytes.extend_from_slice(chunk);
        Ok(())
    });
    assert!(console.event(&BootSmokePassed).is_err());
    console.event(&BootSmokePassed).unwrap();
    let records = bytes
        .split(|&byte| byte == b'\n')
        .filter(|line| line.starts_with(PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert!(
        decode(
            records[0].strip_suffix(b"\r").unwrap(),
            &mut [0; MAX_PAYLOAD + 7]
        )
        .is_err()
    );
    assert_eq!(
        decode(
            records[1].strip_suffix(b"\r").unwrap(),
            &mut [0; MAX_PAYLOAD + 7]
        ),
        Ok(Some((
            1,
            Record::Event {
                kind: BootSmokePassed::KIND,
                words: [0, 0]
            }
        )))
    );
}

#[test]
fn line_frames_remain_byte_identical_to_the_saved_stand_records() {
    struct PlatformEvent([u32; 2]);
    impl Event for PlatformEvent {
        const KIND: Kind = Kind::new(Domain::Platform, 1);
        const CHANNEL: Channel = const {
            match Channel::new(Domain::Platform, 0) {
                Ok(channel) => channel,
                Err(_) => panic!("invalid trace channel"),
            }
        };
        fn encode(&self) -> [u32; 2] {
            self.0
        }
        fn decode(words: [u32; 2]) -> Option<Self> {
            Some(Self(words))
        }
    }
    for fixture in [
        include_str!("fixtures/boot-smoke-static.txt"),
        include_str!("fixtures/boot-smoke-formatted-irq.txt"),
        include_str!("fixtures/radio-root-static.txt"),
        include_str!("fixtures/radio-root-formatted-irq.txt"),
    ] {
        let mut bytes = Vec::new();
        let mut console = Console::new(|part: &[u8]| {
            bytes.extend_from_slice(part);
            Ok::<_, ()>(())
        });
        let mut expected = Vec::new();
        for (sequence, line) in fixture.lines().enumerate() {
            let mut scratch = [0; MAX_PAYLOAD + 7];
            let (received, record) = decode(line.as_bytes(), &mut scratch).unwrap().unwrap();
            assert_eq!(received, sequence as u32);
            match record {
                Record::PreviousPanic(panic) => console.previous_panic(&panic).unwrap(),
                Record::Event { kind, words } if kind == BootSmokePassed::KIND => {
                    assert_eq!(words, [0, 0]);
                    console.event(&BootSmokePassed).unwrap();
                }
                Record::Event { kind, words } => {
                    assert_eq!(kind, PlatformEvent::KIND);
                    console.event(&PlatformEvent(words)).unwrap();
                }
                Record::Note(note) => console.note(&note).unwrap(),
            }
            expected.extend_from_slice(format!("\r\n{line}\r\n").as_bytes());
        }
        assert_eq!(bytes, expected);
    }
}

#[test]
fn failed_header_does_not_encode_or_sample_the_event() {
    use core::cell::Cell;
    struct Counted<'a>(&'a Cell<u32>);
    impl Event for Counted<'_> {
        const KIND: Kind = BootSmokePassed::KIND;
        const CHANNEL: Channel = BootSmokePassed::CHANNEL;
        fn encode(&self) -> [u32; 2] {
            self.0.set(self.0.get() + 1);
            [0, 0]
        }
        fn decode(_: [u32; 2]) -> Option<Self> {
            None
        }
    }
    // Prefix, sequence, tag and kind are sent before reading the event's
    // words, preserving the original writer-error behavior too.
    for failed_write in 1..=5 {
        let calls = Cell::new(0);
        let mut writes = 0;
        let mut console = Console::new(|_: &[u8]| {
            writes += 1;
            if writes == failed_write {
                Err(())
            } else {
                Ok(())
            }
        });
        assert!(console.event(&Counted(&calls)).is_err());
        assert_eq!(calls.get(), u32::from(failed_write == 5));
    }
}

#[test]
fn the_longest_note_fits_the_line_bound() {
    let mut path = crate::DiagPath::new();
    for name in ["abcdefghijklmnop"; 6] {
        path.name(name);
    }
    for word in 0..4 {
        path.word(word);
    }
    let note = crate::Note::registration(true, &path);
    let mut line = Vec::new();
    Console::new(|bytes: &[u8]| {
        line.extend_from_slice(bytes);
        Ok::<_, ()>(())
    })
    .note(&note)
    .unwrap();
    assert!(line.len() <= MAX_NOTE_LINE);
    assert!(line.starts_with(b"\r\nOER-DIAG:01") && line.ends_with(b"\r\n"));
}

#[test]
fn invalid_retained_text_writes_nothing_and_preserves_the_local_sequence() {
    use crate::record::{EncodeError, PanicTextError};
    let mut bytes = Vec::new();
    let mut console = Console::starting_at(
        |chunk: &[u8]| {
            bytes.extend_from_slice(chunk);
            Ok::<_, ()>(())
        },
        42,
    );
    let mut panic = PreviousPanic {
        hart: 0,
        in_interrupt: false,
        interrupted_pc: None,
        line: 1,
        column: 1,
        file: "valid.rs",
        message: None,
    };
    let file = "f".repeat(49);
    panic.file = &file;
    assert_eq!(
        console.previous_panic(&panic),
        Err(EncodeError::Input(PanicTextError::FileTooLong {
            bytes: 49
        }))
    );
    panic.file = "valid.rs";
    let message = "m".repeat(65);
    panic.message = Some(&message);
    assert_eq!(
        console.previous_panic(&panic),
        Err(EncodeError::Input(PanicTextError::MessageTooLong {
            bytes: 65
        }))
    );
    assert_eq!(console.sequence, 42);
    assert!(bytes.is_empty());
}

#[test]
fn a_panic_sink_error_retains_its_value_and_consumes_the_sequence() {
    let mut console = Console::starting_at(|_: &[u8]| Err("sink"), u32::MAX);
    let panic = PreviousPanic {
        hart: 0,
        in_interrupt: false,
        interrupted_pc: None,
        line: 1,
        column: 1,
        file: "valid.rs",
        message: None,
    };
    assert_eq!(
        console.previous_panic(&panic),
        Err(record::EncodeError::Sink("sink"))
    );
    assert_eq!(console.sequence, 0);
}

#[test]
fn a_console_started_at_the_next_sequence_continues_the_frames() {
    let mut first_bytes = Vec::new();
    let mut first = Console::new(|bytes: &[u8]| {
        first_bytes.extend_from_slice(bytes);
        Ok::<(), ()>(())
    });
    first.event(&BootSmokePassed).unwrap();
    let next = first.next_sequence();
    assert_eq!(next, 1);
    let mut second_bytes = Vec::new();
    let mut second = Console::starting_at(
        |bytes: &[u8]| {
            second_bytes.extend_from_slice(bytes);
            Ok::<(), ()>(())
        },
        next,
    );
    second.event(&BootSmokePassed).unwrap();
    assert_eq!(second.next_sequence(), 2);
    let hex: Vec<u8> = 1u32
        .to_le_bytes()
        .iter()
        .flat_map(|byte| format!("{byte:02x}").into_bytes())
        .collect();
    let line = second_bytes
        .strip_prefix(b"\r\nOER-DIAG:01".as_slice())
        .unwrap();
    assert!(
        line.starts_with(&hex),
        "the second console's first frame carries sequence 1"
    );
}
