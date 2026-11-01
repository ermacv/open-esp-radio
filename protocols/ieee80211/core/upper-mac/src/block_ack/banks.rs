//! The receive Block Ack agreements of one association: ADDBA negotiation,
//! reorder, BlockAckReq, inactivity, DELBA and the replies they owe.
//!
//! The owner holds the integration's affine bank slots and retained-frame
//! owners and decides every agreement edge. It never touches a port itself:
//! each bank and frame transition is one call of the caller's
//! [`RxBlockAckPort`], which carries it out. Retained frames are the port's
//! receive owners; their payloads are never copied. A receive credit stays
//! free to close a reorder hole.

use oer_events::{EventSink, report};

use super::{
    AddbaResponse, AddbaStatus, RxAddbaRequest, RxBlockAckNegotiation, RxBlockAckSession,
    RxReorderMpdu, RxReorderRelease,
};
use oer_ieee80211_contracts_port::{
    Ieee80211Capabilities, RxBuffer, RxCryptoStatus, RxEvidence, RxMetadata, SupportedBlockAck, Tid,
};
use oer_ieee80211_wire::{
    address::Bssid,
    block_ack::frame::{
        ADDBA_ACTION_BODY_LEN, BlockAckAction, encode_delba, parse_compressed_block_ack_request,
    },
    frame::{DecodedHeader, QosControl},
    sequence::{SequenceControl, SequenceNumber},
    status::reason_code,
};
use oer_time::{Duration, Instant};

/// What the receive agreements decided, reported to the sink the caller lends
/// a call. The owner keeps no count of these.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBlockAckEvent {
    /// A received MPDU's aggregation evidence, for each admitted MPDU, never
    /// a physical aggregate or member count. `bytes` is the port's MPDU
    /// length, `None` when it does not fit.
    Contained {
        ampdu: RxEvidence<bool>,
        s_mpdu: RxEvidence<bool>,
        bytes: Option<u32>,
    },
    /// One reorder decision of the agreement for `tid`. `released` counts
    /// MPDUs the reorder owner released, not application deliveries;
    /// `missing` counts skipped sequences, not dropped buffered MPDUs.
    Reordered {
        tid: u8,
        cause: ReorderCause,
        held: u8,
        released: u8,
        missing: u16,
    },
    /// An agreement became active: the logical owner, not a confirmed
    /// hardware register write.
    Started { tid: u8, window: u16 },
    /// An agreement ended.
    Stopped { tid: u8, reason: AgreementStop },
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

/// The most frames one agreement retains, and the most in-order releases
/// waiting for delivery: a 16-MPDU window. The image chooses the number of
/// banks and the retained frames independently, at most this many.
pub const RX_BLOCK_ACK_WINDOW: usize = 16;
const WINDOW: usize = RX_BLOCK_ACK_WINDOW;
/// Encoded replies waiting for a control transmit credit.
const PENDING: usize = 8;
/// The Action subtype of a non-data frame.
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1 (Action is subtype 13).
const ACTION_SUBTYPE: u16 = 13;

/// The owners an integration lends the receive agreements.
pub trait RxBlockAckTypes {
    /// A free receive agreement slot of the port.
    type Slot;
    /// An agreement installed in a slot.
    type Running;
    /// A received MPDU's owner.
    type Frame: RxBuffer;
    /// Its receive metadata.
    type Meta: RxMetadata;
}

/// The port calls that carry out the owner's decisions, in the order made.
pub trait RxBlockAckPort<T: RxBlockAckTypes> {
    /// The capabilities that bound an agreement's TID and window.
    type Caps: Ieee80211Capabilities;
    /// Install `agreement` in `slot`.
    fn add(&mut self, slot: T::Slot, agreement: SupportedBlockAck<Self::Caps>) -> T::Running;
    /// End `running`, returning its slot.
    fn remove(&mut self, running: T::Running) -> T::Slot;
    /// Return a retained frame's receive owner.
    fn recycle(&mut self, frame: T::Frame);
}

type Session<T> = RxBlockAckSession<<T as RxBlockAckTypes>::Running, WINDOW, WINDOW>;
type Retained<T> = (<T as RxBlockAckTypes>::Frame, <T as RxBlockAckTypes>::Meta);

struct Bank<T: RxBlockAckTypes> {
    free: Option<T::Slot>,
    active: Option<Session<T>>,
    response: bool,
}

