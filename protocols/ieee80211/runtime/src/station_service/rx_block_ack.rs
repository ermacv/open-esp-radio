//! Association-owned RX banks; retained frames borrow the existing port pool.
use super::*;
use oer_ieee80211_port::{
    PhaseBlockAckSlot, RxCryptoStatus, RxEvidence, RxMetadata, SupportedBlockAck, Tid,
};
use oer_ieee80211_sta::{
    block_ack::{
        AddbaResponse, AddbaStatus, RxAddbaRequest, RxBlockAckNegotiation, RxBlockAckSession,
    },
    epoch::LinkEpoch,
    machine::{PendingRxBlockAck, RxBlockAckReply},
};
use oer_ieee80211_upper_mac::block_ack::{RxReorderMpdu, RxReorderRelease};
use oer_ieee80211_wire::{
    address::Bssid,
    block_ack::frame::{BlockAckAction, DelbaParameters},
    frame::{DecodedHeader, FrameControlFields, FrameType},
    sequence::SequenceNumber,
    trigger::BarControlFields,
};
use oer_time::Duration;

// The station's frame/token bank admits at most eight in flight. The image
// chooses the number of BA banks independently through RxBlockAckStorage.
const WINDOW: usize = 8;
const PENDING: usize = 8;

type Session<'id, Ty> = RxBlockAckSession<<Ty as PhaseTypes<'id>>::BlockAck, WINDOW, WINDOW>;
type Retained<'id, Ty> = (
    <Ty as PhaseTypes<'id>>::RxBuffer,
    <Ty as PhaseTypes<'id>>::RxMeta,
);

struct Bank<'id, Ty: PhaseTypes<'id>> {
    free: Option<PhaseBlockAckSlot<'id, Ty>>,
    active: Option<Session<'id, Ty>>,
    response: bool,
}

