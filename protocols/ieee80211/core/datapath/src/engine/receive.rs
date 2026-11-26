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
//! close. The ring is read in every round: a management frame, EAPOL and a
//! BlockAckReq pass at once, whatever waits for the network.
//!
//! A data unit for the network is decapsulated in its place once (its
//! Ethernet addresses written before the EtherType its LLC/SNAP header
//! holds), then lent to the network while it has room to borrow, else
//! copied into the network's storage while it has room for a copy, else it
//! waits, taken, behind what already waits, in the order of the moment each
//! began to wait (`design/engine-model-rx.py`, `tasks/205`):
//!
//! - a newcomer from the ring keeps a place (lent or waiting) only while a
//!   place stays free after it, the place the control plane passes
//!   through; at the last place it is copied, when nothing waits and the
//!   network can copy it, or refused ([`RxRefusal::NoPlaceToWait`]); a unit
//!   the reorder lets go waits, or is lent, in the place it already holds;
//! - a unit that waited `late` (the composition's policy value) from its
//!   own moment is refused at the head ([`RxRefusal::Late`]), every late
//!   head in one round;
//! - a waiting unit of a link the controller holds is refused when it
//!   reaches the head ([`RxRefusal::LinkHeld`]).

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee802_wire::{EtherType, Eui48};
use oer_protocols_ieee80211_contracts_port::{
    EthernetAddresses, Lendable, ReceivePort, Received, Rest as PortRest, Round, RxCryptoStatus,
    RxEvidence, RxHead, RxHeader, RxMetadata, UnitHandle,
};
use oer_protocols_ieee80211_core_installation::Rest;
use oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN;
use oer_protocols_ieee80211_wire::data::{DataInterfaceRole, decapsulate_data_frames};
use oer_protocols_ieee80211_wire::security::LinkProtection;

use super::{Agreements, Carrying, Network, Told};
use oer_protocols_ieee80211_contracts_port::Station as StationRole;

mod pressure;
use oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_protocols_ieee80211_core_upper_mac::block_ack::{
    RxAgreements, RxBlockAckRefused, RxDisposition, RxView,
};
use oer_protocols_ieee80211_wire::address::Bssid;
use oer_protocols_ieee80211_wire::block_ack::frame::{BlockAckAction, parse_block_ack_action};
use pressure::{Decision, Link, Need, Pressure, Taken};

/// The Action subtype of a management frame.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Action is subtype 13).

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
#[derive(Debug)]
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
    /// The network had no room for it and keeping it would have taken the
    /// last free place: the MAC acknowledged it, so it is lost to the peer.
    NoPlaceToWait,
    /// It waited for the network the composition's `late` from the moment
    /// it began to wait.
    Late,
    /// Protected data without the port's evidence of its integrity, or
    /// unprotected data other than EAPOL on a protected link.
    Unverified,
    /// Data whose MSDU does not decapsulate: no RFC 1042 header, a
    /// fragment, a truncated A-MSDU.
    Undecapsulable,
    /// An A-MSDU of more than [`AMSDU_MSDUS`] MSDUs.
    AmsduTooLong,
    /// The reorder gave it back: an old frame, a duplicate, or a frame it
    /// would retain with no place free beyond the hole's.
    Reorder,
    /// The unit's place could not be read or written.
    Place,
}

/// The receive refusals counted by reason, each saturating.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxRefusals {
    pub link_held: u16,
    pub no_place_to_wait: u16,
    pub late: u16,
    pub unverified: u16,
    pub undecapsulable: u16,
    pub amsdu_too_long: u16,
    pub reorder: u16,
    pub place: u16,
}

