//! The contract of a port frames are published to and taken from in place, and
//! of the egress policy that chooses what goes to it.

use core::task::Context;

use oer_protocols_network_wire::ip::Dscp;

use crate::device::EthernetFrameView;

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
    /// `fill` that returned `Ok`.
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
    ///         _: impl FnOnce(&mut [u8]) -> Result<(), E>,
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
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
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

    /// Show the next received frame to `read`; whether there was one.
    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool;

    /// The next notice.
    fn take_notice(&mut self) -> Option<Self::Notice>;

    /// Make `cx` the one a received frame, a notice or room to publish wakes,
    /// at once if any came since the last look.
    fn register(&mut self, cx: &mut Context<'_>);
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
/// it chose. Reserve in `choose`; exactly one of `accepted`, `refused` or
/// `unknown` answers each reservation. `Ticket` and `Notice` are the port's.
pub trait Egress<Ticket, Notice> {
    /// What a choice holds until it is answered.
    type Reservation;
    /// Why the policy cannot go on.
    type Fault;

    /// Choose among `ready`, which holds every piece of ready work; `None`
    /// until a port's wake if nothing may go now.
    ///
    /// # Errors
    /// [`Self::Fault`], which ends the service.
    fn choose<C: Candidate>(
        &mut self,
        ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<(C, Self::Reservation)>, Self::Fault>;

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
