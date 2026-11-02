//! The block's ownership over the fake block: one holder at a time, a held
//! block drops and counts instead of waiting, the driver takes only a free
//! block, and the fatal path takes only a free block, for good.

use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::vec::Vec;

use oer_diagnostics::{Note, Written};

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
struct Counting<'a> {
    pushed: AtomicUsize,
    /// With this probe, `[` and `]` delimit an actual endpoint record.
    holders: Option<&'a Occupancy>,
}

#[derive(Default)]
struct Occupancy {
    active: AtomicUsize,
    most: AtomicUsize,
}

impl Occupancy {
    fn enter(&self) {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
    }

    fn leave(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    fn assert_exclusive(&self) {
        assert_eq!(self.active.load(Ordering::SeqCst), 0);
        assert_eq!(self.most.load(Ordering::SeqCst), 1, "one block holder");
    }
}

impl EndpointRegisters for Counting<'_> {
    type Interrupt = crate::fake::Irq;

    fn has_room(&self) -> bool {
        true
    }

    fn push(&mut self, byte: u8) {
        if let Some(holders) = self.holders {
            match byte {
                b'[' => holders.enter(),
                b']' => holders.leave(),
                _ => panic!("a probed record uses delimiters only"),
            }
        }
        self.pushed.fetch_add(1, Ordering::SeqCst);
    }

    fn send(&mut self) {}

    fn has_data(&self) -> bool {
        false
    }

    fn pop(&mut self) -> u8 {
        0
    }

    fn enable_events(
        &mut self,
        _authority: &impl oer_espressif_interrupt_table::EventAuthority<Self::Interrupt>,
    ) {
    }
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
    assert_eq!(
        early.write(|writer| writer.text(b"OER_BOOT bootstrap=START\r\n")),
        Written::Whole
    );
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
    assert_eq!(
        early.write(|writer| writer.text(b"start")),
        Written::NoConsole,
        "no console acknowledges nothing"
    );
    assert_eq!(early.take_lost(), 5, "the record is counted");
    assert_eq!(early.retire().err(), Some(RetireError::Empty));
    assert!(
        !early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("early"))),
        "the caller keeps the note elsewhere"
    );
}

#[test]
fn a_retired_writer_hands_the_record_to_the_driver() {
    let block = Shared::default();
    let early = installed(&block);
    let _driver = early.retire().expect("a free block retires");
    assert_eq!(
        early.write(|writer| writer.text(b"late")),
        Written::Retired,
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
    assert_eq!(
        early.write(|writer| writer.text(&record)),
        Written::Partial { dropped: 36 },
        "a prefix acknowledges nothing"
    );
    assert_eq!(block.lock().fifo.len(), 64, "the FIFO took what fit");
    assert_eq!(early.take_lost(), 36, "the rest is counted");
    assert_eq!(early.take_lost(), 0, "the count is taken once");
}

#[test]
fn a_record_that_interrupts_a_record_is_dropped_and_counted() {
    let early = Ownership::new();
    assert!(early.install(Parts(Counting::default())).is_ok());
    assert_eq!(
        early.write(|outer| {
            // An interrupt handler's record, while the boot's one holds the
            // block.
            assert_eq!(early.write(|inner| inner.text(b"inner")), Written::Busy);
            outer.text(b"outer")
        }),
        Written::Whole
    );
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
    assert_eq!(
        early.write(|writer| {
            assert_eq!(early.retire().err(), Some(RetireError::Busy));
            writer.text(b"record")
        }),
        Written::Whole
    );
    assert!(early.retire().is_ok(), "the ended record freed the block");
    assert_eq!(early.retire().err(), Some(RetireError::Taken));
    assert_eq!(early.write(|writer| writer.text(b"late")), Written::Retired);
}

#[test]
fn records_of_two_harts_and_the_driver_never_hold_the_block_together() {
    let holders = Occupancy::default();
    let early = Ownership::new();
    assert!(
        early
            .install(Parts(Counting {
                holders: Some(&holders),
                ..Counting::default()
            }))
            .is_ok()
    );
    // Ensure at least one early record reaches the endpoint even when the
    // driver wins the concurrent race immediately.
    assert_eq!(early.write(|writer| writer.text(b"[]")), Written::Whole);
    let start = Barrier::new(3);
    let finished = Barrier::new(3);
    let driver = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                start.wait();
                for _ in 0..2_000 {
                    let _written = early.write(|writer| {
                        // Refused callbacks use an empty FIFO and never
                        // enter the endpoint's occupancy interval.
                        let opened = writer.text(b"[");
                        std::thread::yield_now();
                        opened + writer.text(b"]")
                    });
                }
                finished.wait();
            });
        }
        scope.spawn(|| {
            start.wait();
            let _parts = loop {
                match early.retire() {
                    Ok(parts) => break parts,
                    Err(RetireError::Busy) => std::thread::yield_now(),
                    Err(error) => panic!("the block is installed and free later: {error:?}"),
                }
            };
            holders.enter();
            driver.store(true, Ordering::SeqCst);
            // Count the driver's ownership until both harts finish trying.
            finished.wait();
            holders.leave();
        });
    });
    assert!(driver.load(Ordering::SeqCst));
    holders.assert_exclusive();
    assert_eq!(
        early.write(|_| panic!("the driver keeps the block")),
        Written::Retired
    );
}

