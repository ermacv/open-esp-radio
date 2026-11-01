use super::keys::RawKeyIdentity;
use super::*;

pub struct BufferClass<const B: usize>;
impl<const B: usize> TxBufferClass for BufferClass<B> {
    const CAPACITY: usize = esp32s31_lower_mac_capabilities(B).max_mpdu_length as usize;
}

/// Affine memory owner. Only MPDU octets are lent to the portable core.
pub struct Buffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) place: Slot<'id, Buffers, Part<Id, 3>>,
    pub(super) dma: ReturnableBuffer<B>,
    pub(super) admission: Option<Instant>,
    pub(super) loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>,
}
type ReturnRight<const B: usize> = oer_memory::ReturnToken<'static, TxSlotFree<'static, B>>;

/// One buffer place's DMA memory and its sole right of return, held
/// together: memory never exists here without its right.
enum Dma<const B: usize> {
    /// The CPU owns the memory; its right travels with it, and dropping the
    /// buffer returns the memory.
    Free {
        memory: TxSlotFree<'static, B>,
        returned: ReturnRight<B>,
    },
    /// The hardware owns the memory; the right waits for the completion
    /// that hands it back. Dropped here, the place stays out of the bank.
    Published { returned: ReturnRight<B> },
    /// Neither: the place had no returned memory, or its pair moved on.
    Released,
}

