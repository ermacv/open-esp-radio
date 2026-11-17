//! Receive as a round (`design/port-contract-2026-11-16.md`, section 4):
//! everything the engine does with received units it does inside one
//! borrowed value, which services the ring once and returns, at its close,
//! every unit not taken.
//!
//! A unit stays in the port's staging place from the ring's service to its
//! return; it is read in place and named by its handle. The round is the
//! contract's: a core reaches the port's primitives only through it (they
//! take a seal this crate alone makes), and it borrows the port for its
//! life, so the bounds of section 5 hold by its code: two port calls of a
//! round's own (its opening and its close), the ring serviced once.

use core::marker::PhantomData;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_wire::frame::{
    DecodedHeader, FrameControl, FrameControlFields, FrameType,
};

use crate::data::{DataPort, Sealed};
use crate::rx::RxMetadata;

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The most staging places a port has: a round's sets are words.
pub const PLACES: usize = 64;

/// A received MPDU's header, decoded and checked once, when the port
/// staged the unit: its fields, without the bytes, which stay in the place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxHeader {
    /// The Frame Control field.
    pub control: FrameControl,
    /// Address 1.
    pub receiver: [u8; 6],
    /// Address 2.
    pub transmitter: [u8; 6],
    /// Address 3.
    pub address3: [u8; 6],
    /// The Sequence Control field.
    pub sequence_control: u16,
    /// The QoS Control field, when the frame has one.
    pub qos: Option<u16>,
    /// The offset of the body.
    pub body: u16,
}

impl RxHeader {
    /// The fields of `decoded`, a header of `frame`.
    pub fn of(decoded: &DecodedHeader<'_>, frame: &[u8]) -> Option<Self> {
        let body = frame.len().checked_sub(decoded.body().len())?;
        Some(Self {
            control: decoded.control(),
            receiver: decoded.receiver(),
            transmitter: decoded.transmitter(),
            address3: decoded.address3(),
            sequence_control: decoded.sequence_control(),
            qos: decoded.qos(),
            body: u16::try_from(body).ok()?,
        })
    }
}

/// What a staged unit is, classified once at its staging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxHead {
    /// A data or a management frame, with its header decoded.
    Frame(RxHeader),
    /// A control frame, with its Frame Control field: its reader parses
    /// its body (a BlockAckReq).
    Control(FrameControlFields),
}

impl RxHead {
    /// The class of `frame`: `None` for a frame that is neither, which the
    /// port counts and gives back to the ring at its staging, as a frame
    /// the air damaged.
    pub fn of(frame: &[u8]) -> Option<Self> {
        if let Some(header) =
            DecodedHeader::decode(frame).and_then(|decoded| RxHeader::of(&decoded, frame))
        {
            return Some(Self::Frame(header));
        }
        let (&control, _) = frame.split_first_chunk::<2>()?;
        let control = FrameControlFields::from_bytes(control);
        (control.version() == 0 && control.kind() == FrameType::Control)
            .then_some(Self::Control(control))
    }

    /// The decoded header of a data or a management frame.
    pub const fn header(&self) -> Option<&RxHeader> {
        match self {
            Self::Frame(header) => Some(header),
            Self::Control(_) => None,
        }
    }
}

/// A unit can be lent: a property of the unit, read before the decision.
/// The port makes it at the unit's staging, under the round's seal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lendable(());

impl Lendable {
    /// The unit the port stages now can be lent.
    pub const fn staged(_seal: &Sealed) -> Self {
        Self(())
    }
}

/// A staged unit, as the port holds it in its place.
#[derive(Debug)]
pub struct Staged<M> {
    /// What it is, classified once.
    pub head: RxHead,
    /// What the hardware observed of it.
    pub meta: M,
    /// Whether it can be lent: [`Lendable`] when it can.
    pub lendable: Option<Lendable>,
}

/// A unit read in place: its header, the hardware's evidence, the bytes.
#[derive(Debug)]
pub struct Received<'a, M> {
    /// What it is, classified once.
    pub head: &'a RxHead,
    /// What the hardware observed.
    pub meta: &'a M,
    /// The MPDU, from its header to the end of its body.
    pub bytes: &'a [u8],
    lendable: Option<Lendable>,
}

