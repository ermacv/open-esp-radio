//! The contract of a port frames are published to and taken from in place, and
//! of the egress policy that chooses what goes to it.

use core::task::{Context, Poll};

use oer_protocols_network_wire::ip::Dscp;

use crate::device::{EthernetFrame, EthernetFrameView, Incoming, LinkSnapshot, ReceivedBatch};
use crate::slot::Slot;

/// Why a port took no frame, which names what it waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortRefusal {
    /// No free buffer, or a full queue: the work stays ready, and the buffer
    /// or the room wakes the service.
    NoRoom,
    /// The link is down: the work stays ready, and a change of link wakes the
    /// service.
    LinkDown,
    /// The frame does not fit a buffer, or the device reads it as malformed:
    /// the same frame would meet the same, so this is final for it.
    BadFrame,
    /// The port's memory refused the buffer the frame was written in, after
    /// the fill: the frame never reached the device, and the buffer of
    /// `place` left the port's reach for good. A fault of the port, not of
    /// the frame ([`FramePort::is_fault`]).
    BufferLost { place: u8 },
}

/// The transmit half of one affine Ethernet port that owns the memory its
/// frames are written in: its free buffers, the queue a frame goes to the
/// device by, and the ends the device gives back, as one owner on the
/// network's executor. A frame is written where the device sends it from.
pub trait OwnedTransmit {
    /// Names one frame the device took.
    type Ticket;
    /// The end of a frame the device took, as the egress policy hears it.
    type Notice;

    /// Write the frame `meta` describes by `fill` in a free buffer and hand
    /// it to the device. The buffer and the room to hand it on are taken
    /// together, once, before any fill; `Ok` says the device took the frame,
    /// or took it and lost it with its buffer, which its notice says.
    ///
    /// # Errors
    /// [`Unpublished`]: a [`PortRefusal`] before any fill, or the error
    /// `fill` returned; the buffer stays free.
    fn publish<E>(
        &mut self,
        meta: FrameMeta,
        fill: impl FnOnce(&mut Slot<'_>) -> Result<(), E>,
    ) -> Result<Self::Ticket, Unpublished<PortRefusal, E>>;

    /// The next end of a frame the device took, what the device returned
    /// taken back first when none waits. A buffer the device gave back is
    /// free again only here, when its end is handed on: so no more frames
    /// are unsettled than there are buffers.
    fn take_notice(&mut self) -> Option<Self::Notice>;

    /// Whether [`Self::take_notice`] has one without taking anything back.
    fn notice_waiting(&self) -> bool;

    /// Take back what the device returned, registering `cx` for its next
    /// return and for room before observing them: ready when a publication
    /// would find a free buffer and room to hand it on.
    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// End a burst of publications: a frame it took may wait for this call
    /// before its device acts on it.
    fn flush(&mut self);

    /// Take back what the device returned, without registering anything.
    fn reclaim(&mut self);
}

/// What a publisher knows of a frame before it is written: its octets, and
/// for an IP frame the DSCP its header carries (RFC 2474, clause 3). A port
/// that prioritises frames by their marking reads it here, not from the
/// written frame; the publisher writes the same DSCP it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameMeta {
    /// The frame's octets, from its Ethernet header to its last.
    pub length: usize,
    /// The DSCP of its IP header; `None` for a frame that is not IP.
    pub dscp: Option<Dscp>,
}

/// Where a frame goes out, and the one answer to it. Whoever publishes keeps
/// its own record of what `fill` did, and a port whose answer contradicts that
/// record is at fault: `Ok` after a failed or missing fill, or `Port` after a
/// failed one.
pub trait FramePort {
    /// Why the port took nothing, which names what it waits for.
    type Refusal;
    /// Names one accepted frame, for its later completion.
    type Ticket;

