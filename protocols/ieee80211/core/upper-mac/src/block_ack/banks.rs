//! The receive Block Ack agreements of one association: ADDBA negotiation,
//! reorder, BlockAckReq, inactivity, DELBA and the replies they owe.
//!
//! The owner holds the integration's affine bank slots and the handles of
//! the frames it retains, and decides every agreement edge. It never
//! touches a port itself: an agreement's addition and its end are commands
//! its caller begins ([`RxBlockAckView::next_command`]) and reports by the
//! bank's ticket ([`RxBlockAckView::added`], [`RxBlockAckView::emptied`]).
//! A received frame is read in place for the length of a call (its view and
//! its metadata by reference); the owner keeps its handle alone, never its
//! bytes. A frame it would retain is taken only while the caller has room
//! beyond the place it keeps for the frame that closes the hole
//! ([`RetainRoom`]).

use oer_portable_events::{EventSink, report};
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::qos::Tid;

use super::{
    AddbaResponse, AddbaStatus, AmsduInAmpdu, BankRoom, Received, RxAddbaRequest,
    RxBlockAckNegotiation, RxBlockAckSession, RxReorderMpdu, RxReorderRelease,
};
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    Aggregation, Ieee80211Capabilities, RxCryptoStatus, RxEvidence, RxHead, RxHeader, RxMetadata,
    SupportedBlockAck,
};
use oer_protocols_ieee80211_wire::{
    address::Bssid,
    block_ack::frame::{
        ADDBA_ACTION_BODY_LEN, BlockAckAction, encode_delba, parse_compressed_block_ack_request,
    },
    sequence::SequenceNumber,
    status::ReasonCode,
};

/// Whether the station admits receive agreements now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxAgreements {
    Admitted,
    Declined,
}

/// What the receive agreements decided, reported to the sink the caller lends
/// a call. The owner keeps no count of these.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckEvent {
    /// A received MPDU's aggregation evidence, for each admitted MPDU, never
    /// a physical aggregate or member count. `bytes` is the port's MPDU
    /// length, `None` when it does not fit.
    Contained {
        aggregation: RxEvidence<Aggregation>,
        bytes: Option<u32>,
    },
    /// One reorder decision of the agreement for `tid`. `released` counts
    /// MPDUs the reorder owner released, not application deliveries;
    /// `missing` counts skipped sequences, not dropped buffered MPDUs.
    Reordered {
        tid: Tid,
        cause: ReorderCause,
        held: u8,
        released: u8,
        missing: u16,
    },
    /// An agreement became active: the logical owner, not a confirmed
    /// hardware register write.
    Started { tid: Tid, window: u16 },
    /// An agreement ended.
    Stopped { tid: Tid, reason: AgreementStop },
    /// One ADDBA Request taken: its TID, the Buffer Size it asked for as
    /// received (zero included) and what became of it. A started agreement
    /// reports [`Self::Started`] first.
    Requested {
        tid: Tid,
        requested: u16,
        outcome: AddbaOutcome,
    },
}

/// What became of one ADDBA Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaOutcome {
    /// A new agreement started with `window`; its success response is owed.
    Accepted { window: u16 },
    /// The running agreement's own request again: its success response,
    /// with the agreement's `window`, is owed once more.
    Repeated { window: u16 },
    /// A refusal response with this status is queued.
    Refused(AddbaStatus),
    /// No reply place or bank remained ([`RxBlockAckCapacity`]): no
    /// response leaves, and the access point asks again.
    Unanswered,
}

/// What a reorder decision answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReorderCause {
    Receive,
    /// The reorder gap timed out: buffered MPDUs are released and missing
    /// sequences skipped.
    Timeout,
    Bar,
    Stop,
    /// Clear recycles its buffered owners.
    Clear,
    /// A buffered duplicate.
    Duplicate,
    /// Other rejected or stale input.
    Rejected,
    /// A frame that would be retained found no place: the caller's room
    /// for the hole's frame, or the owner's frames.
    Full,
}

/// Why an agreement ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgreementStop {
    Peer,
    Replaced,
    Timeout,
    InvalidGap,
    Clear,
}

/// The receive window a composition takes when it names none: 16 MPDUs.
/// The window (`WINDOW` of [`RxBlockAckStorage`]) is the most frames one
/// agreement retains and the most in-order releases waiting for delivery;
/// it is the composition's choice, from 1 to the capabilities'
/// `rx_block_ack_max_window`, and at most the retained frames.
pub const RX_BLOCK_ACK_WINDOW: usize = 16;
/// What is wrong with a receive window of `window` over `frames` retained
/// frames under a capability of `max_window`, if anything: the window is 1
/// to the capability, the retained frames hold a whole window, and the
/// in-order releases are counted in a byte.
pub(crate) const fn window_refusal(
    window: usize,
    frames: usize,
    max_window: u16,
) -> Option<&'static str> {
    if window == 0 || window > max_window as usize {
        Some("the receive window is 1 to the capabilities' rx_block_ack_max_window")
    } else if frames < window {
        Some("the retained frames hold a whole window: WINDOW <= FRAMES")
    } else if window > u8::MAX as usize {
        Some("the in-order releases are counted in a byte")
    } else {
        None
    }
}

