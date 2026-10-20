use super::keys::KeyIdentity;
use super::*;

pub struct BufferClass<const B: usize>;
impl<const B: usize> TxBufferClass for BufferClass<B> {
    const CAPACITY: usize = esp32s31_lower_mac_capabilities(B).max_mpdu_length as usize;
}

/// Affine memory owner. Only MPDU octets are lent to the portable core.
pub struct Buffer<'id, const B: usize> {
    pub(super) place: Slot<'id, Buffers, Part<3>>,
    pub(super) memory: Option<TxSlotFree<'static, B>>,
    pub(super) admission: Option<(Ieee80211Instant, Instant)>,
    pub(super) returned: &'static RefCell<heapless::Deque<TxSlotFree<'static, B>, TX_TICKETS>>,
}
impl<const B: usize> Drop for Buffer<'_, B> {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            if self.returned.borrow_mut().push_back(memory).is_err() {
                panic!("one returned allocation per minted buffer place");
            }
        }
    }
}
impl<const B: usize> Buffer<'_, B> {
    pub fn index(&self) -> u8 {
        self.place.index()
    }
    pub fn admitted_at(&self) -> Option<Ieee80211Instant> {
        self.admission.map(|(at, _)| at)
    }
}
impl<const B: usize> TxBufferMemory for Buffer<'_, B> {
    fn renew_msdu(&mut self) {
        self.admission = None;
        self.memory
            .as_mut()
            .expect("returned buffer owns free DMA")
            .reset_work();
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        let bytes = self
            .memory
            .as_mut()
            .expect("body owns free DMA")
            .buffer_mut();
        &mut bytes[crate::ordinary_tx::TX_METADATA_SIZE..][..BufferClass::<B>::CAPACITY]
    }
}

pub struct Sent<'id, D: Device<B>, const B: usize> {
    pub(super) mpdu: Mpdu<'id, D, B>,
}
impl<D: Device<B>, const B: usize> Sent<'_, D, B> {
    pub fn packet_number(&self) -> Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber> {
        self.mpdu.pn
    }
    pub fn body(&self) -> Option<&D::Body> {
        self.mpdu.body.as_ref()
    }
}
pub enum AnyBuffer<'id, D: Device<B>, const B: usize> {
    Fresh(Buffer<'id, B>),
    Sent(Sent<'id, D, B>),
}

pub struct Mpdu<'id, D: Device<B>, const B: usize> {
    pub(super) buffer: Buffer<'id, B>,
    pub(super) layout: MpduLayout,
    pub(super) body: Option<D::Body>,
    pub(super) key: Option<KeyIdentity>,
    pub(super) pn: Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber>,
    pub(super) role: VifRole,
    pub(super) published: bool,
}
/// A view cannot move, replace or renew the admitted MPDU.
pub struct MpduExecution<'a, 'id, D: Device<B>, const B: usize> {
    pub(super) mpdu: &'a mut Mpdu<'id, D, B>,
}
impl<'id, D: Device<B>, const B: usize> ExecutionPayload for Mpdu<'id, D, B> {
    type Execution<'a>
        = MpduExecution<'a, 'id, D, B>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        MpduExecution { mpdu: self }
    }
}
impl<'id, D: Device<B>, const B: usize> TxMpdu for Mpdu<'id, D, B> {
    type Buffer = AnyBuffer<'id, D, B>;
    type Body = D::Body;
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>) {
        if self.published {
            (AnyBuffer::Sent(Sent { mpdu: self }), None)
        } else {
            (AnyBuffer::Fresh(self.buffer), self.body)
        }
    }
}

