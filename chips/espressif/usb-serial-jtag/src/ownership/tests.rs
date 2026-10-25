//! The block's ownership over the fake block: one holder at a time, a held
//! block drops and counts instead of waiting, the driver takes only a free
//! block, and the fatal path takes only a free block, for good.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::vec::Vec;

use oer_diagnostics::Note;

use super::{Console, Ownership, RetireError};
use crate::EndpointRegisters;
use crate::fake::{Shared, Task};

fn sent(block: &Shared) -> Vec<u8> {
    let mut block = block.lock();
    block.host_reads();
    block.packets.concat()
}

/// The console's parts of the tests: an endpoint alone.
struct Parts<R>(R);

impl<R: EndpointRegisters> Console for Parts<R> {
    type Endpoint = R;

    fn endpoint(&mut self) -> &mut R {
        &mut self.0
    }
}

/// An endpoint that always has room and counts the bytes pushed to it.
#[derive(Default)]
struct Counting {
    pushed: AtomicUsize,
}

impl EndpointRegisters for Counting {
    fn has_room(&self) -> bool {
        true
    }

    fn push(&mut self, _: u8) {
        self.pushed.fetch_add(1, Ordering::SeqCst);
    }

    fn send(&mut self) {}

    fn has_data(&self) -> bool {
        false
    }

    fn pop(&mut self) -> u8 {
        0
    }

    fn enable_events(&mut self) {}
}

fn installed(block: &Shared) -> Ownership<Parts<Task>> {
    let early = Ownership::new();
    assert!(early.install(Parts(Task(block.clone()))).is_ok());
    early
}

#[test]
fn the_early_writer_writes_and_hands_the_record_over() {
    let block = Shared::default();
    let early = installed(&block);
    assert!(early.write(|writer| writer.text(b"OER_BOOT bootstrap=START\r\n")));
    assert!(block.lock().in_flight, "the record went to the host");
    assert_eq!(sent(&block), b"OER_BOOT bootstrap=START\r\n");
    assert_eq!(early.take_lost(), 0);
    assert!(early.retire().is_ok(), "the record freed the block");
}

#[test]
fn parts_are_installed_once() {
    let block = Shared::default();
    let early = installed(&block);
    assert!(early.install(Parts(Task(Shared::default()))).is_err());
}

#[test]
fn a_record_before_the_parts_are_installed_is_dropped_and_counted() {
    let early = Ownership::<Parts<Task>>::new();
    assert!(early.write(|writer| writer.text(b"start")));
    assert_eq!(early.take_lost(), 5, "the record is counted");
    assert_eq!(early.retire().err(), Some(RetireError::Empty));
    assert!(
        !early.write_fatal(&Note::runtime_fail("early")),
        "the caller keeps the note elsewhere"
    );
}

#[test]
fn a_retired_writer_hands_the_record_to_the_driver() {
    let block = Shared::default();
    let early = installed(&block);
    let _driver = early.retire().expect("a free block retires");
    assert!(
        !early.write(|writer| writer.text(b"late")),
        "the driver owns it"
    );
    assert_eq!(early.take_lost(), 0, "the driver writes the record");
    assert!(
        sent(&block).is_empty(),
        "nothing went out beside the driver"
    );
    assert_eq!(
        early.retire().err(),
        Some(RetireError::Taken),
        "the parts never come back"
    );
}

#[test]
fn what_finds_no_room_is_dropped_and_counted_after_the_wait() {
    let block = Shared::default();
    let early = installed(&block);
    // The host never reads: after one packet the FIFO stays full.
    let record = [b'x'; 100];
    assert!(early.write(|writer| writer.text(&record)));
    assert_eq!(block.lock().fifo.len(), 64, "the FIFO took what fit");
    assert_eq!(early.take_lost(), 36, "the rest is counted");
    assert_eq!(early.take_lost(), 0, "the count is taken once");
}

