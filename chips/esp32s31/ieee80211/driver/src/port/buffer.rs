use super::keys::RawKeyIdentity;
use super::*;

pub struct BufferClass<const B: usize>;
impl<const B: usize> TxBufferClass for BufferClass<B> {
    const CAPACITY: usize = esp32s31_lower_mac_capabilities(B).max_mpdu_length as usize;
}

/// Affine memory owner. Only MPDU octets are lent to the portable core.
pub struct Buffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) place: Slot<'id, Buffers, Part<Id, 3>>,
    pub(super) dma: ReturnableBuffer<B>,
    pub(super) admission: Option<Instant>,
    pub(super) loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B>>>,
}
pub(super) struct ReturnableBuffer<const B: usize> {
    pub(super) memory: Option<TxSlotFree<'static, B>>,
    pub(super) returned: Option<oer_memory::ReturnToken<'static, TxSlotFree<'static, B>>>,
}
impl<const B: usize> Drop for ReturnableBuffer<B> {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            self.returned
                .take()
                .expect("buffer retains its sole return right")
                .return_value(memory);
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Buffer<'_, Id, D, B> {
    pub fn index(&self) -> u8 {
        self.place.index()
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> TxBufferMemory for Buffer<'_, Id, D, B> {
    fn renew_msdu(&mut self) {
        self.admission = None;
        self.dma
            .memory
            .as_mut()
            .expect("returned buffer owns free DMA")
            .reset_work();
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        let bytes = self
            .dma
            .memory
            .as_mut()
            .expect("body owns free DMA")
            .buffer_mut();
        &mut bytes[crate::ordinary_tx::TX_METADATA_SIZE..][..BufferClass::<B>::CAPACITY]
    }
}

pub struct Sent<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) mpdu: Mpdu<'id, Id, D, B>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Sent<'_, Id, D, B> {
    pub fn index(&self) -> u8 {
        self.mpdu.buffer.index()
    }
    pub fn packet_number(&self) -> Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber> {
        self.mpdu.pn
    }
    pub fn body(&self) -> Option<&D::Body> {
        self.mpdu.body.as_ref()
    }
    /// The encoded MPDU extent as its last publication left it in memory.
    #[cfg(test)]
    pub(crate) fn published(&mut self) -> &[u8] {
        let len = self.mpdu.layout.mpdu_len();
        self.mpdu.buffer.bytes_mut().get(..len).unwrap_or_default()
    }
}
pub enum AnyBuffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    Fresh(Buffer<'id, Id, D, B>),
    Sent(Sent<'id, Id, D, B>),
}

pub struct Mpdu<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) buffer: Buffer<'id, Id, D, B>,
    pub(super) layout: MpduLayout,
    pub(super) body: Option<D::Body>,
    pub(super) key: Option<RawKeyIdentity>,
    pub(super) pn: Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber>,
    pub(super) role: VifRole,
    pub(super) published: bool,
}
/// A view cannot move, replace or renew the admitted MPDU.
pub struct MpduExecution<'a, 'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub(super) mpdu: &'a mut Mpdu<'id, Id, D, B>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> ExecutionPayload for Mpdu<'id, Id, D, B> {
    type Execution<'a>
        = MpduExecution<'a, 'id, Id, D, B>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        MpduExecution { mpdu: self }
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> TxMpdu for Mpdu<'id, Id, D, B> {
    type Buffer = AnyBuffer<'id, Id, D, B>;
    type Body = D::Body;
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>) {
        if self.published {
            (AnyBuffer::Sent(Sent { mpdu: self }), None)
        } else {
            (AnyBuffer::Fresh(self.buffer), self.body)
        }
    }
}