impl<M> Received<'_, M> {
    /// Whether the unit can be lent.
    pub const fn lendable(&self) -> Option<Lendable> {
        self.lendable
    }
}

/// The Ethernet addresses written in a taken unit's place, before an
/// EtherType its LLC/SNAP header already holds: the place's bytes become an
/// Ethernet frame from `at`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetAddresses {
    /// The Destination Address.
    pub destination: [u8; 6],
    /// The Source Address.
    pub source: [u8; 6],
    /// The offset of the EtherType in the unit: the addresses take the
    /// twelve octets before it.
    pub ether_type: u16,
}

/// The addresses fall outside the unit's bytes: nothing was written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutsideTheUnit;

/// The name of a unit the engine took: it stays in its place.
#[must_use = "a taken unit is released or lent"]
#[derive(Debug)]
pub struct UnitHandle<'id> {
    place: u8,
    brand: Brand<'id>,
}

impl UnitHandle<'_> {
    /// The unit's place.
    pub const fn place(&self) -> u8 {
        self.place
    }
}

/// What comes after a round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rest {
    /// Nothing: the receive signal wakes.
    Signal,
    /// Another round: the ring has more and a place is free.
    Round,
    /// The instant of the ring's bounded continuation.
    At(Instant),
}

/// The port's primitives of a round, each taking the seal.
pub trait ReceivePort<'id>: DataPort<'id> {
    /// What the hardware observed of a unit.
    type Meta: RxMetadata;
    /// A unit lent to the network, in place.
    type Lent;
    /// The port's staging places: every unit out of the ring is in one, at
    /// most [`PLACES`].
    const STAGING: usize;

    /// Open a round: take what the network gave back; its count.
    fn round_open(&mut self, seal: &Sealed, now: Instant) -> usize;
    /// Service the ring once: the places it staged into, in ring order.
    fn round_service(&mut self, seal: &Sealed) -> StagedPlaces;
    /// The unit staged at `place`.
    fn round_unit(&self, seal: &Sealed, place: u8) -> Option<(&Staged<Self::Meta>, &[u8])>;
    /// Lend the unit at `place`, in place.
    fn round_lend(&mut self, seal: &Sealed, place: u8) -> Self::Lent;
    /// Write `addresses` into the unit at `place`, a unit taken and not
    /// lent. The place is the port's, not the hardware's, from the unit's
    /// staging to its return: the write reaches no DMA in flight.
    ///
    /// # Errors
    ///
    /// The twelve octets fall outside the unit's bytes; nothing was
    /// written.
    fn round_write_addresses(
        &mut self,
        seal: &Sealed,
        place: u8,
        addresses: EthernetAddresses,
    ) -> Result<(), OutsideTheUnit>;
    /// Close the round: the units of `back` go back to the ring.
    fn round_close(&mut self, seal: &Sealed, back: u64) -> Rest;
}

/// The places one service of the ring staged into, in ring order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StagedPlaces {
    order: [u8; PLACES],
    len: u8,
}

impl StagedPlaces {
    /// No place.
    pub const EMPTY: Self = Self {
        order: [0; PLACES],
        len: 0,
    };

    /// One more place, in ring order; `false` when the places are full.
    pub fn push(&mut self, place: u8) -> bool {
        match self.order.get_mut(usize::from(self.len)) {
            Some(slot) if usize::from(place) < PLACES => {
                *slot = place;
                self.len = self.len.saturating_add(1);
                true
            }
            Some(_) | None => false,
        }
    }

    /// The places.
    pub fn places(&self) -> &[u8] {
        self.order.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

/// One round over the port's receive.
#[must_use = "a round is closed: its close returns every unit not taken"]
pub struct Round<'p, 'id, P: ReceivePort<'id>> {
    port: &'p mut P,
    seal: Sealed,
    reclaimed: usize,
    staged: StagedPlaces,
    next: u8,
    /// The staged units handed out by `next`.
    handed: u64,
    /// The units the engine took and has not lent or released.
    taken: u64,
    /// The units to go back at the close.
    back: u64,
    brand: Brand<'id>,
}

impl<'p, 'id, P: ReceivePort<'id>> Round<'p, 'id, P> {
    /// Open a round over `port`: the per-round work, once.
    pub fn open(port: &'p mut P, now: Instant) -> Self {
        let seal = Sealed::new();
        let reclaimed = port.round_open(&seal, now);
        Self {
            port,
            seal,
            reclaimed,
            staged: StagedPlaces::EMPTY,
            next: 0,
            handed: 0,
            taken: 0,
            back: 0,
            brand: PhantomData,
        }
    }