#[test]
fn overlapping_callbacks_lend_only_one_endpoint_and_count_every_refused_byte() {
    const HARTS: usize = 8;
    const RECORDS: usize = 64;
    let holders = Occupancy::default();
    let callbacks = Occupancy::default();
    let early = Ownership::new();
    assert!(
        early
            .install(Parts(Counting {
                holders: Some(&holders),
                ..Counting::default()
            }))
            .is_ok()
    );
    let barrier = Barrier::new(HARTS);
    std::thread::scope(|scope| {
        for _ in 0..HARTS {
            scope.spawn(|| {
                for _ in 0..RECORDS {
                    // Every previous write has returned before the next
                    // round; exactly one callback gets the real endpoint.
                    barrier.wait();
                    let written = early.write(|writer| {
                        callbacks.enter();
                        let opened = writer.text(b"[");
                        // Keep the winning loan until every refused
                        // callback is also running, without using time.
                        barrier.wait();
                        let closed = writer.text(b"]");
                        callbacks.leave();
                        opened + closed
                    });
                    assert!(matches!(written, Written::Whole | Written::Busy));
                }
            });
        }
    });
    assert_eq!(callbacks.active.load(Ordering::SeqCst), 0);
    assert_eq!(callbacks.most.load(Ordering::SeqCst), HARTS);
    holders.assert_exclusive();
    let Parts(endpoint) = early.retire().expect("all records ended");
    assert_eq!(endpoint.pushed.load(Ordering::SeqCst), 2 * RECORDS);
    assert_eq!(
        early.take_lost(),
        u32::try_from(2 * (HARTS - 1) * RECORDS).expect("the stress loss count fits"),
        "only refused callbacks lose their two bytes"
    );
}

#[test]
fn the_fatal_path_never_takes_the_block_from_the_driver() {
    let block = Shared::default();
    let early = installed(&block);
    let _driver = early.retire().expect("a free block retires");
    assert!(
        !early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("entropy"))),
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
    assert_eq!(
        early.write(|writer| {
            assert!(!early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("a"))));
            writer.text(b"record")
        }),
        Written::Whole
    );
    assert!(early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("b"))));
    assert!(!early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("c"))));
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
    assert!(early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("layout"))));
    assert!(!sent(&block).is_empty(), "the note went out");
    assert_eq!(early.take_lost(), 0);
    assert_eq!(early.retire().err(), Some(RetireError::Taken));
    assert_eq!(early.write(|writer| writer.text(b"late")), Written::Retired);
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
                    if early.write_fatal(&Note::runtime_fail(oer_diagnostics::name!("x"))) {
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
