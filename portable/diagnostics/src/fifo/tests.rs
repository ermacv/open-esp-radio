use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use super::{Fifo, FifoWriter, Lost, Patience};
use crate::{Note, Record, Topic, decode, record::MAX_PAYLOAD};

/// A FIFO that holds `room` bytes until the receiver empties it.
#[derive(Clone)]
struct Fake(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    room: usize,
    /// Polls of `has_room` so far, and the poll from which the receiver has
    /// emptied the FIFO (none: it never does).
    polls: std::cell::Cell<u32>,
    drains_at: Option<u32>,
    bytes: Vec<u8>,
    flushes: usize,
}

impl Fake {
    fn with_room(room: usize) -> Self {
        Self(Rc::new(RefCell::new(State {
            room,
            ..State::default()
        })))
    }

    /// The receiver reads everything and leaves `room` bytes of space.
    fn drain(&self, room: usize) -> Vec<u8> {
        let mut state = self.0.borrow_mut();
        state.room = room;
        core::mem::take(&mut state.bytes)
    }
}

impl Fifo for Fake {
    fn has_room(&self) -> bool {
        let state = self.0.borrow();
        state.polls.set(state.polls.get() + 1);
        if state.drains_at.is_some_and(|at| state.polls.get() >= at) {
            drop(state);
            let mut state = self.0.borrow_mut();
            state.bytes.clear();
            state.drains_at = None;
            state.room = 64;
            return true;
        }
        state.bytes.len() < state.room
    }

    fn push(&mut self, byte: u8) {
        self.0.borrow_mut().bytes.push(byte);
    }

    fn flush(&mut self) {
        self.0.borrow_mut().flushes += 1;
    }
}

fn lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn decoded(line: &[u8]) -> (Topic, Vec<u32>) {
    let mut scratch = [0; MAX_PAYLOAD + 7];
    let Ok(Some((_, Record::Note(note)))) = decode(line, &mut scratch) else {
        panic!("the line is a note");
    };
    (note.topic, note.words().to_vec())
}

#[test]
fn a_note_that_fits_is_written_whole_and_flushed_once() {
    let fifo = Fake::with_room(4096);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::placement_pass());
    assert_eq!(lost.pending(), 0);
    assert_eq!(fifo.0.borrow().flushes, 1);
    let written = fifo.drain(0);
    let lines = lines(&written);
    assert_eq!(lines.len(), 1);
    assert_eq!(decoded(&lines[0]).0, Topic::PlacementPass);
}

#[test]
fn a_full_fifo_drops_the_note_and_counts_every_byte() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::entropy_pass());
    let counted = lost.pending();
    assert!(counted > 0);
    assert!(fifo.drain(0).is_empty());

    // The same note into a roomy FIFO is as long as the count says.
    let roomy = Fake::with_room(4096);
    FifoWriter::new(roomy.clone(), &Lost::new(), Patience::NONE).note(&Note::entropy_pass());
    assert_eq!(roomy.drain(0).len(), counted as usize);
}

#[test]
fn a_note_is_written_in_parts_until_the_room_ends() {
    let fifo = Fake::with_room(20);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::placement_pass());
    assert_eq!(fifo.drain(0).len(), 20);
    assert!(lost.pending() > 0);
}

#[test]
fn the_loss_is_reported_before_the_next_note_and_then_cleared() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::entropy_pass());
    let counted = lost.pending();

    fifo.drain(4096);
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::placement_pass());
    assert_eq!(lost.pending(), 0);
    let written = fifo.drain(0);
    let lines = lines(&written);
    assert_eq!(lines.len(), 2);
    assert_eq!(decoded(&lines[0]), (Topic::DiagLost, std::vec![counted]));
    assert_eq!(decoded(&lines[1]).0, Topic::PlacementPass);
}

#[test]
fn a_report_that_did_not_fit_keeps_the_count() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::entropy_pass());
    let counted = lost.pending();

    // Room for the start of the report only.
    fifo.drain(10);
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::entropy_pass());
    assert!(lost.pending() >= counted);
}

#[test]
fn text_is_flushed_and_its_loss_counted() {
    let fifo = Fake::with_room(5);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).text(b"OER_BOOT bootstrap=START\r\n");
    assert_eq!(fifo.drain(0), b"OER_B");
    assert_eq!(lost.pending(), 21);
}

#[test]
fn a_writer_with_patience_gets_the_whole_note_when_room_appears_in_time() {
    let fifo = Fake::with_room(0);
    fifo.0.borrow_mut().drains_at = Some(500);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::polls(1_000)).note(&Note::placement_pass());
    assert_eq!(lost.pending(), 0);
    let written = fifo.drain(0);
    assert_eq!(decoded(&lines(&written)[0]).0, Topic::PlacementPass);
}

#[test]
fn a_writer_with_patience_drops_and_counts_when_room_never_comes_and_stops_polling() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::polls(100)).note(&Note::placement_pass());
    assert!(lost.pending() > 0);
    // The budget is per record: the polls of the one record are bounded by
    // it, plus one plain check per byte.
    let polls = fifo.0.borrow().polls.get();
    assert!(polls <= 100 + lost.pending(), "{polls} polls");
}

#[test]
fn a_writer_without_patience_never_waits() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost, Patience::NONE).note(&Note::placement_pass());
    // One check per byte of the note and no more, and no flush before the
    // final one.
    assert!(lost.pending() > 0);
    assert_eq!(fifo.0.borrow().polls.get(), lost.pending());
    assert_eq!(fifo.0.borrow().flushes, 1);
}

#[test]
fn a_record_reports_the_bytes_it_dropped_and_only_none_is_whole() {
    let lost = Lost::new();
    let full = Fake::with_room(5);
    let dropped = FifoWriter::new(full.clone(), &lost, Patience::NONE).text(b"OER_BOOT\r\n");
    assert_eq!(dropped, 5);
    assert_eq!(
        super::Written::of_dropped(dropped),
        super::Written::Partial { dropped: 5 }
    );
    assert!(!super::Written::of_dropped(dropped).is_whole());
    let _ = lost.take();
    let roomy = Fake::with_room(64);
    let dropped = FifoWriter::new(roomy, &lost, Patience::NONE).note(&Note::placement_pass());
    assert_eq!(dropped, 0);
    assert!(super::Written::of_dropped(dropped).is_whole());
    for refused in [
        super::Written::NoConsole,
        super::Written::Busy,
        super::Written::Retired,
    ] {
        assert!(!refused.is_whole(), "{refused:?} acknowledges nothing");
    }
}