pub struct Prepared<'k, 'p, 'id, D: Device<B>, const B: usize, R, K> {
    mpdu: Mpdu<'id, D, B>,
    plan: RetryPlan<PhyRate>,
    proof: PhantomData<(&'k (), fn(&'p ()) -> (&'p (), R, K))>,
}
pub struct Execution {
    pub(super) plan: RetryPlan<PhyRate>,
}
impl<'id, D: Device<B>, const B: usize, R, K> PreparedTx<'id>
    for Prepared<'_, '_, 'id, D, B, R, K>
{
    type Types = Types<D, B>;
    type Execution = Execution;
    fn into_admission(self) -> (Mpdu<'id, D, B>, Execution) {
        (self.mpdu, Execution { plan: self.plan })
    }
    fn into_parts(self) -> (AnyBuffer<'id, D, B>, Option<D::Body>) {
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

impl<'id, D: Device<B>, const B: usize> TxBufferPool<'id, BufferClass<B>> for Types<D, B> {
    type Buffer = Buffer<'id, B>;
    type Sent = Sent<'id, D, B>;
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
                        Some(R::tx_identity(&numbered.key)),
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
    fn erase(buffer: Self::Buffer) -> AnyBuffer<'id, D, B> {
        AnyBuffer::Fresh(buffer)
    }
    fn erase_sent(buffer: Self::Sent) -> AnyBuffer<'id, D, B> {
        AnyBuffer::Sent(buffer)
    }
    fn class(buffer: AnyBuffer<'id, D, B>) -> Result<Self::Buffer, AnyBuffer<'id, D, B>> {
        match buffer {
            AnyBuffer::Fresh(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn sent(buffer: AnyBuffer<'id, D, B>) -> Result<Self::Sent, AnyBuffer<'id, D, B>> {
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
pub struct NoAggregate<'id, D, const B: usize, S> {
    impossible: Infallible,
    marker: PhantomData<fn(&'id ()) -> (D, S)>,
}
pub struct NoAggregatePayload<'id, D, const B: usize> {
    impossible: Infallible,
    marker: PhantomData<fn(&'id ()) -> D>,
}
impl<'id, D: Device<B>, const B: usize> IntoIterator for NoAggregatePayload<'id, D, B> {
    type Item = Mpdu<'id, D, B>;
    type IntoIter = core::iter::Empty<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        match self.impossible {}
    }
}
impl<D: Device<B>, const B: usize> ExecutionPayload for NoAggregatePayload<'_, D, B> {
    type Execution<'a>
        = Infallible
    where
        Self: 'a;
    fn execution(&mut self) -> Infallible {
        match self.impossible {}
    }
}
impl<'id, D: Device<B>, const B: usize, S: AggregateState> PhaseAggregate<'id>
    for NoAggregate<'id, D, B, S>
{
    type Types = Types<D, B>;
    type State = S;
    type Execution = Infallible;
    type Cancelled = NoAggregatePayload<'id, D, B>;
    fn into_admission(self) -> (Self::Cancelled, Infallible)
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

impl<'id, D: Device<B>, const B: usize> PhaseTypes<'id> for Types<D, B> {
    const COMMAND_CAPACITY: usize = COMMANDS;
    const TERMINAL_CAPACITY: usize = TERMINALS;
    type StationCommandPath = Part<8>;
    type AccessPointCommandPath = Part<9>;
    type StationConfigureContext = D::ConfigureContext<'id>;
    type AccessPointConfigureContext = D::ConfigureContext<'id>;
    type EpochPath = Part<4>;
    type GatePath = Part<1>;
    type TicketPath = Part<2>;
    type BlockAckPath = Part<7>;
    type Fault = Fault;
    type Context = D::Context<'id>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = Prepared<'k, 'p, 'id, D, B, R, K>;
    type KeyIdentity = KeyIdentity;
    type Aggregate<'k, 'p, R, K, S>
        = NoAggregate<'id, D, B, S>
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
    type AnyTxBuffer = AnyBuffer<'id, D, B>;
    type TxMpdu = Mpdu<'id, D, B>;
    type TxAggregate = NoAggregatePayload<'id, D, B>;
    type RxBuffer = ReceiveBuffer<D::Receive>;
    type RxMeta = Esp32s31RxMeta;
    type StationKeys = PortKeys<Station>;
    type AccessPointKeys = PortKeys<AccessPoint>;
    type BlockAckKind = BlockAcks;
    type BlockAckPool = association::BlockAckSlots<BlockAcks, 8>;
    type BlockAck = ReceiveAgreement<'id>;
}

pub struct ReceiveAgreement<'id> {
    pub(super) slot: association::BlockAckSlot<'id, BlockAcks, 8, Part<7>>,
}
