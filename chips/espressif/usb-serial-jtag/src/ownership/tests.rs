//! The block's ownership over the fake block: one holder at a time, a held
//! block drops and counts instead of waiting, the driver takes only a free
//! block, and the fatal path takes it from the driver for good.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::vec::Vec;

use oer_diagnostics::Note;

use super::{Guarded, Owner, Ownership, RetireError};
use crate::fake::{Irq, Shared, Task};
use crate::{EndpointRegisters, Events, InterruptRegisters};

fn sent(block: &Shared) -> Vec<u8> {
    let mut block = block.lock();
    block.host_reads();
    block.packets.concat()
}

/// The owners of the block alive at once, and the most there ever were.
#[derive(Default)]
struct Owners {
    live: AtomicUsize,
    most: AtomicUsize,
}

impl Owners {
    fn enter(&self) {
        let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(live, Ordering::SeqCst);
    }

    fn most(&self) -> usize {
        self.most.load(Ordering::SeqCst)
    }
}

/// An owner of a block that always has room, counted while it lives.
struct Counted<'a>(&'a Owners);

impl<'a> Counted<'a> {
    fn new(owners: &'a Owners) -> Self {
        owners.enter();
        Self(owners)
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

impl EndpointRegisters for Counted<'_> {
    fn has_room(&self) -> bool {
        true
    }

    fn push(&mut self, _: u8) {}

    fn send(&mut self) {}

    fn has_data(&self) -> bool {
        false
    }

    fn pop(&mut self) -> u8 {
        0
    }

    fn enable_events(&mut self) {}
}

/// A test's own [`Owner`] with its own [`Ownership`].
macro_rules! owner {
    ($name:ident) => {
        struct $name;

        impl Owner for $name {
            fn ownership() -> &'static Ownership {
                static OWNERSHIP: Ownership = Ownership::new();
                &OWNERSHIP
            }
        }
    };
}

#[test]
fn the_early_writer_writes_and_hands_the_record_over() {
    let early = Ownership::new();
    let block = Shared::default();
    assert!(early.write(
        || Task(block.clone()),
        |writer| writer.text(b"OER_BOOT bootstrap=START\r\n")
    ));
    assert!(block.lock().in_flight, "the record went to the host");
    assert_eq!(sent(&block), b"OER_BOOT bootstrap=START\r\n");
    assert_eq!(early.take_lost(), 0);
    assert_eq!(early.retire(), Ok(()), "the record freed the block");
}

#[test]
fn a_retired_writer_builds_no_owner_and_hands_the_record_to_the_driver() {
    let early = Ownership::new();
    assert_eq!(early.retire(), Ok(()));
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
    assert_eq!(early.take_lost(), 0, "the driver writes the record");
    assert!(early.driver_holds());
}

#[test]
fn what_finds_no_room_is_dropped_and_counted_after_the_wait() {
    let early = Ownership::new();
    let block = Shared::default();
    // The host never reads: after one packet the FIFO stays full.
    let record = [b'x'; 100];
    assert!(early.write(|| Task(block.clone()), |writer| writer.text(&record)));
    assert_eq!(block.lock().fifo.len(), 64, "the FIFO took what fit");
    assert_eq!(early.take_lost(), 36, "the rest is counted");
    assert_eq!(early.take_lost(), 0, "the count is taken once");
}

#[test]
fn a_record_that_interrupts_a_record_is_dropped_and_counted() {
    let early = Ownership::new();
    let owners = Owners::default();
    let mut inner_captured = false;
    assert!(early.write(
        || Counted::new(&owners),
        |outer| {
            // An interrupt handler's record, while the boot's one holds
            // the block.
            assert!(early.write(
                || {
                    inner_captured = true;
                    Counted::new(&owners)
                },
                |inner| inner.text(b"inner"),
            ));
            outer.text(b"outer");
        },
    ));
    assert!(!inner_captured, "no second owner is built");
    assert_eq!(owners.most(), 1);
    assert_eq!(early.take_lost(), 5, "the inner record is counted");
}

