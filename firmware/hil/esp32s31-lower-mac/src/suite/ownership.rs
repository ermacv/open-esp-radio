//! Two affine image-owned subpools backed by real S31 DMA allocations.
use super::*;
use core::sync::atomic::{AtomicU32, Ordering};
use oer_hil_protocol::lower_mac::{Trace, ownership_trace};
use oer_ieee80211_port::conformance::{self, Class, End, Operation, Resources, Scenario};
use oer_ieee80211_wire::qos::WmmAccessCategory;

pub(super) static DROPS: AtomicU32 = AtomicU32::new(0);
use conformance::allocations::{Allocation, Ids};
use core::cell::RefCell;
static BODY_BYTES: StaticCell<[[u8; 17]; 4]> = StaticCell::new();
static BODY_POOL: StaticCell<BodyPool> = StaticCell::new();
pub(super) struct BodyPool {
    ids: Ids<4>,
    slots: RefCell<[Option<&'static mut [u8; 17]>; 4]>,
}
impl BodyPool {
    pub(super) fn new() -> &'static Self {
        let mut slots = BODY_BYTES.init([[0; 17]; 4]).iter_mut();
        BODY_POOL.init(Self {
            ids: Ids::new(),
            slots: RefCell::new(core::array::from_fn(|_| slots.next())),
        })
    }
    fn allocate(&'static self, value: u8, len: usize) -> Result<FixtureBody, &'static str> {
        if len > 17 {
            return Err("body allocation extent");
        }
        let allocation = Allocation::new(&self.ids).ok_or("body allocation slots exhausted")?;
        let bytes = self
            .slots
            .borrow_mut()
            .get_mut(usize::from(allocation.id()))
            .and_then(Option::take)
            .ok_or("body slot owner missing")?;
        bytes.fill(value);
        Ok(FixtureBody {
            pool: self,
            bytes: Some(bytes),
            len,
            allocation,
        })
    }
}
pub(super) struct FixtureBody {
    pool: &'static BodyPool,
    bytes: Option<&'static mut [u8; 17]>,
    len: usize,
    allocation: Allocation<&'static Ids<4>, 4>,
}
impl Drop for FixtureBody {
    fn drop(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            let old = self
                .pool
                .slots
                .borrow_mut()
                .get_mut(usize::from(self.allocation.id()))
                .expect("allocation number names its static body slot")
                .replace(bytes);
            assert!(old.is_none(), "body allocation is returned once");
        }
    }
}
impl TxBody for FixtureBody {
    fn bytes(&self) -> &[u8] {
        self.bytes
            .as_deref()
            .and_then(|bytes| bytes.get(..self.len))
            .expect("live body owns its checked allocation extent")
    }
}
pub(super) enum Context {
    Plain,
    Owner {
        operation: Operation,
        resources: Resources,
        pointer: usize,
    },
}
impl Drop for Context {
    fn drop(&mut self) {
        if matches!(self, Self::Owner { .. }) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }
}
// The image records delivery in the existing journal; no separate buffer or
// retained marker is needed to compare publications after confirmation.
struct GateClosed;
impl oer_trace::Event for GateClosed {
    const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Ieee80211, 106);
    const CHANNEL: oer_trace::Channel = port::trace::Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [0, 0]
    }
    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0, 0]).then_some(Self)
    }
}
const _: () = {
    use oer_trace::Event;
    let kind = GateClosed::KIND.raw();
    assert!(kind != port::trace::Publication::KIND.raw());
    assert!(kind != port::trace::ENCODED_MPDU.raw());
    assert!(kind != port::trace::TerminalPosted::KIND.raw());
    assert!(kind != port::trace::EdcaTable::KIND.raw());
    assert!(kind != port::trace::EdcaRecord::KIND.raw());
    assert!(kind != port::trace::EdcaDraw::KIND.raw());
};