impl RxRefusals {
    /// One more refusal for `reason`.
    pub fn count(&mut self, reason: RxRefusal) {
        let count = match reason {
            RxRefusal::LinkHeld => &mut self.link_held,
            RxRefusal::NoPlaceToWait => &mut self.no_place_to_wait,
            RxRefusal::Late => &mut self.late,
            RxRefusal::Unverified => &mut self.unverified,
            RxRefusal::Undecapsulable => &mut self.undecapsulable,
            RxRefusal::AmsduTooLong => &mut self.amsdu_too_long,
            RxRefusal::Reorder => &mut self.reorder,
            RxRefusal::Place => &mut self.place,
        };
        *count = count.saturating_add(1);
    }
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

/// What a round knows of the links: the data's flow, a frame's linked
/// peer, and a station's receive agreements.
pub(super) struct Links<
    'a,
    'id,
    F,
    K: Ieee80211Capabilities,
    const BANKS: usize,
    const WINDOW: usize,
> {
    pub(super) flow: Flow,
    pub(super) peer_of: F,
    pub(super) reorder: Reordering<'a, 'id, K, BANKS, WINDOW>,
}

/// Whose data a round reads: a station's, with its receive agreements, or
/// an access point's, which has none in phase B.
pub(super) enum Reordering<
    'a,
    'id,
    K: Ieee80211Capabilities,
    const BANKS: usize,
    const WINDOW: usize,
> {
    Station(Reorder<'a, 'id, K, BANKS, WINDOW>),
    AccessPoint,
}

/// A station's receive agreements, lent a round: the reorder, the
/// station's address and its access point's, whether it admits agreements,
/// and its link's generation (`None` without a link).
pub(super) struct Reorder<
    'a,
    'id,
    K: Ieee80211Capabilities,
    const BANKS: usize,
    const WINDOW: usize,
> {
    pub(super) agreements: &'a mut Agreements<'id, StationRole, K, BANKS, WINDOW>,
    pub(super) local: [u8; 6],
    pub(super) bssid: Option<Bssid>,
    pub(super) admitted: RxAgreements,
    pub(super) link: Option<u8>,
}

/// The receive half's state while the data runs.
pub(super) struct Receive<'id> {
    /// The pressure rule: what waits for the network, the units it holds.
    pressure: Pressure<UnitHandle<'id>>,
    refusals: RxRefusals,
    /// The MSDUs of the unit being delivered, found before any is written:
    /// kept in place and filled again for each unit.
    msdus: heapless::Vec<Msdu, AMSDU_MSDUS>,
}

impl<'id> Receive<'id> {
    pub(super) const fn new(late: Duration) -> Self {
        Self {
            pressure: Pressure::new(late),
            refusals: RxRefusals {
                link_held: 0,
                no_place_to_wait: 0,
                late: 0,
                unverified: 0,
                undecapsulable: 0,
                amsdu_too_long: 0,
                reorder: 0,
                place: 0,
            },
            msdus: heapless::Vec::new(),
        }
    }

