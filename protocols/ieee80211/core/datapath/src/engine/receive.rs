//! The engine's receive half (`design/engine-contract-2026-11-15.md`, D7;
//! `design/engine-model-rx.py`; `design/zero-copy-2026-11-17.md`): every
//! section that touches received units opens one round over the port's
//! receive and closes it.
//!
//! A unit stays in the port's staging place from the ring's service to its
//! return. In a round the engine reads each staged unit in place and routes
//! it: a management frame and EAPOL to the interface's controller, read by
//! reference inside the round ([`Told::frame`]); a data frame of a linked
//! peer to the network; anything else back to the ring at the round's
//! close. A data unit for the network is decapsulated in its place once
//! (its Ethernet addresses written before the EtherType its LLC/SNAP header
//! holds), then lent to the network while it has room to borrow, else
//! copied into the network's storage while it has room for a copy, else it
//! waits, taken, for the network: the pressure episode. A unit waits for the
//! network at most [`NETWORK_LATE`] from the episode's first wait, not
//! extended by returns; after it, a unit the network has no room for is
//! refused, counted. The episode ends once the ring was walked to its end
//! with no unit waiting.

use heapless::Deque;
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    EthernetAddresses, PLACES, ReceivePort, Received, Rest as PortRest, Round, RxCryptoStatus,
    RxEvidence, RxHead, RxHeader, RxMetadata, UnitHandle,
};
use oer_protocols_ieee80211_core_installation::Rest;
use oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN;
use oer_protocols_ieee80211_wire::data::{DataInterfaceRole, decapsulate_data_frames};
use oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL;
use oer_protocols_ieee80211_wire::security::LinkProtection;

use super::{Carrying, Network, Told};

/// How long the network may hold the ring back: the bound of a pressure
/// episode, from its first wait.
///
/// SOURCE: the deleted actor's `NETWORK_LATE` (`b88a3d2cc`,
/// `protocols/ieee80211/runtime/src/datapath.rs:147`), a policy value.
pub const NETWORK_LATE: Duration = Duration::from_millis(10);

/// The most MSDUs one received A-MSDU is delivered as; an aggregate of more
/// is refused, counted ([`RxRefusal::AmsduTooLong`]). A policy value: a
/// received unit is one DMA buffer, and 32 subframes of a TCP
/// acknowledgement (80 octets each with their headers) fill 2 560 octets.
pub const AMSDU_MSDUS: usize = 32;

/// What the network takes of received frames now: units it borrows in
/// place, and frames it copies into its own storage. A value the engine
/// spends: each delivery takes a permit, so a delivery is never refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Room {
    lend: u8,
    copy: u8,
}

impl Room {
    /// Room to borrow `lend` units and to copy `copy` frames.
    pub const fn new(lend: u8, copy: u8) -> Self {
        Self { lend, copy }
    }

    /// A permit to lend one unit, when there is room.
    fn lend(&mut self) -> Option<LendPermit> {
        self.lend = self.lend.checked_sub(1)?;
        Some(LendPermit(()))
    }

    /// Permits to copy `count` frames, when there is room for all.
    fn copies(&mut self, count: usize) -> Option<CopyPermits> {
        let count = u8::try_from(count).ok()?;
        self.copy = self.copy.checked_sub(count)?;
        Some(CopyPermits(count))
    }
}

/// A permit to lend one unit: the network's room, spent.
#[derive(Debug)]
pub struct LendPermit(());

/// A permit to copy one frame: the network's room, spent.
#[derive(Debug)]
pub struct CopyPermit(());

/// Permits to copy the frames of one unit.
struct CopyPermits(u8);

impl CopyPermits {
    fn take(&mut self) -> Option<CopyPermit> {
        self.0 = self.0.checked_sub(1)?;
        Some(CopyPermit(()))
    }
}

/// An Ethernet frame inside a lent unit: from `offset`, `length` octets
/// (its destination, source, EtherType and payload).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetSpan {
    pub offset: u16,
    pub length: u16,
}

/// Why a received unit was not delivered: the counts a controller reads
/// ([`super::FlowingData::refusals`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxRefusal {
    /// A data frame of a link the controller holds.
    LinkHeld,
    /// The network had no room for it after the episode's bound.
    Pressure,
    /// Protected data without the port's evidence of its integrity, or
    /// unprotected data other than EAPOL on a protected link.
    Unverified,
    /// Data whose MSDU does not decapsulate: no RFC 1042 header, a
    /// fragment, a truncated A-MSDU.
    Undecapsulable,
    /// An A-MSDU of more than [`AMSDU_MSDUS`] MSDUs.
    AmsduTooLong,
    /// The unit's place could not be read or written.
    Place,
}

