extern crate std;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::vec::Vec;

use core::task::{Context, Poll, Waker};

use oer_protocols_network_port::{
    CopyReason, EthernetFrame, EthernetFrameView, FrameLengthError, FramePort, FrameSource,
    FrameStorage, Holding, Keep, LentFrame, LinkSnapshot, LinkState, NetDevice, NetTransmit,
    ReceivedFrame, StorageAdvanceRefused, Taken, TransmitError, TransmitRefused, Unpublished,
};

use super::{DevicePort, PortRefusal, TxBuffers};

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

enum Incoming {
    Lent(LentFrame<Lent>),
    Copied,
}

impl ReceivedFrame for Incoming {
    type Storage = Lent;
    type Copied = ();
    fn into_lent(self) -> Result<LentFrame<Lent>, ()> {
        match self {
            Self::Lent(frame) => Ok(frame),
            Self::Copied => Err(()),
        }
    }
    fn copy_reason((): &()) -> CopyReason {
        CopyReason::Pressure
    }
}

const FIELDS: [u8; 6] = [2, 0, 0, 0, 0, 1];

fn lent(payload: &[u8]) -> Incoming {
    lent_from(Lent(payload.to_vec(), None))
}

fn lent_from(storage: Lent) -> Incoming {
    let range = 0..storage.0.len();
    Incoming::Lent(
        LentFrame::new(storage, FIELDS, FIELDS, 0x0800, range)
            .ok()
            .unwrap(),
    )
}

#[derive(Default)]
struct Device {
    incoming: VecDeque<Incoming>,
    bad_copy: bool,
    link_changed: bool,
}

impl NetDevice for Device {
    type RxFrame = Incoming;
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
    fn try_receive(&mut self) -> Option<Incoming> {
        self.incoming.pop_front()
    }
    fn read_copied<R>(
        &mut self,
        (): (),
        read: impl FnOnce(EthernetFrameView<'_>) -> R,
    ) -> Result<R, FrameLengthError> {
        if self.bad_copy {
            return Err(FrameLengthError::MissingCopy);
        }
        Ok(read(EthernetFrameView {
            destination: FIELDS,
            source: FIELDS,
            ether_type: 0x0800,
            payload: b"copied",
        }))
    }
}

#[derive(Debug)]
struct Written(Vec<u8>);

impl EthernetFrame for Written {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Ok(EthernetFrameView {
            destination: FIELDS,
            source: FIELDS,
            ether_type: 0x0800,
            payload: &self.0,
        })
    }
}

#[derive(Default)]
struct Transmit {
    sent: Vec<Vec<u8>>,
    refuse: Option<TransmitError>,
    full: bool,
    flushes: u32,
    room_polls: u32,
    /// Times the port asked for what the device returned.
    takes: u32,
    /// Memory the device returned that the stack has not taken back.
    returned: u32,
    /// Memory the stack took back.
    taken_back: u32,
}

impl Transmit {
    fn take_back(&mut self) {
        self.taken_back += core::mem::take(&mut self.returned);
    }
}

