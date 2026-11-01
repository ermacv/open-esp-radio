//! The port's aggregates.
//!
//! A builder and the sealed payload it becomes name their subframes by
//! buffer place. Each subframe's MPDU record (its pool buffer with the
//! buffer's return right, admission, layout, body, PN and whether it went
//! out) waits in the installation's subframe table at its place while the
//! aggregate is built, queued, published and returned; only the places
//! travel through the port's queues, so an aggregate costs those queues no
//! more than an MPDU. The places are affine, so no two owners name one
//! record; a record the table does not hold where its place says poisons
//! the installation.
//!
//! The builder applies every rule a caller controls when it accepts a
//! subframe, as the reference model does: the subframe count, the BlockAck
//! window rooted at the first subframe's sequence number, no repeated
//! sequence number, a retransmission's key, role, recipient and TID, and the
//! aggregate's length at the widest MPDU start spacing under the access
//! category's TXOP limit the builder samples. Planning on P2 applies the
//! same rule at the recipient's own spacing, which needs no more room, so
//! a submitted aggregate is never refused there.

use super::buffer::{Buffer, Mpdu, ReturnableBuffer};
use super::keys::RawKeyIdentity;
use super::*;
use crate::lower_mac::ampdu::AmpduBudget;
use crate::mac::tx::{TxBacking, TxPhyRate};
use oer_ieee80211_wire::{ccmp::CcmpPacketNumber, sequence::SequenceNumber};

/// Subframes of one aggregate: one per BlockAck bit the hardware window
/// reports.
pub const TX_AMPDU_SUBFRAMES: usize = crate::mac::tx::ampdu::TX_AMPDU_SLOT_CAPACITY;

const _: () = {
    if TX_AMPDU_SUBFRAMES != crate::mac::tx::ampdu::TX_BLOCK_ACK_MAX_WINDOW as usize
        || TX_AMPDU_SUBFRAMES > BlockAckReport::BITMAP_BITS as usize
    {
        panic!("an aggregate's subframes fit its BlockAck window and report");
    }
};

/// One subframe's MPDU while an aggregate names its place.
pub(super) struct SubframeRecord<const B: usize, Body> {
    pub(super) dma: ReturnableBuffer<B>,
    pub(super) admission: Option<Instant>,
    pub(super) layout: MpduLayout,
    pub(super) body: Option<Body>,
    pub(super) pn: Option<CcmpPacketNumber>,
    pub(super) published: bool,
}

/// The subframe records, one place per pool buffer.
pub(super) type SubframeTable<const B: usize, Body> =
    oer_memory::ReturnBank<Option<SubframeRecord<B, Body>>, TX_BUFFERS>;

impl<const B: usize, P: LentNotifies, Body> PortSharedStorage<B, P, Body> {
    /// Put `record` at `place`, which holds none.
    fn place_subframe(&self, place: usize, record: SubframeRecord<B, Body>) -> bool {
        match self.subframes.take(place) {
            Some((None, token)) => {
                token.return_value(Some(record));
                true
            }
            Some((held, token)) => {
                token.return_value(held);
                self.poison(Fault::Capacity);
                false
            }
            None => {
                self.poison(Fault::Capacity);
                false
            }
        }
    }
    /// The record at `place`, leaving none.
    fn take_subframe(&self, place: usize) -> Option<SubframeRecord<B, Body>> {
        let Some((held, token)) = self.subframes.take(place) else {
            self.poison(Fault::Capacity);
            return None;
        };
        token.return_value(None);
        if held.is_none() {
            self.poison(Fault::Capacity);
        }
        held
    }
    /// Change the record at `place` in place.
    fn with_subframe<R>(
        &self,
        place: usize,
        change: impl FnOnce(&mut SubframeRecord<B, Body>) -> R,
    ) -> Option<R> {
        let Some((mut held, token)) = self.subframes.take(place) else {
            self.poison(Fault::Capacity);
            return None;
        };
        let result = held.as_mut().map(change);
        token.return_value(held);
        if result.is_none() {
            self.poison(Fault::Capacity);
        }
        result
    }
    /// Whether no aggregate names any place: every one is empty.
    pub(super) fn subframes_vacant(&self) -> bool {
        (0..TX_BUFFERS).all(|place| match self.subframes.take(place) {
            Some((held, token)) => {
                let vacant = held.is_none();
                token.return_value(held);
                vacant
            }
            None => false,
        })
    }
}