/// The receive refusals counted by reason, each saturating.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxRefusals {
    pub link_held: u16,
    pub pressure: u16,
    pub unverified: u16,
    pub undecapsulable: u16,
    pub amsdu_too_long: u16,
    pub place: u16,
}

impl RxRefusals {
    /// One more refusal for `reason`.
    pub fn count(&mut self, reason: RxRefusal) {
        let count = match reason {
            RxRefusal::LinkHeld => &mut self.link_held,
            RxRefusal::Pressure => &mut self.pressure,
            RxRefusal::Unverified => &mut self.unverified,
            RxRefusal::Undecapsulable => &mut self.undecapsulable,
            RxRefusal::AmsduTooLong => &mut self.amsdu_too_long,
            RxRefusal::Place => &mut self.place,
        };
        *count = count.saturating_add(1);
    }
}

/// The pressure episode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pressure {
    /// No unit waits for the network.
    Clear,
    /// A unit has waited for the network since the instant.
    Since(Instant),
    /// The episode's bound passed: what the network has no room for is
    /// refused until the ring was walked with no unit waiting.
    Late,
}

/// Who a received frame is from, as the form's links know it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RxPeer {
    /// The peer's index.
    pub(super) peer: u8,
    /// Whether its data is protected.
    pub(super) protection: LinkProtection,
    /// Whether its data goes, or the controller holds the link.
    pub(super) carrying: Carrying,
}

/// Whether the data flows or stops, as a round reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Flow {
    /// Units are routed and delivered.
    Flowing,
    /// Nothing is delivered: what waits and what the ring stages go back.
    Stopping,
}

/// The receive half's state while the data runs.
pub(super) struct Receive<'id> {
    /// Data units taken for the network, oldest first.
    waiting: Deque<UnitHandle<'id>, PLACES>,
    pressure: Pressure,
    refusals: RxRefusals,
    /// The MSDUs of the unit being delivered, found before any is written:
    /// kept in place and filled again for each unit.
    msdus: heapless::Vec<Msdu, AMSDU_MSDUS>,
}

/// What became of a unit offered to the network.
enum Delivery<'id> {
    /// Delivered or refused: it is no longer the engine's.
    Done,
    /// The network has no room: it waits, taken.
    Wait(UnitHandle<'id>),
}

impl<'id> Receive<'id> {
    pub(super) const fn new() -> Self {
        Self {
            waiting: Deque::new(),
            pressure: Pressure::Clear,
            refusals: RxRefusals {
                link_held: 0,
                pressure: 0,
                unverified: 0,
                undecapsulable: 0,
                amsdu_too_long: 0,
                place: 0,
            },
            msdus: heapless::Vec::new(),
        }
    }

    /// The refusals counted so far.
    pub(super) const fn refusals(&self) -> RxRefusals {
        self.refusals
    }

    /// The receive half's state, for the engine's search: the places that
    /// wait, the episode (not its instant: the search jumps to the bound),
    /// the refusals.
    #[cfg(test)]
    pub(super) fn key(&self) -> std::string::String {
        let waiting: std::vec::Vec<u8> = self.waiting.iter().map(UnitHandle::place).collect();
        let pressure = match self.pressure {
            Pressure::Clear => "clear",
            Pressure::Since(_) => "since",
            Pressure::Late => "late",
        };
        std::format!("{waiting:?}|{pressure}|{:?}", self.refusals)
    }