struct Small;
struct Large;
impl TxBufferClass for Small {
    const CAPACITY: usize = 128;
}
impl TxBufferClass for Large {
    const CAPACITY: usize = 256;
}
struct PoolBuffer<'id, const C: usize>(Buffer<'id, TX>);
impl<const C: usize> TxBufferMemory for PoolBuffer<'_, C> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self.0
            .bytes_mut()
            .get_mut(..C)
            .expect("image subpool fits physical DMA")
    }
    fn renew_msdu(&mut self) {
        self.0.renew_msdu();
    }
}
// Class membership is a partition of the original affine pool slots. It is
// recovered from the returned owner, independently of the operation context.
fn class(index: u8) -> Class {
    if index % 2 == 0 {
        Class::Small
    } else {
        Class::Large
    }
}
macro_rules! pool {
    ($class:ty, $capacity:literal, $parity:literal) => {
        impl<'id> TxBufferPool<'id, $class> for Family {
            type Buffer = PoolBuffer<'id, $capacity>;
            type Sent = Sent<'id, Device, TX>;
            fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
                source: CheckedPoolSource<'k, 'p, 'id, Self, $class, K, R>,
            ) -> PoolPreparation<'k, 'p, 'id, Self, $class, K, R>
            where
                'id: 'k,
            {
                let source = match source.into_source() {
                    TxSource::Fresh {
                        buffer,
                        frame,
                        body,
                        protection,
                    } => TxSource::Fresh {
                        buffer: buffer.0,
                        frame: ClassFrame::<BufferClass<TX>, K>::new(frame.into_frame())
                            .ok()
                            .expect("checked subset frame fits physical class"),
                        body,
                        protection,
                    },
                    TxSource::Retransmission { mpdu, plan } => {
                        TxSource::Retransmission { mpdu, plan }
                    }
                };
                // The subset only narrows writable capacity; the driver receives
                // its ordinary checked source and retains real DMA ownership.
                let checked = source
                    .check()
                    .ok()
                    .expect("unchanged checked subset layout");
                <Family as TxBufferPool<BufferClass<TX>>>::prepare::<R, K>(checked).map_err(
                    |refused| {
                        let operation = match refused.operation {
                            TxSource::Fresh {
                                buffer,
                                frame,
                                body,
                                protection,
                            } => TxSource::Fresh {
                                buffer: PoolBuffer(buffer),
                                frame: ClassFrame::new(frame.into_frame())
                                    .ok()
                                    .expect("original subset frame"),
                                body,
                                protection,
                            },
                            TxSource::Retransmission { mpdu, plan } => {
                                TxSource::Retransmission { mpdu, plan }
                            }
                        };
                        PreparationRefused {
                            reason: refused.reason,
                            operation,
                        }
                    },
                )
            }
            fn push_aggregate<'k, 'p, R, K, S>(
                _operation: CheckedPoolAggregate<'k, 'p, 'id, Self, $class, R, K, S>,
            ) -> AggregatePushResult<'k, 'p, 'id, Self, $class, R, K, S>
            where
                R: RoleKeys<'id, Self>,
                K: Ieee80211Capabilities,
                S: AggregateState,
                'id: 'k + 'p,
            {
                unreachable!("S31 has no constructible aggregate owner")
            }
            fn erase(buffer: Self::Buffer) -> AnyBuffer<'id, Device, TX> {
                AnyBuffer::Fresh(buffer.0)
            }
            fn erase_sent(buffer: Self::Sent) -> AnyBuffer<'id, Device, TX> {
                AnyBuffer::Sent(buffer)
            }
            fn class(
                buffer: AnyBuffer<'id, Device, TX>,
            ) -> Result<Self::Buffer, AnyBuffer<'id, Device, TX>> {
                match buffer {
                    AnyBuffer::Fresh(buffer) if buffer.index() % 2 == $parity => {
                        Ok(PoolBuffer(buffer))
                    }
                    other => Err(other),
                }
            }
            fn sent(
                buffer: AnyBuffer<'id, Device, TX>,
            ) -> Result<Self::Sent, AnyBuffer<'id, Device, TX>> {
                match buffer {
                    AnyBuffer::Sent(sent) if sent.index() % 2 == $parity => Ok(sent),
                    other => Err(other),
                }
            }
            fn renew(sent: Self::Sent) -> (Self::Buffer, Option<FixtureBody>) {
                let (buffer, body) = <Family as TxBufferPool<BufferClass<TX>>>::renew(sent);
                (PoolBuffer(buffer), body)
            }
        }
    };
}
pool!(Small, 128, 0);
pool!(Large, 256, 1);