/// The subframes of one aggregate, by place, in order. Dropping it drops
/// every record it names.
pub struct Subframes<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    places: [Option<Slot<'id, Buffers, Part<Id, 3>>>; TX_AMPDU_SUBFRAMES],
    count: usize,
    shared: &'static PortSharedStorage<B, D::Lent, D::Body>,
    key: Option<RawKeyIdentity>,
    role: VifRole,
    loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>,
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Subframes<'id, Id, D, B, O>
{
    fn new(
        shared: &'static PortSharedStorage<B, D::Lent, D::Body>,
        key: Option<RawKeyIdentity>,
        role: VifRole,
        loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>,
    ) -> Self {
        Self {
            places: [const { None }; TX_AMPDU_SUBFRAMES],
            count: 0,
            shared,
            key,
            role,
            loan,
        }
    }

    /// The subframes it names.
    pub const fn len(&self) -> usize {
        self.count
    }

    /// Whether it names none.
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The key of every subframe.
    pub(super) const fn key(&self) -> Option<RawKeyIdentity> {
        self.key
    }

    /// The role of every subframe.
    pub(super) const fn role(&self) -> VifRole {
        self.role
    }

    /// The place index of subframe `index`.
    fn place(&self, index: usize) -> Option<usize> {
        self.places
            .get(index)?
            .as_ref()
            .map(|place| usize::from(place.index()))
    }

    /// Append `record` at `place`. Without a free place, or with its
    /// table place held, the installation poisons: the record drops, which
    /// returns a CPU-owned buffer.
    fn push(&mut self, place: Slot<'id, Buffers, Part<Id, 3>>, record: SubframeRecord<B, D::Body>) {
        let Some(free) = self.places.get_mut(self.count) else {
            self.shared.poison(Fault::Capacity);
            return;
        };
        // A refused table place poisons; the place stays named, and its
        // later take reports the record lost.
        let _placed = self
            .shared
            .place_subframe(usize::from(place.index()), record);
        *free = Some(place);
        self.count += 1;
    }

    /// The MPDU of subframe `place` with its record.
    fn mpdu(
        &self,
        place: Slot<'id, Buffers, Part<Id, 3>>,
        record: SubframeRecord<B, D::Body>,
    ) -> Mpdu<'id, Id, D, B, O> {
        Mpdu {
            buffer: Buffer {
                place,
                dma: record.dma,
                admission: record.admission,
                loan: self.loan,
            },
            layout: record.layout,
            body: record.body,
            key: self.key,
            pn: record.pn,
            role: self.role,
            published: record.published,
        }
    }

    /// Move every place out, under another brand.
    fn rebrand<'q>(
        mut self,
        place: impl Fn(Slot<'id, Buffers, Part<Id, 3>>) -> Slot<'q, Buffers, Part<Id, 3>>,
        loan: Option<BodyLoan<'q, Id, Esp32s31Backend<Id, D, B, O>>>,
    ) -> Subframes<'q, Id, D, B, O> {
        let mut places = [const { None }; TX_AMPDU_SUBFRAMES];
        for (to, from) in places.iter_mut().zip(self.places.iter_mut()) {
            *to = from.take().map(&place);
        }
        let count = core::mem::replace(&mut self.count, 0);
        Subframes {
            places,
            count,
            shared: self.shared,
            key: self.key,
            role: self.role,
            loan,
        }
    }

    pub(super) fn into_owned(
        self,
        loan: &BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Subframes<'static, Id, D, B, O> {
        self.rebrand(|place| loan.reclaim_slot(place), None)
    }

    /// Set the admission of every subframe that has none to `now`: the
    /// earliest admission among them.
    pub(super) fn admit(&mut self, now: Instant) -> Option<Instant> {
        let mut oldest: Option<Instant> = None;
        for index in 0..self.count {
            let place = self.place(index)?;
            let at = self
                .shared
                .with_subframe(place, |record| *record.admission.get_or_insert(now))?;
            oldest = Some(oldest.map_or(at, |oldest| oldest.min(at)));
        }
        oldest
    }

    /// The subframes, by bit, whose MSDU aged past `lifetime` at `now`.
    pub(super) fn expired(&mut self, now: Instant, lifetime: Duration) -> u64 {
        (0..self.count).fold(0, |mask, index| {
            let aged = self.place(index).and_then(|place| {
                self.shared.with_subframe(place, |record| {
                    record
                        .admission
                        .is_some_and(|at| now.saturating_duration_since(at) >= lifetime)
                })
            });
            match (aged, 1_u64.checked_shl(index as u32)) {
                (Some(true), Some(bit)) => mask | bit,
                _ => mask,
            }
        })
    }

    /// Hand subframe `index`'s buffer to the hardware: its record keeps the
    /// return right. The buffer, the MPDU's length, its admission, whether
    /// it went out before, its header length and its PN.
    pub(super) fn lease(&mut self, index: usize) -> Option<LeasedSubframe<B>> {
        let place = self.place(index)?;
        self.shared
            .with_subframe(place, |record| {
                Some(LeasedSubframe {
                    buffer: record.dma.publish()?,
                    len: record.layout.mpdu_len(),
                    header_len: record.layout.header_len(),
                    admitted_at: record.admission?,
                    published: record.published,
                    pn: record.pn,
                })
            })
            .flatten()
    }

    /// The hardware handed subframe `index`'s buffer back: it rejoins its
    /// right, and the MPDU went out when `published`.
    pub(super) fn restore(&mut self, index: usize, buffer: TxBacking<B>, published: bool) -> bool {
        let Some(place) = self.place(index) else {
            return false;
        };
        self.shared
            .with_subframe(place, |record| {
                record.published |= published;
                record.dma.complete(buffer).is_ok()
            })
            .unwrap_or(false)
    }
}