impl NetTransmit for Transmit {
    type Frame = Written;
    fn reclaim(&mut self) {
        self.take_back();
        self.takes += 1;
    }
    fn has_room(&mut self) -> bool {
        !self.full
    }
    fn poll_room(&mut self, _: &mut Context<'_>) -> Poll<()> {
        self.take_back();
        self.room_polls += 1;
        if self.full {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn try_transmit(&mut self, frame: Written) -> Result<(), TransmitRefused<Written>> {
        if let Some(reason) = self.refuse {
            return Err(TransmitRefused { frame, reason });
        }
        self.sent.push(frame.0);
        Ok(())
    }
    fn flush(&mut self) {
        self.flushes += 1;
    }
}

/// `free` buffers of `room` octets.
struct Buffers {
    free: usize,
    room: usize,
    /// `write` refuses though a buffer looked free.
    refuse: Option<PortRefusal>,
    free_polls: u32,
    /// The tickets given so far.
    tickets: u32,
    /// Frames whose buffer the device gave back, their ends not yet handed
    /// on: each buffer is free again only when its notice is taken.
    ended: std::collections::VecDeque<u32>,
}

impl TxBuffers for Buffers {
    type Frame = Written;
    type Ticket = u32;
    type Notice = u32;
    fn available(&self) -> bool {
        self.free != 0
    }
    fn poll_free(&mut self, _: &mut Context<'_>) -> Poll<()> {
        self.free_polls += 1;
        if self.free == 0 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn write<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<Written, Unpublished<PortRefusal, E>> {
        if let Some(refusal) = self.refuse {
            return Err(Unpublished::Port(refusal));
        }
        if self.free == 0 {
            return Err(Unpublished::Port(PortRefusal::NoRoom));
        }
        if length > self.room {
            return Err(Unpublished::Port(PortRefusal::BadFrame));
        }
        let mut bytes = std::vec![0; length];
        fill(&mut bytes).map_err(Unpublished::Fill)?;
        self.free -= 1;
        Ok(Written(bytes))
    }
    fn restore(&mut self, _: Written) {
        self.free += 1;
    }
    fn ticket(&self, _: &Written) -> u32 {
        self.tickets + 1
    }
    fn take_notice(&mut self) -> Option<u32> {
        let ticket = self.ended.pop_front()?;
        self.free += 1;
        Some(ticket)
    }
    fn notice_waiting(&self) -> bool {
        !self.ended.is_empty()
    }
}

type Port = DevicePort<Device, Transmit, Buffers>;

fn port() -> Port {
    DevicePort::new(
        Device::default(),
        Transmit::default(),
        Buffers {
            free: 2,
            room: 64,
            refuse: None,
            free_polls: 0,
            tickets: 0,
            ended: std::collections::VecDeque::new(),
        },
    )
}

/// A fill's own error.
#[derive(Debug, Eq, PartialEq)]
struct Declined;

fn octets(slot: &mut [u8]) -> Result<(), Declined> {
    slot.fill(7);
    Ok(())
}

/// A fill no port may call.
fn unreachable_fill(why: &'static str) -> impl FnOnce(&mut [u8]) -> Result<(), Declined> {
    move |_| panic!("{why}")
}

/// Whether registering says the port has something now. Registration
/// reports it and never wakes the caller for it.
fn ready_at_once(port: &mut Port) -> bool {
    let woken = std::sync::Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    let ready = port.register(&mut Context::from_waker(&waker));
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

#[test]
fn a_lent_frame_and_a_copied_one_are_both_shown() {
    let mut port = port();
    port.device
        .incoming
        .extend([lent(b"lent"), Incoming::Copied]);
    let mut seen = Vec::new();
    let mut show = |frame: EthernetFrameView<'_>, holding| {
        seen.push((frame.payload.to_vec(), holding));
        Keep::Drop
    };
    assert!(matches!(port.take_frame(&mut show), Taken::Read));
    assert!(matches!(port.take_frame(&mut show), Taken::Read));
    assert!(matches!(
        port.take_frame(|_, _| panic!("nothing waits")),
        Taken::Empty
    ));
    assert_eq!(
        seen,
        [
            (b"lent".to_vec(), Holding::Lendable),
            (b"copied".to_vec(), Holding::Copied(CopyReason::Pressure))
        ]
    );
}

#[test]
fn a_kept_lent_frame_is_the_reader_s_until_it_drops_and_a_copy_stays_the_device_s() {
    let returned = Arc::new(AtomicU32::new(0));
    let mut port = port();
    port.device
        .incoming
        .push_back(lent_from(Lent(b"kept".to_vec(), Some(returned.clone()))));
    port.device.incoming.push_back(Incoming::Copied);
    let Taken::Kept(kept) = port.take_frame(|_, _| Keep::Keep) else {
        panic!("a lendable frame is kept");
    };
    assert_eq!(returned.load(Ordering::SeqCst), 0, "the reader holds it");
    assert_eq!(kept.view().unwrap().payload, b"kept");
    drop(kept);
    assert_eq!(returned.load(Ordering::SeqCst), 1, "its drop returns it");
    assert!(
        matches!(port.take_frame(|_, _| Keep::Keep), Taken::Read),
        "a copy is not the reader's to keep"
    );
}

#[test]
fn a_frame_the_device_cannot_show_is_counted() {
    let mut port = port();
    port.device.bad_copy = true;
    port.device.incoming.push_back(Incoming::Copied);
    assert!(matches!(
        port.take_frame(|_, _| panic!("unreadable")),
        Taken::Read
    ));
    assert_eq!(port.unreadable(), 1);
}

#[test]
fn a_frame_goes_to_the_device_in_a_buffer() {
    let mut port = port();
    port.publish(4, octets).unwrap();
    assert_eq!(port.transmit.sent, [std::vec![7; 4]]);
    assert_eq!(port.buffers.free, 1);
    port.flush();
    assert_eq!(port.transmit.flushes, 1);
}

#[test]
fn no_buffer_refuses_before_the_frame_is_written() {
    let mut port = port();
    port.buffers.free = 0;
    let refused = port.publish(4, unreachable_fill("no buffer to write"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::NoRoom)));
}

#[test]
fn a_full_device_refuses_before_the_frame_is_written() {
    let mut port = port();
    port.transmit.full = true;
    let refused = port.publish(4, unreachable_fill("no room"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::NoRoom)));
}

#[test]
fn a_frame_the_device_refuses_frees_its_buffer() {
    for (reason, expected) in [
        (TransmitError::LinkDown, PortRefusal::LinkDown),
        (TransmitError::QueueFull, PortRefusal::NoRoom),
        (
            TransmitError::InvalidLength(FrameLengthError::TooLong),
            PortRefusal::BadFrame,
        ),
    ] {
        let mut port = port();
        port.transmit.refuse = Some(reason);
        assert_eq!(port.publish(4, octets), Err(Unpublished::Port(expected)));
        assert_eq!(port.buffers.free, 2);
    }
}

#[test]
fn a_frame_too_long_for_its_buffer_keeps_the_buffer() {
    let mut port = port();
    let refused = port.publish(100, unreachable_fill("no buffer holds it"));
    assert_eq!(refused, Err(Unpublished::Port(PortRefusal::BadFrame)));
    assert_eq!(port.buffers.free, 2);
}

#[test]
fn a_failed_fill_comes_back_as_the_fill_s_and_keeps_the_buffer() {
    let mut port = port();
    let refused = port.publish(4, |_| Err(Declined));
    assert_eq!(refused, Err(Unpublished::Fill(Declined)));
    assert_eq!(port.buffers.free, 2);
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
    assert!(port.publish(4, octets).is_err());
    assert!(!ready_at_once(&mut port), "still no room");
    assert_eq!(port.transmit.room_polls, 1, "it asked");
    port.transmit.full = false;
    assert!(ready_at_once(&mut port));
    port.publish(4, octets).unwrap();
    assert!(!ready_at_once(&mut port), "that frame went");
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
fn a_down_link_with_a_free_queue_wakes_nobody_until_it_changes() {
    let mut port = port();
    port.transmit.refuse = Some(TransmitError::LinkDown);
    assert_eq!(
        port.publish(4, octets),
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
fn no_buffer_with_a_free_queue_wakes_nobody_until_one_is_free() {
    let mut port = port();
    port.buffers.free = 0;
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    assert!(!ready_at_once(&mut port), "room is not what it waits for");
    assert_eq!(port.buffers.free_polls, 1, "it asked for a buffer");
    port.buffers.free = 1;
    assert!(ready_at_once(&mut port));
}

#[test]
fn a_full_queue_is_watched_after_the_device_refused() {
    let mut port = port();
    port.transmit.refuse = Some(TransmitError::QueueFull);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    assert!(ready_at_once(&mut port));
    assert_eq!(port.transmit.room_polls, 1);
}

#[test]
fn a_write_refused_for_room_watches_for_a_buffer() {
    let mut port = port();
    port.buffers.refuse = Some(PortRefusal::NoRoom);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    port.buffers.free = 0;
    assert!(!ready_at_once(&mut port));
    assert_eq!(port.buffers.free_polls, 1);
}

#[test]
fn a_write_refused_for_the_link_waits_for_the_link() {
    let mut port = port();
    port.buffers.refuse = Some(PortRefusal::LinkDown);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::LinkDown))
    );
    assert!(!ready_at_once(&mut port));
    assert_eq!(
        (port.transmit.room_polls, port.buffers.free_polls),
        (1, 0),
        "asked for returned memory only"
    );
}

#[test]
fn only_a_bad_frame_is_final() {
    assert!(Port::is_final(&PortRefusal::BadFrame));
    assert!(!Port::is_final(&PortRefusal::NoRoom));
    assert!(!Port::is_final(&PortRefusal::LinkDown));
}

#[test]
fn a_bad_frame_waits_for_nothing() {
    let mut port = port();
    port.transmit.refuse = Some(TransmitError::InvalidLength(FrameLengthError::TooLong));
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::BadFrame))
    );
    assert!(!ready_at_once(&mut port));
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
    port.serve_returns(&mut Context::from_waker(&waker));
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
    port.device.incoming.push_back(Incoming::Copied);
    port.transmit.returned = 2;
    port.transmit.full = true;
    assert!(port.publish(4, octets).is_err(), "a frame waits for room");
    port.transmit.full = false;
    port.give_back();
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