struct Header<'id, Ty: PhaseTypes<'id>> {
    association: Option<(LinkEpoch<'id, Ty::EpochPath>, Bssid)>,
    pending: heapless::Deque<[u8; 9], PENDING>,
    ready: [u8; WINDOW],
    ready_len: u8,
    // Slot reuse must not reorder management CCMP packet numbers.
    deferred: heapless::Deque<u8, WINDOW>,
    failed: bool,
    gap_timeout: Duration,
}

/// Placed backing selected by an image. Bank tokens come from its existing
/// shared port pool; frame storage contains owners and metadata, never copies
/// of MPDU payloads. A receive credit remains free to close a reorder hole.
pub struct RxBlockAckStorage<'id, Ty: PhaseTypes<'id>, const BANKS: usize, const FRAMES: usize> {
    header: Header<'id, Ty>,
    banks: [Bank<'id, Ty>; BANKS],
    frames: [Option<Retained<'id, Ty>>; FRAMES],
}

impl<'id, Ty: PhaseTypes<'id>, const BANKS: usize, const FRAMES: usize>
    RxBlockAckStorage<'id, Ty, BANKS, FRAMES>
{
    pub fn new(slots: [PhaseBlockAckSlot<'id, Ty>; BANKS], gap_timeout: Duration) -> Self {
        const {
            if FRAMES == 0 || FRAMES > WINDOW {
                panic!("RX storage must reserve one to eight existing frame credits");
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

    pub fn view(&'id mut self) -> RxBlockAckView<'id, Ty> {
        RxBlockAckView {
            header: &mut self.header,
            banks: &mut self.banks,
            frames: &mut self.frames,
        }
    }

    #[cfg(feature = "placement")]
    pub fn init_in_place<'a>(
        storage: &'a mut core::mem::MaybeUninit<Self>,
        slots: [PhaseBlockAckSlot<'id, Ty>; BANKS],
        gap_timeout: Duration,
    ) -> &'a mut Self {
        // The complete owner is bounded by the image's explicit memory
        // budget. No retained payload allocation is constructed here.
        storage.write(Self::new(slots, gap_timeout))
    }
}

/// The service carries only these loans, including through stop and restart.
pub struct RxBlockAckView<'id, Ty: PhaseTypes<'id>> {
    header: &'id mut Header<'id, Ty>,
    banks: &'id mut [Bank<'id, Ty>],
    frames: &'id mut [Option<Retained<'id, Ty>>],
}

#[derive(Clone, Copy)]
pub(super) enum ReplyPlace {
    Queued,
    Bank(usize),
}
pub(super) enum RxDisposition<'id, Ty: PhaseTypes<'id>> {
    Deliver(Retained<'id, Ty>),
    Retained,
    Recycle(Ty::RxBuffer),
    Capacity(Ty::RxBuffer),
}

impl<'id, Ty: PhaseTypes<'id>> RxBlockAckView<'id, Ty> {
    fn queue(&mut self, response: [u8; 9]) -> bool {
        self.header.pending.push_back(response).is_ok()
    }

    pub(super) fn can_receive_management(&self) -> bool {
        !self.header.pending.is_full()
    }

    pub(super) fn failed(&self) -> bool {
        self.header.failed
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

    pub(super) fn pop_ready(&mut self) -> Option<Retained<'id, Ty>> {
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

    pub(super) fn defer(
        &mut self,
        frame: Ty::RxBuffer,
        meta: Ty::RxMeta,
    ) -> Result<(), Retained<'id, Ty>> {
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

    pub(super) fn pop_deferred(&mut self) -> Option<Retained<'id, Ty>> {
        if !self.can_receive_management() {
            return None;
        }
        let slot = self.header.deferred.pop_front()?;
        self.frames.get_mut(usize::from(slot))?.take()
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.banks
            .iter()
            .filter_map(|bank| bank.active.as_ref()?.deadline())
            .min()
    }

    /// Observe retained owners without consuming or reordering them.
    pub(super) fn observe_retained(&self, mut observe: impl FnMut(&Ty::RxBuffer)) {
        for (frame, _) in self.frames.iter().filter_map(Option::as_ref) {
            observe(frame);
        }
    }

    /// End this role's agreements and return every retained owner.
    pub(super) fn clear<'p, K, P>(&mut self, phase: &mut P)
    where
        'id: 'p,
        K: Ieee80211Capabilities,
        P: LowerMacPhase<'p, 'id, Caps = K, Types = Ty>,
    {
        for bank in self.banks.iter_mut() {
            if let Some(active) = bank.active.take() {
                let (running, _released) = active.stop();
                bank.free = Some(phase.station().remove_block_ack(running));
            }
            bank.response = false;
        }
        for frame in self.frames.iter_mut().filter_map(Option::take) {
            phase.recycle(frame.0);
        }
        self.header.association = None;
        self.header.pending.clear();
        self.header.ready_len = 0;
        self.header.deferred.clear();
        self.header.failed = false;
    }

    pub(super) fn association(&self) -> Option<LinkEpoch<'id, Ty::EpochPath>> {
        self.header.association.map(|(epoch, _)| epoch)
    }

    #[inline(never)]
    pub(super) fn offer<'p, K, P>(
        &mut self,
        now: Instant,
        request: PendingRxBlockAck<'id, Ty::EpochPath>,
        phase: &mut P,
    ) -> Result<(), PendingRxBlockAck<'id, Ty::EpochPath>>
    where
        'id: 'p,
        K: Ieee80211Capabilities,
        P: LowerMacPhase<'p, 'id, Caps = K, Types = Ty>,
    {
        if self.association() != Some(request.epoch()) {
            self.clear(phase);
            self.header.association = Some((request.epoch(), request.bssid()));
        }
        let action = request.action();
        match action {
            BlockAckAction::Delba { .. } => {
                let found = self.banks.iter().position(|bank| {
                    bank.active
                        .as_ref()
                        .is_some_and(|active| active.peer_delba(action))
                });
                if let Some(index) = found {
                    self.stop(index, phase);
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
            return match response.encode().filter(|body| self.queue(*body)) {
                Some(_) => Ok(()),
                None => Err(request),
            };
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
        // `capacity` was bounded by WINDOW (eight) before this conversion.
        let capacity = (capacity as u16).min(K::CAPS.rx_block_ack_max_window);
        let negotiation = match RxBlockAckNegotiation::request(
            parsed,
            K::CAPS.rx_block_ack_max_tid,
            capacity,
            available.is_some() && request.supported(),
            now,
        ) {
            Ok(negotiation) => negotiation,
            Err(response) => {
                return match response.encode().filter(|body| self.queue(*body)) {
                    Some(_) => Ok(()),
                    None => Err(request),
                };
            }
        };
        let response = negotiation.response();
        let Some(agreement) = Tid::new(response.tid).and_then(|tid| {
            SupportedBlockAck::<K>::new(
                request.bssid().individual(),
                tid,
                negotiation.starting_sequence(),
                response.window,
            )
        }) else {
            let refusal = AddbaResponse {
                status: AddbaStatus::RequestDeclined,
                ..response
            };
            return match refusal.encode().filter(|body| self.queue(*body)) {
                Some(_) => Ok(()),
                None => Err(request),
            };
        };
        let Some(index) = available else {
            return Err(request);
        };
        if existing.is_some() {
            self.stop(index, phase);
        }
        let Some(bank) = self.banks.get_mut(index) else {
            return Err(request);
        };
        let Some(slot) = bank.free.take() else {
            return Err(request);
        };
        let running = phase.station().add_block_ack(slot, agreement);
        match RxBlockAckSession::activate(negotiation, running) {
            Ok(active) => {
                bank.active = Some(active);
                bank.response = true;
                Ok(())
            }
            Err((running, _)) => {
                bank.free = Some(phase.station().remove_block_ack(running));
                let refusal = AddbaResponse {
                    status: AddbaStatus::RequestDeclined,
                    ..response
                };
                match refusal.encode().filter(|body| self.queue(*body)) {
                    Some(_) => Ok(()),
                    None => Err(request),
                }
            }
        }
    }

    fn stop<'p, K, P>(&mut self, index: usize, phase: &mut P)
    where
        'id: 'p,
        K: Ieee80211Capabilities,
        P: LowerMacPhase<'p, 'id, Caps = K, Types = Ty>,
    {
        if let Some(bank) = self.banks.get_mut(index) {
            bank.response = false;
            if let Some(active) = bank.active.take() {
                let (running, released) = active.stop();
                bank.free = Some(phase.station().remove_block_ack(running));
                self.release(released);
            }
        }
    }

    pub(super) fn next_reply(&self) -> Option<(ReplyPlace, RxBlockAckReply<'id, Ty::EpochPath>)> {
        let (epoch, bssid) = self.header.association?;
        if let Some(body) = self.header.pending.front().copied() {
            return RxBlockAckReply::from_body(epoch, bssid, body)
                .map(|reply| (ReplyPlace::Queued, reply));
        }
        self.banks.iter().enumerate().find_map(|(index, bank)| {
            if !bank.response {
                return None;
            }
            let response = bank.active.as_ref()?.response();
            RxBlockAckReply::for_association(epoch, bssid, response)
                .map(|reply| (ReplyPlace::Bank(index), reply))
        })
    }

    pub(super) fn sent(&mut self, place: ReplyPlace) {
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

    #[inline(never)]
    pub(super) fn tick<'p, K, P>(&mut self, now: Instant, phase: &mut P)
    where
        'id: 'p,
        K: Ieee80211Capabilities,
        P: LowerMacPhase<'p, 'id, Caps = K, Types = Ty>,
    {
        for index in 0..self.banks.len() {
            let Some(active) = self.banks.get(index).and_then(|bank| bank.active.as_ref()) else {
                continue;
            };
            if active.expired(now) {
                if !self.can_receive_management() {
                    continue;
                }
                let tid = active.tid();
                self.stop(index, phase);
                if let Ok(parameters) = DelbaParameters::from_bytes([0; 2]).with_tid(tid) {
                    let [a, b] = parameters.with_initiator(false).to_bytes();
                    // IEEE 802.11 reason 39: the requested QoS agreement timed out.
                    let _queued = self.queue([3, 2, a, b, 39, 0, 0, 0, 0]);
                }
            } else if let Some(active) = self
                .banks
                .get_mut(index)
                .and_then(|bank| bank.active.as_mut())
            {
                match active.expire_gap(now, self.header.gap_timeout) {
                    Ok(Some(release)) => self.release(release),
                    Ok(None) => {}
                    Err(_) => self.stop(index, phase),
                }
            }
        }
    }

    /// Return None only after moving the input into the retained pool.
    pub(super) fn ingest(
        &mut self,
        now: Instant,
        local: [u8; 6],
        frame: Ty::RxBuffer,
        meta: Ty::RxMeta,
    ) -> RxDisposition<'id, Ty> {
        if let Some((tid, sequence)) = self.bar(frame.bytes(), local) {
            if let Some(active) = self
                .banks
                .iter_mut()
                .find_map(|bank| bank.active.as_mut().filter(|active| active.tid() == tid))
                && let Ok(Some(release)) =
                    active.block_ack_request(now, sequence, self.header.gap_timeout)
            {
                self.release(release);
            }
            return RxDisposition::Recycle(frame);
        }
        let key = DecodedHeader::decode(frame.bytes()).and_then(|header| {
            let (_, bssid) = self.header.association?;
            let control = header.control();
            if !control.is_data()
                || control.ds() != 2
                || control.more_fragments()
                || header.sequence_control() & 15 != 0
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
            let qos = header.qos()?;
            let tid = (qos & 15) as u8;
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
        let release = active.ingest(
            now,
            RxReorderMpdu {
                sequence,
                slot: slot_id,
            },
            self.header.gap_timeout,
        );
        match release {
            Ok(release) if release.rejected.is_none() => {
                if let Some(place) = self.frames.get_mut(slot) {
                    *place = Some((frame, meta));
                }
                self.release(release);
                RxDisposition::Retained
            }
            _ => RxDisposition::Recycle(frame),
        }
    }

    fn bar(&self, bytes: &[u8], local: [u8; 6]) -> Option<(u8, SequenceNumber)> {
        let (_, bssid) = self.header.association?;
        let [a, b] = *bytes.first_chunk::<2>()?;
        let control = FrameControlFields::from_bytes([a, b]);
        if control.version() != 0
            || control.kind() != FrameType::Control
            || control.subtype() != 8
            || control.ds() != 0
            || control.protected()
            || control.more_fragments()
            || control.order()
            || bytes.get(4..10)? != local
            || bytes.get(10..16)? != bssid.get()
        {
            return None;
        }
        let parameters = BarControlFields::from_bytes(*bytes.get(16..18)?.first_chunk()?);
        if parameters.bar_type() != 2 {
            return None;
        } // single-TID compressed BAR
        let sequence = u16::from_le_bytes(*bytes.get(18..20)?.first_chunk()?);
        if sequence & 15 != 0 {
            return None;
        }
        Some((
            parameters.tid().get(),
            SequenceNumber::from_sequence_control(sequence),
        ))
    }
}
