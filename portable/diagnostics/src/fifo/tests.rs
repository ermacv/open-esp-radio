use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use super::{Fifo, FifoWriter, Lost};
use crate::{Note, Record, Topic, decode, record::MAX_PAYLOAD};

/// A FIFO that holds `room` bytes until the receiver empties it.
#[derive(Clone)]
struct Fake(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    room: usize,
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
    FifoWriter::new(fifo.clone(), &lost).note(&Note::placement_pass());
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
    FifoWriter::new(fifo.clone(), &lost).note(&Note::entropy_pass());
    let counted = lost.pending();
    assert!(counted > 0);
    assert!(fifo.drain(0).is_empty());

    // The same note into a roomy FIFO is as long as the count says.
    let roomy = Fake::with_room(4096);
    FifoWriter::new(roomy.clone(), &Lost::new()).note(&Note::entropy_pass());
    assert_eq!(roomy.drain(0).len(), counted as usize);
}

#[test]
fn a_note_is_written_in_parts_until_the_room_ends() {
    let fifo = Fake::with_room(20);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost).note(&Note::placement_pass());
    assert_eq!(fifo.drain(0).len(), 20);
    assert!(lost.pending() > 0);
}

#[test]
fn the_loss_is_reported_before_the_next_note_and_then_cleared() {
    let fifo = Fake::with_room(0);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost).note(&Note::entropy_pass());
    let counted = lost.pending();

    fifo.drain(4096);
    FifoWriter::new(fifo.clone(), &lost).note(&Note::placement_pass());
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
    FifoWriter::new(fifo.clone(), &lost).note(&Note::entropy_pass());
    let counted = lost.pending();

    // Room for the start of the report only.
    fifo.drain(10);
    FifoWriter::new(fifo.clone(), &lost).note(&Note::entropy_pass());
    assert!(lost.pending() >= counted);
}

#[test]
fn text_is_flushed_and_its_loss_counted() {
    let fifo = Fake::with_room(5);
    let lost = Lost::new();
    FifoWriter::new(fifo.clone(), &lost).text(b"OER_BOOT bootstrap=START\r\n");
    assert_eq!(fifo.drain(0), b"OER_B");
    assert_eq!(lost.pending(), 21);
}