struct Returns {
    rows: heapless::Vec<conformance::Returned, 8>,
    pointers: heapless::Vec<conformance::ReturnEvidence, 8>,
}
impl Returns {
    fn new() -> Self {
        Self {
            rows: heapless::Vec::new(),
            pointers: heapless::Vec::new(),
        }
    }
    fn len(&self) -> usize {
        self.rows.len()
    }
}
fn observe<'id>(
    context: Context,
    ticket: radio::Slot<'id, Tickets, <Family as PhaseTypes<'id>>::TicketPath>,
    buffer: AnyBuffer<'id, Device, TX>,
    body: Option<FixtureBody>,
    (end, evidence): (End, conformance::ReturnEvidence),
    seen: &mut Returns,
) -> Result<(), &'static str> {
    let Context::Owner {
        operation,
        resources,
        pointer,
    } = context
    else {
        return Err("ownership context");
    };
    let (buffer, body) = match buffer {
        AnyBuffer::Fresh(buffer) => (buffer, body),
        AnyBuffer::Sent(sent) => <Family as TxBufferPool<BufferClass<TX>>>::renew(sent),
    };
    let body = body.ok_or("lost external body")?;
    if !body.bytes().iter().all(|byte| *byte == operation.id) {
        return Err("changed external body");
    }
    seen.pointers
        .push(evidence.with_pointer(pointer == body.bytes().as_ptr() as usize))
        .map_err(|_| "too many pointer returns")?;
    seen.rows
        .push(conformance::Returned {
            operation,
            expected: resources,
            actual: Resources {
                ticket: ticket.index(),
                buffer: buffer.index() / 2,
                class: class(buffer.index()),
                body: usize::from(body.allocation.id()),
            },
            end,
        })
        .map_err(|_| "too many ownership returns")
}
pub(super) async fn run<'p, 'id>(
    mut owner: Owner<'p, 'id, Backend, radio::Enabled>,
    resources: <Backend as PortBackend>::Resources<'id>,
    peer: Peer,
    scenario: Scenario,
    body_pool: &'static BodyPool,
) -> Ended<'id, Backend, Result<Trace, &'static str>> {
    DROPS.store(0, Ordering::Relaxed);
    let mut access = resources.terminal_access;
    let mut seen = Returns::new();
    let result = async {
        let mut tickets = resources.tickets.into_iter();
        let mut buffers = resources.buffers.into_iter();
        let mut phase = owner.phase();
        phase.debug_hold_publication(WmmAccessCategory::BestEffort, true);
        oer_trace::start(port::trace::Publication::CHANNEL.mask());
        for operation in conformance::OPERATIONS {
            let ticket = tickets.next().ok_or("ownership ticket")?;
            let mut buffer = buffers.next().ok_or("ownership buffer")?;
            buffer.bytes_mut().fill(0);
            if class(buffer.index()) != operation.class {
                return Err("subpool partition");
            }
            let index = buffer.index() / 2;
            let actual_class = class(buffer.index());
            let body = body_pool.allocate(
                operation.id,
                if scenario == Scenario::Refuse { 17 } else { 4 },
            )?;
            let pointer = body.bytes().as_ptr() as usize;
            let body_id = body.allocation.id();
            let rate = phase
                .rate(PhyRate::Legacy(peer.rate))
                .ok_or("ownership rate")?;
            let header = HeaderSpec::new(
                FrameControl::new(0x0108).ok_or("data control")?,
                0,
                if scenario == Scenario::CloseGate {
                    IndividualAddress::new([2, 0, 0, 0, 0, 254]).ok_or("absent peer")?
                } else {
                    peer.address
                },
                peer.station,
                peer.address.get(),
                0,
                None,
                None,
                None,
            )
            .and_then(AckHeader::new)
            .ok_or("data header")?;
            let frame = TxFrame::Individual {
                frame: SizedFrame::new(header, MpduLength::new(40).ok_or("length")?)
                    .ok_or("sized data")?,
                plan: RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::new(2).ok_or("retry budget")?,
                    protection: Protection::None,
                }),
            };
            macro_rules! source {
                ($class:ty,$capacity:literal) => {{
                    let source = TxSource::Fresh {
                        buffer: PoolBuffer::<'id, $capacity>(buffer),
                        frame: ClassFrame::new(frame).map_err(|_| "ownership class")?,
                        body: Some(body),
                        protection: TxProtection::Open,
                    }
                    .check();
                    match source {
                        Ok(source) => Ok(
                            <Family as TxBufferPool<$class>>::prepare::<Station, Caps>(source)
                                .map_err(|_| "ownership preparation")?,
                        ),
                        Err(refused) => match refused.operation {
                            TxSource::Fresh { buffer, body, .. } => {
                                Err((AnyBuffer::Fresh(buffer.0), body))
                            }
                            _ => return Err("fresh ownership refusal"),
                        },
                    }
                }};
            }
            let source = match operation.class {
                Class::Small => source!(Small, 128),
                Class::Large => source!(Large, 256),
            };
            let context = Context::Owner {
                operation,
                pointer,
                resources: Resources {
                    ticket: ticket.index(),
                    buffer: index,
                    class: actual_class,
                    body: usize::from(body_id),
                },
            };
            match source {
                Ok(source) => phase
                    .station()
                    .submit(
                        Attempt {
                            context,
                            ticket,
                            source,
                            access_category: operation.category,
                            protection: Protection::None,
                            power: SupportedPower::new(TxPower::Calibrated).ok_or("power")?,
                            coex_operation: TxOperation::Data,
                        }
                        .admit(),
                    )
                    .map_err(|_| "ownership submit")?,
                Err((buffer, body)) => observe(
                    context,
                    ticket,
                    buffer,
                    body,
                    (End::Refused, conformance::ReturnEvidence::refused(false)),
                    &mut seen,
                )?,
            }
        }
        if scenario != Scenario::Refuse {
            while seen.len() < 2 {
                next(&mut phase, &mut access, &mut seen).await?;
            }
            match scenario {
                Scenario::Complete => {
                    phase.debug_hold_publication(WmmAccessCategory::BestEffort, false)
                }
                Scenario::CloseGate => phase.close_gate(resources.gate),
                Scenario::Disable => {}
                _ => return Err("physical ownership scenario required"),
            }
            if matches!(scenario, Scenario::Complete | Scenario::CloseGate) {
                while seen.len() < 4 {
                    next(&mut phase, &mut access, &mut seen).await?;
                }
            }
        }
        Ok(())
    }
    .await;
    let mut error = result.err();
    let owner = match owner
        .disable(&mut |terminal| {
            if let Err(why) = terminal_return(terminal, &mut access, &mut seen) {
                error = Some(why);
            }
            core::ops::ControlFlow::Continue(())
        })
        .await
    {
        Ok(owner) => owner,
        Err(failed) => {
            return Ended::fail_stop(failed.fail_stop(), Err("ownership disable"));
        }
    };
    let output = match error {
        Some(why) => Err(why),
        None => ownership_trace(conformance::Trace {
            returned: seen.rows,
            contexts_dropped: DROPS.load(Ordering::Relaxed) as usize,
            before_teardown_dropped: DROPS.load(Ordering::Relaxed) as usize,
            publications: TRACE
                .records()
                .filter(|record| record.decode::<port::trace::Publication>().is_some())
                .count(),
            terminal_evidence: seen.pointers,
            publication_events: TRACE.records().filter_map(|record| {
                if let Some(publication) = record.decode::<port::trace::Publication>() {
                    Some(conformance::PublicationEvent::Tx(publication.ticket))
                } else {
                    record
                        .decode::<GateClosed>()
                        .map(|_| conformance::PublicationEvent::GateClosed)
                }
            }),
        })
        .map(Trace::Ownership)
        .ok_or("ownership trace extent"),
    };
    Ended::uninstall(owner, (), output)
}
type Access<'id> = TerminalAccess<'id, Tickets, <Family as PhaseTypes<'id>>::TicketPath>;
fn terminal_return<'id>(
    terminal: LowerMacTerminal<'id, Family>,
    access: &mut Access<'id>,
    seen: &mut Returns,
) -> Result<(), &'static str> {
    match terminal {
        LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
            let parts = terminal.into_parts(access);
            let evidence = conformance::ReturnEvidence::terminal(false, &parts.outcome)
                .ok_or("ownership terminal evidence extent")?;
            let end = match parts.outcome {
                TxOutcome::Ended(_) => End::Ended,
                TxOutcome::Withdrawn(_) => End::Withdrawn,
                _ => return Err("ownership terminal outcome"),
            };
            let (buffer, body) = parts.payload.into_parts();
            observe(
                parts.context,
                parts.ticket,
                buffer,
                body,
                (end, evidence),
                seen,
            )
        }
        LowerMacTerminal::GateClosed(_) => {
            oer_trace::emit(&GateClosed);
            Ok(())
        }
        _ => Err("ownership terminal kind"),
    }
}
async fn next<'p, 'id: 'p>(
    phase: &mut Esp32s31Phase<'p, 'id, Device, TX>,
    access: &mut Access<'id>,
    seen: &mut Returns,
) -> Result<(), &'static str> {
    match phase
        .next_event()
        .await
        .map_err(|_| "ownership poisoned")?
        .map_err(|_| "ownership event loss")?
    {
        LowerMacEvent::Terminal(terminal) => terminal_return(terminal, access, seen)?,
        LowerMacEvent::Received { frame, .. } => phase.recycle(frame),
        LowerMacEvent::Tbtt(_) => {}
    }
    Ok(())
}