    /// The units the network gave back, taken at the opening.
    pub const fn reclaimed(&self) -> usize {
        self.reclaimed
    }

    /// The next staged unit, in ring order: the ring is serviced once a
    /// round, at the first call.
    pub fn staged(&mut self) -> Option<Unit<'_, 'p, 'id, P>> {
        if self.handed == 0 && self.next == 0 {
            self.staged = self.port.round_service(&self.seal);
        }
        let place = *self.staged.places().get(usize::from(self.next))?;
        self.next = self.next.saturating_add(1);
        let bit = 1_u64 << place;
        self.handed |= bit;
        // A unit not taken goes back at the close.
        self.back |= bit;
        Some(Unit { round: self, place })
    }

    /// A taken unit, read in place.
    ///
    /// `None` does not happen: the port's places are a table read by a
    /// handle's index, a handle is made only for a place the port staged,
    /// and the place is given up only at the handle's release or loan. The
    /// `Option` is the table's read, which safe Rust gives a failure path.
    pub fn unit(&self, handle: &UnitHandle<'id>) -> Option<Received<'_, P::Meta>> {
        let (staged, bytes) = self.port.round_unit(&self.seal, handle.place)?;
        Some(Received {
            head: &staged.head,
            meta: &staged.meta,
            bytes,
            lendable: staged.lendable,
        })
    }

    /// Write the Ethernet addresses of a decapsulated MSDU into a taken
    /// unit's place, once, before it is lent or copied.
    ///
    /// # Errors
    ///
    /// The addresses fall outside the unit; nothing was written.
    pub fn write_addresses(
        &mut self,
        handle: &UnitHandle<'id>,
        addresses: EthernetAddresses,
    ) -> Result<(), OutsideTheUnit> {
        self.port
            .round_write_addresses(&self.seal, handle.place, addresses)
    }

    /// Lend a taken unit to the network: it keeps its place.
    pub fn lend(&mut self, handle: UnitHandle<'id>, lendable: Lendable) -> P::Lent {
        let Lendable(()) = lendable;
        let UnitHandle { place, brand: _ } = handle;
        self.taken &= !(1 << place);
        self.port.round_lend(&self.seal, place)
    }

    /// Release a taken unit: it goes back at the close.
    pub fn release(&mut self, handle: UnitHandle<'id>) {
        let UnitHandle { place, brand: _ } = handle;
        let bit = 1_u64 << place;
        self.taken &= !bit;
        self.back |= bit;
    }

    /// Close the round: one return of everything not taken or released.
    pub fn close(self) -> Rest {
        let Self {
            port, seal, back, ..
        } = self;
        port.round_close(&seal, back)
    }
}

/// A staged unit of a round, before the engine takes it.
pub struct Unit<'r, 'p, 'id, P: ReceivePort<'id>> {
    round: &'r mut Round<'p, 'id, P>,
    place: u8,
}

impl<'id, P: ReceivePort<'id>> Unit<'_, '_, 'id, P> {
    /// The unit read in place; `None` does not happen, for the reason
    /// [`Round::unit`] gives.
    pub fn view(&self) -> Option<Received<'_, P::Meta>> {
        let (staged, bytes) = self.round.port.round_unit(&self.round.seal, self.place)?;
        Some(Received {
            head: &staged.head,
            meta: &staged.meta,
            bytes,
            lendable: staged.lendable,
        })
    }

    /// Take the unit: it stays in its place, and the engine names it.
    pub fn take(self) -> UnitHandle<'id> {
        let bit = 1_u64 << self.place;
        self.round.back &= !bit;
        self.round.taken |= bit;
        UnitHandle {
            place: self.place,
            brand: PhantomData,
        }
    }
}