/// Publish a frame and count its ticket, as the buffers hand them out.
fn sent(port: &mut Port) -> u32 {
    let ticket = port.publish(4, octets).unwrap();
    port.buffers.tickets += 1;
    ticket
}

#[test]
fn each_frame_the_device_takes_has_its_own_ticket() {
    let mut port = port();
    let first = sent(&mut port);
    let second = sent(&mut port);
    assert_ne!(first, second);
    assert_eq!(port.buffers.free, 0);
}

/// A returned buffer and a settled end are two events: the buffer is free
/// again only once its end went on, so no more frames than buffers are
/// ever unsettled.
#[test]
fn a_buffer_frees_only_when_its_end_is_handed_on() {
    let mut port = port();
    let first = sent(&mut port);
    let second = sent(&mut port);
    port.buffers.ended.extend([first, second]);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom)),
        "two buffers, two unsettled ends: no third frame"
    );
    let takes = port.transmit.takes;
    assert_eq!(port.take_notice(), Some(first));
    assert_eq!(
        port.transmit.takes, takes,
        "an end already held goes without asking the device"
    );
    assert_eq!(port.buffers.free, 1);
    let third = sent(&mut port);
    assert_ne!(third, first);
    assert_eq!(port.take_notice(), Some(second));
    let takes = port.transmit.takes;
    assert_eq!(port.take_notice(), None);
    assert!(
        port.transmit.takes > takes,
        "with no end held, the device is asked for what it returned"
    );
}

#[test]
fn a_waiting_end_wakes_the_service() {
    let mut port = port();
    assert!(!ready_at_once(&mut port));
    let first = sent(&mut port);
    port.buffers.ended.push_back(first);
    assert!(ready_at_once(&mut port));
    assert_eq!(port.take_notice(), Some(first));
    assert!(!ready_at_once(&mut port));
}

/// After its service ended, the port's give-back frees the buffers whose
/// ends no policy will settle.
#[test]
fn a_give_back_frees_the_buffers_of_ends_no_policy_settles() {
    let mut port = port();
    let first = sent(&mut port);
    let second = sent(&mut port);
    port.buffers.ended.extend([first, second]);
    assert_eq!(port.buffers.free, 0);
    port.give_back();
    assert_eq!(port.buffers.free, 2);
    assert_eq!(port.take_notice(), None);
}
