use super::keys::RawKeyIdentity;
use super::*;
use crate::mac::tx::TxBacking;
use oer_portable_memory::{IndexedStableDmaLease as _, StableDmaBacking as _};

pub struct BufferClass<const B: usize>;
impl<const B: usize> TxBufferClass for BufferClass<B> {
    const CAPACITY: usize = esp32s31_lower_mac_capabilities(B).max_mpdu_length as usize;
}

/// Affine memory owner. Only MPDU octets are lent to the portable core.
pub struct Buffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) place: Slot<'id, radio::Slots<Buffers, TX_BUFFERS>, Part<Id, 3>>,
    pub(super) dma: ReturnableBuffer<B>,
    pub(super) admission: Option<Instant>,
    pub(super) loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>,
}
type ReturnRight<const B: usize> = oer_portable_memory::ReturnToken<'static, TxBacking<B>>;

/// One buffer place's memory and the right that returns it to its bank,
/// held together: memory never exists here without its home.
enum Dma<const B: usize> {
    /// The CPU owns the memory; its return right travels with it. Dropping
    /// the buffer returns the memory to the bank.
    Free {
        memory: TxBacking<B>,
        home: ReturnRight<B>,
    },
    /// The hardware owns the memory of pool place `index`; its return
    /// right waits for the completion that hands that memory back. Dropped
    /// here, the memory stays out.
    Published { home: ReturnRight<B>, index: u8 },
    /// Neither: the place holds no memory, or its pair moved on.
    Released,
}