    /// One round over `port`; `peer_of` names a frame's linked peer.
    pub(super) fn round<P, N, C>(
        &mut self,
        port: &mut P,
        network: &mut N,
        told: &mut C,
        now: Instant,
        flow: Flow,
        peer_of: impl Fn(&RxHeader) -> Option<RxPeer>,
    ) -> Rest
    where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
        C: Told<'id>,
    {
        if let Pressure::Since(since) = self.pressure
            && now
                .checked_duration_since(since)
                .is_some_and(|waited| waited >= NETWORK_LATE)
        {
            self.pressure = Pressure::Late;
        }
        let mut round = Round::open(port, now);
        let mut room = network.room();
        // What waits goes first, in its order; a stopping data's goes back.
        while let Some(handle) = self.waiting.pop_front() {
            if flow == Flow::Stopping {
                round.release(handle);
                continue;
            }
            match self.deliver(&mut round, handle, &mut room, network) {
                Delivery::Done => {}
                Delivery::Wait(handle) => {
                    if let Err(handle) = self.waiting.push_front(handle) {
                        // One was just taken from it: this does not happen.
                        round.release(handle);
                        self.refusals.count(RxRefusal::Place);
                    }
                    break;
                }
            }
        }
        let held_back = !self.waiting.is_empty();
        if !held_back {
            while let Some(unit) = round.staged() {
                if flow == Flow::Stopping {
                    // A stopping data takes nothing: it goes back unread.
                    continue;
                }
                let Some(view) = unit.view() else {
                    // `None` does not happen (`Round::unit`): it goes back.
                    self.refusals.count(RxRefusal::Place);
                    continue;
                };
                match route(&view, &peer_of) {
                    Route::Back => {}
                    Route::Controller(peer) => told.frame(peer, &view),
                    Route::Refuse(reason) => self.refusals.count(reason),
                    Route::Network => {
                        let handle = unit.take();
                        if let Delivery::Wait(handle) =
                            self.deliver(&mut round, handle, &mut room, network)
                            && let Err(handle) = self.waiting.push_back(handle)
                        {
                            // A place a unit: the deque has one for each.
                            round.release(handle);
                            self.refusals.count(RxRefusal::Place);
                        }
                    }
                }
            }
        }
        let rest = round.close();
        // The episode ends once the ring holds nothing more and nothing
        // waits (a held-back round leaves its units waiting).
        if matches!(rest, PortRest::Signal) && self.waiting.is_empty() {
            self.pressure = Pressure::Clear;
        } else if !self.waiting.is_empty() && self.pressure == Pressure::Clear {
            self.pressure = Pressure::Since(now);
        }
        let bound = match self.pressure {
            Pressure::Since(since) => since.checked_add(NETWORK_LATE),
            Pressure::Clear | Pressure::Late => None,
        };
        match (rest, held_back || !self.waiting.is_empty()) {
            // Held back: the network's return or the bound wakes.
            (_, true) => Rest::Wake(bound),
            (PortRest::Round, false) => Rest::Round,
            (PortRest::At(at), false) => Rest::Wake(Some(at)),
            (PortRest::Signal, false) => Rest::Wake(None),
        }
    }

    /// Offer the taken unit to the network: lent, copied, waiting, or
    /// refused.
    fn deliver<P, N>(
        &mut self,
        round: &mut Round<'_, 'id, P>,
        handle: UnitHandle<'id>,
        room: &mut Room,
        network: &mut N,
    ) -> Delivery<'id>
    where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
    {
        let Some(view) = round.unit(&handle) else {
            round.release(handle);
            self.refusals.count(RxRefusal::Place);
            return Delivery::Done;
        };
        let lendable = view.lendable();
        if let Err(reason) = msdus(&view, &mut self.msdus) {
            round.release(handle);
            self.refusals.count(reason);
            return Delivery::Done;
        }
        let msdus = &self.msdus;
        let single = msdus.len() == 1;
        // Lend while the network has room to borrow, else copy while it has
        // room to copy, else wait; past the bound, refuse.
        if let (true, Some(lendable)) = (single, lendable)
            && let Some(permit) = room.lend()
        {
            let Some(msdu) = msdus.first().copied() else {
                round.release(handle);
                self.refusals.count(RxRefusal::Place);
                return Delivery::Done;
            };
            if round.write_addresses(&handle, msdu.addresses).is_err() {
                round.release(handle);
                self.refusals.count(RxRefusal::Place);
                return Delivery::Done;
            }
            let span = msdu.span;
            let lent = round.lend(handle, lendable);
            network.lent(permit, lent, span);
            return Delivery::Done;
        }
        if let Some(mut permits) = room.copies(msdus.len()) {
            for msdu in msdus {
                if round.write_addresses(&handle, msdu.addresses).is_err() {
                    self.refusals.count(RxRefusal::Place);
                    continue;
                }
                let start = usize::from(msdu.span.offset);
                let end = start.saturating_add(usize::from(msdu.span.length));
                match (
                    permits.take(),
                    round
                        .unit(&handle)
                        .and_then(|view| view.bytes.get(start..end)),
                ) {
                    (Some(permit), Some(frame)) => network.copied(permit, frame),
                    _ => self.refusals.count(RxRefusal::Place),
                }
            }
            // Copied: the unit goes back at the round's close.
            round.release(handle);
            return Delivery::Done;
        }
        if self.pressure == Pressure::Late {
            round.release(handle);
            self.refusals.count(RxRefusal::Pressure);
            return Delivery::Done;
        }
        Delivery::Wait(handle)
    }
}

/// Where a staged unit goes.
enum Route {
    /// Back to the ring at the round's close: a control frame (its reader
    /// is the reorder, C2), a frame of no link of ours, a Null.
    Back,
    /// Read by the controller inside the round, then back.
    Controller(Option<u8>),
    /// Taken for the network.
    Network,
    /// Refused, counted, back.
    Refuse(RxRefusal),
}

