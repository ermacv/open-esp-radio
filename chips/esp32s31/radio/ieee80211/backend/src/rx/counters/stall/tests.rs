use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::Wake;

/// A waker that counts its wakes.
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A snapshot whose every field differs from every other and from zero.
fn distinct() -> RxStallSnapshot {
    let bank = |from: u32| MacBank {
        counts: core::array::from_fn(|index| from + index as u32),
        frequency_offset: -(from as i32),
    };
    RxStallSnapshot {
        at: 1,
        dma: RxProgress { count: 2, at: 3 },
        driver: RxProgress { count: 4, at: 5 },
        device: RxDeviceView {
            rx_pending: true,
            continuation: true,
            cut: true,
            pausing: true,
            paused: true,
            queued: 6,
        },
        hardware: Some(RxHardwareView {
            walker: RxWalkerView {
                enabled: true,
                reload_doorbell: true,
                next_low: 7,
                next_upper: true,
                last_low: 8,
                base: Some(9),
                buffer_full: Some(10),
                runtime_state: 11,
            },
            ring: Some(RxRingView {
                base: 12,
                recycle_start: 13,
                accepted_tail: 14,
                observed: 15,
                completed: 16,
                remaining: Some(17),
                reload_pending: true,
            }),
            session: RxSessionView {
                descriptors: RxDescriptorCounts {
                    total: 18,
                    armed: 19,
                    done: 20,
                    software: 21,
                    reserved_owner: true,
                    reserved_done: true,
                },
                released: 22,
                detached: 23,
                staged: 24,
                masked: true,
                frozen: true,
                line_request_pending: true,
                receive_fact_pending: Some(true),
                mac_irqs: 25,
                doorbell_unmasks: 26,
                ignored_unmasks: 27,
                service_passes: 28,
            },
        }),
        mac: Some(bank(100)),
        periodic: Some(MacSample {
            at: 29,
            bank: bank(200),
        }),
    }
}

#[test]
fn a_snapshot_reads_back_whole_for_its_ticket_only() {
    let stall = RxStall::new();
    let ticket = stall.request();
    assert_eq!(stall.read(ticket, 0), None, "nothing taken yet");
    let snapshot = distinct();
    stall.record_snapshot(ticket, &snapshot);
    let reading = stall.read(ticket, 40).expect("taken");
    assert_eq!(reading.snapshot, snapshot, "every field fits its words");
    assert_eq!(reading.resume, None, "no service after it yet");
    assert_eq!(reading.age(snapshot.at), 39);

    // A later request names a new snapshot: the old one is not its.
    let next = stall.request();
    assert_ne!(next, ticket);
    assert_eq!(stall.read(next, 40), None);
}

#[test]
fn a_snapshot_without_registers_or_banks_reads_back_as_such() {
    let stall = RxStall::new();
    let ticket = stall.request();
    let snapshot = RxStallSnapshot {
        hardware: Some(RxHardwareView {
            ring: None,
            session: RxSessionView {
                receive_fact_pending: None,
                ..RxSessionView::default()
            },
            ..RxHardwareView::default()
        }),
        ..RxStallSnapshot::default()
    };
    stall.record_snapshot(ticket, &snapshot);
    assert_eq!(stall.read(ticket, 0).unwrap().snapshot, snapshot);
    let bare = RxStallSnapshot::default();
    stall.record_snapshot(ticket, &bare);
    let reading = stall.read(ticket, 0).unwrap().snapshot;
    assert_eq!(reading, bare);
    assert!(reading.hardware.is_none() && reading.mac.is_none() && reading.periodic.is_none());
}

#[test]
fn a_resume_belongs_to_the_snapshot_of_its_ticket() {
    let stall = RxStall::new();
    let first = stall.request();
    stall.record_snapshot(first, &distinct());
    let resume = RxStallResume {
        at: 50,
        dma: RxProgress { count: 3, at: 49 },
        driver: RxProgress { count: 5, at: 50 },
        serviced: true,
    };
    stall.record_resume(first, &resume);
    assert_eq!(stall.read(first, 60).unwrap().resume, Some(resume));
    // The next snapshot has no resume until its own is recorded.
    let second = stall.request();
    stall.record_snapshot(second, &distinct());
    assert_eq!(stall.read(second, 60).unwrap().resume, None);
}

#[test]
fn a_reader_never_takes_a_record_being_written() {
    let stall = RxStall::new();
    let ticket = stall.request();
    stall.record_snapshot(ticket, &distinct());
    let sequence = stall.snapshot.sequence.load(Ordering::Relaxed);
    stall
        .snapshot
        .sequence
        .store(sequence | 1, Ordering::Relaxed);
    assert_eq!(stall.read(ticket, 0), None, "an odd sequence is a write");
}

#[test]
fn a_request_wakes_the_registered_owner_and_names_a_new_ticket() {
    let stall = RxStall::new();
    let count = Arc::new(Count(AtomicUsize::new(0)));
    stall.register(&Waker::from(count.clone()));
    assert_eq!(stall.requested(0), None, "nothing asked");
    let ticket = stall.request();
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(stall.requested(0), Some(ticket));
    assert_eq!(stall.requested(ticket.get()), None, "served");
    // A wrap never makes zero a ticket.
    stall.request.store(u32::MAX, Ordering::Relaxed);
    assert_eq!(stall.request().get(), 1);
}

#[test]
fn progress_counts_from_one_and_never_wraps_to_never() {
    let mut progress = RxProgress::default();
    assert!(!progress.seen());
    progress.advance(0, 7);
    assert_eq!(progress, RxProgress::default(), "no step, no time");
    progress.advance(2, 7);
    assert_eq!(progress, RxProgress { count: 2, at: 7 });
    progress.count = u32::MAX;
    progress.advance(1, 9);
    assert!(progress.seen());
    assert_eq!(progress.at, 9);
}