#[test]
fn a_record_that_interrupts_a_record_is_dropped_and_counted() {
    let early = Ownership::new();
    assert!(early.install(Parts(Counting::default())).is_ok());
    assert!(early.write(|outer| {
        // An interrupt handler's record, while the boot's one holds the
        // block.
        assert!(early.write(|inner| inner.text(b"inner")));
        outer.text(b"outer");
    }));
    assert_eq!(early.take_lost(), 5, "the inner record is counted");
    let Parts(endpoint) = early.retire().expect("the block is free again");
    assert_eq!(
        endpoint.pushed.load(Ordering::SeqCst),
        b"outer".len(),
        "only the outer record reached the block"
    );
}

#[test]
fn the_driver_is_refused_while_a_record_holds_the_block() {
    let early = Ownership::new();
    assert!(early.install(Parts(Counting::default())).is_ok());
    assert!(early.write(|writer| {
        assert_eq!(early.retire().err(), Some(RetireError::Busy));
        writer.text(b"record");
    }));
    assert!(early.retire().is_ok(), "the ended record freed the block");
    assert_eq!(early.retire().err(), Some(RetireError::Taken));
    assert!(!early.write(|writer| writer.text(b"late")));
}

#[test]
fn records_of_two_harts_and_the_driver_never_hold_the_block_together() {
    let early = Ownership::new();
    assert!(early.install(Parts(Counting::default())).is_ok());
    let holders = AtomicUsize::new(0);
    let most = AtomicUsize::new(0);
    let driver = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                for _ in 0..2_000 {
                    early.write(|writer| {
                        let now = holders.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        writer.text(b"boot");
                        holders.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            });
        }
        scope.spawn(|| {
            loop {
                match early.retire() {
                    Ok(_parts) => break,
                    Err(RetireError::Busy) => std::thread::yield_now(),
                    Err(error) => panic!("the block is installed and free later: {error:?}"),
                }
            }
            driver.store(true, Ordering::SeqCst);
        });
    });
    assert!(driver.load(Ordering::SeqCst));
    assert!(most.load(Ordering::SeqCst) <= 1, "one record at a time");
}

#[test]
fn the_fatal_path_never_takes_the_block_from_the_driver() {
    let block = Shared::default();
    let early = installed(&block);
    let _driver = early.retire().expect("a free block retires");
    assert!(
        !early.write_fatal(&Note::runtime_fail("entropy")),
        "the caller keeps the note elsewhere"
    );
    assert!(
        sent(&block).is_empty(),
        "nothing went out beside the driver"
    );
    assert_eq!(early.take_lost(), 0, "a kept note is not lost");
    assert_eq!(
        early.retire().err(),
        Some(RetireError::Taken),
        "the driver keeps it"
    );
}

#[test]
fn a_fatal_note_during_a_record_and_a_second_one_are_refused() {
    let early = Ownership::new();
    assert!(early.install(Parts(Counting::default())).is_ok());
    assert!(early.write(|writer| {
        assert!(!early.write_fatal(&Note::runtime_fail("a")));
        writer.text(b"record");
    }));
    assert!(early.write_fatal(&Note::runtime_fail("b")));
    assert!(!early.write_fatal(&Note::runtime_fail("c")));
    assert_eq!(
        early.take_lost(),
        0,
        "refused notes are kept by their callers"
    );
    assert_eq!(early.retire().err(), Some(RetireError::Taken));
}

#[test]
fn a_fatal_note_before_the_driver_writes_and_keeps_the_block() {
    let block = Shared::default();
    let early = installed(&block);
    assert!(early.write_fatal(&Note::runtime_fail("layout")));
    assert!(!sent(&block).is_empty(), "the note went out");
    assert_eq!(early.take_lost(), 0);
    assert_eq!(early.retire().err(), Some(RetireError::Taken));
    assert!(!early.write(|writer| writer.text(b"late")));
}

#[test]
fn fatal_notes_of_two_harts_and_the_driver_never_hold_the_block_together() {
    for _ in 0..200 {
        let early = Ownership::new();
        assert!(early.install(Parts(Counting::default())).is_ok());
        let written = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    if early.write_fatal(&Note::runtime_fail("x")) {
                        written.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
            scope.spawn(|| {
                if early.retire().is_ok() {
                    written.fetch_add(1, Ordering::SeqCst);
                }
            });
        });
        assert_eq!(
            written.load(Ordering::SeqCst),
            1,
            "one holder took the block"
        );
    }
}