/// A buffer place's [`Dma`]; dropping it returns CPU-owned memory.
pub(super) struct ReturnableBuffer<const B: usize>(Dma<B>);
impl<const B: usize> ReturnableBuffer<B> {
    /// The pair the bank's place hands out, or nothing to return.
    pub(super) fn taken(pair: Option<(TxSlotFree<'static, B>, ReturnRight<B>)>) -> Self {
        Self(match pair {
            Some((memory, returned)) => Dma::Free { memory, returned },
            None => Dma::Released,
        })
    }
    /// The CPU's memory, while it owns it.
    pub(super) fn memory_mut(&mut self) -> Option<&mut TxSlotFree<'static, B>> {
        match &mut self.0 {
            Dma::Free { memory, .. } => Some(memory),
            _ => None,
        }
    }
    /// Hand the memory to the hardware, keeping its right; `None`, and no
    /// change, unless the CPU owns it.
    pub(super) fn publish(&mut self) -> Option<TxSlotFree<'static, B>> {
        match core::mem::replace(&mut self.0, Dma::Released) {
            Dma::Free { memory, returned } => {
                self.0 = Dma::Published { returned };
                Some(memory)
            }
            other => {
                self.0 = other;
                None
            }
        }
    }
    /// The completion handed `memory` back: it rejoins its right. Refused,
    /// with the memory and no change, unless the hardware owned it.
    pub(super) fn complete(
        &mut self,
        memory: TxSlotFree<'static, B>,
    ) -> Result<(), TxSlotFree<'static, B>> {
        match core::mem::replace(&mut self.0, Dma::Released) {
            Dma::Published { returned } => {
                self.0 = Dma::Free { memory, returned };
                Ok(())
            }
            other => {
                self.0 = other;
                Err(memory)
            }
        }
    }
}
impl<const B: usize> Drop for ReturnableBuffer<B> {
    fn drop(&mut self) {
        if let Dma::Free { memory, returned } = core::mem::replace(&mut self.0, Dma::Released) {
            returned.return_value(memory);
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Buffer<'_, Id, D, B, O> {
    pub fn index(&self) -> u8 {
        self.place.index()
    }
    /// When P1 admitted the MPDU in this memory, until it is renewed.
    #[cfg(test)]
    pub(crate) fn admitted_at(&self) -> Option<Instant> {
        self.admission
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> TxBufferMemory
    for Buffer<'_, Id, D, B, O>
{
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable> {
        let memory = self.dma.memory_mut().ok_or(BufferMemoryUnavailable)?;
        memory.reset_work();
        self.admission = None;
        Ok(())
    }
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable> {
        self.dma
            .memory_mut()
            .ok_or(BufferMemoryUnavailable)?
            .buffer_mut()
            .map_err(|_| BufferMemoryUnavailable)?
            .get_mut(crate::ordinary_tx::TX_METADATA_SIZE..)
            .and_then(|bytes| bytes.get_mut(..BufferClass::<B>::CAPACITY))
            .ok_or(BufferMemoryUnavailable)
    }
}

pub struct Sent<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) mpdu: Mpdu<'id, Id, D, B, O>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Sent<'_, Id, D, B, O> {
    pub fn index(&self) -> u8 {
        self.mpdu.buffer.index()
    }
    pub fn packet_number(&self) -> Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber> {
        self.mpdu.pn
    }
    pub fn body(&self) -> Option<&D::Body> {
        self.mpdu.body.as_ref()
    }
    /// When P1 admitted the MPDU; retransmission keeps it.
    #[cfg(test)]
    pub(crate) fn admitted_at(&self) -> Option<Instant> {
        self.mpdu.buffer.admission
    }
    /// The encoded MPDU extent as its last publication left it in memory.
    #[cfg(test)]
    pub(crate) fn published(&mut self) -> &[u8] {
        let len = self.mpdu.layout.mpdu_len();
        let bytes = self
            .mpdu
            .buffer
            .bytes_mut()
            .expect("a returned Sent owns its memory");
        &bytes[..len]
    }
}
pub enum AnyBuffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Fresh(Buffer<'id, Id, D, B, O>),
    Sent(Sent<'id, Id, D, B, O>),
}

pub struct Mpdu<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) buffer: Buffer<'id, Id, D, B, O>,
    pub(super) layout: MpduLayout,
    pub(super) body: Option<D::Body>,
    pub(super) key: Option<RawKeyIdentity>,
    pub(super) pn: Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber>,
    pub(super) role: VifRole,
    pub(super) published: bool,
}
/// A view cannot move, replace or renew the admitted MPDU.
pub struct MpduExecution<'a, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) mpdu: &'a mut Mpdu<'id, Id, D, B, O>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> ExecutionPayload
    for Mpdu<'id, Id, D, B, O>
{
    type Execution<'a>
        = MpduExecution<'a, 'id, Id, D, B, O>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        MpduExecution { mpdu: self }
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> TxMpdu
    for Mpdu<'id, Id, D, B, O>
{
    type Buffer = AnyBuffer<'id, Id, D, B, O>;
    type Body = D::Body;
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>) {
        if self.published {
            (AnyBuffer::Sent(Sent { mpdu: self }), None)
        } else {
            (AnyBuffer::Fresh(self.buffer), self.body)
        }
    }
}

pub struct Prepared<
    'k,
    'p,
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
    R,
    K,
> {
    mpdu: Mpdu<'id, Id, D, B, O>,
    plan: RetryPlan<PhyRate>,
    /// The body loan the prepared buffer was lent under.
    loan: BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>,
    proof: PhantomData<(&'k (), fn(&'p ()) -> (&'p (), R, K))>,
}
pub struct Execution {
    pub(super) plan: RetryPlan<PhyRate>,
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R, K>
    PreparedTx<'id> for Prepared<'_, '_, 'id, Id, D, B, O, R, K>
{
    type Types = Types<Id, D, B, O>;
    type Backend = Esp32s31Backend<Id, D, B, O>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = Mpdu<'static, Id, D, B, O>;
    type Execution = Execution;
    fn into_admission(
        self,
        context: D::Context<'id, Id>,
        ticket: Slot<'id, Tickets, Part<Id, 2>>,
        _: Admission<Self>,
    ) -> AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, Tickets, Part<Id, 2>>,
        Self::OwnedPayload,
        Execution,
    > {
        let loan = self.loan;
        AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            payload: self.mpdu.into_owned(&loan),
            execution: Execution { plan: self.plan },
        }
    }
    fn into_parts(self) -> (AnyBuffer<'id, Id, D, B, O>, Option<D::Body>) {
        self.mpdu.into_parts()
    }
}

fn one(rate: PhyRate) -> RetryPlan<PhyRate> {
    RetryPlan::new(RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::MIN,
        protection: Protection::None,
    })
}
fn stages<K>(frame: &TxFrame<'_, '_, K>) -> RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one(rate.rate().get()),
        TxFrame::Group { rate, .. } => one(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    TxBufferPool<'id, BufferClass<B>> for Types<Id, D, B, O>
{
    type Buffer = Buffer<'id, Id, D, B, O>;
    type Sent = Sent<'id, Id, D, B, O>;
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, BufferClass<B>, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, BufferClass<B>, K, R>
    where
        'id: 'k,
    {
        let (mpdu, plan, loan) = match source {
            CheckedTxSource::Fresh(fresh) => {
                let (
                    FreshSource {
                        buffer,
                        frame,
                        body,
                        protection,
                    },
                    layout,
                ) = fresh.into_parts();
                // Only a buffer lent to the body enters its preparation.
                let Some(loan) = buffer.loan else {
                    return Err(PreparationRefused {
                        reason: PreparationError::MissingBodyLoan,
                        operation: TxSource::Fresh {
                            buffer,
                            frame,
                            body,
                            protection,
                        },
                    });
                };
                let (key, pn) = match protection {
                    TxProtection::Open => (None, None),
                    TxProtection::Ccmp(numbered) => (
                        Some(R::tx_identity(&numbered.key).owned),
                        Some(numbered.packet_number),
                    ),
                };
                let plan = stages(frame.frame());
                (
                    Mpdu {
                        buffer,
                        layout,
                        body,
                        key,
                        pn,
                        role: R::ROLE,
                        published: false,
                    },
                    plan,
                    loan,
                )
            }
            CheckedTxSource::Retransmission { mpdu, plan } => {
                let Some(loan) = mpdu.mpdu.buffer.loan else {
                    return Err(PreparationRefused {
                        reason: PreparationError::MissingBodyLoan,
                        operation: TxSource::Retransmission { mpdu, plan },
                    });
                };
                if mpdu.mpdu.key.as_ref().is_some_and(|key| !key.active()) {
                    return Err(PreparationRefused {
                        reason: PreparationError::RetransmissionKey,
                        operation: TxSource::Retransmission { mpdu, plan },
                    });
                }
                let stages = match (mpdu.mpdu.layout.kind(), &plan) {
                    (MpduKind::Individual, RetransmissionPlan::Individual(plan))
                        if mpdu.mpdu.role == R::ROLE =>
                    {
                        Some(plan.map(|rate| rate.get()))
                    }
                    (MpduKind::IndividualNoAck, RetransmissionPlan::IndividualNoAck(rate))
                        if mpdu.mpdu.role == R::ROLE =>
                    {
                        Some(one(rate.rate().get()))
                    }
                    (MpduKind::Group, RetransmissionPlan::Group(rate))
                        if mpdu.mpdu.role == R::ROLE =>
                    {
                        Some(one(rate.get()))
                    }
                    _ => None,
                };
                let Some(stages) = stages else {
                    return Err(PreparationRefused {
                        reason: PreparationError::RetransmissionKind,
                        operation: TxSource::Retransmission { mpdu, plan },
                    });
                };
                (mpdu.mpdu, stages, loan)
            }
        };
        Ok(Prepared {
            mpdu,
            plan,
            loan,
            proof: PhantomData,
        })
    }
    fn push_aggregate<'k, 'p, R, K, S>(
        operation: CheckedPoolAggregate<'k, 'p, 'id, Self, BufferClass<B>, R, K, S>,
    ) -> AggregatePushResult<'k, 'p, 'id, Self, BufferClass<B>, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        match operation.into_parts().0.impossible {}
    }
    fn erase(buffer: Self::Buffer) -> AnyBuffer<'id, Id, D, B, O> {
        AnyBuffer::Fresh(buffer)
    }
    fn erase_sent(buffer: Self::Sent) -> AnyBuffer<'id, Id, D, B, O> {
        AnyBuffer::Sent(buffer)
    }
    fn class(
        buffer: AnyBuffer<'id, Id, D, B, O>,
    ) -> Result<Self::Buffer, AnyBuffer<'id, Id, D, B, O>> {
        match buffer {
            AnyBuffer::Fresh(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn sent(
        buffer: AnyBuffer<'id, Id, D, B, O>,
    ) -> Result<Self::Sent, AnyBuffer<'id, Id, D, B, O>> {
        match buffer {
            AnyBuffer::Sent(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn renew(buffer: Self::Sent) -> (Self::Buffer, Option<D::Body>) {
        // The MPDU and its PN end here; `renew_msdu` releases its admission.
        (buffer.mpdu.buffer, buffer.mpdu.body)
    }
}

/// This port exposes ordinary MPDUs. Aggregate ownership is uninhabited,
/// and the optional `AmpduPhase` is not implemented.
pub struct NoAggregate<'id, Id: SlotIdentity, D, const B: usize, O: Observations, S> {
    pub(super) impossible: Infallible,
    marker: PhantomData<fn(&'id (), Id) -> (&'id (), Id, D, O, S)>,
}
pub struct NoAggregatePayload<'id, Id: SlotIdentity, D, const B: usize, O: Observations> {
    pub(super) impossible: Infallible,
    marker: PhantomData<fn(&'id (), Id) -> (&'id (), Id, D, O)>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> IntoIterator
    for NoAggregatePayload<'id, Id, D, B, O>
{
    type Item = Mpdu<'id, Id, D, B, O>;
    type IntoIter = core::iter::Empty<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        match self.impossible {}
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> ExecutionPayload
    for NoAggregatePayload<'_, Id, D, B, O>
{
    type Execution<'a>
        = Infallible
    where
        Self: 'a;
    fn execution(&mut self) -> Infallible {
        match self.impossible {}
    }
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, S: AggregateState>
    PhaseAggregate<'id> for NoAggregate<'id, Id, D, B, O, S>
{
    type Types = Types<Id, D, B, O>;
    type State = S;
    type Backend = Esp32s31Backend<Id, D, B, O>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = NoAggregatePayload<'static, Id, D, B, O>;
    type Execution = Infallible;
    type Cancelled = NoAggregatePayload<'id, Id, D, B, O>;
    fn into_admission(
        self,
        _: D::Context<'id, Id>,
        _: Slot<'id, Tickets, Part<Id, 2>>,
        _: Admission<Self>,
    ) -> AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, Tickets, Part<Id, 2>>,
        Self::OwnedPayload,
        Infallible,
    >
    where
        S: NonEmptyState,
    {
        match self.impossible {}
    }
    fn receiver(&self) -> IndividualAddress {
        match self.impossible {}
    }
    fn tid(&self) -> Tid {
        match self.impossible {}
    }
    fn protection(&self) -> MpduProtection {
        match self.impossible {}
    }
    fn subframes(&self) -> usize {
        match self.impossible {}
    }
    fn into_parts(self) -> Self::Cancelled {
        match self.impossible {}
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> PhaseTypes<'id>
    for Types<Id, D, B, O>
{
    const COMMAND_CAPACITY: usize = COMMANDS;
    const TERMINAL_CAPACITY: usize = TERMINAL_CAPACITY;
    type StationCommandPath = Part<Id, 8>;
    type AccessPointCommandPath = Part<Id, 9>;
    type StationConfigureContext = D::ConfigureContext<'id, Id>;
    type AccessPointConfigureContext = D::ConfigureContext<'id, Id>;
    type EpochPath = Part<Id, 4>;
    type GatePath = Part<Id, 1>;
    type TicketPath = Part<Id, 2>;
    type BlockAckPath = Part<Id, 7>;
    type Fault = Fault;
    type Context = D::Context<'id, Id>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = Prepared<'k, 'p, 'id, Id, D, B, O, R, K>;
    type KeyIdentity = keys::KeyIdentity<'id, Id>;
    type Aggregate<'k, 'p, R, K, S>
        = NoAggregate<'id, Id, D, B, O, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k;
    type TxBody = D::Body;
    type TicketKind = Tickets;
    type AckEvidence = Ack;
    type BlockAckEvidence = Ack;
    type AirDenied = Infallible;
    type AnyTxBuffer = AnyBuffer<'id, Id, D, B, O>;
    type TxMpdu = Mpdu<'id, Id, D, B, O>;
    type TxAggregate = NoAggregatePayload<'id, Id, D, B, O>;
    type RxBuffer = ReceiveBuffer<'id, Id, D::Receive>;
    type LentRx = super::LentReceive<D::Receive>;
    type RxMeta = Esp32s31RxMeta;
    type StationKeys = PortKeys<Id, Station>;
    type AccessPointKeys = PortKeys<Id, AccessPoint>;
    type BlockAckKind = BlockAcks;
    type BlockAckPool = association::BlockAckSlots<BlockAcks, 8>;
    type BlockAck = ReceiveAgreement<'id, Id>;
}

pub struct ReceiveAgreement<'id, Id: SlotIdentity> {
    pub(super) slot: association::BlockAckSlot<'id, BlockAcks, 8, Part<Id, 7>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;

    type Bank = oer_memory::ReturnBank<TxSlotFree<'static, 512>, 1>;

    /// A bank whose one place holds a free model slot.
    fn bank() -> &'static Bank {
        let slot = Pin::static_mut(Box::leak(Box::new(
            crate::mac::tx::TxSlot::<512>::new_model().expect("model slot"),
        )));
        let TxSlotAccess::Free(memory) = slot.access() else {
            panic!("a new slot is free")
        };
        let bank = Box::leak(Box::new(Bank::new()));
        assert!(bank.seed(0, memory).is_ok());
        bank
    }

    #[test]
    fn dropping_a_free_buffer_returns_its_memory_to_the_place() {
        let bank = bank();
        drop(ReturnableBuffer::taken(bank.take(0)));
        assert!(bank.take(0).is_some(), "the memory is back in its place");
    }

    #[test]
    fn a_published_buffer_keeps_its_right_and_its_place_stays_out_when_dropped() {
        let bank = bank();
        let mut buffer = ReturnableBuffer::taken(bank.take(0));
        let memory = buffer.publish().expect("the CPU owned it");
        assert!(buffer.memory_mut().is_none(), "the hardware owns it now");
        assert!(buffer.publish().is_none(), "published once");
        drop(buffer);
        // The hardware never handed it back: nothing reuses the place.
        assert!(bank.take(0).is_none());
        drop(memory);
    }

    #[test]
    fn a_completion_rejoins_the_right_and_a_wrong_transition_changes_nothing() {
        let bank = bank();
        let mut buffer = ReturnableBuffer::taken(bank.take(0));
        let memory = buffer.publish().expect("the CPU owned it");
        let memory = match buffer.complete(memory) {
            Ok(()) => buffer.publish().expect("free again"),
            Err(_) => panic!("the hardware owned it"),
        };
        assert!(buffer.complete(memory).is_ok());
        // A second completion is refused with its memory, unchanged.
        let other = bank_memory();
        assert!(buffer.complete(other).is_err());
        assert!(buffer.memory_mut().is_some());
        drop(buffer);
        assert!(bank.take(0).is_some());
    }

    #[test]
    fn a_place_without_returned_memory_holds_nothing_to_return() {
        let bank = bank();
        let _held = bank.take(0);
        let mut buffer = ReturnableBuffer::taken(bank.take(0));
        assert!(buffer.memory_mut().is_none());
        assert!(buffer.publish().is_none());
    }

    /// Free memory of another model slot.
    fn bank_memory() -> TxSlotFree<'static, 512> {
        let slot = Pin::static_mut(Box::leak(Box::new(
            crate::mac::tx::TxSlot::<512>::new_model().expect("model slot"),
        )));
        let TxSlotAccess::Free(memory) = slot.access() else {
            panic!("a new slot is free")
        };
        memory
    }
}