pub struct Prepared<'k, 'p, 'id, Id: SlotIdentity, D: Device<B>, const B: usize, R, K> {
    mpdu: Mpdu<'id, Id, D, B>,
    plan: RetryPlan<PhyRate>,
    proof: PhantomData<(&'k (), fn(&'p ()) -> (&'p (), R, K))>,
}
pub struct Execution {
    pub(super) plan: RetryPlan<PhyRate>,
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, R, K> PreparedTx<'id>
    for Prepared<'_, '_, 'id, Id, D, B, R, K>
{
    type Types = Types<Id, D, B>;
    type Backend = Esp32s31Backend<Id, D, B>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = Mpdu<'static, Id, D, B>;
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
        let loan = self
            .mpdu
            .buffer
            .loan
            .expect("only lent buffers can enter frontend preparation");
        AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            payload: self.mpdu.into_owned(&loan),
            execution: Execution { plan: self.plan },
        }
    }
    fn into_parts(self) -> (AnyBuffer<'id, Id, D, B>, Option<D::Body>) {
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

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize> TxBufferPool<'id, BufferClass<B>>
    for Types<Id, D, B>
{
    type Buffer = Buffer<'id, Id, D, B>;
    type Sent = Sent<'id, Id, D, B>;
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, BufferClass<B>, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, BufferClass<B>, K, R>
    where
        'id: 'k,
    {
        let (mpdu, plan) = match source {
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
                )
            }
            CheckedTxSource::Retransmission { mpdu, plan } => {
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
                (mpdu.mpdu, stages)
            }
        };
        Ok(Prepared {
            mpdu,
            plan,
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
    fn erase(buffer: Self::Buffer) -> AnyBuffer<'id, Id, D, B> {
        AnyBuffer::Fresh(buffer)
    }
    fn erase_sent(buffer: Self::Sent) -> AnyBuffer<'id, Id, D, B> {
        AnyBuffer::Sent(buffer)
    }
    fn class(buffer: AnyBuffer<'id, Id, D, B>) -> Result<Self::Buffer, AnyBuffer<'id, Id, D, B>> {
        match buffer {
            AnyBuffer::Fresh(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn sent(buffer: AnyBuffer<'id, Id, D, B>) -> Result<Self::Sent, AnyBuffer<'id, Id, D, B>> {
        match buffer {
            AnyBuffer::Sent(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn renew(buffer: Self::Sent) -> (Self::Buffer, Option<D::Body>) {
        let mut memory = buffer.mpdu.buffer;
        memory.renew_msdu();
        (memory, buffer.mpdu.body)
    }
}

/// This port exposes ordinary MPDUs. Aggregate ownership is uninhabited,
/// and the optional `AmpduPhase` is not implemented.
pub struct NoAggregate<'id, Id: SlotIdentity, D, const B: usize, S> {
    pub(super) impossible: Infallible,
    marker: PhantomData<fn(&'id (), Id) -> (&'id (), Id, D, S)>,
}
pub struct NoAggregatePayload<'id, Id: SlotIdentity, D, const B: usize> {
    pub(super) impossible: Infallible,
    marker: PhantomData<fn(&'id (), Id) -> (&'id (), Id, D)>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> IntoIterator
    for NoAggregatePayload<'id, Id, D, B>
{
    type Item = Mpdu<'id, Id, D, B>;
    type IntoIter = core::iter::Empty<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        match self.impossible {}
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> ExecutionPayload
    for NoAggregatePayload<'_, Id, D, B>
{
    type Execution<'a>
        = Infallible
    where
        Self: 'a;
    fn execution(&mut self) -> Infallible {
        match self.impossible {}
    }
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, S: AggregateState>
    PhaseAggregate<'id> for NoAggregate<'id, Id, D, B, S>
{
    type Types = Types<Id, D, B>;
    type State = S;
    type Backend = Esp32s31Backend<Id, D, B>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = NoAggregatePayload<'static, Id, D, B>;
    type Execution = Infallible;
    type Cancelled = NoAggregatePayload<'id, Id, D, B>;
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

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize> PhaseTypes<'id>
    for Types<Id, D, B>
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
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = Prepared<'k, 'p, 'id, Id, D, B, R, K>;
    type KeyIdentity = keys::KeyIdentity<'id, Id>;
    type Aggregate<'k, 'p, R, K, S>
        = NoAggregate<'id, Id, D, B, S>
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
    type AnyTxBuffer = AnyBuffer<'id, Id, D, B>;
    type TxMpdu = Mpdu<'id, Id, D, B>;
    type TxAggregate = NoAggregatePayload<'id, Id, D, B>;
    type RxBuffer = ReceiveBuffer<'id, Id, D::Receive>;
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
