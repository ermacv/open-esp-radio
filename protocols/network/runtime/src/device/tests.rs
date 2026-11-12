extern crate std;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::vec::Vec;

use core::task::{Context, Poll, Waker};

use oer_protocols_network_port::{
    BatchEnd, CopyReason, EthernetFrameView, FrameLengthError, FrameMeta, FramePort, FrameSource,
    FrameStorage, Incoming, LentFrame, LinkSnapshot, LinkState, NetDevice, OwnedTransmit,
    PortRefusal, ReceivedBatch, StorageAdvanceRefused, Unpublished,
};

use super::DevicePort;

struct Lent(Vec<u8>, Option<Arc<AtomicU32>>);

impl Drop for Lent {
    fn drop(&mut self) {
        if let Some(returned) = &self.1 {
            returned.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl FrameStorage for Lent {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
    fn advance(&mut self, by: usize) -> Result<(), StorageAdvanceRefused> {
        Err(StorageAdvanceRefused {
            by,
            length: self.0.len(),
        })
    }
}

/// A frame the device queues.
enum Queued {
    Lent(LentFrame<Lent>),
    Copied,
}

const FIELDS: [u8; 6] = [2, 0, 0, 0, 0, 1];

fn lent(payload: &[u8]) -> Queued {
    lent_from(Lent(payload.to_vec(), None))
}

fn lent_from(storage: Lent) -> Queued {
    let range = 0..storage.0.len();
    Queued::Lent(
        LentFrame::new(storage, FIELDS, FIELDS, 0x0800, range)
            .ok()
            .unwrap(),
    )
}

#[derive(Default)]
struct Device {
    incoming: VecDeque<Queued>,
    bad_copy: bool,
    link_changed: bool,
    /// The receive queue's break, which every later read answers.
    fault: Option<oer_protocols_network_port::ReceiveFault>,
    /// Receive batches asked of it.
    receives: usize,
}

impl NetDevice for Device {
    type Storage = Lent;
    fn link(&self) -> LinkSnapshot {
        LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        }
    }
    fn poll_link(&mut self, _: &mut Context<'_>) -> Poll<LinkSnapshot> {
        if core::mem::take(&mut self.link_changed) {
            Poll::Ready(self.link())
        } else {
            Poll::Pending
        }
    }
    fn receive_queue_len(&self) -> usize {
        self.incoming.len()
    }
    fn poll_receive_ready(&mut self, _: &mut Context<'_>) -> Poll<()> {
        if self.incoming.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(Incoming<'_, LentFrame<Lent>>),
    ) -> ReceivedBatch {
        self.receives += 1;
        if let Some(fault) = self.fault {
            return ReceivedBatch {
                taken: 0,
                end: BatchEnd::Fault(fault),
            };
        }
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let Some(frame) = self.incoming.pop_front() else {
                break BatchEnd::Drained;
            };
            taken += 1;
            match frame {
                Queued::Lent(frame) => take(Incoming::Lent(frame)),
                Queued::Copied => take(Incoming::Copied {
                    frame: if self.bad_copy {
                        Err(FrameLengthError::MissingCopy)
                    } else {
                        Ok(EthernetFrameView {
                            destination: FIELDS,
                            source: FIELDS,
                            ether_type: 0x0800,
                            payload: b"copied",
                        })
                    },
                    reason: CopyReason::Pressure,
                }),
            }
        };
        ReceivedBatch { taken, end }
    }
}

/// The transmit half that owns `free` buffers of `room` octets each, and
/// the queue a frame goes to the device by.
#[derive(Default)]
struct Transmit {
    sent: Vec<Vec<u8>>,
    free: usize,
    room: usize,
    /// The device's queue has no room.
    full: bool,
    /// The link is down.
    down: bool,
    flushes: u32,
    room_polls: u32,
    /// Times the port asked for what the device returned.
    takes: u32,
    /// Memory the device returned that the stack has not taken back.
    returned: u32,
    /// Memory the stack took back.
    taken_back: u32,
    /// The tickets given so far.
    tickets: u32,
    /// Frames whose buffer the device gave back, their ends not yet handed
    /// on: each buffer is free again only when its notice is taken.
    ended: VecDeque<u32>,
}

impl Transmit {
    fn take_back(&mut self) {
        self.taken_back += core::mem::take(&mut self.returned);
    }
}

impl OwnedTransmit for Transmit {
    type Ticket = u32;
    type Notice = u32;
    fn publish<E>(
        &mut self,
        meta: FrameMeta,
        fill: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ) -> Result<u32, Unpublished<PortRefusal, E>> {
        if self.down {
            return Err(Unpublished::Port(PortRefusal::LinkDown));
        }
        if self.free == 0 || self.full {
            return Err(Unpublished::Port(PortRefusal::NoRoom));
        }
        if meta.length > self.room {
            return Err(Unpublished::Port(PortRefusal::BadFrame));
        }
        let mut bytes = std::vec![0; meta.length];
        fill(&mut oer_protocols_network_port::Slot::new(&mut bytes, None))
            .map_err(Unpublished::Fill)?;
        self.free -= 1;
        self.tickets += 1;
        self.sent.push(bytes);
        Ok(self.tickets)
    }
    fn take_notice(&mut self) -> Option<u32> {
        if self.ended.is_empty() {
            self.take_back();
            self.takes += 1;
        }
        let ticket = self.ended.pop_front()?;
        self.free += 1;
        Some(ticket)
    }
    fn notice_waiting(&self) -> bool {
        !self.ended.is_empty()
    }
    fn poll_room(&mut self, _: &mut Context<'_>) -> Poll<()> {
        self.take_back();
        self.room_polls += 1;
        if self.free == 0 || self.full {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn flush(&mut self) {
        self.flushes += 1;
    }
    fn reclaim(&mut self) {
        self.take_back();
        self.takes += 1;
    }
}

type Port = DevicePort<Device, Transmit>;

fn port() -> Port {
    DevicePort::new(
        Device::default(),
        Transmit {
            free: 2,
            room: 64,
            ..Transmit::default()
        },
    )
}

/// A frame of `length` octets that is not IP.
const fn meta(length: usize) -> FrameMeta {
    FrameMeta { length, dscp: None }
}

/// A fill's own error.
#[derive(Debug, Eq, PartialEq)]
struct Declined;

fn octets(slot: &mut oer_protocols_network_port::Slot<'_>) -> Result<(), Declined> {
    slot.open().0.fill(7);
    Ok(())
}

/// A fill no port may call.
fn unreachable_fill(
    why: &'static str,
) -> impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), Declined> {
    move |_| panic!("{why}")
}

/// Whether registering says the port has something now. Registration
/// reports it and never wakes the caller for it.
fn ready_at_once(port: &mut Port) -> bool {
    let woken = std::sync::Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    let ready = port.register(&mut Context::from_waker(&waker)).any();
    assert!(
        !woken.0.load(std::sync::atomic::Ordering::SeqCst),
        "readiness is returned, not woken"
    );
    ready
}

#[derive(Default)]
struct Woken(std::sync::atomic::AtomicBool);

impl std::task::Wake for Woken {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// What one batch showed: each frame's payload and whether it was lent.
fn shown(port: &mut Port, limit: usize) -> (Vec<(Vec<u8>, bool)>, ReceivedBatch) {
    let mut seen = Vec::new();
    let batch = port.take_frames(limit, |incoming| match incoming {
        Incoming::Lent(frame) => seen.push((frame.view().unwrap().payload.to_vec(), true)),
        Incoming::Copied { frame, reason } => {
            assert_eq!(reason, CopyReason::Pressure);
            seen.push((
                frame.map_or(Vec::new(), |view| view.payload.to_vec()),
                false,
            ));
        }
    });
    (seen, batch)
}

#[test]
fn a_batch_shows_the_ready_prefix_in_order_up_to_its_limit() {
    let mut port = port();
    port.device
        .incoming
        .extend([lent(b"lent"), Queued::Copied, lent(b"last")]);
    let (seen, batch) = shown(&mut port, 2);
    assert_eq!(
        seen,
        [(b"lent".to_vec(), true), (b"copied".to_vec(), false)]
    );
    assert_eq!(
        batch,
        ReceivedBatch {
            taken: 2,
            end: BatchEnd::Limit
        }
    );
    let (seen, batch) = shown(&mut port, 8);
    assert_eq!(seen, [(b"last".to_vec(), true)]);
    assert_eq!(batch.end, BatchEnd::Drained, "never waits for a full batch");
    let (seen, batch) = shown(&mut port, 8);
    assert!(seen.is_empty() && !batch.found());
}

#[test]
fn a_kept_lent_frame_is_the_reader_s_until_it_drops() {
    let returned = Arc::new(AtomicU32::new(0));
    let mut port = port();
    port.device
        .incoming
        .push_back(lent_from(Lent(b"kept".to_vec(), Some(returned.clone()))));
    let mut kept = None;
    let _batch = port.take_frames(1, |incoming| {
        if let Incoming::Lent(frame) = incoming {
            kept = Some(frame);
        }
    });
    let kept = kept.expect("a lendable frame comes lent");
    assert_eq!(returned.load(Ordering::SeqCst), 0, "the reader holds it");
    assert_eq!(kept.view().unwrap().payload, b"kept");
    drop(kept);
    assert_eq!(returned.load(Ordering::SeqCst), 1, "its drop returns it");
}

#[test]
fn a_copy_the_device_cannot_show_comes_as_its_error() {
    let mut port = port();
    port.device.bad_copy = true;
    port.device.incoming.push_back(Queued::Copied);
    let mut errors = 0;
    let batch = port.take_frames(1, |incoming| {
        if let Incoming::Copied { frame: Err(_), .. } = incoming {
            errors += 1;
        }
    });
    assert_eq!((batch.taken, errors), (1, 1));
}

/// A broken receive queue is the port's named fault, never an empty
/// queue: no frame is shown, and a give-back stops at it.
#[test]
fn a_broken_receive_queue_is_a_fault_not_an_empty_queue() {
    use oer_protocols_network_port::ReceiveFault;
    let mut port = port();
    port.device.incoming.push_back(lent(&[1, 2]));
    port.device.fault = Some(ReceiveFault::Unfilled);
    let batch = port.take_frames(8, |_| panic!("no frame is shown"));
    assert_eq!(batch.end, BatchEnd::Fault(ReceiveFault::Unfilled));
    assert!(!batch.found());
    // The give-back stops at the break, says so, and leaves the device's
    // frames to the port's owner.
    assert_eq!(port.give_back(), Err(ReceiveFault::Unfilled));
    assert_eq!(port.device.incoming.len(), 1);
}

/// A give-back that meets a broken queue names the fault, still takes back
/// the transmit memory and notices, and never reads the queue again: a
/// later give-back or return service answers the same fault unread.
#[test]
fn a_give_back_that_meets_a_broken_queue_names_it_and_reads_it_no_more() {
    use oer_protocols_network_port::ReceiveFault;
    let mut port = port();
    port.device.incoming.push_back(lent(&[1, 2]));
    port.device.fault = Some(ReceiveFault::Lost);
    port.transmit.ended.push_back(7);
    assert_eq!(port.give_back(), Err(ReceiveFault::Lost));
    assert!(port.transmit.ended.is_empty(), "the notices were taken");
    let reads = port.device.receives;
    assert_eq!(port.give_back(), Err(ReceiveFault::Lost));
    let waker = Waker::from(std::sync::Arc::new(Woken::default()));
    assert_eq!(
        port.serve_returns(&mut Context::from_waker(&waker)),
        Err(ReceiveFault::Lost)
    );
    assert_eq!(
        port.device.receives, reads,
        "the broken queue is read no more"
    );
    assert_eq!(
        port.device.incoming.len(),
        1,
        "its frames stay with the port"
    );
}

#[test]
fn a_frame_goes_to_the_device_in_a_buffer() {
    let mut port = port();
    port.publish(meta(4), octets).unwrap();
    assert_eq!(port.transmit.sent, [std::vec![7; 4]]);
    assert_eq!(port.transmit.free, 1);
    port.flush();
    assert_eq!(port.transmit.flushes, 1);
}

#[test]
fn no_room_refuses_before_the_frame_is_written() {
    let mut port = port();
    port.transmit.free = 0;
    let refused = port.publish(meta(4), unreachable_fill("no buffer to write"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::NoRoom)));
    let mut port = self::port();
    port.transmit.full = true;
    let refused = port.publish(meta(4), unreachable_fill("no room"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::NoRoom)));
}

#[test]
fn a_frame_too_long_for_its_buffer_keeps_the_buffer_and_waits_for_nothing() {
    let mut port = port();
    let refused = port.publish(meta(100), unreachable_fill("no buffer holds it"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::BadFrame)));
    assert_eq!(port.transmit.free, 2);
    assert!(!ready_at_once(&mut port));
}

#[test]
fn a_failed_fill_comes_back_as_the_fill_s_and_keeps_the_buffer() {
    let mut port = port();
    let refused = port.publish(meta(4), |_| Err(Declined));
    assert_eq!(refused, Err(Unpublished::Fill(Declined)));
    assert_eq!(port.transmit.free, 2);
    assert!(port.transmit.sent.is_empty());
    assert!(
        !ready_at_once(&mut port),
        "it waits for nothing of the port"
    );
}

#[test]
fn an_idle_port_with_room_wakes_nobody() {
    assert!(!ready_at_once(&mut port()));
}

/// The memory the device returned comes back at each registration, with no
/// frame to send, and wakes nobody by itself.
#[test]
fn an_idle_port_takes_back_the_memory_the_device_returned() {
    let mut port = port();
    port.transmit.returned = 3;
    assert!(!ready_at_once(&mut port));
    assert_eq!(port.transmit.taken_back, 3);
    assert!(port.transmit.sent.is_empty());
}

#[test]
fn room_wakes_only_a_frame_that_met_none() {
    let mut port = port();
    port.transmit.full = true;
    assert!(port.publish(meta(4), octets).is_err());
    assert!(!ready_at_once(&mut port), "still no room");
    assert_eq!(port.transmit.room_polls, 1, "it asked");
    port.transmit.full = false;
    assert!(ready_at_once(&mut port));
    port.publish(meta(4), octets).unwrap();
    assert!(!ready_at_once(&mut port), "that frame went");
}

#[test]
fn no_buffer_wakes_nobody_until_one_is_free() {
    let mut port = port();
    port.transmit.free = 0;
    assert_eq!(
        port.publish(meta(4), octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    assert!(!ready_at_once(&mut port));
    port.transmit.free = 1;
    assert!(ready_at_once(&mut port));
}

#[test]
fn a_received_frame_and_a_link_change_wake_at_once() {
    let mut port = port();
    port.device.incoming.push_back(lent(b"in"));
    assert!(ready_at_once(&mut port));
    port.device.incoming.clear();
    port.device.link_changed = true;
    assert!(ready_at_once(&mut port));
    assert!(!ready_at_once(&mut port));
}

#[test]
fn a_down_link_with_room_wakes_nobody_until_it_changes() {
    let mut port = port();
    port.transmit.down = true;
    assert_eq!(
        port.publish(meta(4), octets),
        Err(Unpublished::Port(PortRefusal::LinkDown))
    );
    assert!(!ready_at_once(&mut port), "room is not what it waits for");
    assert_eq!(
        port.transmit.room_polls, 1,
        "asked for returned memory only"
    );
    port.device.link_changed = true;
    assert!(ready_at_once(&mut port));
}

#[test]
fn only_a_bad_frame_is_final() {
    assert!(Port::is_final(&PortRefusal::BadFrame));
    assert!(!Port::is_final(&PortRefusal::NoRoom));
    assert!(!Port::is_final(&PortRefusal::LinkDown));
}

/// The return path without a service: the port's owner gives back the
/// received frames and takes back the returned memory, and registers for
/// the next of each.
#[test]
fn a_port_without_its_service_still_gives_back_and_takes_back() {
    let returned = Arc::new(AtomicU32::new(0));
    let mut port = port();
    port.device
        .incoming
        .push_back(lent_from(Lent(b"held".to_vec(), Some(returned.clone()))));
    port.transmit.returned = 2;
    let woken = std::sync::Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    assert_eq!(port.serve_returns(&mut Context::from_waker(&waker)), Ok(()));
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    assert_eq!(port.transmit.taken_back, 2);
    assert!(port.device.incoming.is_empty());
}

#[test]
fn a_stop_gives_back_every_frame_and_takes_back_the_returned_memory() {
    let returned = Arc::new(AtomicU32::new(0));
    let mut port = port();
    for _ in 0..3 {
        let storage = Lent(b"held".to_vec(), Some(returned.clone()));
        port.device.incoming.push_back(lent_from(storage));
    }
    port.device.incoming.push_back(Queued::Copied);
    port.transmit.returned = 2;
    port.transmit.full = true;
    assert!(
        port.publish(meta(4), octets).is_err(),
        "a frame waits for room"
    );
    port.transmit.full = false;
    assert_eq!(port.give_back(), Ok(()));
    assert!(
        !ready_at_once(&mut port),
        "the ended exchange leaves no frame waiting for room"
    );
    assert_eq!(returned.load(Ordering::SeqCst), 3);
    assert_eq!(port.device.incoming.len(), 0);
    assert_eq!(
        port.transmit.taken_back, 2,
        "and takes back the returned memory"
    );
}

#[test]
fn each_frame_the_device_takes_has_its_own_ticket() {
    let mut port = port();
    let first = port.publish(meta(4), octets).unwrap();
    let second = port.publish(meta(4), octets).unwrap();
    assert_ne!(first, second);
    assert_eq!(port.transmit.free, 0);
}

/// The port hands each end on as its transmit half gives it, and asks for
/// what the device returned only when none is held.
#[test]
fn the_ends_go_on_as_the_transmit_half_gives_them() {
    let mut port = port();
    let first = port.publish(meta(4), octets).unwrap();
    let second = port.publish(meta(4), octets).unwrap();
    port.transmit.ended.extend([first, second]);
    let takes = port.transmit.takes;
    assert_eq!(port.take_notice(), Some(first));
    assert_eq!(port.take_notice(), Some(second));
    assert_eq!(port.transmit.takes, takes, "ends held go without asking");
    assert_eq!(port.take_notice(), None);
    assert!(port.transmit.takes > takes);
}

#[test]
fn a_waiting_end_wakes_the_service() {
    let mut port = port();
    assert!(!ready_at_once(&mut port));
    let first = port.publish(meta(4), octets).unwrap();
    port.transmit.ended.push_back(first);
    assert!(ready_at_once(&mut port));
    assert_eq!(port.take_notice(), Some(first));
    assert!(!ready_at_once(&mut port));
}

/// After its service ended, the port's give-back frees the buffers whose
/// ends no policy will settle.
#[test]
fn a_give_back_frees_the_buffers_of_ends_no_policy_settles() {
    let mut port = port();
    let first = port.publish(meta(4), octets).unwrap();
    let second = port.publish(meta(4), octets).unwrap();
    port.transmit.ended.extend([first, second]);
    assert_eq!(port.transmit.free, 0);
    assert_eq!(port.give_back(), Ok(()));
    assert_eq!(port.transmit.free, 2);
    assert_eq!(port.take_notice(), None);
}

/// Over a real slot queue: an empty published slot breaks it, with a lent
/// frame after the break. The give-back names the fault instead of ending
/// as a drained queue, and the frame stays in the queue's slot, reached
/// through the port its owner keeps, never through a dropped one.
#[test]
fn a_give_back_over_a_broken_slot_queue_names_its_fault_and_the_port_keeps_the_rest() {
    use oer_portable_memory::WakingSlotQueue;
    use oer_protocols_network_port::ReceiveFault;
    let returned = Arc::new(AtomicU32::new(0));
    let mut queue = WakingSlotQueue::<Option<LentFrame<Lent>>, 2>::new();
    let (mut sender, receiver) = queue.split(|| None);
    assert!(
        sender
            .try_send_with(|slot| {
                *slot = None;
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    let Queued::Lent(frame) = lent_from(Lent(b"after".to_vec(), Some(returned.clone()))) else {
        panic!("a lent frame");
    };
    assert!(
        sender
            .try_send_with(|slot| {
                *slot = Some(frame);
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    let port = DevicePort::new(Slots(receiver), Transmit::default());
    // A stop's end of the port: it does not go, it comes back with its fault.
    let Err(broken) = port.close() else {
        panic!("a broken queue is no closed port");
    };
    assert_eq!(broken.fault, ReceiveFault::Unfilled);
    let mut port = broken.port;
    assert_eq!(
        port.give_back(),
        Err(ReceiveFault::Unfilled),
        "and again, unread"
    );
    assert_eq!(
        port.device.0.len(),
        1,
        "the frame after the break stays queued"
    );
    assert_eq!(
        returned.load(Ordering::SeqCst),
        0,
        "held, not lost or dropped"
    );
    // Whoever keeps the port decides the frame's end: dropped with the queue,
    // it goes back once.
    drop(port);
    sender.close();
    drop(queue);
    assert_eq!(returned.load(Ordering::SeqCst), 1);
}

/// A station-like device over a real slot queue: held reads, one commit.
struct Slots<'a>(oer_portable_memory::SlotReceiver<'a, Option<LentFrame<Lent>>, 2>);
impl NetDevice for Slots<'_> {
    type Storage = Lent;
    fn link(&self) -> LinkSnapshot {
        LinkSnapshot::DOWN
    }
    fn poll_link(&mut self, _: &mut Context<'_>) -> Poll<LinkSnapshot> {
        Poll::Pending
    }
    fn receive_queue_len(&self) -> usize {
        self.0.len()
    }
    fn poll_receive_ready(&mut self, _: &mut Context<'_>) -> Poll<()> {
        if self.0.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(Incoming<'_, LentFrame<Lent>>),
    ) -> ReceivedBatch {
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            match self.0.try_receive_held_with(Option::take) {
                Ok(Some(frame)) => {
                    taken += 1;
                    take(Incoming::Lent(frame));
                }
                Ok(None) => {
                    break BatchEnd::Fault(oer_protocols_network_port::ReceiveFault::Unfilled);
                }
                Err(oer_portable_memory::SlotReceiveError::Lost) => {
                    break BatchEnd::Fault(oer_protocols_network_port::ReceiveFault::Lost);
                }
                Err(oer_portable_memory::SlotReceiveError::Empty) => break BatchEnd::Drained,
            }
        };
        self.0.commit();
        ReceivedBatch { taken, end }
    }
}

/// A queue whose break the service's intake meets first: the port keeps
/// that fault, so its close reads the queue no more and hands the port back
/// with the same fault, the frame after the break still queued.
#[test]
fn a_fault_the_intake_met_first_is_kept_through_the_close() {
    use oer_portable_memory::WakingSlotQueue;
    use oer_protocols_network_port::ReceiveFault;
    let returned = Arc::new(AtomicU32::new(0));
    let mut queue = WakingSlotQueue::<Option<LentFrame<Lent>>, 2>::new();
    let (mut sender, receiver) = queue.split(|| None);
    assert!(
        sender
            .try_send_with(|slot| {
                *slot = None;
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    let Queued::Lent(frame) = lent_from(Lent(b"after".to_vec(), Some(returned.clone()))) else {
        panic!("a lent frame");
    };
    assert!(
        sender
            .try_send_with(|slot| {
                *slot = Some(frame);
                Ok::<_, ()>(())
            })
            .is_ok()
    );
    let mut port = DevicePort::new(Slots(receiver), Transmit::default());
    let batch = port.take_frames(8, |_| panic!("no frame before the break"));
    assert_eq!(batch.end, BatchEnd::Fault(ReceiveFault::Unfilled));
    let again = port.take_frames(8, |_| panic!("the broken queue is read no more"));
    assert_eq!(again.end, BatchEnd::Fault(ReceiveFault::Unfilled));
    let Err(broken) = port.close() else {
        panic!("a queue the intake found broken is no closed port");
    };
    assert_eq!(broken.fault, ReceiveFault::Unfilled);
    assert_eq!(
        broken.port.device.0.len(),
        1,
        "the frame after the break stays queued"
    );
    assert_eq!(returned.load(Ordering::SeqCst), 0);
}