/// Encoded replies waiting for a control transmit credit.
const PENDING: usize = 8;

/// The owners an integration lends the receive agreements.
pub trait RxBlockAckTypes {
    /// A free receive agreement slot of the port.
    type Slot;
    /// An agreement installed in a slot.
    type Running;
    /// A received MPDU's owner, a handle the reorder names: the agreements
    /// know nothing of it, and decide from its view ([`RxView`]).
    type Frame;
    /// The capabilities that bound an agreement's TID and window.
    type Caps: Ieee80211Capabilities;
}

/// A received MPDU read in place, beside its owner: its header, decoded
/// once, and its bytes. Nothing of it is kept.
#[derive(Clone, Copy, Debug)]
pub struct RxView<'a> {
    /// What it is, classified once.
    pub head: &'a RxHead,
    /// The MPDU, from its header to the end of its body.
    pub bytes: &'a [u8],
}

type Session<T, const WINDOW: usize> =
    RxBlockAckSession<<T as RxBlockAckTypes>::Running, WINDOW, WINDOW>;

/// Whether an agreement's success response is owed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reply {
    Owed,
    Sent,
}

/// One agreement bank, a sum of where its slot is.
enum Bank<T: RxBlockAckTypes, const WINDOW: usize> {
    /// Free: its slot.
    Free(T::Slot),
    /// A request accepted: its slot and agreement wait for the command
    /// place ([`RxBlockAckView::next_command`]).
    Adding {
        slot: T::Slot,
        agreement: SupportedBlockAck<T::Caps>,
        negotiation: RxBlockAckNegotiation,
    },
    /// Its addition runs in the port.
    AddCommand { negotiation: RxBlockAckNegotiation },
    /// The agreement runs.
    Active {
        session: Session<T, WINDOW>,
        reply: Reply,
    },
    /// Ended: the agreement of `tid` waits for the command place to be
    /// removed.
    Ending { running: T::Running, tid: Tid },
    /// The removal of `tid`'s agreement runs in the port.
    EndCommand { tid: Tid },
    /// The slot went home with the data's stop ([`RxBlockAckStorage::slots_home`]);
    /// the storage serves nothing until it is built again or the slots come
    /// back ([`RxBlockAckStorage::slots_back`]).
    Away,
    /// Inside one method, between two of the above: the slot or the
    /// agreement moves by value. Never seen outside it.
    InTransition,
}

impl<T: RxBlockAckTypes, const WINDOW: usize> Bank<T, WINDOW> {
    /// The TID the bank is not free for: its agreement's, added, running or
    /// ending.
    fn tid(&self) -> Option<Tid> {
        match self {
            Self::Adding { negotiation, .. } | Self::AddCommand { negotiation } => {
                Some(negotiation.response().tid)
            }
            Self::Active { session, .. } => Some(session.tid()),
            Self::Ending { tid, .. } | Self::EndCommand { tid } => Some(*tid),
            Self::Free(_) | Self::Away | Self::InTransition => None,
        }
    }

    const fn active(&self) -> Option<&Session<T, WINDOW>> {
        match self {
            Self::Active { session, .. } => Some(session),
            _ => None,
        }
    }

    const fn active_mut(&mut self) -> Option<&mut Session<T, WINDOW>> {
        match self {
            Self::Active { session, .. } => Some(session),
            _ => None,
        }
    }
}

/// A command an agreement bank owes the port: its addition or its end, with
/// the ticket its end is reported by.
pub enum BankCommand<T: RxBlockAckTypes> {
    /// Add `agreement` in `slot` for the bank.
    Add {
        ticket: BankTicket,
        slot: T::Slot,
        agreement: SupportedBlockAck<T::Caps>,
    },
    /// End the bank's agreement.
    End {
        ticket: BankTicket,
        running: T::Running,
    },
}

/// The bank whose command runs and the TID it serves: made only by
/// [`RxBlockAckView::next_command`] and consumed by the command's end, so an
/// end names the bank that began it and the agreement's TID.
#[derive(Debug, Eq, PartialEq)]
pub struct BankTicket {
    bank: u8,
    tid: Tid,
}

/// Whether the caller has a place for one more retained frame: the reorder
/// retains a frame that does not close its window's hole only while a place
/// stays free for the one that does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainRoom {
    /// A place is free beyond the one kept for the hole.
    Free,
    /// Only the hole's place is free: a frame that would be retained is
    /// given back.
    Full,
}