    /// Take room for the frame `meta` describes, let `fill` write it there and
    /// accept it. `Ok` says the frame is the port's, and only follows a
    /// `fill` that returned `Ok`. The room comes as a [`Slot`] with the run
    /// the port knows its buffer holds, if it keeps one; what run the room
    /// holds after `fill` is the buffer's for its next frame.
    ///
    /// The port is generic in `E`, so the only `E` it can hand back is the one
    /// `fill` returned: a port cannot claim a fill failed when it did not.
    ///
    /// ```compile_fail,E0308
    /// use oer_protocols_network_port::{FramePort, Unpublished};
    /// struct Liar;
    /// struct Short;
    /// impl FramePort for Liar {
    ///     type Refusal = ();
    ///     type Ticket = ();
    ///     fn publish<E>(
    ///         &mut self,
    ///         _: oer_protocols_network_port::FrameMeta,
    ///         _: impl FnOnce(&mut oer_protocols_network_port::Slot<'_>) -> Result<(), E>,
    ///     ) -> Result<(), Unpublished<(), E>> {
    ///         Err(Unpublished::Fill(Short))
    ///     }
    /// }
    /// ```
    ///
    /// # Errors
    /// [`Unpublished`]: the port accepted nothing.
    fn publish<E>(
        &mut self,
        meta: FrameMeta,
        fill: impl FnOnce(&mut Slot<'_>) -> Result<(), E>,
    ) -> Result<Self::Ticket, Unpublished<Self::Refusal, E>>;

    /// Whether `refusal` is final for that frame: no wake will change it, so
    /// the work ends instead of waiting.
    fn is_final(refusal: &Self::Refusal) -> bool {
        let _ = refusal;
        false
    }

    /// Whether `refusal` is a fault of the port itself: the frame did not go,
    /// and the port can no longer be trusted to publish. Its work ends, and
    /// nothing more is published until the owner retires the core and takes
    /// the port back.
    fn is_fault(refusal: &Self::Refusal) -> bool {
        let _ = refusal;
        false
    }

    /// End a burst of publications: a frame may wait for this before the port
    /// acts on it.
    fn flush(&mut self) {}
}

/// Frames coming in, news for the egress policy, and the wakes of a port.
pub trait FrameSource {
    /// What the port tells the policy: how an accepted frame ended, and
    /// whatever else its account needs. The network passes it on unread.
    type Notice;

    /// A received frame's buffer the port lets its reader keep:
    /// [`NeverHeld`] for a port that lends none. Dropping it gives the
    /// buffer back to the port.
    type Held: EthernetFrame;

    /// Hand the oldest received frames to `take`, at most `limit`, in
    /// order, as [`NetDevice::receive`](crate::NetDevice::receive) does: a
    /// frame in a buffer the port may lend comes as [`Incoming::Lent`],
    /// the reader's to keep or drop; any other is shown and stays the
    /// port's. The port's places of the frames handed are freed once, after
    /// the last; a broken queue ends the batch after the frames before it.
    fn take_frames(
        &mut self,
        limit: usize,
        take: impl FnMut(Incoming<'_, Self::Held>),
    ) -> ReceivedBatch;

    /// The next notice.
    fn take_notice(&mut self) -> Option<Self::Notice>;

    /// The link now: its state and its epoch, which a new association moves.
    /// A change of it wakes what [`Self::register`] registered.
    fn link(&self) -> LinkSnapshot;

    /// Make `cx` the one the next received frame, notice, change of link or
    /// room for a refused frame wakes, and return what is here already: a
    /// frame not yet taken ([`PortReady::received`]), or a notice not yet
    /// taken, a link changed since the last registration, or room for the
    /// frame the port last refused ([`PortReady::other`]). This is the
    /// caller's readiness, checked after registering, so nothing that comes
    /// between the two is missed. Readiness is not also a wake: `cx` is woken
    /// for what comes after this call.
    fn register(&mut self, cx: &mut Context<'_>) -> PortReady;
}

/// What a port had for its reader when it registered
/// ([`FrameSource::register`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortReady {
    /// A received frame waits.
    pub received: bool,
    /// Anything else: a notice, a changed link, or room for a refused frame.
    pub other: bool,
}

impl PortReady {
    /// Whether anything waits.
    pub const fn any(self) -> bool {
        self.received || self.other
    }
}

/// The buffer of a port that lends none: it cannot be.
#[derive(Debug)]
pub enum NeverHeld {}

impl EthernetFrame for NeverHeld {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, crate::FrameLengthError> {
        match *self {}
    }
}

/// How a port holds a received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Holding {
    /// In a buffer the port may lend to its reader.
    Lendable,
    /// In the device's own copy, for this reason.
    Copied(crate::CopyReason),
}

/// What kind of work a frame is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// The network's own, which keeps it working.
    Control,
    /// An application's.
    Data,
}

/// What is known of work that is ready, before any frame of it exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ready {
    pub class: Class,
    /// The frame's length: what a slot must hold.
    pub length: usize,
    /// The next hop's station address, whose budget this work spends.
    pub peer: [u8; 6],
    /// The socket that wants it; control work has none.
    pub flow: Option<u8>,
    /// Its priority.
    pub dscp: Dscp,
}

/// Why a port published nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unpublished<R, E> {
    /// The port's own refusal, before `fill` or after it wrote the frame; the
    /// port took its room back.
    Port(R),
    /// `fill` failed with this, and the port took its room back.
    Fill(E),
}