#[test]
fn the_driver_is_refused_while_a_record_holds_the_block() {
    let early = Ownership::new();
    let owners = Owners::default();
    assert!(early.write(
        || Counted::new(&owners),
        |writer| {
            assert_eq!(early.retire(), Err(RetireError::Busy));
            writer.text(b"record");
        },
    ));
    assert_eq!(early.retire(), Ok(()), "the ended record freed the block");
    assert_eq!(early.retire(), Err(RetireError::Taken));
    assert!(!early.write(|| Counted::new(&owners), |writer| writer.text(b"late")));
    assert_eq!(owners.most(), 1);
}

#[test]
fn records_of_two_harts_and_the_driver_never_hold_the_block_together() {
    let early = Ownership::new();
    let owners = Owners::default();
    let driver = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                for _ in 0..2_000 {
                    early.write(|| Counted::new(&owners), |writer| writer.text(b"boot"));
                }
            });
        }
        scope.spawn(|| {
            while early.retire().is_err() {
                std::thread::yield_now();
            }
            // The driver's owner lives from here on.
            owners.enter();
            driver.store(true, Ordering::SeqCst);
        });
    });
    assert!(driver.load(Ordering::SeqCst));
    assert_eq!(owners.most(), 1, "one owner at a time");
}

#[test]
fn the_fatal_path_takes_the_block_from_the_suspended_driver_for_good() {
    owner!(Chip);
    let early = Chip::ownership();
    assert_eq!(early.retire(), Ok(()));
    let block = Shared::default();
    let mut endpoint = Guarded::<Chip, _>::new(Task(block.clone()));
    let mut interrupt = Guarded::<Chip, _>::new(Irq(block.clone()));
    assert!(
        endpoint.has_room(),
        "the driver's part works while it holds"
    );

    assert!(early.write_fatal(true, || Task(block.clone()), &Note::runtime_fail("entropy")));
    assert!(!sent(&block).is_empty(), "the note went out");
    assert!(!early.driver_holds());

    block.lock().events.out_received = true;
    block.lock().out.push_back(b'x');
    assert!(!endpoint.has_room(), "the driver sees no room");
    assert!(!endpoint.has_data(), "the driver sees no data");
    endpoint.push(b'y');
    endpoint.send();
    assert!(block.lock().fifo.is_empty(), "the driver writes nothing");
    assert_eq!(interrupt.take_events(), Events::NONE);
    assert!(
        block.lock().acknowledged.is_empty(),
        "the handler acknowledges nothing"
    );
    assert!(!early.write(|| Task(block.clone()), |writer| writer.text(b"late")));
}

#[test]
fn the_fatal_path_of_another_hart_leaves_the_driver_the_block() {
    owner!(Chip);
    let early = Chip::ownership();
    assert_eq!(early.retire(), Ok(()));
    let block = Shared::default();
    let endpoint = Guarded::<Chip, _>::new(Task(block.clone()));
    let mut captured = false;
    let wrote = early.write_fatal(
        false,
        || {
            captured = true;
            Task(block.clone())
        },
        &Note::runtime_fail("layout"),
    );
    assert!(!wrote && !captured, "no owner beside the running driver");
    assert!(early.take_lost() > 0, "the note's bytes are counted");
    assert!(early.driver_holds());
    assert!(endpoint.has_room());
}

#[test]
fn a_second_fatal_note_and_one_during_a_record_are_only_counted() {
    let early = Ownership::new();
    let owners = Owners::default();
    assert!(early.write(
        || Counted::new(&owners),
        |writer| {
            assert!(!early.write_fatal(true, || Counted::new(&owners), &Note::runtime_fail("a")));
            writer.text(b"record");
        },
    ));
    assert!(early.write_fatal(true, || Counted::new(&owners), &Note::runtime_fail("b")));
    assert!(!early.write_fatal(true, || Counted::new(&owners), &Note::runtime_fail("c")));
    assert_eq!(owners.most(), 1);
    assert!(early.take_lost() > 0);
    assert_eq!(early.retire(), Err(RetireError::Taken));
}

#[test]
fn a_fatal_note_before_the_driver_writes_and_keeps_the_block() {
    let early = Ownership::new();
    let block = Shared::default();
    assert!(early.write_fatal(false, || Task(block.clone()), &Note::runtime_fail("layout")));
    assert!(!sent(&block).is_empty(), "the note went out");
    assert_eq!(early.take_lost(), 0);
    assert_eq!(early.retire(), Err(RetireError::Taken));
}