struct Header<E> {
    association: Option<(E, Bssid)>,
    pending: heapless::Deque<[u8; ADDBA_ACTION_BODY_LEN], PENDING>,
    ready: [u8; WINDOW],
    ready_len: u8,
    // Slot reuse must not reorder management CCMP packet numbers.
    deferred: heapless::Deque<u8, WINDOW>,
    failed: bool,
    gap_timeout: Duration,
}

/// Placed backing selected by an image: `BANKS` agreement slots and `FRAMES`
/// retained-frame owners of association identity `E`. Frame storage contains
/// owners and metadata, never copies of MPDU payloads.
pub struct RxBlockAckStorage<T: RxBlockAckTypes, E, const BANKS: usize, const FRAMES: usize> {
    header: Header<E>,
    banks: [Bank<T>; BANKS],
    frames: [Option<Retained<T>>; FRAMES],
}

impl<T: RxBlockAckTypes, E: Copy + Eq, const BANKS: usize, const FRAMES: usize>
    RxBlockAckStorage<T, E, BANKS, FRAMES>
{
    pub fn new(slots: [T::Slot; BANKS], gap_timeout: Duration) -> Self {
        const {
            if FRAMES == 0 || FRAMES > WINDOW {
                panic!("RX storage must reserve one to RX_BLOCK_ACK_WINDOW existing frame credits");
            }
        }
        Self {
            header: Header {
                association: None,
                pending: heapless::Deque::new(),
                ready: [0; WINDOW],
                ready_len: 0,
                deferred: heapless::Deque::new(),
                failed: false,
                gap_timeout,
            },
            banks: slots.map(|slot| Bank {
                free: Some(slot),
                active: None,
                response: false,
            }),
            frames: [const { None }; FRAMES],
        }
    }

    pub fn view(&mut self) -> RxBlockAckView<'_, T, E> {
        RxBlockAckView {
            header: &mut self.header,
            banks: &mut self.banks,
            frames: &mut self.frames,
        }
    }

    /// Construct directly in the image's placed storage. The complete owner
    /// is bounded by the image's explicit memory budget; no retained payload
    /// allocation is constructed here.
    pub fn init_in_place(
        storage: &mut core::mem::MaybeUninit<Self>,
        slots: [T::Slot; BANKS],
        gap_timeout: Duration,
    ) -> &mut Self {
        storage.write(Self::new(slots, gap_timeout))
    }
}

/// The loans a service carries, including through stop and restart.
pub struct RxBlockAckView<'a, T: RxBlockAckTypes, E> {
    header: &'a mut Header<E>,
    banks: &'a mut [Bank<T>],
    frames: &'a mut [Option<Retained<T>>],
}

/// Where the reply [`RxBlockAckView::next_reply`] returned waits.
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
    Deliver(Retained<T>),
    /// The owner retains it: reordering, or a management Action deferred
    /// until a reply place is free.
    Retained,
    /// Return its receive owner without delivery.
    Recycle(T::Frame),
    /// No retained-frame credit remains: a capacity fault.
    Capacity(T::Frame),
}

/// What [`RxBlockAckView::park`] did with a frame its consumer had no room
/// for.
pub enum Parked<T: RxBlockAckTypes> {
    /// Held: it leaves through [`RxBlockAckView::pop_ready`] after every
    /// frame queued before it.
    Waiting,
    /// Not a data frame: deliver it now.
    Deliver(Retained<T>),
    /// No retained-frame credit remains: a capacity fault.
    Capacity(T::Frame),
}

/// No reply place or bank remains for an admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBlockAckCapacity;

