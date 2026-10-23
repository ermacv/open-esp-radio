use std::vec::Vec;

use super::*;
use crate::path::{DiagPath, DiagnosticPath, MAX_PATH_NAMES};
use crate::record::{self, DecodeError, Record};

fn encode(note: &Note<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    record::encode(&Record::Note(*note), |chunk| {
        bytes.extend_from_slice(chunk);
        Ok::<_, ()>(())
    })
    .unwrap();
    bytes
}

fn decode(bytes: &[u8]) -> Result<Note<'_>, DecodeError> {
    match record::decode(bytes)? {
        Record::Note(note) => Ok(note),
        other => panic!("not a note: {other:?}"),
    }
}

#[test]
fn every_topic_round_trips_with_a_full_set_of_values() {
    for &topic in ALL_TOPICS {
        let mut note = Note::new(topic);
        for index in 0..topic.word_count() {
            note = note.word(0x1000_0000 + index as u32);
        }
        for _ in 0..topic.name_count() {
            note = note.named("Name");
        }
        let bytes = encode(&note);
        assert_eq!(decode(&bytes), Ok(note), "{topic:?}");
        // Every strict prefix is an incomplete record, never a different one.
        for cut in 0..bytes.len() {
            assert_eq!(
                decode(&bytes[..cut]),
                Err(DecodeError::Truncated),
                "{topic:?} {cut}"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(DecodeError::Payload), "{topic:?}");
    }
}

#[test]
fn a_typed_constructor_keeps_each_value_in_its_field() {
    let note = Note::cca_checkpoint("RingSample", Some(3), u64::MAX - 1, true, false, 1, 2, 3);
    assert_eq!(note.topic, Topic::CcaCheckpoint);
    assert_eq!(note.names(), ["RingSample"]);
    assert_eq!(note.words(), [4, u32::MAX - 1, u32::MAX, 1, 0, 1, 2, 3]);
    let none = Note::cca_checkpoint("BeforeRx", None, 0, false, false, 0, 0, 0);
    assert_eq!(none.words()[0], 0);
}

#[test]
fn an_unknown_topic_or_a_wrong_count_is_an_error_not_a_note() {
    let known = encode(&Note::rearm(1, 2, 3));
    let mut unknown = known.clone();
    unknown[1..3].copy_from_slice(&0xfff0_u16.to_le_bytes());
    assert_eq!(decode(&unknown), Err(DecodeError::UnknownRecord));
    let mut fewer = known.clone();
    fewer[3] -= 1;
    assert_eq!(decode(&fewer), Err(DecodeError::Payload));
    // A name where the topic has none.
    let mut named = known;
    named[4] = 1;
    named.extend([1, b'x']);
    assert_eq!(decode(&named), Err(DecodeError::Payload));
}

#[test]
fn a_value_that_keeps_less_than_it_has_marks_its_path() {
    let mut path = DiagPath::of(&Pair(&["Ok", "Tracked"]));
    assert!(!path.overflowed());
    path.truncated();
    let note = Note::registration(true, &path);
    assert_eq!(note.names(), ["Ok", "Tracked"]);
    assert!(note.overflowed());
}

#[test]
fn a_count_past_the_note_arrays_is_refused_when_read() {
    // Seven names declared, six sent: the count alone is refused.
    let mut bytes = encode(&Note::cca_rx_fail("step", &DiagPath::of(&Pair(&["A"]))));
    bytes[4] = 1 + MAX_PATH_NAMES as u8;
    for _ in 0..MAX_NAMES - 1 {
        bytes.extend([1, b'x']);
    }
    assert_eq!(decode(&bytes), Err(DecodeError::Payload));
}

#[test]
fn names_beyond_their_limits_are_refused_when_read() {
    let mut bytes = encode(&Note::registration(true, &DiagPath::of(&Pair(&["Full"]))));
    // Same record with the name longer than a name may be.
    let name_at = bytes.len() - 5;
    bytes[name_at] = (MAX_NAME + 1) as u8;
    bytes.extend(core::iter::repeat_n(b'x', MAX_NAME));
    assert_eq!(decode(&bytes), Err(DecodeError::Payload));
    let mut text = encode(&Note::registration(true, &DiagPath::of(&Pair(&["Full"]))));
    let last = text.len() - 1;
    text[last] = 0xff;
    assert_eq!(decode(&text), Err(DecodeError::Payload));
}

/// A value whose path is the names it lists.
struct Pair(&'static [&'static str]);

impl DiagnosticPath for Pair {
    fn push_path(&self, path: &mut DiagPath) {
        for name in self.0 {
            path.name(name);
        }
    }
}