struct Header<E, const WINDOW: usize> {
    association: Option<(E, Bssid)>,
    pending: heapless::Deque<[u8; ADDBA_ACTION_BODY_LEN], PENDING>,
    ready: [u8; WINDOW],
    ready_len: u8,
    failed: bool,
    gap_timeout: Duration,
}

/// Placed backing selected by an image: `BANKS` agreement slots, `FRAMES`
/// retained frames of association identity `E` and the receive window
/// `WINDOW`. Frame storage holds the frames' handles, never their bytes.
pub struct RxBlockAckStorage<
    T: RxBlockAckTypes,
    E,
    const BANKS: usize,
    const FRAMES: usize,
    const WINDOW: usize,
> {
    header: Header<E, WINDOW>,
    banks: [Bank<T, WINDOW>; BANKS],
    frames: [Option<T::Frame>; FRAMES],
    /// The relations were checked: no storage is built without them.
    _fits: WindowFits,
}

/// The proof that a storage's relations hold, of no size: made only by
/// [`RxBlockAckStorage::GEOMETRY`], and held by every storage.
#[derive(Clone, Copy, Debug)]
pub struct WindowFits(());

impl<T: RxBlockAckTypes, E: Copy + Eq, const BANKS: usize, const FRAMES: usize, const WINDOW: usize>
    RxBlockAckStorage<T, E, BANKS, FRAMES, WINDOW>
{
    /// The storage's relations, checked where it is built
    /// ([`window_refusal`]).
    pub const GEOMETRY: WindowFits = {
        if let Some(refusal) = window_refusal(WINDOW, FRAMES, T::Caps::CAPS.rx_block_ack_max_window)
        {
            panic!("{}", refusal);
        }
        WindowFits(())
    };

    pub fn new(slots: [T::Slot; BANKS], gap_timeout: Duration) -> Self {
        Self {
            header: Header {
                association: None,
                pending: heapless::Deque::new(),
                ready: [0; WINDOW],
                ready_len: 0,
                failed: false,
                gap_timeout,
            },
            banks: slots.map(Bank::Free),
            frames: [const { None }; FRAMES],
            _fits: Self::GEOMETRY,
        }
    }

    pub fn view(&mut self) -> RxBlockAckView<'_, T, E, WINDOW> {
        RxBlockAckView {
            header: &mut self.header,
            banks: &mut self.banks,
            frames: &mut self.frames,
        }
    }

    /// Whether every bank is free: no agreement runs and no command of one
    /// is in the port.
    pub fn is_free(&self) -> bool {
        self.banks.iter().all(|bank| matches!(bank, Bank::Free(_)))
    }

    /// Every slot back, when every bank is free; the storage keeps no
    /// slot after it and serves no agreement until it is built again.
    ///
    /// # Errors
    ///
    /// A bank is not free ([`Self::is_free`]); nothing moved.
    pub fn slots_home(&mut self) -> Result<[T::Slot; BANKS], BanksBusy> {
        if !self.is_free() {
            return Err(BanksBusy);
        }
        // Every bank is free: each slot goes into one of BANKS places, so no
        // push below is refused; a refused one stays in its bank.
        let mut slots = heapless::Vec::<T::Slot, BANKS>::new();
        for bank in self.banks.iter_mut() {
            if let Bank::Free(slot) = core::mem::replace(bank, Bank::Away)
                && let Err(slot) = slots.push(slot)
            {
                *bank = Bank::Free(slot);
            }
        }
        slots.into_array::<BANKS>().map_err(|slots| {
            // Not every bank gave its slot: this does not happen. The slots
            // go back to the banks they left, in order.
            let mut slots = slots.into_iter();
            for bank in self.banks.iter_mut() {
                if matches!(bank, Bank::Away)
                    && let Some(slot) = slots.next()
                {
                    *bank = Bank::Free(slot);
                }
            }
            BanksBusy
        })
    }
}

impl<T: RxBlockAckTypes, E, const BANKS: usize, const FRAMES: usize, const WINDOW: usize>
    RxBlockAckStorage<T, E, BANKS, FRAMES, WINDOW>
{
    /// The earliest inactivity or reorder-gap deadline of the running
    /// agreements: the one place it is read.
    pub fn deadline(&self) -> Option<Instant> {
        self.banks
            .iter()
            .filter_map(|bank| bank.active()?.deadline())
            .min()
    }

    /// The slots [`Self::slots_home`] gave, back in their banks, each free.
    pub fn slots_back(&mut self, slots: [T::Slot; BANKS]) {
        for (bank, slot) in self.banks.iter_mut().zip(slots) {
            *bank = Bank::Free(slot);
        }
    }
}

/// A bank is not free: an agreement runs or a command of one is in the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BanksBusy;