/// One subframe's buffer on its way to the hardware.
pub(super) struct LeasedSubframe<const B: usize> {
    pub(super) buffer: TxBacking<B>,
    pub(super) len: usize,
    pub(super) header_len: usize,
    pub(super) admitted_at: Instant,
    pub(super) published: bool,
    pub(super) pn: Option<CcmpPacketNumber>,
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Subframes<'static, Id, D, B, O>
{
    pub(super) fn lend<'p>(
        self,
        loan: &BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
    ) -> Subframes<'p, Id, D, B, O> {
        self.rebrand(|place| loan.lend_slot(place), Some(*loan))
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Drop
    for Subframes<'_, Id, D, B, O>
{
    fn drop(&mut self) {
        for place in self.places.iter_mut().filter_map(Option::take) {
            // Dropping a CPU-owned record returns its buffer to the pool.
            drop(self.shared.take_subframe(usize::from(place.index())));
        }
    }
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> ExecutionPayload
    for Subframes<'id, Id, D, B, O>
{
    type Execution<'a>
        = &'a mut Subframes<'id, Id, D, B, O>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        self
    }
}

/// The MPDUs of an aggregate's subframes, in order.
pub struct SubframeMpdus<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    subframes: Subframes<'id, Id, D, B, O>,
    next: usize,
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Iterator
    for SubframeMpdus<'id, Id, D, B, O>
{
    type Item = Mpdu<'id, Id, D, B, O>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.subframes.count {
            let place = self.subframes.places.get_mut(self.next)?.take();
            self.next += 1;
            let Some(place) = place else { continue };
            // A record the table lost poisons it; its place ends here.
            if let Some(record) = self
                .subframes
                .shared
                .take_subframe(usize::from(place.index()))
            {
                return Some(self.subframes.mpdu(place, record));
            }
        }
        None
    }
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> IntoIterator
    for Subframes<'id, Id, D, B, O>
{
    type Item = Mpdu<'id, Id, D, B, O>;
    type IntoIter = SubframeMpdus<'id, Id, D, B, O>;
    fn into_iter(self) -> Self::IntoIter {
        SubframeMpdus {
            subframes: self,
            next: 0,
        }
    }
}

