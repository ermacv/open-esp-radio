//! The contract of a port frames are published to and taken from in place, and
//! of the egress policy that chooses what goes to it.

use core::task::Context;

use oer_protocols_network_wire::ip::Dscp;

use crate::device::{EthernetFrame, EthernetFrameView, LinkSnapshot};
use crate::slot::Slot;

/// Where a frame goes out, and the one answer to it. Whoever publishes keeps
/// its own record of what `fill` did, and a port whose answer contradicts that
/// record is at fault: `Ok` after a failed or missing fill, or `Port` after a
/// failed one.
pub trait FramePort {
    /// Why the port took nothing, which names what it waits for.
    type Refusal;
    /// Names one accepted frame, for its later completion.
    type Ticket;

    /// Take room for `length` octets, let `fill` write the frame there and
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
    ///         _: usize,
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
        length: usize,
        fill: impl FnOnce(&mut Slot<'_>) -> Result<(), E>,
    ) -> Result<Self::Ticket, Unpublished<Self::Refusal, E>>;

    /// Whether `refusal` is final for that frame: no wake will change it, so
    /// the work ends instead of waiting.
    fn is_final(refusal: &Self::Refusal) -> bool {
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
    /// [`NeverHeld`] for a port that lends none.
    type Held: EthernetFrame;

    /// Show the next received frame to `read`, with how the port holds it.
    /// A frame in a buffer the port may lend ([`Holding::Lendable`]) is
    /// handed over when `read` answers [`Keep::Keep`]; any other frame stays
    /// the port's whatever the answer.
    fn take_frame(
        &mut self,
        read: impl FnOnce(EthernetFrameView<'_>, Holding) -> Keep,
    ) -> Taken<Self::Held>;

    /// The next notice.
    fn take_notice(&mut self) -> Option<Self::Notice>;

    /// The link now: its state and its epoch, which a new association moves.
    /// A change of it wakes what [`Self::register`] registered.
    fn link(&self) -> LinkSnapshot;

    /// Make `cx` the one the next received frame, notice, change of link or
    /// room for a refused frame wakes, and return whether one is here
    /// already: a frame or a notice not yet taken, a link changed since the
    /// last registration, or room for the frame the port last refused. This
    /// is the caller's readiness, checked after registering, so nothing that
    /// comes between the two is missed. A `true` is not also a wake: `cx` is
    /// woken for what comes after this call.
    fn register(&mut self, cx: &mut Context<'_>) -> bool;
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

/// What a reader of a received frame does with its buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Keep {
    /// Done with it: the buffer stays the port's.
    Drop,
    /// Keep a lendable frame's buffer.
    Keep,
}

/// What a look for a received frame found.
#[derive(Debug)]
pub enum Taken<H> {
    /// No frame waited.
    Empty,
    /// A frame was shown, and its buffer stayed the port's.
    Read,
    /// A frame was shown and its buffer kept: the reader's now.
    Kept(H),
}

impl<H> Taken<H> {
    /// Whether a frame was there.
    pub const fn found(&self) -> bool {
        !matches!(self, Self::Empty)
    }
}

/// What kind of work a frame is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// The network's own, which keeps it working.
    Control,
    /// An application's.
    Data,
}

/// What a policy may know of work that is ready, before any frame of it exists.
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

/// A piece of ready work a policy can choose whole.
pub trait Candidate {
    fn ready(&self) -> &Ready;
}

/// Chooses what goes out before any frame exists, and keeps the account of what
/// it chose. `Ticket` and `Notice` are the port's.
///
/// A choice is made in two steps. [`Self::plan`] collects the ready work once
/// and reserves nothing: it keeps the continuation of its choice, at most
/// [`QUANTUM`](crate::QUANTUM) frames, and what the choice read. Each
/// [`Self::next`] then chooses one frame from the plan and reserves for that
/// frame alone, exactly what a choice over the same ready work would choose
/// and reserve; or it says nothing may go, or that the plan no longer knows
/// ([`Next::Stop`](crate::Next::Stop)), reserving nothing. Exactly one of
/// [`Self::accepted`], [`Self::refused`] or [`Self::unknown`] answers each
/// reservation. A plan goes on only after its last frame was accepted, and only
/// while nothing but its own frames' answers reached the policy or the work
/// since it was collected: its caller drops it at any other answer, at a
/// notice, and at the end of its publishing. A plan holds no owner, so
/// dropping it returns nothing and costs nothing.
pub trait Egress<Ticket, Notice> {
    /// What a choice holds until it is answered.
    type Reservation;
    /// Why the policy cannot go on.
    type Fault;
    /// What one collection of the ready work keeps for the frames after it.
    type Plan<C: Candidate>;

    /// Collect a plan into `plan`, in place, from the ready work: each call
    /// of `ready` lists every piece of it, and a collection that needs a
    /// second look calls it again. What `plan` held before is gone; it
    /// reserves nothing. The caller keeps `plan`'s storage from one
    /// collection to the next, so a collection moves no plan. Returns the
    /// plan collected, in `plan`.
    ///
    /// # Errors
    /// [`Self::Fault`], which ends the service.
    fn plan<'p, C: Candidate, I: Iterator<Item = C>>(
        &mut self,
        ready: impl Fn() -> I,
        plan: &'p mut Option<Self::Plan<C>>,
    ) -> Result<&'p mut Self::Plan<C>, Self::Fault>;

    /// The plan's next frame, reserved; [`Next::Idle`](crate::Next::Idle)
    /// until a port's wake or a notice if nothing may go now; or
    /// [`Next::Stop`](crate::Next::Stop), with nothing reserved, once the plan
    /// cannot say what a choice over the ready work would.
    ///
    /// # Errors
    /// [`Self::Fault`], which ends the service.
    fn next<C: Candidate>(
        &mut self,
        plan: &mut Self::Plan<C>,
    ) -> Result<crate::Next<C, Self::Reservation>, Self::Fault>;

    /// No work is ready, and its caller knows it with no collection: the
    /// policy makes the same transition as [`Self::plan`] over no work and
    /// that plan's first [`Self::next`], which then says
    /// [`Next::Idle`](crate::Next::Idle), with no plan built and nothing
    /// reserved. A policy whose look at no work changes nothing does nothing.
    ///
    /// # Errors
    /// What that step over no work would return: [`Self::Fault`], which ends
    /// the service.
    fn nothing_ready(&mut self) -> Result<(), Self::Fault>;

    /// The port took the frame: its charge waits for a notice.
    ///
    /// # Errors
    /// [`Self::Fault`].
    fn accepted(
        &mut self,
        reservation: Self::Reservation,
        ticket: Ticket,
    ) -> Result<(), Self::Fault>;

    /// Nothing was published: the reservation comes back whole.
    ///
    /// # Errors
    /// [`Self::Fault`].
    fn refused(&mut self, reservation: Self::Reservation) -> Result<(), Self::Fault>;

    /// Whether the frame went out is not known, and no completion follows. Its
    /// cost is not zero for that.
    ///
    /// # Errors
    /// [`Self::Fault`].
    fn unknown(&mut self, reservation: Self::Reservation) -> Result<(), Self::Fault>;

    /// The port has news.
    ///
    /// # Errors
    /// [`Self::Fault`].
    fn notice(&mut self, notice: Notice) -> Result<(), Self::Fault>;
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
