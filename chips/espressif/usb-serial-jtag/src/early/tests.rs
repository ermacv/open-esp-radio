//! The early writer over the fake block: it writes until retired, drops and
//! counts what finds no room, and a fatal note never waits.

use std::vec::Vec;

use oer_diagnostics::Note;

use super::Early;
use crate::fake::{Shared, Task};

fn sent(block: &Shared) -> Vec<u8> {
    let mut block = block.lock();
    block.host_reads();
    block.packets.concat()
}

#[test]
fn the_early_writer_writes_and_hands_the_record_over() {
    let early = Early::new();
    let block = Shared::default();
    assert!(early.write(
        || Task(block.clone()),
        |writer| writer.text(b"OER_BOOT bootstrap=START\r\n")
    ));
    assert!(block.lock().in_flight, "the record went to the host");
    assert_eq!(sent(&block), b"OER_BOOT bootstrap=START\r\n");
    assert_eq!(early.take_lost(), 0);
}

#[test]
fn a_retired_writer_builds_no_owner_and_writes_nothing() {
    let early = Early::new();
    early.retire();
    let mut captured = false;
    let wrote = early.write(
        || {
            captured = true;
            Task(Shared::default())
        },
        |writer| writer.text(b"late"),
    );
    assert!(!wrote, "the driver owns the block");
    assert!(!captured, "no owner is built beside the driver's");
}

#[test]
fn what_finds_no_room_is_dropped_and_counted_after_the_wait() {
    let early = Early::new();
    let block = Shared::default();
    // The host never reads: after one packet the FIFO stays full.
    let record = [b'x'; 100];
    assert!(early.write(|| Task(block.clone()), |writer| writer.text(&record)));
    assert_eq!(block.lock().fifo.len(), 64, "the FIFO took what fit");
    assert_eq!(early.take_lost(), 36, "the rest is counted");
    assert_eq!(early.take_lost(), 0, "the count is taken once");
}

#[test]
fn a_fatal_note_without_the_block_is_only_counted() {
    let early = Early::new();
    early.retire();
    early.write_fatal::<Task>(None, &Note::runtime_fail("layout"));
    assert!(early.take_lost() > 0, "the note's bytes are counted");
}

#[test]
fn a_fatal_note_writes_even_after_the_driver_took_the_block() {
    let early = Early::new();
    early.retire();
    let block = Shared::default();
    early.write_fatal(Some(Task(block.clone())), &Note::runtime_fail("layout"));
    let bytes = sent(&block);
    assert!(!bytes.is_empty(), "the note went out");
    assert_eq!(early.take_lost(), 0);
}