    /// The units waiting for the network, for the crate's search.
    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.pressure.waiting()
    }

    /// The refusals counted so far.
    pub(super) const fn refusals(&self) -> RxRefusals {
        self.refusals
    }

    /// The receive half's state, for the engine's search: the core's (the
    /// places that wait, the order of their deadlines with their ties, the
    /// agreements' `gap` deadline's place in it, the units lent) and the
    /// refusals.
    #[cfg(test)]
    pub(super) fn key(&self, gap: Option<Instant>) -> std::string::String {
        std::format!(
            "{}|{:?}",
            self.pressure.key(gap, UnitHandle::place),
            self.refusals
        )
    }

    /// One round over `port`, the links as `links` names them.
    pub(super) fn round<P, N, C, F, K, const BANKS: usize, const WINDOW: usize>(
        &mut self,
        port: &mut P,
        network: &mut N,
        told: &mut C,
        now: Instant,
        links: Links<'_, 'id, F, K, BANKS, WINDOW>,
    ) -> Rest
    where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
        C: Told<'id>,
        F: Fn(&RxHeader) -> Option<RxPeer>,
        K: Ieee80211Capabilities,
    {
        let Links {
            flow,
            peer_of,
            reorder: mut reordering,
        } = links;
        let mut reorder = match &mut reordering {
            Reordering::Station(reorder) => Some(reorder),
            Reordering::AccessPoint => None,
        };
        let mut round = Round::open(port, now);
        self.pressure.reclaimed(round.reclaimed());
        let mut room = network.room();
        // A link that ended, or a stopping data, ends its agreements: what
        // the reorder retains goes back undelivered.
        if let Some(reorder) = &mut reorder {
            let mut agreements = reorder.agreements.view();
            // The stop ends the links before its round: the link is none.
            let ended = reorder.link.is_none();
            if ended && agreements.association().is_some() {
                agreements.clear(&mut (), |handle| round.release(handle));
            } else if flow == Flow::Flowing {
                agreements.tick(now, &mut ());
            }
        }
        // What waits goes first, from its head, as the core decides with
        // what the round reads of each: until the head still waits.
        while let Some(head) = self.pressure.head() {
            let need = need(&round, head, &mut self.msdus);
            let link = link(&round, head, &peer_of);
            match self.pressure.next(need, link, flow, now, &mut room) {
                Decision::Kept => break,
                decision => self.carry(&mut round, decision, network),
            }
        }
        // Then what the reorder released, in its order.
        self.ready(
            &mut round,
            &mut reorder,
            &mut room,
            network,
            told,
            &peer_of,
            now,
            None,
        );
        // The ring is read in every round, whatever waits.
        {
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
                    Route::Agreements(action) => {
                        if let Some(reorder) = &mut reorder
                            && let (Some(bssid), Some(association)) = (reorder.bssid, reorder.link)
                        {
                            let mut agreements = reorder.agreements.view();
                            let answer = agreements.offer(
                                now,
                                association,
                                bssid,
                                action,
                                reorder.admitted,
                                &mut (),
                                |handle| round.release(handle),
                            );
                            // Unanswered (no reply place or bank yet): the
                            // peer asks again.
                            let (Ok(()) | Err(RxBlockAckRefused::Capacity(_))) = answer;
                            if agreements.next_reply().is_some() {
                                told.reply_owed();
                            }
                        }
                    }
                    Route::Control | Route::Network | Route::Eapol => {
                        let control = matches!(view.head, RxHead::Control(_));
                        let handle = unit.take();
                        // The places taken outside the core before the frame
                        // joins: the reorder's retained (a release keeps
                        // their places).
                        let outside = reorder
                            .as_mut()
                            .map_or(0, |reorder| reorder.agreements.view().retained());
                        let taken = Taken::Newcomer { outside };
                        let place = handle.place();
                        let delivered = match &mut reorder {
                            Some(reorder) => self.reordered(&mut round, handle, reorder, now),
                            None => Some(handle),
                        };
                        match delivered {
                            // A control frame no reorder reads goes back.
                            Some(handle) if control => round.release(handle),
                            Some(handle) => {
                                self.next_in_order(
                                    &mut round, handle, &mut room, network, told, &peer_of, now,
                                    taken,
                                );
                            }
                            None => {}
                        }
                        // What the frame released follows it (the frame
                        // itself, when the reorder let it go with them).
                        self.ready(
                            &mut round,
                            &mut reorder,
                            &mut room,
                            network,
                            told,
                            &peer_of,
                            now,
                            Some((place, taken)),
                        );
                    }
                }
            }
        }
        let rest = round.close();
        // The head's bound: the oldest waiting unit's moment and `late`.
        let bound = self.pressure.bound();
        // The reorder's gap and the agreements' inactivity wake too.
        let deadline = reorder
            .as_mut()
            .and_then(|reorder| reorder.agreements.deadline());
        let earliest = |at: Option<Instant>| match (at, deadline) {
            (Some(at), Some(deadline)) => Some(at.min(deadline)),
            (at, None) => at,
            (None, deadline) => deadline,
        };
        let soonest = |first: Option<Instant>, second: Option<Instant>| match (first, second) {
            (Some(first), Some(second)) => Some(first.min(second)),
            (first, None) => first,
            (None, second) => second,
        };
        // What waits wakes at the network's return or at the head's bound.
        match rest {
            PortRest::Round => Rest::Round,
            PortRest::At(at) => Rest::Wake(earliest(soonest(Some(at), bound))),
            PortRest::Signal => Rest::Wake(earliest(bound)),
        }
    }

    /// A taken data unit or control frame through the reorder: delivered
    /// now (its handle back), or retained or given back (`None`).
    fn reordered<P, K, const BANKS: usize, const WINDOW: usize>(
        &mut self,
        round: &mut Round<'_, 'id, P>,
        handle: UnitHandle<'id>,
        reorder: &mut Reorder<'_, 'id, K, BANKS, WINDOW>,
        now: Instant,
    ) -> Option<UnitHandle<'id>>
    where
        P: ReceivePort<'id>,
        K: Ieee80211Capabilities,
    {
        let mut agreements = reorder.agreements.view();
        // One place stays free for the frame that closes a hole: the core's
        // rule, beside the reorder's retained.
        let retain = self.pressure.retain_room(agreements.retained(), P::STAGING);
        let control = round
            .unit(&handle)
            .is_some_and(|view| matches!(view.head, RxHead::Control(_)));
        let disposition = match round.unit(&handle) {
            Some(view) => agreements.receive(
                now,
                Some(Eui48::new(reorder.local)),
                handle,
                RxView {
                    head: view.head,
                    bytes: view.bytes,
                },
                view.meta,
                retain,
                &mut (),
            ),
            // `None` does not happen (`Round::unit`): it goes back.
            None => RxDisposition::Recycle(handle),
        };
        match disposition {
            RxDisposition::Deliver(handle) => Some(handle),
            RxDisposition::Retained => None,
            RxDisposition::Recycle(handle) => {
                // A BlockAckReq read goes back; an old frame, a duplicate or
                // a frame with no place is refused, counted.
                round.release(handle);
                if !control {
                    self.refusals.count(RxRefusal::Reorder);
                }
                None
            }
        }
    }

    /// Deliver what the reorder released, in its order, behind what waits;
    /// a held link's is refused.
    #[allow(
        clippy::too_many_arguments,
        reason = "the round, the reorder, the network's room and end, the controller and the links"
    )]
    fn ready<P, N, C, F, K, const BANKS: usize, const WINDOW: usize>(
        &mut self,
        round: &mut Round<'_, 'id, P>,
        reorder: &mut Option<&mut Reorder<'_, 'id, K, BANKS, WINDOW>>,
        room: &mut Room,
        network: &mut N,
        told: &mut C,
        peer_of: &F,
        now: Instant,
        newcomer: Option<(u8, Taken)>,
    ) where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
        C: Told<'id>,
        F: Fn(&RxHeader) -> Option<RxPeer>,
        K: Ieee80211Capabilities,
    {
        let Some(reorder) = reorder else {
            return;
        };
        let mut agreements = reorder.agreements.view();
        // A unit the reorder lets go holds its place already, but for the
        // newcomer of this round, which the reorder may let go itself (the
        // frame that closes a hole).
        while let Some(handle) = agreements.pop_ready() {
            let taken = match newcomer {
                Some((place, taken)) if place == handle.place() => taken,
                _ => Taken::InPlace,
            };
            self.next_in_order(round, handle, room, network, told, peer_of, now, taken);
        }
    }

    /// A unit the reorder let go in order: EAPOL to the controller, read in
    /// place and back; data offered to the pressure rule with its need and
    /// link, and what it decides carried out.
    #[allow(
        clippy::too_many_arguments,
        reason = "the round, the unit, the network's room and end, the controller, the links, the clock and its place"
    )]
    fn next_in_order<P, N, C, F>(
        &mut self,
        round: &mut Round<'_, 'id, P>,
        handle: UnitHandle<'id>,
        room: &mut Room,
        network: &mut N,
        told: &mut C,
        peer_of: &F,
        now: Instant,
        taken: Taken,
    ) where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
        C: Told<'id>,
        F: Fn(&RxHeader) -> Option<RxPeer>,
    {
        let read = match round.unit(&handle) {
            Some(view) if eapol(&view) => {
                let peer = match view.head {
                    RxHead::Frame(header) => peer_of(header).map(|from| from.peer),
                    RxHead::Control(_) => None,
                };
                told.frame(peer, &view);
                true
            }
            _ => false,
        };
        if read {
            round.release(handle);
            return;
        }
        let need = need(round, &handle, &mut self.msdus);
        let link = link(round, &handle, peer_of);
        let decision = self
            .pressure
            .offer(handle, need, link, taken, P::STAGING, now, room);
        self.carry(round, decision, network);
    }

    /// Carry out what the core decided of a unit, its MSDUs the ones its
    /// need was read with just before.
    fn carry<P, N>(
        &mut self,
        round: &mut Round<'_, 'id, P>,
        decision: Decision<UnitHandle<'id>, Loan>,
        network: &mut N,
    ) where
        P: ReceivePort<'id>,
        N: Network<P::Memory, P::Lent>,
    {
        match decision {
            Decision::Kept => {}
            Decision::Back(handle) => round.release(handle),
            Decision::Refused(handle, reason) => {
                round.release(handle);
                self.refusals.count(reason);
            }
            Decision::Lend(handle, Loan { lendable, msdu }, permit) => {
                if round.write_addresses(&handle, msdu.addresses).is_ok() {
                    let lent = round.lend(handle, lendable);
                    network.lent(permit, lent, msdu.span);
                } else {
                    // The MSDU was found inside the unit: the port refuses
                    // no write of it. The loan the core counted comes back.
                    self.pressure.reclaimed(1);
                    round.release(handle);
                    self.refusals.count(RxRefusal::Place);
                }
            }
            Decision::Copy(handle, mut permits) => {
                for msdu in &self.msdus {
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
            }
        }
    }
}