/// The route of a staged unit.
fn route<M: RxMetadata>(
    view: &Received<'_, M>,
    peer_of: &impl Fn(&RxHeader) -> Option<RxPeer>,
) -> Route {
    let RxHead::Frame(header) = view.head else {
        return Route::Back;
    };
    let from = peer_of(header);
    if header.control.is_management_frame() {
        return Route::Controller(from.map(|from| from.peer));
    }
    let Some(from) = from.filter(|_| header.control.is_data()) else {
        return Route::Back;
    };
    if header.control.is_null_data() {
        return Route::Back;
    }
    let protected = header.control.protected();
    if protected && !verified(view.meta.common().crypto) {
        return Route::Refuse(RxRefusal::Unverified);
    }
    let eapol = matches!(first_ether_type(view), Some(ETHER_TYPE_EAPOL));
    if eapol {
        return Route::Controller(Some(from.peer));
    }
    if !protected && from.protection == LinkProtection::Ccmp {
        return Route::Refuse(RxRefusal::Unverified);
    }
    if from.carrying == Carrying::Held {
        return Route::Refuse(RxRefusal::LinkHeld);
    }
    Route::Network
}

/// Whether the port's evidence says the frame was decrypted and its
/// integrity checked.
const fn verified(crypto: RxEvidence<RxCryptoStatus>) -> bool {
    matches!(
        crypto,
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
            | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified)
    )
}

/// Where a data frame's payload lies in its bytes: after its MAC header and,
/// when protected, the CCMP header the port keeps; to the end of the bytes
/// (the port's bytes end at the body: a MIC the hardware checked is not in
/// them).
fn payload_of(header: &RxHeader, bytes: &[u8]) -> Option<(usize, usize)> {
    let offset = usize::from(header.body).checked_add(if header.control.protected() {
        CCMP_HEADER_LEN
    } else {
        0
    })?;
    Some((offset, bytes.len().checked_sub(offset)?))
}

/// How a data frame is addressed, from its DS bits: from the DS to a
/// station, or to the DS from a client (IEEE Std 802.11-2020, 9.2.4.1.4).
const fn role_of(header: &RxHeader) -> DataInterfaceRole {
    match header.control.ds() {
        2 => DataInterfaceRole::Station,
        _ => DataInterfaceRole::AccessPoint,
    }
}

/// The EtherType of a data frame's first MSDU, when it decapsulates.
fn first_ether_type<M>(view: &Received<'_, M>) -> Option<u16> {
    let RxHead::Frame(header) = view.head else {
        return None;
    };
    let role = role_of(header);
    let (offset, length) = payload_of(header, view.bytes)?;
    let mut frames = decapsulate_data_frames(role, view.bytes, offset, length).ok()?;
    frames.next()?.ok().map(|msdu| msdu.ether_type)
}

/// One MSDU of a unit: where its Ethernet frame lies once its addresses
/// are written.
#[derive(Clone, Copy)]
struct Msdu {
    addresses: EthernetAddresses,
    span: EthernetSpan,
}

/// The MSDUs of a data unit into `msdus`, validated whole before any is
/// written.
fn msdus<M>(
    view: &Received<'_, M>,
    msdus: &mut heapless::Vec<Msdu, AMSDU_MSDUS>,
) -> Result<(), RxRefusal> {
    msdus.clear();
    let RxHead::Frame(header) = view.head else {
        return Err(RxRefusal::Undecapsulable);
    };
    let role = role_of(header);
    let (offset, length) = payload_of(header, view.bytes).ok_or(RxRefusal::Undecapsulable)?;
    let frames = decapsulate_data_frames(role, view.bytes, offset, length)
        .map_err(|_| RxRefusal::Undecapsulable)?;
    let start = view.bytes.as_ptr().addr();
    for frame in frames {
        let frame = frame.map_err(|_| RxRefusal::Undecapsulable)?;
        // The payload follows the EtherType: its offset in the unit names
        // where the Ethernet frame begins once its addresses are written.
        let payload = frame
            .payload
            .as_ptr()
            .addr()
            .checked_sub(start)
            .ok_or(RxRefusal::Undecapsulable)?;
        let ether_type = payload.checked_sub(2).ok_or(RxRefusal::Undecapsulable)?;
        let begin = ether_type
            .checked_sub(12)
            .ok_or(RxRefusal::Undecapsulable)?;
        let end = payload
            .checked_add(frame.payload.len())
            .ok_or(RxRefusal::Undecapsulable)?;
        let msdu = Msdu {
            addresses: EthernetAddresses {
                destination: frame.destination,
                source: frame.source,
                ether_type: u16::try_from(ether_type).map_err(|_| RxRefusal::Undecapsulable)?,
            },
            span: EthernetSpan {
                offset: u16::try_from(begin).map_err(|_| RxRefusal::Undecapsulable)?,
                length: u16::try_from(end.saturating_sub(begin))
                    .map_err(|_| RxRefusal::Undecapsulable)?,
            },
        };
        msdus.push(msdu).map_err(|_| RxRefusal::AmsduTooLong)?;
    }
    if msdus.is_empty() {
        return Err(RxRefusal::Undecapsulable);
    }
    Ok(())
}