impl<T: RxBlockAckTypes, E: Copy + Eq> RxBlockAckView<'_, T, E> {
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
        tid: u8,
        cause: ReorderCause,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        report(events, || RxBlockAckEvent::Reordered {
            tid,
            cause,
            held: u8::from(release.buffered),
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
    pub fn pop_ready(&mut self) -> Option<Retained<T>> {
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

    /// Hold one data MPDU released for delivery while its consumer has no
    /// room: it waits behind the frames already in delivery order. A
    /// management or control frame is not held; the caller delivers it.
    pub fn park(&mut self, frame: T::Frame, meta: T::Meta) -> Parked<T> {
        let data =
            DecodedHeader::decode(frame.bytes()).is_some_and(|header| header.control().is_data());
        if !data {
            return Parked::Deliver((frame, meta));
        }
        let next = usize::from(self.header.ready_len);
        let Some(slot) = self.frames.iter().position(Option::is_none) else {
            return Parked::Capacity(frame);
        };
        let (Ok(id), true) = (u8::try_from(slot), next < self.header.ready.len()) else {
            return Parked::Capacity(frame);
        };
        let (Some(place), Some(stored)) =
            (self.header.ready.get_mut(next), self.frames.get_mut(slot))
        else {
            return Parked::Capacity(frame);
        };
        *place = id;
        *stored = Some((frame, meta));
        self.header.ready_len = self.header.ready_len.saturating_add(1);
        Parked::Waiting
    }

    fn defer(&mut self, frame: T::Frame, meta: T::Meta) -> Result<(), Retained<T>> {
        let Some((slot, place)) = self
            .frames
            .iter_mut()
            .enumerate()
            .find(|(_, place)| place.is_none())
        else {
            return Err((frame, meta));
        };
        let Ok(slot) = u8::try_from(slot) else {
            return Err((frame, meta));
        };
        if self.header.deferred.push_back(slot).is_err() {
            return Err((frame, meta));
        }
        *place = Some((frame, meta));
        Ok(())
    }

    /// A deferred management frame, once its reply has a place.
    pub fn pop_deferred(&mut self) -> Option<Retained<T>> {
        if !self.can_receive_management() {
            return None;
        }
        let slot = self.header.deferred.pop_front()?;
        self.frames.get_mut(usize::from(slot))?.take()
    }

    /// The earliest inactivity or reorder-gap deadline.
    pub fn deadline(&self) -> Option<Instant> {
        self.banks
            .iter()
            .filter_map(|bank| bank.active.as_ref()?.deadline())
            .min()
    }

    /// End only this role's agreements; every bank and retained frame returns
    /// through the same port before a healthy Stop; temporary absences keep them.
    pub fn clear(
        &mut self,
        port: &mut impl RxBlockAckPort<T>,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        for bank in self.banks.iter_mut() {
            if let Some(active) = bank.active.take() {
                let tid = active.tid();
                report(events, || RxBlockAckEvent::Stopped {
                    tid,
                    reason: AgreementStop::Clear,
                });
                let (running, released) = active.stop();
                report(events, || RxBlockAckEvent::Reordered {
                    tid,
                    cause: ReorderCause::Clear,
                    held: 0,
                    released: u8::try_from(released.iter().count()).unwrap_or(u8::MAX),
                    missing: 0,
                });
                bank.free = Some(port.remove(running));
            }
            bank.response = false;
        }
        for frame in self.frames.iter_mut().filter_map(Option::take) {
            port.recycle(frame.0);
        }
        self.header.association = None;
        self.header.pending.clear();
        self.header.ready_len = 0;
        self.header.deferred.clear();
        self.header.failed = false;
    }

    /// The association the agreements belong to.
    pub fn association(&self) -> Option<E> {
        self.header.association.map(|(association, _)| association)
    }

    /// Take one Block Ack Action the station validated for `association`
    /// with `bssid`; `supported` is whether the station admits agreements.
    /// A new association first ends every agreement of the previous one.
    #[inline(never)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the validated action's fields, the port and the caller's event sink"
    )]
    pub fn offer<P: RxBlockAckPort<T>>(
        &mut self,
        now: Instant,
        association: E,
        bssid: Bssid,
        action: BlockAckAction,
        supported: bool,
        port: &mut P,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> Result<(), RxBlockAckCapacity> {
        if self.association() != Some(association) {
            self.clear(port, events);
            self.header.association = Some((association, bssid));
        }
        match action {
            BlockAckAction::Delba { .. } => {
                let found = self.banks.iter().position(|bank| {
                    bank.active
                        .as_ref()
                        .is_some_and(|active| active.peer_delba(action))
                });
                if let Some(index) = found {
                    self.stop(index, port, AgreementStop::Peer, events);
                }
                return Ok(());
            }
            BlockAckAction::AddbaResponse { .. } => return Ok(()),
            BlockAckAction::AddbaRequest { .. } => {}
        }
        let Some(parsed) = RxAddbaRequest::from_action(action) else {
            return Ok(());
        };
        if let Some((index, response)) = self.banks.iter().enumerate().find_map(|(index, bank)| {
            bank.active
                .as_ref()?
                .repeated_request(parsed)
                .map(|response| (index, response))
        }) {
            if response.status == AddbaStatus::Success {
                if let Some(bank) = self.banks.get_mut(index) {
                    bank.response = true;
                }
                return Ok(());
            }
            return self.queue_response(response);
        }
        let BlockAckAction::AddbaRequest { tid, .. } = action else {
            return Ok(());
        };
        let existing = self.banks.iter().position(|bank| {
            bank.active
                .as_ref()
                .is_some_and(|active| active.tid() == tid)
        });
        let available = existing.or_else(|| self.banks.iter().position(|bank| bank.free.is_some()));
        let reserved: usize = self
            .banks
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != existing)
            .filter_map(|(_, bank)| bank.active.as_ref())
            .map(|active| usize::from(active.window()).saturating_sub(1))
            .sum();
        let capacity = self.frames.len().saturating_sub(reserved).min(WINDOW);
        let capacity = u16::try_from(capacity)
            .unwrap_or(u16::MAX)
            .min(P::Caps::CAPS.rx_block_ack_max_window);
        let negotiation = match RxBlockAckNegotiation::request(
            parsed,
            P::Caps::CAPS.rx_block_ack_max_tid,
            P::Caps::CAPS.rx_amsdu_in_ampdu,
            capacity,
            available.is_some() && supported,
            now,
        ) {
            Ok(negotiation) => negotiation,
            Err(response) => return self.queue_response(response),
        };
        let response = negotiation.response();
        let Some(agreement) = Tid::new(response.tid).and_then(|tid| {
            SupportedBlockAck::<P::Caps>::new(
                bssid.individual(),
                tid,
                negotiation.starting_sequence(),
                response.window,
            )
        }) else {
            return self.queue_response(AddbaResponse {
                status: AddbaStatus::RequestDeclined,
                ..response
            });
        };
        let Some(index) = available else {
            return Err(RxBlockAckCapacity);
        };
        if existing.is_some() {
            self.stop(index, port, AgreementStop::Replaced, events);
        }
        let Some(bank) = self.banks.get_mut(index) else {
            return Err(RxBlockAckCapacity);
        };
        let Some(slot) = bank.free.take() else {
            return Err(RxBlockAckCapacity);
        };
        let running = port.add(slot, agreement);
        match RxBlockAckSession::activate(negotiation, running) {
            Ok(active) => {
                let (tid, window) = (active.tid(), active.window());
                report(events, || RxBlockAckEvent::Started { tid, window });
                bank.active = Some(active);
                bank.response = true;
                Ok(())
            }
            Err((running, _)) => {
                bank.free = Some(port.remove(running));
                self.queue_response(AddbaResponse {
                    status: AddbaStatus::RequestDeclined,
                    ..response
                })
            }
        }
    }

    fn queue_response(&mut self, response: AddbaResponse) -> Result<(), RxBlockAckCapacity> {
        match response.encode().filter(|body| self.queue(*body)) {
            Some(_) => Ok(()),
            None => Err(RxBlockAckCapacity),
        }
    }

    fn stop(
        &mut self,
        index: usize,
        port: &mut impl RxBlockAckPort<T>,
        reason: AgreementStop,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        if let Some(bank) = self.banks.get_mut(index) {
            bank.response = false;
            if let Some(active) = bank.active.take() {
                let tid = active.tid();
                report(events, || RxBlockAckEvent::Stopped { tid, reason });
                let (running, released) = active.stop();
                bank.free = Some(port.remove(running));
                self.released(released, tid, ReorderCause::Stop, events);
            }
        }
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
            if !bank.response {
                return None;
            }
            let body = bank.active.as_ref()?.response().encode()?;
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
                if let Some(bank) = self.banks.get_mut(index) {
                    bank.response = false;
                }
            }
        }
    }

    /// Expire inactive agreements, each with a DELBA, and reorder gaps.
    #[inline(never)]
    pub fn tick(
        &mut self,
        now: Instant,
        port: &mut impl RxBlockAckPort<T>,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) {
        for index in 0..self.banks.len() {
            let Some(active) = self.banks.get(index).and_then(|bank| bank.active.as_ref()) else {
                continue;
            };
            if active.expired(now) {
                if !self.can_receive_management() {
                    continue;
                }
                let tid = active.tid();
                self.stop(index, port, AgreementStop::Timeout, events);
                if let Ok(delba) = encode_delba(tid, false, reason_code::TIMEOUT) {
                    // The reply queue holds ADDBA-sized bodies; the DELBA
                    // fills its prefix.
                    let mut body = [0; ADDBA_ACTION_BODY_LEN];
                    for (target, source) in body.iter_mut().zip(delba) {
                        *target = source;
                    }
                    let _queued = self.queue(body);
                }
            } else if let Some(active) = self
                .banks
                .get_mut(index)
                .and_then(|bank| bank.active.as_mut())
            {
                match active.expire_gap(now, self.header.gap_timeout) {
                    Ok(Some(release)) => {
                        let tid = active.tid();
                        self.released(release, tid, ReorderCause::Timeout, events);
                    }
                    Ok(None) => {}
                    Err(_) => self.stop(index, port, AgreementStop::InvalidGap, events),
                }
            }
        }
    }

    /// Take one received MPDU for the station whose own address is `local`
    /// (`None` before it has a link). A management Action whose reply would
    /// find no place waits retained until [`Self::pop_deferred`].
    #[inline(never)]
    pub fn receive(
        &mut self,
        now: Instant,
        local: Option<[u8; 6]>,
        frame: T::Frame,
        meta: T::Meta,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<T> {
        let action = DecodedHeader::decode(frame.bytes()).is_some_and(|header| {
            !header.control().is_data() && header.control().subtype() == ACTION_SUBTYPE
        });
        if action && !self.can_receive_management() {
            return match self.defer(frame, meta) {
                Ok(()) => RxDisposition::Retained,
                Err((frame, _)) => RxDisposition::Capacity(frame),
            };
        }
        report(events, || {
            let common = meta.common();
            RxBlockAckEvent::Contained {
                ampdu: common.ampdu,
                s_mpdu: common.s_mpdu,
                bytes: u32::try_from(frame.bytes().len()).ok(),
            }
        });
        match local {
            Some(local) => self.ingest(now, local, frame, meta, events),
            None => RxDisposition::Deliver((frame, meta)),
        }
    }

    fn ingest(
        &mut self,
        now: Instant,
        local: [u8; 6],
        frame: T::Frame,
        meta: T::Meta,
        events: &mut impl EventSink<RxBlockAckEvent>,
    ) -> RxDisposition<T> {
        if let Some((tid, sequence)) = self.bar(frame.bytes(), local) {
            if let Some(active) = self
                .banks
                .iter_mut()
                .find_map(|bank| bank.active.as_mut().filter(|active| active.tid() == tid))
                && let Ok(Some(release)) =
                    active.block_ack_request(now, sequence, self.header.gap_timeout)
            {
                self.released(release, tid, ReorderCause::Bar, events);
            }
            return RxDisposition::Recycle(frame);
        }
        let key = DecodedHeader::decode(frame.bytes()).and_then(|header| {
            let (_, bssid) = self.header.association?;
            let control = header.control();
            let fragment =
                SequenceControl::from_bytes(header.sequence_control().to_le_bytes()).fragment();
            // QoS Data subtypes 8 to 11 from the DS (IEEE Std 802.11-2020,
            // 9.2.4.1.3, Table 9-1).
            if !control.is_data()
                || control.ds() != 2
                || control.more_fragments()
                || fragment != 0
                || header.transmitter() != bssid.get()
                || header.receiver() != local
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
            let tid = QosControl::from_bytes(header.qos()?.to_le_bytes())
                .tid()
                .get();
            let index = self.banks.iter().position(|bank| {
                bank.active
                    .as_ref()
                    .is_some_and(|active| active.tid() == tid)
            })?;
            Some((
                index,
                SequenceNumber::from_sequence_control(header.sequence_control()),
            ))
        });
        let Some((bank_index, sequence)) = key else {
            return RxDisposition::Deliver((frame, meta));
        };
        let Some(slot) = self.frames.iter().position(Option::is_none) else {
            return RxDisposition::Capacity(frame);
        };
        let Some(active) = self
            .banks
            .get_mut(bank_index)
            .and_then(|bank| bank.active.as_mut())
        else {
            return RxDisposition::Deliver((frame, meta));
        };
        let slot_id = match u8::try_from(slot) {
            Ok(slot) => slot,
            Err(_) => return RxDisposition::Deliver((frame, meta)),
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
            return RxDisposition::Deliver((frame, meta));
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
            Ok(release) if release.rejected.is_none() => {
                if let Some(place) = self.frames.get_mut(slot) {
                    *place = Some((frame, meta));
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

    fn bar(&self, bytes: &[u8], local: [u8; 6]) -> Option<(u8, SequenceNumber)> {
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