/// What a loan needs: the unit's proof that it can be lent and its one
/// MSDU.
#[derive(Clone, Copy, Debug)]
struct Loan {
    lendable: Lendable,
    msdu: Msdu,
}

/// What a unit needs of the network, from its bytes: one MSDU it may lend,
/// else the frames to copy, else the reason it is unread. Its MSDUs are left
/// in `msdus` for the delivery.
fn need<'id, P: ReceivePort<'id>>(
    round: &Round<'_, 'id, P>,
    handle: &UnitHandle<'id>,
    msdus: &mut heapless::Vec<Msdu, AMSDU_MSDUS>,
) -> Need<Loan> {
    let Some(view) = round.unit(handle) else {
        // `None` does not happen (`Round::unit`).
        msdus.clear();
        return Need::Unread(RxRefusal::Place);
    };
    if let Err(reason) = self::msdus(&view, msdus) {
        return Need::Unread(reason);
    }
    match (msdus.as_slice(), view.lendable()) {
        ([msdu], Some(lendable)) => Need::Lendable(Loan {
            lendable,
            msdu: *msdu,
        }),
        (msdus, _) => Need::Copies(u8::try_from(msdus.len()).unwrap_or(u8::MAX)),
    }
}

/// Whether a unit's link carries: a held link's (or of no link) is held.
fn link<'id, P, F>(round: &Round<'_, 'id, P>, handle: &UnitHandle<'id>, peer_of: &F) -> Link
where
    P: ReceivePort<'id>,
    F: Fn(&RxHeader) -> Option<RxPeer>,
{
    let held = round
        .unit(handle)
        .and_then(|view| match view.head {
            RxHead::Frame(header) => peer_of(header),
            RxHead::Control(_) => None,
        })
        .is_none_or(|from| from.carrying == Carrying::Held);
    if held { Link::Held } else { Link::Carrying }
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
    /// A control frame: the reorder reads a BlockAckReq.
    Control,
    /// EAPOL of the linked peer: it takes its number in a receive
    /// agreement's window first, and the controller reads it when the
    /// reorder releases it (at once without an agreement).
    Eapol,
    /// A Block Ack Action of the linked peer: the agreements take it.
    Agreements(BlockAckAction),
    /// Refused, counted, back.
    Refuse(RxRefusal),
}

