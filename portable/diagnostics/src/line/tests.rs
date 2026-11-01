use std::vec::Vec;

use super::*;
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

/// An error of two levels, as a protocol's would describe itself.
enum Failure {
    Peer { reason: u16 },
}
enum Connect {
    Failed(Failure),
}
impl DiagnosticPath for Failure {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Peer { reason } => {
                path.name("Peer");
                path.word(u32::from(*reason));
            }
        }
    }
}
impl DiagnosticPath for Connect {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::Failed(failure) => {
                path.name("Failed");
                failure.push_path(path);
            }
        }
    }
}

fn address() -> Line {
    crate::typed_line!("OER-WIFI ipv4 {}/{}", Ipv4([192, 168, 1, 7]), 24_u8)
}

#[test]
fn a_format_splits_into_its_pieces_around_its_holes() {
    let format = Format::parse("ipv4 {}/{}");
    assert_eq!(format.error(), None);
    assert_eq!(format.holes(), 2);
    assert_eq!(format.pieces(), ["ipv4 ", "/", ""]);
    assert!(format.ends_open());
    assert!(!Format::parse("{} again").ends_open());
    assert_eq!(Format::parse("plain").pieces(), ["plain"]);
    // Two holes side by side leave an empty piece, which adds no name.
    let note = crate::typed_line!("{}{}", 1_u8, 2_u8).note();
    assert_eq!(note.names().len(), 0);
    assert_eq!(note.words()[1..], [1, 2]);
}

#[test]
fn a_format_that_makes_no_line_says_why() {
    for brace in ["{:?}", "{x}", "{", "}", "{{}}", "a}b{}"] {
        assert_eq!(
            Format::parse(brace).error(),
            Some(FormatError::Brace),
            "{brace}"
        );
    }
    // Four holes and five pieces: nine segments.
    assert_eq!(
        Format::parse("a{}b{}c{}d{}e").error(),
        Some(FormatError::Segments)
    );
    assert_eq!(Format::parse("{}{}{}{}{}{}{}{}").error(), None);
    assert_eq!(
        Format::parse("{}{}{}{}{}{}{}{}{}").error(),
        Some(FormatError::Segments)
    );
    assert_eq!(
        Format::parse("a{}b{}c{}d{}e{}f{}g").error(),
        Some(FormatError::Pieces)
    );
    let long = "0123456789012345678901234567890123456789012345678901234567890123x";
    assert_eq!(Format::parse(long).error(), Some(FormatError::Bytes));
    let together = "0123456789012345678901234567890123456789012345678901234567890123\
                    {}01234567890123456789012345678901234";
    assert_eq!(Format::parse(together).error(), Some(FormatError::Bytes));
}

#[test]
fn a_line_round_trips_with_its_pieces_and_values_in_order() {
    let note = address().note();
    assert_eq!(note.topic, Topic::Line);
    assert_eq!(note.names(), ["OER-WIFI ipv4 ", "/"]);
    // The shape, then the address and the prefix length.
    assert_eq!(note.words()[1..], [0xc0a8_0107, 24]);
    assert!(!note.overflowed());
    let bytes = encode(&note);
    assert_eq!(decode(&bytes), Ok(note));
    for cut in 0..bytes.len() {
        assert_eq!(decode(&bytes[..cut]), Err(DecodeError::Truncated), "{cut}");
    }
}

#[test]
fn an_error_path_ends_the_line_and_nothing_follows_it() {
    let failed = Connect::Failed(Failure::Peer { reason: 15 });
    // A line built by hand; typed_line! refuses text after a path.
    let note = Line::new()
        .text("OER-WIFI connect-failed ")
        .value(&failed)
        .text("ignored")
        .note();
    assert_eq!(note.names(), ["OER-WIFI connect-failed ", "Failed", "Peer"]);
    assert_eq!(note.words()[1..], [15]);
    assert!(note.overflowed(), "a segment after the path is left out");
    assert_eq!(decode(&encode(&note)), Ok(note));
}

#[test]
fn what_does_not_fit_is_left_out_and_marked() {
    let mut line = Line::new();
    for _ in 0..MAX_SEGMENTS {
        line = line.value(&1_u32);
    }
    let full = line.value(&2_u32).note();
    assert!(full.overflowed());
    // Seven value words after the shape: the eighth segment has no room.
    assert_eq!(full.words().len(), MAX_WORDS);
    assert_eq!(decode(&encode(&full)), Ok(full));

    let long = Line::new()
        .text("0123456789012345678901234567890123456789012345678901234567890123")
        .text("01234567890123456789012345678901234")
        .note();
    assert_eq!(
        long.names().len(),
        1,
        "the second piece passes the byte budget"
    );
    assert!(long.overflowed());
}

#[test]
fn a_shape_that_its_words_or_names_do_not_match_is_refused() {
    // The prefix length's word is missing: one word fewer than the shape.
    let mut short = address().note();
    short.count -= 1;
    assert_eq!(decode(&encode(&short)), Err(DecodeError::Payload));
    // A piece is missing.
    let mut unnamed = address().note();
    unnamed.name_count -= 1;
    assert_eq!(decode(&encode(&unnamed)), Err(DecodeError::Payload));

    for shape in [0x9_u32, 0x20_u32, 0x82_u32] {
        // An unknown code, a gap before a code, a segment after the path.
        let mut note = Line::new().note();
        note.words[0] = shape;
        assert_eq!(
            decode(&encode(&note)),
            Err(DecodeError::Payload),
            "{shape:#x}"
        );
    }
}

#[cfg(feature = "render")]
#[test]
fn the_host_prints_the_line_as_its_application_wrote_it() {
    let text = |line: Line| {
        let mut text = std::string::String::new();
        crate::render::note(&line.note(), &mut text).unwrap();
        text
    };
    assert_eq!(text(address()), "OER-WIFI ipv4 192.168.1.7/24");
    assert_eq!(
        text(crate::typed_line!(
            "OER-WIFI connect-failed {}",
            Connect::Failed(Failure::Peer { reason: 15 })
        )),
        "OER-WIFI connect-failed Failed(Peer) numbers=15"
    );
    assert_eq!(
        text(crate::typed_line!(
            "{} {} {} {}",
            -3_i32,
            true,
            Hex(0xbeef),
            u64::MAX
        )),
        "-3 true 0xbeef 18446744073709551615"
    );
    assert_eq!(
        text(crate::typed_line!("OER-RESULT: pass wifi-station")),
        "OER-RESULT: pass wifi-station"
    );
    let mut full = Line::new();
    for _ in 0..=MAX_SEGMENTS {
        full = full.text("x");
    }
    assert_eq!(text(full), "xxxxxx line-truncated");
}