/// The loans a service carries, including through stop and restart.
pub struct RxBlockAckView<'a, T: RxBlockAckTypes, E, const WINDOW: usize> {
    header: &'a mut Header<E, WINDOW>,
    banks: &'a mut [Bank<T, WINDOW>],
    frames: &'a mut [Option<T::Frame>],
}

/// Where the reply [`RxBlockAckView::next_reply`] returned waits: a TID's
/// bank, or the queue of refusals and DELBAs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyPlace {
    Queued,
    Bank(usize),
}

/// One reply the association owes: an ADDBA Response or a DELBA body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckReplyBody<E> {
    pub place: ReplyPlace,
    pub association: E,
    pub bssid: Bssid,
    pub body: [u8; ADDBA_ACTION_BODY_LEN],
}

/// What becomes of one received MPDU.
pub enum RxDisposition<T: RxBlockAckTypes> {
    /// Deliver it now, in order.
    Deliver(T::Frame),
    /// The reorder retains it.
    Retained,
    /// Give its owner back without delivery.
    Recycle(T::Frame),
}

/// No reply place or bank remains for an admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckCapacity;

/// Why [`RxBlockAckView::offer`] did not take an Action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckRefused {
    /// No reply place or bank remains for an admitted request.
    Capacity(RxBlockAckCapacity),
}