/// A buffer place's [`Dma`]; dropping it returns CPU-owned memory.
pub(super) struct ReturnableBuffer<const B: usize>(Dma<B>);
impl<const B: usize> ReturnableBuffer<B> {
    /// The pair the bank's place hands out, or a place without memory.
    pub(super) fn taken(pair: Option<(TxBacking<B>, ReturnRight<B>)>) -> Self {
        Self(match pair {
            Some((memory, home)) => Dma::Free { memory, home },
            None => Dma::Released,
        })
    }
    /// The CPU's whole buffer, from its metadata prefix, while it owns it.
    pub(super) fn memory_mut(&mut self) -> Option<&mut [u8]> {
        match &mut self.0 {
            Dma::Free { memory, .. } => Some(memory.stable_dma_region().into_mut_slice()),
            _ => None,
        }
    }
    /// Hand the memory to the hardware, keeping its home; `None`, and no
    /// change, unless the CPU owns it.
    pub(super) fn publish(&mut self) -> Option<TxBacking<B>> {
        match core::mem::replace(&mut self.0, Dma::Released) {
            Dma::Free { memory, home } => {
                self.0 = Dma::Published {
                    home,
                    index: memory.index(),
                };
                Some(memory)
            }
            other => {
                self.0 = other;
                None
            }
        }
    }
    /// The completion handed `memory` back: it rejoins its home. Refused,
    /// with the memory and no change, unless the hardware owned it and it
    /// is the memory it published: another place's never joins this home.
    pub(super) fn complete(&mut self, memory: TxBacking<B>) -> Result<(), TxBacking<B>> {
        match core::mem::replace(&mut self.0, Dma::Released) {
            Dma::Published { home, index } if index == memory.index() => {
                self.0 = Dma::Free { memory, home };
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
        if let Dma::Free { memory, home } = core::mem::replace(&mut self.0, Dma::Released) {
            home.return_value(memory);
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Buffer<'_, Id, D, B, O> {
    pub fn index(&self) -> u8 {
        self.place.index()
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> TxBufferMemory
    for Buffer<'_, Id, D, B, O>
{
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable> {
        self.dma.memory_mut().ok_or(BufferMemoryUnavailable)?;
        self.admission = None;
        Ok(())
    }
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable> {
        self.dma
            .memory_mut()
            .ok_or(BufferMemoryUnavailable)?
            .get_mut(crate::ordinary_tx::TX_METADATA_SIZE..)
            .and_then(|bytes| bytes.get_mut(..BufferClass::<B>::CAPACITY))
            .ok_or(BufferMemoryUnavailable)
    }
}

/// A network's memory is a pool buffer as a port buffer is: its frame
/// region starts behind the metadata word, on a word, and holds what a
/// port buffer's does ([`BufferClass`]).
const _: () = {
    if !crate::ordinary_tx::TX_METADATA_SIZE.is_multiple_of(4) {
        panic!("the MPDU starts on a word behind the metadata");
    }
};
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> NetworkCapacity
    for Types<Id, D, B, O>
{
    const NETWORK_CAPACITY: usize = BufferClass::<B>::CAPACITY;
}

/// A network's memory as one single MPDU's buffer ([`TxNetworkPool`]): the
/// pool buffer the network wrote one MSDU in, lent under the installation.
/// It has no bank place: its terminal returns the memory itself. Dropped
/// free (its owner abandoned it), the memory goes back to the pool, not to
/// the network: the next composition's claim of the network's places finds
/// it there.
pub struct NetworkBuffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) dma: NetworkDma<B>,
    pub(super) admission: Option<Instant>,
    pub(super) loan: Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>,
}

/// A network buffer's memory: the CPU's, or the hardware's from its
/// publication until its completion hands it back.
pub(super) enum NetworkDma<const B: usize> {
    Free(TxBacking<B>),
    /// The hardware owns the memory of pool place `index`.
    Published(u8),
}
impl<const B: usize> NetworkDma<B> {
    fn memory_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            Self::Free(memory) => Some(memory.stable_dma_region().into_mut_slice()),
            Self::Published(_) => None,
        }
    }
    fn publish(&mut self) -> Option<TxBacking<B>> {
        match core::mem::replace(self, Self::Published(0)) {
            // The memory leaves, its place recorded.
            Self::Free(memory) => {
                *self = Self::Published(memory.index());
                Some(memory)
            }
            Self::Published(index) => {
                *self = Self::Published(index);
                None
            }
        }
    }
    /// The completion handed `memory` back. Refused, with the memory and no
    /// change, unless the hardware owned it and it is the memory published.
    fn complete(&mut self, memory: TxBacking<B>) -> Result<(), TxBacking<B>> {
        match self {
            Self::Published(index) if *index == memory.index() => {
                *self = Self::Free(memory);
                Ok(())
            }
            Self::Published(_) | Self::Free(_) => Err(memory),
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> TxBufferMemory
    for NetworkBuffer<'_, Id, D, B, O>
{
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable> {
        self.dma.memory_mut().ok_or(BufferMemoryUnavailable)?;
        self.admission = None;
        Ok(())
    }
    /// The frame region from the MPDU's first octet, behind the metadata.
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable> {
        self.dma
            .memory_mut()
            .ok_or(BufferMemoryUnavailable)?
            .get_mut(crate::ordinary_tx::TX_METADATA_SIZE..)
            .and_then(|bytes| bytes.get_mut(..BufferClass::<B>::CAPACITY))
            .ok_or(BufferMemoryUnavailable)
    }
}

/// The memory an MPDU is in: a port buffer of its bank, or a network's.
pub enum MpduBuffer<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Bank(Buffer<'id, Id, D, B, O>),
    Network(NetworkBuffer<'id, Id, D, B, O>),
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    MpduBuffer<'id, Id, D, B, O>
{
    /// When the port admitted the MPDU, once it did.
    pub(super) fn admission(&self) -> Option<Instant> {
        match self {
            Self::Bank(buffer) => buffer.admission,
            Self::Network(buffer) => buffer.admission,
        }
    }
    /// The MSDU ages from `now`, its first admission.
    pub(super) fn admit(&mut self, now: Instant) {
        let admission = match self {
            Self::Bank(buffer) => &mut buffer.admission,
            Self::Network(buffer) => &mut buffer.admission,
        };
        admission.get_or_insert(now);
    }
    /// The CPU's whole memory, from its metadata prefix, while it owns it.
    pub(super) fn memory_mut(&mut self) -> Option<&mut [u8]> {
        match self {
            Self::Bank(buffer) => buffer.dma.memory_mut(),
            Self::Network(buffer) => buffer.dma.memory_mut(),
        }
    }
    /// Hand the memory to the hardware; `None`, and no change, unless the
    /// CPU owns it.
    pub(super) fn publish(&mut self) -> Option<TxBacking<B>> {
        match self {
            Self::Bank(buffer) => buffer.dma.publish(),
            Self::Network(buffer) => buffer.dma.publish(),
        }
    }
    /// The completion handed `memory` back; refused, with the memory, unless
    /// the hardware owned it.
    pub(super) fn complete(&mut self, memory: TxBacking<B>) -> Result<(), TxBacking<B>> {
        match self {
            Self::Bank(buffer) => buffer.dma.complete(memory),
            Self::Network(buffer) => buffer.dma.complete(memory),
        }
    }
    /// The bank place of a port buffer; `None` for a network's memory.
    #[cfg(all(oer_trace, oer_debug_control))]
    pub(super) fn index(&self) -> Option<u8> {
        match self {
            Self::Bank(buffer) => Some(buffer.index()),
            Self::Network(_) => None,
        }
    }
}

pub struct Sent<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) mpdu: MpduOf<Buffer<'id, Id, D, B, O>, D::Body>,
}
/// A published MPDU in a network's memory.
pub struct NetworkSent<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) mpdu: MpduOf<NetworkBuffer<'id, Id, D, B, O>, D::Body>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> NetworkSent<'_, Id, D, B, O> {
    pub fn packet_number(&self) -> Option<oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber> {
        self.mpdu.pn
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
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Sent<'_, Id, D, B, O> {
    pub fn index(&self) -> u8 {
        self.mpdu.buffer.index()
    }
    pub fn packet_number(&self) -> Option<oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber> {
        self.mpdu.pn
    }
    pub fn body(&self) -> Option<&D::Body> {
        self.mpdu.body.as_ref()
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
    /// A network's memory, never published.
    Network(NetworkBuffer<'id, Id, D, B, O>),
    /// A published MPDU in a network's memory.
    NetworkSent(NetworkSent<'id, Id, D, B, O>),
}

/// One admitted MPDU in its memory.
pub type Mpdu<'id, Id, D, const B: usize, O> =
    MpduOf<MpduBuffer<'id, Id, D, B, O>, <D as Device<B>>::Body>;

/// An MPDU in the memory `M`, with its external body `Body`.
pub struct MpduOf<M, Body> {
    pub(super) buffer: M,
    pub(super) layout: MpduLayout,
    pub(super) body: Option<Body>,
    pub(super) key: Option<RawKeyIdentity>,
    pub(super) pn: Option<oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber>,
    pub(super) role: VifRole,
    pub(super) published: bool,
    /// Whether its next publication carries the Retry bit: once decided by
    /// [`sets_retry_bit`](crate::ordinary_tx::sets_retry_bit), for the rest
    /// of the MPDU's life, across every `Sent` and retransmission.
    pub(super) retry: bool,
}
impl<M, Body> MpduOf<M, Body> {
    /// The same MPDU in the memory `buffer` makes of its own.
    pub(super) fn with_buffer<N>(self, buffer: impl FnOnce(M) -> N) -> MpduOf<N, Body> {
        MpduOf {
            buffer: buffer(self.buffer),
            layout: self.layout,
            body: self.body,
            key: self.key,
            pn: self.pn,
            role: self.role,
            published: self.published,
            retry: self.retry,
        }
    }
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
        match (self.published, self.buffer) {
            (true, MpduBuffer::Bank(buffer)) => (
                AnyBuffer::Sent(Sent {
                    mpdu: MpduOf {
                        buffer,
                        layout: self.layout,
                        body: self.body,
                        key: self.key,
                        pn: self.pn,
                        role: self.role,
                        published: true,
                        retry: self.retry,
                    },
                }),
                None,
            ),
            (true, MpduBuffer::Network(buffer)) => (
                AnyBuffer::NetworkSent(NetworkSent {
                    mpdu: MpduOf {
                        buffer,
                        layout: self.layout,
                        body: self.body,
                        key: self.key,
                        pn: self.pn,
                        role: self.role,
                        published: true,
                        retry: self.retry,
                    },
                }),
                None,
            ),
            (false, MpduBuffer::Bank(buffer)) => (AnyBuffer::Fresh(buffer), self.body),
            (false, MpduBuffer::Network(buffer)) => (AnyBuffer::Network(buffer), self.body),
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
pub struct Execution<Id: SlotIdentity> {
    pub(super) plan: RetryPlan<PhyRate>,
    /// A beacon's permission, which its terminal returns, and the instant
    /// it must be published by.
    pub(super) beacon: Option<(BeaconPermission<'static, Part<Id, 10>>, Instant)>,
}
impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations, R, K>
    PreparedTx<'id> for Prepared<'_, '_, 'id, Id, D, B, O, R, K>
{
    type Types = Types<Id, D, B, O>;
    type Backend = Esp32s31Backend<Id, D, B, O>;
    type OwnedContext = D::OwnedContext<Id>;
    type OwnedPayload = Mpdu<'static, Id, D, B, O>;
    type Execution = Execution<Id>;
    fn into_admission(
        self,
        context: D::Context<'id, Id>,
        ticket: Slot<'id, radio::Slots<Tickets, TX_TICKETS>, Part<Id, 2>>,
        _: Admission<Self>,
    ) -> AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, radio::Slots<Tickets, TX_TICKETS>, Part<Id, 2>>,
        Self::OwnedPayload,
        Execution<Id>,
    > {
        let loan = self.loan;
        AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            payload: self.mpdu.into_owned(&loan),
            execution: Execution {
                plan: self.plan,
                beacon: None,
            },
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
fn stages<K>(frame: &TxFrame<'_, K>) -> RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one(rate.rate().get()),
        TxFrame::Group { rate, .. } => one(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}

/// The memory of a buffer class as preparation takes it: its loan, and the
/// MPDU memory it becomes.
pub(super) trait ClassMemory<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>:
    Sized
{
    fn loan(&self) -> Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>>;
    fn into_mpdu(self) -> MpduBuffer<'id, Id, D, B, O>;
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    ClassMemory<'id, Id, D, B, O> for Buffer<'id, Id, D, B, O>
{
    fn loan(&self) -> Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>> {
        self.loan
    }
    fn into_mpdu(self) -> MpduBuffer<'id, Id, D, B, O> {
        MpduBuffer::Bank(self)
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    ClassMemory<'id, Id, D, B, O> for NetworkBuffer<'id, Id, D, B, O>
{
    fn loan(&self) -> Option<BodyLoan<'id, Id, Esp32s31Backend<Id, D, B, O>>> {
        self.loan
    }
    fn into_mpdu(self) -> MpduBuffer<'id, Id, D, B, O> {
        MpduBuffer::Network(self)
    }
}

/// A class's published MPDU as a retransmission's preparation takes it.
pub(super) trait ClassSent<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>:
    Sized
{
    type Memory: ClassMemory<'id, Id, D, B, O>;
    fn mpdu(&self) -> &MpduOf<Self::Memory, D::Body>;
    fn into_mpdu(self) -> MpduOf<Self::Memory, D::Body>;
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    ClassSent<'id, Id, D, B, O> for Sent<'id, Id, D, B, O>
{
    type Memory = Buffer<'id, Id, D, B, O>;
    fn mpdu(&self) -> &MpduOf<Self::Memory, D::Body> {
        &self.mpdu
    }
    fn into_mpdu(self) -> MpduOf<Self::Memory, D::Body> {
        self.mpdu
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    ClassSent<'id, Id, D, B, O> for NetworkSent<'id, Id, D, B, O>
{
    type Memory = NetworkBuffer<'id, Id, D, B, O>;
    fn mpdu(&self) -> &MpduOf<Self::Memory, D::Body> {
        &self.mpdu
    }
    fn into_mpdu(self) -> MpduOf<Self::Memory, D::Body> {
        self.mpdu
    }
}

/// A checked source of a class whose fresh memory is `M` and whose
/// published MPDU is `S`.
type ClassSource<'k, 'p, 'id, Id, D, const B: usize, O, M, S, C, K, R> = CheckedTxSource<
    M,
    S,
    ClassFrame<'p, C, K>,
    <D as Device<B>>::Body,
    <R as RoleKeys<'id, Types<Id, D, B, O>>>::TxKey<'k>,
    RetransmissionPlan<'p, K>,
>;
/// What a class's preparation returns.
type ClassPrepared<'k, 'p, 'id, Id, D, const B: usize, O, M, S, C, K, R> = Result<
    Prepared<'k, 'p, 'id, Id, D, B, O, R, K>,
    PreparationRefused<
        TxSource<
            M,
            S,
            ClassFrame<'p, C, K>,
            <D as Device<B>>::Body,
            <R as RoleKeys<'id, Types<Id, D, B, O>>>::TxKey<'k>,
            RetransmissionPlan<'p, K>,
        >,
    >,
>;

/// Prepare a source of any class: fresh memory records the installed key's
/// identity and the caller's PN; a retransmission keeps its own and only
/// checks that its key and response kind still hold.
#[allow(
    clippy::type_complexity,
    clippy::result_large_err,
    reason = "a refusal returns the class's whole source by value"
)]
fn prepare_class<
    'k,
    'p,
    'id: 'k,
    Id: SlotIdentity + 'id,
    D: Device<B>,
    const B: usize,
    O: Observations,
    M: ClassMemory<'id, Id, D, B, O>,
    S: ClassSent<'id, Id, D, B, O, Memory = M>,
    C: TxBufferClass,
    K: Ieee80211Capabilities,
    R: RoleKeys<'id, Types<Id, D, B, O>>,
>(
    source: ClassSource<'k, 'p, 'id, Id, D, B, O, M, S, C, K, R>,
) -> ClassPrepared<'k, 'p, 'id, Id, D, B, O, M, S, C, K, R> {
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
            // Only a buffer lent under the installation's brand enters
            // its preparation.
            let Some(loan) = buffer.loan() else {
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
                MpduOf {
                    buffer: buffer.into_mpdu(),
                    layout,
                    body,
                    key,
                    pn,
                    role: R::ROLE,
                    published: false,
                    retry: false,
                },
                plan,
                loan,
            )
        }
        CheckedTxSource::Retransmission { mpdu, plan } => {
            let Some(loan) = mpdu.mpdu().buffer.loan() else {
                return Err(PreparationRefused {
                    reason: PreparationError::MissingBodyLoan,
                    operation: TxSource::Retransmission { mpdu, plan },
                });
            };
            if mpdu.mpdu().key.as_ref().is_some_and(|key| !key.active()) {
                return Err(PreparationRefused {
                    reason: PreparationError::RetransmissionKey,
                    operation: TxSource::Retransmission { mpdu, plan },
                });
            }
            let same_role = mpdu.mpdu().role == R::ROLE;
            let stages = match (mpdu.mpdu().layout.kind(), &plan) {
                (MpduKind::Individual, RetransmissionPlan::Individual(plan)) if same_role => {
                    Some(plan.map(|rate| rate.get()))
                }
                (MpduKind::IndividualNoAck, RetransmissionPlan::IndividualNoAck(rate))
                    if same_role =>
                {
                    Some(one(rate.rate().get()))
                }
                (MpduKind::Group, RetransmissionPlan::Group(rate)) if same_role => {
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
            (mpdu.into_mpdu().with_buffer(M::into_mpdu), stages, loan)
        }
    };
    Ok(Prepared {
        mpdu,
        plan,
        loan,
        proof: PhantomData,
    })
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
        prepare_class(source)
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
    fn class_of(mpdu: &Mpdu<'id, Id, D, B, O>) -> Option<TxBufferForm> {
        // The bank's class: its MPDU comes back as its `into_parts` returns
        // it; a network's memory is another class's.
        match mpdu.buffer {
            MpduBuffer::Bank(_) => Some(if mpdu.published {
                TxBufferForm::Sent
            } else {
                TxBufferForm::Fresh
            }),
            MpduBuffer::Network(_) => None,
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations>
    TxBufferPool<'id, NetworkFrames<Types<Id, D, B, O>>> for Types<Id, D, B, O>
{
    type Buffer = NetworkBuffer<'id, Id, D, B, O>;
    type Sent = NetworkSent<'id, Id, D, B, O>;
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, NetworkFrames<Self>, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, NetworkFrames<Self>, K, R>
    where
        'id: 'k,
    {
        prepare_class(source)
    }
    fn erase(buffer: Self::Buffer) -> AnyBuffer<'id, Id, D, B, O> {
        AnyBuffer::Network(buffer)
    }
    fn erase_sent(buffer: Self::Sent) -> AnyBuffer<'id, Id, D, B, O> {
        AnyBuffer::NetworkSent(buffer)
    }
    fn class(
        buffer: AnyBuffer<'id, Id, D, B, O>,
    ) -> Result<Self::Buffer, AnyBuffer<'id, Id, D, B, O>> {
        match buffer {
            AnyBuffer::Network(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn sent(
        buffer: AnyBuffer<'id, Id, D, B, O>,
    ) -> Result<Self::Sent, AnyBuffer<'id, Id, D, B, O>> {
        match buffer {
            AnyBuffer::NetworkSent(buffer) => Ok(buffer),
            other => Err(other),
        }
    }
    fn renew(buffer: Self::Sent) -> (Self::Buffer, Option<D::Body>) {
        // The MPDU and its PN end here; `renew_msdu` releases its admission.
        (buffer.mpdu.buffer, buffer.mpdu.body)
    }
    fn class_of(mpdu: &Mpdu<'id, Id, D, B, O>) -> Option<TxBufferForm> {
        match mpdu.buffer {
            MpduBuffer::Network(_) => Some(if mpdu.published {
                TxBufferForm::Sent
            } else {
                TxBufferForm::Fresh
            }),
            MpduBuffer::Bank(_) => None,
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> TxNetworkPool<'id>
    for Types<Id, D, B, O>
{
    /// Behind the metadata word, as the buffer's own region.
    fn frame_region(memory: &mut TxBacking<B>) -> Option<&mut [u8]> {
        memory
            .stable_dma_region()
            .into_mut_slice()
            .get_mut(crate::ordinary_tx::TX_METADATA_SIZE..)
            .and_then(|bytes| bytes.get_mut(..BufferClass::<B>::CAPACITY))
    }
    fn into_memory(
        buffer: NetworkBuffer<'id, Id, D, B, O>,
    ) -> Result<TxBacking<B>, NetworkBuffer<'id, Id, D, B, O>> {
        match buffer.dma {
            NetworkDma::Free(memory) => Ok(memory),
            NetworkDma::Published(index) => Err(NetworkBuffer {
                dma: NetworkDma::Published(index),
                admission: buffer.admission,
                loan: buffer.loan,
            }),
        }
    }
}

impl<'id, Id: SlotIdentity + 'id, D: Device<B>, const B: usize, O: Observations> PortTypes<'id>
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
    type BeaconPath = Part<Id, 10>;
    type Fault = Fault;
    type Context = D::Context<'id, Id>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = Prepared<'k, 'p, 'id, Id, D, B, O, R, K>;
    type KeyIdentity = keys::KeyIdentity<'id, Id>;
    type TxBody = D::Body;
    type TicketKind = radio::Slots<Tickets, TX_TICKETS>;
    type AckEvidence = Ack;
    type BlockAckEvidence = BlockAckAnswer;
    type AirDenied = Infallible;
    type AnyTxBuffer = AnyBuffer<'id, Id, D, B, O>;
    type TxMpdu = Mpdu<'id, Id, D, B, O>;
    type DataMemory = TxBacking<B>;
    type RxBuffer = ReceiveBuffer<'id, Id, D::Receive>;
    type LentRx = super::LentReceive<D::Receive, D::Lent>;
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

    type Bank = oer_portable_memory::ReturnBank<TxBacking<512>, 1>;

    /// A bank whose one place holds a free model buffer.
    fn bank() -> &'static Bank {
        let bank = Box::leak(Box::new(Bank::new()));
        assert!(bank.seed(0, bank_memory()).is_ok());
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

    /// Free memory of another model pool.
    fn bank_memory() -> TxBacking<512> {
        crate::mac::tx::test_support::model_backing().expect("model buffer")
    }

    /// Two places of one pool: a completion returning another place's
    /// memory is refused, with that memory, and the buffer stays the
    /// hardware's until its own memory comes back.
    #[test]
    fn a_completion_with_another_place_s_memory_is_refused_and_moves_nothing() {
        use crate::mac::tx::test_support::{model_pool, pool_backing};
        let pool = model_pool::<512, 2>().expect("model pool");
        let other = pool_backing(pool, 1).expect("the second place");
        let bank = Box::leak(Box::new(Bank::new()));
        assert!(
            bank.seed(0, pool_backing(pool, 0).expect("the first place"))
                .is_ok()
        );
        let mut buffer = ReturnableBuffer::taken(bank.take(0));
        let own = buffer.publish().expect("the CPU owned it");
        let Err(other) = buffer.complete(other) else {
            panic!("another place's memory never joins this home");
        };
        assert!(buffer.memory_mut().is_none(), "still the hardware's");
        assert!(buffer.complete(own).is_ok(), "its own memory rejoins");
        // The network's memory checks its place the same way.
        let mut network = NetworkDma::Free(other);
        let published = network.publish().expect("the CPU owned it");
        let own = buffer.publish().expect("free again");
        let Err(own) = network.complete(own) else {
            panic!("another place's memory never takes this one's");
        };
        assert!(network.memory_mut().is_none(), "still the hardware's");
        assert!(network.complete(published).is_ok());
        assert!(buffer.complete(own).is_ok());
    }
}