/// What the driver needs of an admitted aggregate beside its subframes.
#[derive(Clone, Copy, Debug)]
pub struct AggregateExecution {
    pub(super) rate: PhyRate,
    pub(super) tid: Tid,
    pub(super) txop_limit_units_32_us: u16,
}

/// One aggregate being built: its fixed recipient, TID, rate and key, and
/// the subframes accepted so far.
pub struct Aggregate<
    'k,
    'p,
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
    R,
    K,
    S: AggregateState,
> {
    subframes: Subframes<'id, Id, D, B, O>,
    loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    receiver: IndividualAddress,
    tid: Tid,
    rate: AmpduRate<'p, 'id, K>,
    txop_limit_units_32_us: u16,
    first: S::Head<SequenceNumber>,
    /// The BlockAck bits of the subframes accepted so far.
    bits: u64,
    /// `None` at a rate no aggregate takes: every subframe is refused.
    budget: Option<AmpduBudget>,
    proof: PhantomData<(&'k (), fn() -> R)>,
}

/// What a builder fixes at its creation.
pub(super) struct AggregateSetup<
    'p,
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
    K,
> {
    pub(super) shared: &'static PortSharedStorage<B, D::Lent, D::Body>,
    pub(super) loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    pub(super) receiver: IndividualAddress,
    pub(super) tid: Tid,
    pub(super) rate: AmpduRate<'p, 'id, K>,
    pub(super) key: Option<RawKeyIdentity>,
    pub(super) role: VifRole,
    /// The access category's advertised TXOP limit, in 32 µs units.
    pub(super) txop_limit_units_32_us: u16,
}

impl<'k, 'p, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations, R, K>
    Aggregate<'k, 'p, 'id, Id, D, B, O, R, K, EmptyAggregate>
{
    pub(super) fn new(setup: AggregateSetup<'p, 'id, Id, D, B, O, K>) -> Self {
        let budget = TxPhyRate::try_from(setup.rate.rate().get())
            .ok()
            .and_then(|rate| {
                AmpduBudget::new(
                    rate,
                    setup.txop_limit_units_32_us,
                    AmpduBudget::WIDEST_DENSITY,
                    TX_AMPDU_SUBFRAMES,
                )
                .ok()
            });
        Self {
            subframes: Subframes::new(setup.shared, setup.key, setup.role, Some(setup.loan)),
            loan: setup.loan,
            receiver: setup.receiver,
            tid: setup.tid,
            rate: setup.rate,
            txop_limit_units_32_us: setup.txop_limit_units_32_us,
            first: (),
            bits: 0,
            budget,
            proof: PhantomData,
        }
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations, R, K, S: AggregateState>
    Aggregate<'_, '_, '_, Id, D, B, O, R, K, S>
{
    /// Whether the subframe of `len` MPDU octets at `sequence` fits: a free
    /// place, the BlockAck window rooted at the first subframe's sequence
    /// number (its modulo-4096 distance, IEEE Std 802.11-2020, 10.24.7)
    /// without a repeated one, and the length budget, which it then takes.
    fn admit(
        &mut self,
        len: usize,
        sequence: Option<SequenceNumber>,
        loaned: bool,
    ) -> Result<(SequenceNumber, u64), PreparationError> {
        if self.subframes.len() >= TX_AMPDU_SUBFRAMES {
            return Err(PreparationError::AggregateFull);
        }
        if !loaned {
            return Err(PreparationError::MissingBodyLoan);
        }
        let sequence = sequence.ok_or(PreparationError::BlockAckWindow)?;
        let first = S::retain_head(&self.first, sequence);
        let distance =
            SequenceNumber::from_low_bits(sequence.get().wrapping_sub(first.get())).get();
        let bit = 1_u64
            .checked_shl(u32::from(distance))
            .filter(|bit| self.bits & bit == 0)
            .ok_or(PreparationError::BlockAckWindow)?;
        // The hardware adds the MIC of a protected subframe and every FCS.
        let mic = match self.subframes.key() {
            Some(_) => crate::ordinary_tx::TX_CCMP_MIC_SIZE,
            None => 0,
        };
        let psdu = len
            .checked_add(mic + crate::ordinary_tx::TX_FCS_SIZE)
            .ok_or(PreparationError::AggregateFull)?;
        self.budget
            .as_mut()
            .ok_or(PreparationError::AggregateFull)?
            .push(psdu)
            .map_err(|_| PreparationError::AggregateFull)?;
        Ok((sequence, bit))
    }
}

/// The sequence number of the MPDU of `len` octets in `dma`.
fn sequence<const B: usize>(dma: &mut ReturnableBuffer<B>, len: usize) -> Option<SequenceNumber> {
    oer_ieee80211_wire::frame::sequence_control(dma.mpdu(len)?)
        .map(SequenceNumber::from_sequence_control)
}

type Pushed<'k, 'p, 'id, Id, D, const B: usize, O, R, K, S> =
    AggregatePushResult<'k, 'p, 'id, Types<Id, D, B, O>, buffer::BufferClass<B>, R, K, S>;

/// Accept one checked subframe into its builder, or return both unchanged.
#[allow(clippy::result_large_err, reason = "a refusal returns every owner")]
pub(super) fn push<'k, 'p, 'id, Id, D, const B: usize, O, R, K, S>(
    operation: CheckedPoolAggregate<
        'k,
        'p,
        'id,
        Types<Id, D, B, O>,
        buffer::BufferClass<B>,
        R,
        K,
        S,
    >,
) -> Pushed<'k, 'p, 'id, Id, D, B, O, R, K, S>
where
    Id: SlotIdentity + 'id,
    D: Device<B>,
    O: Observations,
    R: RoleKeys<'id, Types<Id, D, B, O>>,
    K: Ieee80211Capabilities,
    S: AggregateState,
    'id: 'k + 'p,
{
    let (mut aggregate, source) = operation.into_parts();
    let (place, record, (sequence, bit)) = match source {
        CheckedSubframe::Sent(mut sent) => {
            let mpdu = &mut sent.mpdu;
            let reason = if mpdu.key != aggregate.subframes.key()
                || mpdu.key.as_ref().is_some_and(|key| !key.active())
            {
                Some(PreparationError::RetransmissionKey)
            } else if mpdu.role != R::ROLE
                || mpdu.layout.recipient() != Some((aggregate.receiver, aggregate.tid))
            {
                Some(PreparationError::RetransmissionKind)
            } else {
                None
            };
            let len = mpdu.layout.mpdu_len();
            let sequence = sequence(&mut mpdu.buffer.dma, len);
            let loaned = mpdu.buffer.loan.is_some();
            let admitted = match reason {
                Some(reason) => Err(reason),
                None => aggregate.admit(len, sequence, loaned),
            };
            let admitted = match admitted {
                Ok(admitted) => admitted,
                Err(reason) => {
                    return Err(AggregateRefused {
                        reason,
                        aggregate,
                        subframe: SubframeSource::Sent(sent),
                    });
                }
            };
            let Mpdu {
                buffer,
                layout,
                body,
                pn,
                published,
                ..
            } = sent.mpdu;
            let record = SubframeRecord {
                dma: buffer.dma,
                admission: buffer.admission,
                layout,
                body,
                pn,
                published,
            };
            (buffer.place, record, admitted)
        }
        CheckedSubframe::Fresh(fresh) => {
            let (
                FreshSubframe {
                    numbering,
                    mut buffer,
                    frame,
                    body,
                },
                layout,
            ) = fresh.into_parts();
            let len = layout.mpdu_len();
            let sequence = sequence(&mut buffer.dma, len);
            let loaned = buffer.loan.is_some();
            let admitted = match aggregate.admit(len, sequence, loaned) {
                Ok(admitted) => admitted,
                Err(reason) => {
                    return Err(AggregateRefused {
                        reason,
                        aggregate,
                        subframe: SubframeSource::Fresh {
                            numbering,
                            buffer,
                            frame,
                            body,
                        },
                    });
                }
            };
            let record = SubframeRecord {
                dma: buffer.dma,
                admission: buffer.admission,
                layout,
                body,
                pn: numbering.packet_number(),
                published: false,
            };
            (buffer.place, record, admitted)
        }
    };
    // `admit` checked for a free place.
    aggregate.subframes.push(place, record);
    Ok(nonempty(aggregate, sequence, bit))
}

/// The builder after it accepted the subframe at `sequence` holding `bit`.
fn nonempty<'k, 'p, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations, R, K, S>(
    aggregate: Aggregate<'k, 'p, 'id, Id, D, B, O, R, K, S>,
    sequence: SequenceNumber,
    bit: u64,
) -> Aggregate<'k, 'p, 'id, Id, D, B, O, R, K, NonEmptyAggregate>
where
    S: AggregateState,
{
    let first = S::retain_head(&aggregate.first, sequence);
    let Aggregate {
        subframes,
        loan,
        receiver,
        tid,
        rate,
        txop_limit_units_32_us,
        bits,
        budget,
        ..
    } = aggregate;
    Aggregate {
        subframes,
        loan,
        receiver,
        tid,
        rate,
        txop_limit_units_32_us,
        first,
        bits: bits | bit,
        budget,
        proof: PhantomData,
    }
}

impl<'k, 'p, 'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R, K, S>
    PhaseAggregate<'id> for Aggregate<'k, 'p, 'id, Id, D, B, O, R, K, S>
where
    S: AggregateState,
{
    type Types = Types<Id, D, B, O>;
    type Backend = Esp32s31Backend<Id, D, B, O>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = Subframes<'static, Id, D, B, O>;
    type State = S;
    type Execution = AggregateExecution;
    type Cancelled = Subframes<'id, Id, D, B, O>;
    fn into_admission(
        self,
        context: D::Context<'id, Id>,
        ticket: Slot<'id, Tickets, Part<Id, 2>>,
        _: Admission<Self>,
    ) -> AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, Tickets, Part<Id, 2>>,
        Self::OwnedPayload,
        AggregateExecution,
    >
    where
        S: NonEmptyState,
    {
        let loan = self.loan;
        AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            execution: AggregateExecution {
                rate: self.rate.rate().get(),
                tid: self.tid,
                txop_limit_units_32_us: self.txop_limit_units_32_us,
            },
            payload: self.subframes.into_owned(&loan),
        }
    }
    fn receiver(&self) -> IndividualAddress {
        self.receiver
    }
    fn tid(&self) -> Tid {
        self.tid
    }
    fn protection(&self) -> MpduProtection {
        match self.subframes.key() {
            None => MpduProtection::Open,
            Some(_) => MpduProtection::Ccmp,
        }
    }
    fn subframes(&self) -> usize {
        self.subframes.len()
    }
    fn into_parts(self) -> Self::Cancelled {
        self.subframes
    }
}
