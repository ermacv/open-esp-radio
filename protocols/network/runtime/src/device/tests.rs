extern crate std;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::vec::Vec;

use core::task::{Context, Poll, Waker};

use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, FramePort, FrameSource, FrameStorage,
    LentFrame, LinkSnapshot, LinkState, NetDevice, NetTransmit, ReceivedFrame,
    StorageAdvanceRefused, TransmitError, TransmitRefused, Unpublished,
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
    fn has_room(&mut self) -> bool {
        self.take_back();
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
}

impl TxBuffers for Buffers {
    type Frame = Written;
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

/// Whether registering wakes at once.
fn wakes_at_once(port: &mut Port) -> bool {
    let woken = std::sync::Arc::new(Woken::default());
    let waker = Waker::from(woken.clone());
    port.register(&mut Context::from_waker(&waker));
    woken.0.load(std::sync::atomic::Ordering::SeqCst)
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
    assert!(port.take_frame(|frame| seen.push(frame.payload.to_vec())));
    assert!(port.take_frame(|frame| seen.push(frame.payload.to_vec())));
    assert!(!port.take_frame(|_| panic!("nothing waits")));
    assert_eq!(seen, [b"lent".to_vec(), b"copied".to_vec()]);
}

#[test]
fn a_frame_the_device_cannot_show_is_counted() {
    let mut port = port();
    port.device.bad_copy = true;
    port.device.incoming.push_back(Incoming::Copied);
    assert!(port.take_frame(|_| panic!("unreadable")));
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
        !wakes_at_once(&mut port),
        "it waits for nothing of the port"
    );
}

#[test]
fn an_idle_port_with_room_wakes_nobody() {
    assert!(!wakes_at_once(&mut port()));
}

/// The memory the device returned comes back at each registration, with no
/// frame to send, and wakes nobody by itself.
#[test]
fn an_idle_port_takes_back_the_memory_the_device_returned() {
    let mut port = port();
    port.transmit.returned = 3;
    assert!(!wakes_at_once(&mut port));
    assert_eq!(port.transmit.taken_back, 3);
    assert!(port.transmit.sent.is_empty());
}

#[test]
fn room_wakes_only_a_frame_that_met_none() {
    let mut port = port();
    port.transmit.full = true;
    assert!(port.publish(4, octets).is_err());
    assert!(!wakes_at_once(&mut port), "still no room");
    assert_eq!(port.transmit.room_polls, 1, "it asked");
    port.transmit.full = false;
    assert!(wakes_at_once(&mut port));
    port.publish(4, octets).unwrap();
    assert!(!wakes_at_once(&mut port), "that frame went");
}

#[test]
fn a_received_frame_and_a_link_change_wake_at_once() {
    let mut port = port();
    port.device.incoming.push_back(lent(b"in"));
    assert!(wakes_at_once(&mut port));
    port.device.incoming.clear();
    port.device.link_changed = true;
    assert!(wakes_at_once(&mut port));
    assert!(!wakes_at_once(&mut port));
}

#[test]
fn a_down_link_with_a_free_queue_wakes_nobody_until_it_changes() {
    let mut port = port();
    port.transmit.refuse = Some(TransmitError::LinkDown);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::LinkDown))
    );
    assert!(!wakes_at_once(&mut port), "room is not what it waits for");
    assert_eq!(
        port.transmit.room_polls, 1,
        "asked for returned memory only"
    );
    port.device.link_changed = true;
    assert!(wakes_at_once(&mut port));
}

#[test]
fn no_buffer_with_a_free_queue_wakes_nobody_until_one_is_free() {
    let mut port = port();
    port.buffers.free = 0;
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    assert!(!wakes_at_once(&mut port), "room is not what it waits for");
    assert_eq!(port.buffers.free_polls, 1, "it asked for a buffer");
    port.buffers.free = 1;
    assert!(wakes_at_once(&mut port));
}

#[test]
fn a_full_queue_is_watched_after_the_device_refused() {
    let mut port = port();
    port.transmit.refuse = Some(TransmitError::QueueFull);
    assert_eq!(
        port.publish(4, octets),
        Err(Unpublished::Port(PortRefusal::NoRoom))
    );
    assert!(wakes_at_once(&mut port));
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
    assert!(!wakes_at_once(&mut port));
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
    assert!(!wakes_at_once(&mut port));
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
    assert!(!wakes_at_once(&mut port));
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
    port.give_back();
    assert_eq!(returned.load(Ordering::SeqCst), 3);
    assert_eq!(port.device.incoming.len(), 0);
    assert_eq!(
        port.transmit.taken_back, 2,
        "and takes back the returned memory"
    );
}