impl<T: RxBlockAckTypes, E: Copy + Eq, const WINDOW: usize> RxBlockAckView<'_, T, E, WINDOW> {
    fn queue(&mut self, response: [u8; ADDBA_ACTION_BODY_LEN]) -> bool {
        self.header.pending.push_back(response).is_ok()
    }

    /// Whether a management Action can be handled now: its reply has a place.
    pub fn can_receive_management(&self) -> bool {
        !self.header.pending.is_full()
    }

    /// A release overflowed its delivery order: the service must stop.
    pub fn failed(&self) -> bool {
        self.header.failed
    }

    /// Report one reorder decision and queue what it released.
    fn released<const N: usize>(
        &mut self,
        release: RxReorderRelease<N>,
        tid: Tid,
        cause: ReorderCause,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        report(events, || RxBlockAckEvent::Reordered {
            tid,
            cause,
            held: u8::from(release.received == Received::Buffered),
            released: u8::try_from(release.iter().count()).unwrap_or(u8::MAX),
            missing: release.missing,
        });
        self.release(release);
    }

    fn release<const N: usize>(&mut self, release: RxReorderRelease<N>) {
        for frame in release.iter() {
            if let Some(place) = self
                .header
                .ready
                .get_mut(usize::from(self.header.ready_len))
            {
                *place = frame.slot;
                self.header.ready_len = self.header.ready_len.saturating_add(1);
            } else {
                // Every released slot still owns its frame in `frames`.
                // A capacity violation poisons the service rather than
                // losing an owner or delivering outside sequence order.
                self.header.failed = true;
            }
        }
    }

    /// The next frame released in sequence order.
    pub fn pop_ready(&mut self) -> Option<T::Frame> {
        let (first, rest) = self.header.ready.split_first_mut()?;
        if self.header.ready_len == 0 {
            return None;
        }
        let slot = usize::from(*first);
        let mut previous = first;
        for next in rest {
            *previous = *next;
            previous = next;
        }
        self.header.ready_len = self.header.ready_len.saturating_sub(1);
        self.frames.get_mut(slot)?.take()
    }

    /// Whether a frame waits in delivery order ([`Self::pop_ready`]).
    pub fn has_ready(&self) -> bool {
        self.header.ready_len != 0
    }

    /// The frames the agreements retain: in a reorder window or in delivery
    /// order.
    pub fn retained(&self) -> usize {
        self.frames.iter().filter(|frame| frame.is_some()).count()
    }

    /// End this role's agreements: each running one waits to be removed
    /// ([`Self::next_command`]), one not yet added frees its slot, and every
    /// retained frame goes to `released`, undelivered.
    pub fn clear(
        &mut self,
        events: &mut impl EventSink<RxBlockAckEvent>,
        mut released: impl FnMut(T::Frame),
    ) {
        for bank in self.banks.iter_mut() {
            *bank = match core::mem::replace(bank, Bank::InTransition) {
                Bank::Active { session, .. } => {
                    let tid = session.tid();
                    report(events, || RxBlockAckEvent::Stopped {
                        tid,
                        reason: AgreementStop::Clear,
                    });
                    let (running, freed) = session.stop();
                    report(events, || RxBlockAckEvent::Reordered {
                        tid,
                        cause: ReorderCause::Clear,
                        held: 0,
                        released: u8::try_from(freed.iter().count()).unwrap_or(u8::MAX),
                        missing: 0,
                    });
                    Bank::Ending { running, tid }
                }
                Bank::Adding { slot, .. } => Bank::Free(slot),
                other => other,
            };
        }
        for place in self.frames.iter_mut() {
            if let Some(frame) = place.take() {
                released(frame);
            }
        }
        self.header.association = None;
        self.header.pending.clear();
        self.header.ready_len = 0;
        self.header.failed = false;
    }

    /// The association the agreements belong to.
    pub fn association(&self) -> Option<E> {
        self.header.association.map(|(association, _)| association)
    }

    /// The next command a bank owes the port, its bank waiting for its end:
    /// an addition, else a removal.
    pub fn next_command(&mut self) -> Option<BankCommand<T>> {
        let (index, bank) = (0_u8..)
            .zip(self.banks.iter_mut())
            .find(|(_, bank)| matches!(bank, Bank::Adding { .. } | Bank::Ending { .. }))?;
        match core::mem::replace(bank, Bank::InTransition) {
            Bank::Adding {
                slot,
                agreement,
                negotiation,
            } => {
                let ticket = BankTicket {
                    bank: index,
                    tid: negotiation.response().tid,
                };
                *bank = Bank::AddCommand { negotiation };
                Some(BankCommand::Add {
                    ticket,
                    slot,
                    agreement,
                })
            }
            Bank::Ending { running, tid } => {
                *bank = Bank::EndCommand { tid };
                let ticket = BankTicket { bank: index, tid };
                Some(BankCommand::End { ticket, running })
            }
            // Found adding or ending above: this does not happen. Should
            // it, the bank keeps what it was.
            other => {
                *bank = other;
                None
            }
        }
    }

    /// The addition of `bank` ended: its agreement runs, and its success
    /// response is owed. An agreement the session refuses (its geometry)
    /// waits to be removed, and the request is declined.
    pub fn added(
        &mut self,
        ticket: BankTicket,
        running: T::Running,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        let BankTicket { bank, tid } = ticket;
        let Some(place) = self.banks.get_mut(usize::from(bank)) else {
            return;
        };
        let Bank::AddCommand { negotiation } = core::mem::replace(place, Bank::InTransition) else {
            // The ticket was this bank's addition's: this does not happen.
            // Should it, the agreement waits to be removed under the TID
            // its ticket names.
            *place = Bank::Ending { running, tid };
            return;
        };
        let response = negotiation.response();
        match RxBlockAckSession::activate(negotiation, running) {
            Ok(session) => {
                let (tid, window) = (session.tid(), session.window());
                report(events, || RxBlockAckEvent::Started { tid, window });
                *place = Bank::Active {
                    session,
                    reply: Reply::Owed,
                };
            }
            Err((running, _)) => {
                *place = Bank::Ending {
                    running,
                    tid: response.tid,
                };
                let _queued = self.queue_response(AddbaResponse {
                    status: AddbaStatus::RequestDeclined,
                    ..response
                });
            }
        }
    }

    /// `running` waits again to be removed: the end [`Self::next_command`]
    /// gave could not begin.
    pub fn owe_end(&mut self, ticket: BankTicket, running: T::Running) {
        let BankTicket { bank, tid } = ticket;
        if let Some(place) = self.banks.get_mut(usize::from(bank)) {
            *place = Bank::Ending { running, tid };
        }
    }

    /// The command of the ticket's bank ended with its slot empty: an
    /// addition that expired, or a removal.
    pub fn emptied(&mut self, ticket: BankTicket, slot: T::Slot) {
        let BankTicket { bank, .. } = ticket;
        if let Some(place) = self.banks.get_mut(usize::from(bank)) {
            *place = Bank::Free(slot);
        }
    }

    /// Take one Block Ack Action the station validated for `association`
    /// with `bssid`; `agreements` is whether the station admits them. A new
    /// association first ends every agreement of the previous one, its
    /// retained frames to `released`.
    #[inline(never)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the validated action's fields, the caller's event sink and where released frames go"
    )]
    pub fn offer(
        &mut self,
        now: Instant,
        association: E,
        bssid: Bssid,
        action: BlockAckAction,
        agreements: RxAgreements,
        events: &mut impl EventSink<RxBlockAckEvent>,
        released: impl FnMut(T::Frame),
    ) -> Result<(), RxBlockAckRefused> {
        if self.association() != Some(association) {
            self.clear(events, released);
            self.header.association = Some((association, bssid));
        }
        match action {
            BlockAckAction::Delba { .. } => {
                let found = self.banks.iter().position(|bank| {
                    bank.active()
                        .is_some_and(|active| active.peer_delba(action))
                });
                if let Some(index) = found {
                    self.stop(index, AgreementStop::Peer, events);
                }
                return Ok(());
            }
            BlockAckAction::AddbaResponse { .. } => return Ok(()),
            BlockAckAction::AddbaRequest { .. } => {}
        }
        let Some(parsed) = RxAddbaRequest::from_action(action) else {
            return Ok(());
        };
        let answered = self.request(now, bssid, parsed, agreements, events);
        report(events, || RxBlockAckEvent::Requested {
            tid: parsed.tid(),
            requested: parsed.window(),
            outcome: match answered {
                Ok(outcome) => outcome,
                Err(RxBlockAckCapacity) => AddbaOutcome::Unanswered,
            },
        });
        answered.map(|_| ()).map_err(RxBlockAckRefused::Capacity)
    }

    /// Answer one ADDBA Request: start, repeat or refuse its agreement. A
    /// running agreement of the TID asked anew is ended first, and the
    /// request goes unanswered until its bank is free: the peer asks again.
    fn request(
        &mut self,
        now: Instant,
        bssid: Bssid,
        parsed: RxAddbaRequest,
        agreements: RxAgreements,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> Result<AddbaOutcome, RxBlockAckCapacity> {
        if let Some((index, response)) = self.banks.iter().enumerate().find_map(|(index, bank)| {
            bank.active()?
                .repeated_request(parsed)
                .map(|response| (index, response))
        }) {
            if response.status == AddbaStatus::Success {
                if let Some(Bank::Active { reply, .. }) = self.banks.get_mut(index) {
                    *reply = Reply::Owed;
                }
                return Ok(AddbaOutcome::Repeated {
                    window: response.window,
                });
            }
            return self.queue_response(response);
        }
        // A TID has at most one bank that is not free: a request for a TID
        // being added is that agreement's own (its response follows the
        // addition); one being ended, or running and asked anew, is
        // unanswered until its bank is free.
        let tid = parsed.tid();
        if let Some(existing) = self.banks.iter().position(|bank| bank.tid() == Some(tid)) {
            {
                return match self.banks.get(existing) {
                    Some(Bank::Adding { negotiation, .. } | Bank::AddCommand { negotiation }) => {
                        Ok(AddbaOutcome::Repeated {
                            window: negotiation.response().window,
                        })
                    }
                    Some(Bank::Active { .. }) => {
                        self.stop(existing, AgreementStop::Replaced, events);
                        Err(RxBlockAckCapacity)
                    }
                    _ => Err(RxBlockAckCapacity),
                };
            }
        }
        let available = self
            .banks
            .iter()
            .position(|bank| matches!(bank, Bank::Free(_)));
        // A bank whose command runs or waits frees itself: the request goes
        // unanswered until then, and the peer asks again.
        let freeing = self.banks.iter().any(|bank| {
            matches!(
                bank,
                Bank::Adding { .. }
                    | Bank::AddCommand { .. }
                    | Bank::Ending { .. }
                    | Bank::EndCommand { .. }
            )
        });
        if available.is_none() && freeing && agreements == RxAgreements::Admitted {
            return Err(RxBlockAckCapacity);
        }
        let reserved: usize = self
            .banks
            .iter()
            .filter_map(Bank::active)
            .map(|active| usize::from(active.window()).saturating_sub(1))
            .sum();
        // At most the window, which is at most the capabilities' (`new`).
        let capacity = self.frames.len().saturating_sub(reserved).min(WINDOW);
        let capacity = u16::try_from(capacity).unwrap_or(u16::MAX);
        let negotiation = match RxBlockAckNegotiation::request(
            parsed,
            T::Caps::CAPS.rx_block_ack_max_tid,
            if T::Caps::CAPS.rx_amsdu_in_ampdu {
                AmsduInAmpdu::Delivered
            } else {
                AmsduInAmpdu::NotDelivered
            },
            capacity,
            match (available, agreements) {
                (Some(_), RxAgreements::Admitted) => BankRoom::Free,
                _ => BankRoom::Exhausted,
            },
            now,
        ) {
            Ok(negotiation) => negotiation,
            Err(response) => return self.queue_response(response),
        };
        let response = negotiation.response();
        let Some(agreement) = SupportedBlockAck::<T::Caps>::new(
            bssid.individual(),
            response.tid,
            negotiation.starting_sequence(),
            response.window,
        ) else {
            return self.queue_response(AddbaResponse {
                status: AddbaStatus::RequestDeclined,
                ..response
            });
        };
        let Some(bank) = available.and_then(|index| self.banks.get_mut(index)) else {
            return Err(RxBlockAckCapacity);
        };
        *bank = match core::mem::replace(bank, Bank::InTransition) {
            Bank::Free(slot) => Bank::Adding {
                slot,
                agreement,
                negotiation,
            },
            // Found free above: this does not happen. Should it, the bank
            // keeps what it was.
            other => {
                *bank = other;
                return Err(RxBlockAckCapacity);
            }
        };
        Ok(AddbaOutcome::Accepted {
            window: response.window,
        })
    }

    fn queue_response(
        &mut self,
        response: AddbaResponse,
    ) -> Result<AddbaOutcome, RxBlockAckCapacity> {
        match response.encode().filter(|body| self.queue(*body)) {
            Some(_) => Ok(AddbaOutcome::Refused(response.status)),
            None => Err(RxBlockAckCapacity),
        }
    }

    /// End the running agreement of `index`: what it retained is released
    /// in order, and the agreement waits to be removed.
    fn stop(
        &mut self,
        index: usize,
        reason: AgreementStop,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        let Some(bank) = self.banks.get_mut(index) else {
            return;
        };
        let Bank::Active { session, .. } = core::mem::replace(bank, Bank::InTransition) else {
            return;
        };
        let tid = session.tid();
        report(events, || RxBlockAckEvent::Stopped { tid, reason });
        let (running, released) = session.stop();
        *bank = Bank::Ending { running, tid };
        self.released(released, tid, ReorderCause::Stop, events);
    }

    /// The next reply the association owes, until [`Self::sent`] retires it.
    pub fn next_reply(&self) -> Option<RxBlockAckReplyBody<E>> {
        let (association, bssid) = self.header.association?;
        if let Some(body) = self.header.pending.front().copied() {
            return Some(RxBlockAckReplyBody {
                place: ReplyPlace::Queued,
                association,
                bssid,
                body,
            });
        }
        self.banks.iter().enumerate().find_map(|(index, bank)| {
            let Bank::Active {
                session,
                reply: Reply::Owed,
            } = bank
            else {
                return None;
            };
            let body = session.response().encode()?;
            Some(RxBlockAckReplyBody {
                place: ReplyPlace::Bank(index),
                association,
                bssid,
                body,
            })
        })
    }

    /// The reply at `place` left, or will never be owed again.
    pub fn sent(&mut self, place: ReplyPlace) {
        match place {
            ReplyPlace::Queued => {
                let _sent = self.header.pending.pop_front();
            }
            ReplyPlace::Bank(index) => {
                if let Some(Bank::Active { reply, .. }) = self.banks.get_mut(index) {
                    *reply = Reply::Sent;
                }
            }
        }
    }

    /// Expire inactive agreements, each with a DELBA, and reorder gaps.
    #[inline(never)]
    pub fn tick(&mut self, now: Instant, events: &mut impl EventSink<RxBlockAckEvent>) {
        for index in 0..self.banks.len() {
            let Some(active) = self.banks.get(index).and_then(Bank::active) else {
                continue;
            };
            if active.expired(now) {
                if !self.can_receive_management() {
                    continue;
                }
                let tid = active.tid();
                self.stop(index, AgreementStop::Timeout, events);
                let delba = encode_delba(
                    tid,
                    oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Recipient,
                    ReasonCode::TIMEOUT,
                );
                // The reply queue holds ADDBA-sized bodies; the DELBA fills
                // its prefix.
                let mut body = [0; ADDBA_ACTION_BODY_LEN];
                for (target, source) in body.iter_mut().zip(delba) {
                    *target = source;
                }
                let _queued = self.queue(body);
            } else if let Some(active) = self.banks.get_mut(index).and_then(Bank::active_mut) {
                match active.expire_gap(now, self.header.gap_timeout) {
                    Ok(Some(release)) => {
                        let tid = active.tid();
                        self.released(release, tid, ReorderCause::Timeout, events);
                    }
                    Ok(None) => {}
                    Err(_) => self.stop(index, AgreementStop::InvalidGap, events),
                }
            }
        }
    }

    /// Take one received data MPDU or BlockAckReq for the station whose own
    /// address is `local` (`None` before it has a link); `room` says whether
    /// a frame that would be retained has a place. The metadata is read for
    /// the length of the call.
    #[inline(never)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the frame's owner, view and metadata, the caller's room and event sink"
    )]
    pub fn receive<M: RxMetadata>(
        &mut self,
        now: Instant,
        local: Option<Eui48>,
        frame: T::Frame,
        view: RxView<'_>,
        meta: &M,
        room: RetainRoom,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<T> {
        report(events, || {
            let common = meta.common();
            RxBlockAckEvent::Contained {
                aggregation: common.aggregation,
                bytes: u32::try_from(view.bytes.len()).ok(),
            }
        });
        match local {
            Some(local) => self.ingest(now, local, frame, view, meta, room, events),
            None => RxDisposition::Deliver(frame),
        }
    }

    /// The bank and the sequence a QoS Data MPDU of the association is
    /// reordered at, from its header: `None` for any other frame.
    fn reorder_key<M: RxMetadata>(
        &self,
        header: &RxHeader,
        local: Eui48,
        meta: &M,
    ) -> Option<(usize, SequenceNumber)> {
        let (_, bssid) = self.header.association?;
        let control = header.control;
        let fragment = header.sequence_control.fragment();
        // QoS Data subtypes 8 to 11 from the DS (IEEE Std 802.11-2020,
        // 9.2.4.1.3, Table 9-1).
        if !control.is_data()
            || control.ds() != 2
            || control.more_fragments()
            || fragment != 0
            || header.transmitter != bssid.get()
            || header.receiver != local
            || !matches!(control.subtype(), 8..=11)
            || (control.protected()
                && !matches!(
                    meta.common().crypto,
                    RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
                        | RxEvidence::ProtocolValidated(
                            RxCryptoStatus::DecryptedAndIntegrityVerified
                        )
                ))
        {
            return None;
        }
        let tid = header.qos?.tid();
        let index = self
            .banks
            .iter()
            .position(|bank| bank.active().is_some_and(|active| active.tid() == tid))?;
        Some((
            index,
            SequenceNumber::from_sequence_control(header.sequence_control),
        ))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the frame's owner, view and metadata, the caller's room and event sink"
    )]
    fn ingest<M: RxMetadata>(
        &mut self,
        now: Instant,
        local: Eui48,
        frame: T::Frame,
        view: RxView<'_>,
        meta: &M,
        room: RetainRoom,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<T> {
        if let Some((tid, sequence)) = self.bar(view.bytes, local) {
            if let Some(active) = self
                .banks
                .iter_mut()
                .find_map(|bank| bank.active_mut().filter(|active| active.tid() == tid))
                && let Ok(Some(release)) =
                    active.block_ack_request(now, sequence, self.header.gap_timeout)
            {
                self.released(release, tid, ReorderCause::Bar, events);
            }
            return RxDisposition::Recycle(frame);
        }
        let key = match view.head {
            RxHead::Frame(header) => self.reorder_key(header, local, meta),
            RxHead::Control(_) => None,
        };
        let Some((bank_index, sequence)) = key else {
            return RxDisposition::Deliver(frame);
        };
        let Some(active) = self.banks.get_mut(bank_index).and_then(Bank::active_mut) else {
            return RxDisposition::Deliver(frame);
        };
        // A frame that would be retained needs a place beyond the one the
        // caller keeps for the frame that closes the hole, and a frame
        // place here; else it goes back.
        let free = self.frames.iter().position(Option::is_none);
        let retains = active.retains(sequence).unwrap_or(false);
        let (slot, slot_id) = match (
            free.and_then(|slot| Some((slot, u8::try_from(slot).ok()?))),
            room,
        ) {
            (Some(found), RetainRoom::Free) => found,
            (Some(found), RetainRoom::Full) if !retains => found,
            _ => {
                let tid = active.tid();
                report(events, || RxBlockAckEvent::Reordered {
                    tid,
                    cause: ReorderCause::Full,
                    held: 0,
                    released: 0,
                    missing: 0,
                });
                return RxDisposition::Recycle(frame);
            }
        };
        // The common in-order MPDU, with no frame released ahead of it,
        // goes out at once: it takes no slot and no place in delivery
        // order. Any other MPDU, or a refusal, takes the full ingest.
        if self.header.ready_len == 0
            && let Ok(true) = active.try_ingest_immediate(
                now,
                RxReorderMpdu {
                    sequence,
                    slot: slot_id,
                },
            )
        {
            let tid = active.tid();
            report(events, || RxBlockAckEvent::Reordered {
                tid,
                cause: ReorderCause::Receive,
                held: 0,
                released: 1,
                missing: 0,
            });
            return RxDisposition::Deliver(frame);
        }
        let release = active.ingest(
            now,
            RxReorderMpdu {
                sequence,
                slot: slot_id,
            },
            self.header.gap_timeout,
        );
        let tid = active.tid();
        match release {
            Ok(release) if !matches!(release.received, Received::Rejected(_)) => {
                if let Some(place) = self.frames.get_mut(slot) {
                    *place = Some(frame);
                }
                self.released(release, tid, ReorderCause::Receive, events);
                RxDisposition::Retained
            }
            other => {
                let cause = if matches!(
                    other,
                    Err(super::RxBlockAckError::Reorder(
                        super::RxReorderError::DuplicateSequence(_)
                    ))
                ) {
                    ReorderCause::Duplicate
                } else {
                    ReorderCause::Rejected
                };
                report(events, || RxBlockAckEvent::Reordered {
                    tid,
                    cause,
                    held: 0,
                    released: 0,
                    missing: 0,
                });
                RxDisposition::Recycle(frame)
            }
        }
    }

    fn bar(&self, bytes: &[u8], local: Eui48) -> Option<(Tid, SequenceNumber)> {
        let (_, bssid) = self.header.association?;
        let request = parse_compressed_block_ack_request(bytes)?;
        if request.receiver != local || request.transmitter != bssid.get() {
            return None;
        }
        Some((request.tid, request.starting_sequence))
    }
}

#[cfg(test)]
mod tests;