#[test]
fn the_deepest_real_path_fits_with_its_numbers_and_longest_names() {
    // `Ok(Unavailable(Acquire(TrackingTime(TimeReversed(BluetoothIeee802154)))))`
    // and its two 64-bit numbers.
    let mut path = DiagPath::new();
    for name in [
        "Ok",
        "Unavailable",
        "Acquire",
        "TrackingTime",
        "TimeReversed",
        "BluetoothIeee802154",
    ] {
        path.name(name);
    }
    path.wide(u64::MAX);
    path.wide(1);
    assert!(!path.overflowed());
    assert_eq!(path.names().len(), MAX_PATH_NAMES);
    let note = Note::hold_fail(9, &path);
    assert!(!note.overflowed());
    assert_eq!(note.names().len(), MAX_PATH_NAMES);
    let bytes = encode(&note);
    assert!(bytes.len() <= record::MAX_RECORD_BYTES);
    assert_eq!(decode(&bytes), Ok(note));
}

#[test]
fn a_path_that_does_not_fit_says_so_and_keeps_what_fits() {
    let mut path = DiagPath::new();
    for _ in 0..MAX_PATH_NAMES + 1 {
        path.name("Deeper");
    }
    assert!(path.overflowed());
    assert_eq!(path.names().len(), MAX_PATH_NAMES);
    let note = Note::ieee802154_join_fail(&path);
    assert!(note.overflowed());
    let bytes = encode(&note);
    let read = decode(&bytes).unwrap();
    assert!(read.overflowed());
    assert_eq!(read.names().len(), MAX_PATH_NAMES);

    // The name budget of the note, not of the path, is also explicit.
    let long = "x".repeat(MAX_NAME);
    let long: &'static str = std::boxed::Box::leak(long.into_boxed_str());
    let mut wide = DiagPath::new();
    for _ in 0..MAX_PATH_NAMES {
        wide.name(long);
    }
    let note = Note::hold_fail(0, &wide);
    assert!(note.overflowed());
    assert!(note.names().iter().map(|name| name.len()).sum::<usize>() <= MAX_NAME_BYTES);
    assert_eq!(
        decode(&encode(&note)).map(|read| read.overflowed()),
        Ok(true)
    );
}

#[test]
fn a_result_names_its_side_before_the_value() {
    let ok: Result<Pair, Pair> = Ok(Pair(&["A"]));
    let err: Result<Pair, Pair> = Err(Pair(&["B", "C"]));
    assert_eq!(DiagPath::of(&ok).names(), ["Ok", "A"]);
    assert_eq!(DiagPath::of(&err).names(), ["Err", "B", "C"]);
}

#[cfg(feature = "render")]
mod render {
    use super::*;
    use crate::note::Field;

    fn text(note: &Note<'_>) -> std::string::String {
        let mut text = std::string::String::new();
        crate::render::note(note, &mut text).unwrap();
        text
    }

    #[test]
    fn every_layout_consumes_exactly_the_words_and_names_its_topic_declares() {
        for &topic in ALL_TOPICS {
            let (_, fields) = topic.layout();
            let words: usize = fields
                .iter()
                .map(|(_, field)| match field {
                    Field::Wide => 2,
                    Field::Bytes8 => 3,
                    Field::Bytes16 => 5,
                    Field::Name | Field::Path => 0,
                    _ => 1,
                })
                .sum();
            let names = fields
                .iter()
                .filter(|(_, field)| *field == Field::Name)
                .count();
            assert_eq!(
                (words, names),
                (topic.word_count(), topic.name_count()),
                "{topic:?}"
            );
            assert_eq!(
                topic.has_path(),
                fields.iter().any(|(_, field)| *field == Field::Path),
                "{topic:?}"
            );
            if topic.has_path() {
                assert_eq!(
                    fields.last().map(|(_, field)| *field),
                    Some(Field::Path),
                    "{topic:?}"
                );
            }
        }
    }

    #[test]
    fn a_sampled_point_prints_its_number_on_every_line_of_the_point() {
        assert_eq!(
            text(&Note::cca_enable("RingSample", Some(3), 1, 0, 1)),
            "OPEN_RADIO_HIL phy=cca-enable point=RingSample sample=Some(3) force=1 aux_force=0 mac_active=1"
        );
        assert!(
            text(&Note::cca_enable_register("Settled", Some(2), "R", 0x10))
                .contains("point=Settled sample=Some(2) name=R")
        );
        assert!(
            text(&Note::cca_register("RingSample", Some(0), "R", 0x10))
                .contains("point=RingSample sample=Some(0) name=R")
        );
    }