/// The route of a staged unit.
fn route<M: RxMetadata>(
    view: &Received<'_, M>,
    peer_of: &impl Fn(&RxHeader) -> Option<RxPeer>,
) -> Route {
    let RxHead::Frame(header) = view.head else {
        return Route::Control;
    };
    let from = peer_of(header);
    if header.control.is_management_frame() {
        // A Block Ack Action of the linked peer is the agreements'.
        if from.is_some()
            && header.control.management_subtype()
                == oer_protocols_ieee80211_wire::frame::ManagementSubtype::Action
            && let Some(action) = view
                .bytes
                .get(usize::from(header.body)..)
                .and_then(parse_block_ack_action)
        {
            return Route::Agreements(action);
        }
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
    if eapol(view) {
        return Route::Eapol;
    }
    if !protected && from.protection == LinkProtection::Ccmp {
        return Route::Refuse(RxRefusal::Unverified);
    }
    if from.carrying == Carrying::Held {
        return Route::Refuse(RxRefusal::LinkHeld);
    }
    Route::Network
}

/// Whether a data frame's first MSDU is EAPOL.
fn eapol<M>(view: &Received<'_, M>) -> bool {
    matches!(first_ether_type(view), Some(EtherType::EAPOL))
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
fn first_ether_type<M>(view: &Received<'_, M>) -> Option<EtherType> {
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
#[derive(Clone, Copy, Debug)]
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
                ether_type_at: u16::try_from(ether_type).map_err(|_| RxRefusal::Undecapsulable)?,
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