    #[test]
    fn notes_print_the_lines_the_firmware_used_to_format() {
        assert_eq!(
            text(&Note::stack_guard(1, 0xab, 0x4000_0010)),
            "OPEN_RADIO_HIL stack_guard hart=1 control=000000ab address=40000010"
        );
        assert_eq!(
            text(&Note::fatal_irq_pending(1, 2, 3, 4, 5, 0xffff_ffff)),
            "fatal irq_pending words=00000001,00000002,00000003,00000004,00000005,ffffffff"
        );
        assert_eq!(
            text(&Note::cca_register(
                "BeforeRx",
                None,
                "PHY_AGC_ORACLE.RX_11B_PATH_CONTROL_0",
                0xabc
            )),
            "OPEN_RADIO_HIL phy=cca-register point=BeforeRx sample=None \
             name=PHY_AGC_ORACLE.RX_11B_PATH_CONTROL_0 raw=0x00000abc"
        );
        assert_eq!(
            text(&Note::sar2_checkpoint("AfterRfWake", 0x2a, 9)),
            "OPEN_RADIO_HIL phy=sar2-checkpoint point=AfterRfWake raw=0x2a init=9"
        );
        assert_eq!(
            text(&Note::tx_rate_checkpoint(
                "AfterWifiCommonPower",
                1,
                2,
                0x30
            )),
            "OPEN_RADIO_HIL phy=tx-rate-checkpoint point=AfterWifiCommonPower raw=0x00000001 low=0x2 high=0x30"
        );
        assert_eq!(
            text(&Note::cca_checkpoint(
                "Settled",
                Some(2),
                5_000_000_000,
                true,
                false,
                0x10,
                0x20,
                0x30
            )),
            "OPEN_RADIO_HIL phy=cca-checkpoint point=Settled sample=Some(2) micros=5000000000 \
             mac_rx=true reload=false base=0x10 next=0x20 last=0x30"
        );
        assert_eq!(
            text(&Note::enter_ieee802154(11, -3)),
            "OPEN_RADIO_HIL phy=enter radio=Ieee802154 channel=11 power_dbm=-3"
        );
        assert_eq!(
            text(&Note::rearm(3, 20, 20_010)),
            "OPEN_RADIO_HIL rearm step=3 delay_ms=20 elapsed_us=20010"
        );
        assert_eq!(
            text(&Note::t7_prepare_failure_busy(None, true)),
            "OPEN_RADIO_HIL t7-prepare failure-busy=None complete=true"
        );
        assert_eq!(
            text(&Note::sar_tail_largest(2, 4, 7, u64::MAX)),
            "OPEN_RADIO_HIL sar_tail tick=2 first=4 largest=7,18446744073709551615"
        );
    }

    #[test]
    fn a_truncated_path_is_marked_in_the_text() {
        let mut path = DiagPath::new();
        for _ in 0..MAX_PATH_NAMES + 1 {
            path.name("X");
        }
        assert!(text(&Note::joined(&path)).ends_with("path-truncated"));
    }
}

#[cfg(feature = "render")]
mod parts {
    use super::*;
    use crate::note::Part;

    #[test]
    fn a_topic_with_no_text_of_its_own_continues_a_line_and_only_such_a_topic_does() {
        for &topic in ALL_TOPICS {
            let (prefix, _) = topic.layout();
            assert_eq!(
                prefix.is_empty(),
                matches!(topic.part(), Part::Middle | Part::Last),
                "{topic:?}"
            );
        }
    }
}

#[cfg(feature = "render")]
mod reports {
    use super::*;

    fn text(note: &Note<'_>) -> std::string::String {
        let mut text = std::string::String::new();
        crate::render::note(note, &mut text).unwrap();
        text
    }

    #[test]
    fn w2_lines_read_as_the_console_text_the_check_parses() {
        assert_eq!(
            text(&Note::w2_config(13, *b"DE", 10, 233, 2100, [0, 1], [0, 2])),
            "w2.config channel=13 country=DE cap=10 antenna_mbm=233 chip_mbm=2100 \
             target=xx:xx:xx:xx:00:01 stand=xx:xx:xx:xx:00:02"
        );
        assert_eq!(
            text(&Note::w2_channel_first(13, 12, true, 1, true, 1))
                + &text(&Note::w2_channel_last(1000, 1100, [10, 0, -10, 10], -3)),
            "w2.channel channel=13 sequence=12 allowed=1 sent=1 success=1 responses=1 \
             rate_kbps=1000 retune_us=1100 power=10,0,-10,10 ceiling=-3"
        );
        assert_eq!(
            text(&Note::w2_basic(&[0x82, 0x84], &[])),
            "w2.basic supported=8284 extended=-"
        );
        assert_eq!(
            text(&Note::w2_basic(&[], &[1, 2, 3, 4, 5, 6, 7, 8, 9])),
            "w2.basic supported=- extended=010203040506070809"
        );
        assert_eq!(
            text(&Note::w2_stats(0, 0, 0, 0, 16000, "-")),
            "w2.stats timeouts=0 collisions=0 colors=0 errors=0 irq_free=16000 error=-"
        );
    }

    #[test]
    fn w3_lease_reads_as_the_console_text_the_check_parses() {
        assert_eq!(
            text(&Note::w3_lease(true, [192, 0, 2, 8], [192, 0, 2, 1], 24)),
            "w3.lease acquired=1 address=192.0.2.8 router=192.0.2.1 prefix=24"
        );
    }
}
