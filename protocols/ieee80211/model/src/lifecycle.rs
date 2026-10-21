//! P2-owned driver and caller-backed local model registration.

use super::*;
use oer_ieee80211_port::{LowerMacEvent, LowerMacTerminal};

type CompositionIdentity<Id, C, X> = PhantomData<fn(Id, C, X) -> (Id, C, X)>;

/// The model's registered hardware and driver owner.
pub struct ModelBackend<
    Id: SlotIdentity,
    C: Ieee80211Capabilities = ModelCaps,
    X: ModelContext = (),
> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    driver_state: Driver<'static, Self>,
    identity: CompositionIdentity<Id, C, X>,
}

/// Stopped modeled hardware retained by its slot until a joined reinstallation.
pub struct ModelVacant<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    identity: CompositionIdentity<Id, C, X>,
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> ModelBackend<Id, C, X> {
    /// Construct modeled hardware and its external air observer. Only the
    /// supplied clock's owner advances time; radio time remains a projection.
    pub fn new(
        channel: Channel,
        addresses: [IndividualAddress; 2],
        clock: Rc<dyn Clock>,
    ) -> (Self, Rc<ModelAir>) {
        let air = Rc::new(ModelAir::new(clock));
        (Self::from_air(channel, addresses, air.clone()), air)
    }

    pub(super) fn from_air(
        channel: Channel,
        addresses: [IndividualAddress; 2],
        air: Rc<ModelAir>,
    ) -> Self {
        Self {
            air,
            channel,
            addresses,
            driver_state: Driver::new(),
            identity: PhantomData,
        }
    }

    /// Resume stopped hardware on P2; the slot requires its unique Joined first.
    pub fn resume(vacant: ModelVacant<Id, C, X>) -> Self {
        Self {
            air: vacant.air,
            channel: vacant.channel,
            addresses: vacant.addresses,
            driver_state: Driver::new(),
            identity: PhantomData,
        }
    }
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> ModelComposition
    for ModelBackend<Id, C, X>
{
    type Identity = Id;
    type Caps = C;
    type Contexts = X;
}

/// Separate model command/event storage and the one initial owned resource set.
pub struct ModelQueues<B: ModelComposition> {
    held: ModelHeld<'static, B>,
    resources: Option<ModelResources<'static, B>>,
    channel: Cell<Channel>,
    addresses: [IndividualAddress; 2],
}

/// Local P1 endpoints borrowing queue backing, with no borrow of the P2 driver.
pub struct ModelRawFrontend<'s, B: ModelComposition> {
    held: &'s ModelHeld<'static, B>,
    resources: &'s mut Option<ModelResources<'static, B>>,
    channel: &'s Cell<Channel>,
    addresses: [IndividualAddress; 2],
}

/// Local model P2 endpoints retained independently of every frontend future.
pub struct ModelInvocation<'s, B: ModelComposition> {
    held: &'s ModelHeld<'static, B>,
    channel: &'s Cell<Channel>,
}

/// The public body view over local queue endpoints and configuration cells.
pub struct ModelFrontend<'p, B: ModelComposition> {
    pub(super) air: Rc<ModelAir>,
    pub(super) held: &'p ModelHeld<'static, B>,
    pub(super) channel: &'p Cell<Channel>,
    pub(super) addresses: [IndividualAddress; 2],
    pub(super) loan: BodyLoan<'p, B::Identity, B>,
}

/// Disabled-only model settings, borrowing just the queue's channel cell.
pub struct ModelSetup<'a, C> {
    channel: &'a Cell<Channel>,
    caps: PhantomData<fn() -> C>,
}
impl<C: Ieee80211Capabilities> LowerMacSetup for ModelSetup<'_, C> {
    type Caps = C;
    fn set_channel(&mut self, channel: SupportedChannel<C>) {
        self.channel.set(channel.channel());
    }
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> SlotOwner<Id>
    for ModelBackend<Id, C, X>
{
    type Vacant = ModelVacant<Id, C, X>;
    type Pools = (
        oer_ieee80211_port::ArbiterPool,
        oer_ieee80211_port::TransmitPool<ModelTickets, MODEL_TICKETS>,
        oer_radio_port::Slots<ModelSmall, MODEL_SMALL_BUFFERS>,
        oer_radio_port::Slots<ModelLarge, MODEL_LARGE_BUFFERS>,
        oer_radio_port::Slots<ConnectionEpochKind, 1>,
        ModelAssociationPool<Station>,
        ModelAssociationPool<AccessPoint>,
        ModelBlockAckPool,
        oer_radio_port::Slots<ModelGroupPermission<Station>, 1>,
        (
            oer_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
            InterfaceCommandPool<Station>,
            InterfaceCommandPool<AccessPoint>,
        ),
    );
    type Queues = Option<ModelQueues<Self>>;
    const VACANT_QUEUES: Self::Queues = None;
    type Frontend<'s>
        = ModelRawFrontend<'s, Self>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = ModelInvocation<'s, Self>
    where
        Self: 's,
        Id: 's;

    fn prepare_queues(&mut self, backing: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        let (
            (gate, gate_issuer, arbiter_authority),
            (tickets, terminal_access),
            small,
            large,
            [epoch_slot],
            station_vacant,
            access_point_vacant,
            block_acks,
            [station_group_permission],
            ([access_point_group_permission], station_commands, access_point_commands),
        ) = mint.into_pools();
        let (station_association, station_bank) = station_vacant.into_parts();
        let (station_pairwise, station_group) = station_bank.into_parts();
        let (access_point_association, access_point_bank) = access_point_vacant.into_parts();
        let (access_point_pairwise, access_point_group) = access_point_bank.into_parts();
        self.air.stream.set(ModelStream::Running);
        self.air.receive_agreements.borrow_mut().clear();
        *self.air.associations.borrow_mut() = InterfaceAssociations::default();
        *self.air.observation.borrow_mut() = DriverObservation::default();
        let held = ModelHeld {
            air: self.air.clone(),
            commands: Bounded::new(MODEL_COMMANDS, "command"),
            recycled: Bounded::new(MODEL_RX_EVENTS, "recycle"),
            settings: RefCell::default(),
            events: Bounded::new(MODEL_EVENTS, "event"),
            quarantined: Bounded::new(1, "answer shape rejection"),
            failed_group: Bounded::new(2, "failed GTK command"),
            failed_group_terminal: Bounded::new(2, "uncertain GTK terminal"),
            rx_out: Cell::new(0),
            lost: Cell::new(false),
            body: RefCell::default(),
            gate_issuer: RefCell::new(gate_issuer),
            draining: Cell::new(false),
            returned: Cell::new(true),
        };
        let resources = ModelResources {
            station_association,
            access_point_association,
            station_commands,
            access_point_commands,
            connection_epoch: epoch_slot,
            tickets: tickets.into_iter().collect(),
            terminal_access,
            small: small
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    loan: None,
                    memory: ModelMemory::Small(Box::new([0; ModelSmall::CAPACITY])),
                    admitted_at: RadioInstant::from_micros(0),
                    admitted: false,
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    loan: None,
                    memory: ModelMemory::Large(Box::new([0; ModelLarge::CAPACITY])),
                    admitted_at: RadioInstant::from_micros(0),
                    admitted: false,
                })
                .collect(),
            station_pairwise,
            station_group,
            access_point_pairwise,
            access_point_group,
            block_acks,
            station_group_permission,
            access_point_group_permission,
            gate,
            arbiter_authority,
        };
        *backing = Some(ModelQueues {
            held,
            resources: Some(resources),
            channel: Cell::new(self.channel),
            addresses: self.addresses,
        });
    }

    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        #[expect(
            clippy::expect_used,
            reason = "PortSlot prepares queues before splitting a registration"
        )]
        let queues = queues.as_mut().expect("prepared model queues");
        (
            ModelRawFrontend {
                held: &queues.held,
                resources: &mut queues.resources,
                channel: &queues.channel,
                addresses: queues.addresses,
            },
            ModelInvocation {
                held: &queues.held,
                channel: &queues.channel,
            },
        )
    }

    async fn run<'s>(
        &mut self,
        invocation: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        let held = invocation.held;
        poll_fn(|cx| {
            held.air.passes.set(held.air.passes.get().saturating_add(1));
            if retire.is_requested() {
                return Poll::Ready(());
            }
            if !held.air.failed() {
                if matches!(held.air.stream.get(), ModelStream::Draining) {
                    held.draining.set(true);
                }
                self.driver_state.serve(held);
                let driver = &self.driver_state;
                *held.air.observation.borrow_mut() = driver.observation();
                let returned = driver
                    .queues
                    .iter()
                    .all(|queue| queue.air.is_none() && queue.waiting.is_empty())
                    && driver.removing.is_empty()
                    && !matches!(driver.gate, DriverGate::Closing(_));
                if held.returned.replace(returned) != returned {
                    held.wake_body();
                }
            }
            if held.air.failed() {
                held.wake_body();
            }
            *held.air.driver.borrow_mut() = Some(cx.waker().clone());
            let wait = retire.wait();
            let mut wait = core::pin::pin!(wait);
            wait.as_mut().poll(cx)
        })
        .await;
        if matches!(
            held.air.stream.get(),
            ModelStream::Ended(EventEnd::Quarantined { .. })
        ) || !held.returned.get()
            || !held.commands.is_empty()
            || !held.events.is_empty()
            || !held.quarantined.is_empty()
            || !held.failed_group.is_empty()
            || !held.failed_group_terminal.is_empty()
            || held.rx_out.get() != 0
        {
            retire.request_quarantine();
            core::future::pending::<()>().await;
        }
        self.channel = invocation.channel.get();
        self.air.driver.borrow_mut().take();
        held.body.borrow_mut().take();
        retire.requested().await
    }

    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        self.air.poison();
        self.air.end_installation();
        self.air.driver.borrow_mut().take();
        if let Some(invocation) = invocation {
            invocation.held.body.borrow_mut().take();
        }
        Abandon::Stopped(authority.stopped())
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        self.air.end_installation();
        ModelVacant {
            air: self.air.clone(),
            channel: self.channel,
            addresses: self.addresses,
            identity: PhantomData,
        }
    }
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> SlotBodyOwner<Id>
    for ModelBackend<Id, C, X>
{
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(
        frontend: &'p mut Self::Frontend<'s>,
        loan: BodyLoan<'p, Id, Self>,
    ) -> Self::Body<'p>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        #[expect(
            clippy::expect_used,
            reason = "the core issues the registration's sole body loan once"
        )]
        let resources = frontend
            .resources
            .take()
            .expect("one resource loan per registration");
        InstallParts {
            view: ModelFrontend {
                air: frontend.held.air.clone(),
                held: frontend.held,
                channel: frontend.channel,
                addresses: frontend.addresses,
                loan,
            },
            resources: resources.lend(&loan),
        }
    }
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> PortBackend<Id>
    for ModelBackend<Id, C, X>
{
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;
    type Refusal = ();
    type Event<'p>
        = LowerMacEvent<'p, ModelTypes<Self>>
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = LowerMacTerminal<'p, ModelTypes<Self>>
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = ModelFrontend<'p, Self>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = ModelResources<'p, Self>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = ModelSetup<'a, C>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = ModelPhase<'a, 'p, C, Self>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Uninstall = ();

    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        body
    }

    async fn enable<'a, 'p>(
        _call: Call,
        frontend: &'a mut Self::View<'p>,
    ) -> EnableResult<(), ModelFault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        if frontend.air.admission_closed() {
            Err(Poisoned { cause: ModelFault })
        } else if frontend.air.refuse_enable.replace(false) {
            Ok(Err(()))
        } else {
            Ok(Ok(()))
        }
    }

    async fn disable<'a, 'p>(
        _call: Call,
        frontend: &'a mut Self::View<'p>,
        sink: &'a mut dyn FnMut(Self::Terminal<'p>) -> core::ops::ControlFlow<Self::Terminal<'p>>,
    ) -> Result<(), Poisoned<ModelFault>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        frontend
            .air
            .disables
            .set(frontend.air.disables.get().saturating_add(1));
        let held = frontend.held;
        let loan = frontend.loan;
        held.draining.set(true);
        held.air.wake_driver();
        poll_fn(move |cx| {
            if held.air.failed() {
                return Poll::Ready(Err(Poisoned { cause: ModelFault }));
            }
            if held.draining.get() || !held.commands.is_empty() || !held.recycled.is_empty() {
                *held.body.borrow_mut() = Some(cx.waker().clone());
                held.air.wake_driver();
                return Poll::Pending;
            }
            while let Some(event) = held.take_event() {
                match event {
                    OwnedLowerMacEvent::Terminal(terminal) => {
                        if let core::ops::ControlFlow::Break(terminal) = sink(terminal.lend(&loan))
                        {
                            held.events.items.borrow_mut().push_front(
                                OwnedLowerMacEvent::Terminal(OwnedLowerMacTerminal::retain(
                                    terminal, &loan,
                                )),
                            );
                            held.poison();
                            return Poll::Ready(Err(Poisoned { cause: ModelFault }));
                        }
                    }
                    OwnedLowerMacEvent::Received { mut frame, .. } => {
                        frame.recycled = true;
                        held.release_rx();
                        held.lost.set(true);
                    }
                }
            }
            if !held.air.group_rx.borrow().is_empty() {
                held.air.group_rx.borrow_mut().clear();
                held.lost.set(true);
            }
            Poll::Ready(Ok(()))
        })
        .await
    }

    async fn now<'a, 'p>(
        _call: Call,
        frontend: &'a Self::View<'p>,
    ) -> PortResult<RadioInstant<Ieee80211Radio>, ClockError, ModelFault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Ok(frontend.air.radio_now())
    }

    fn setup<'a, 'p>(_call: Call, frontend: &'a mut Self::View<'p>) -> Self::Setup<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        ModelSetup {
            channel: frontend.channel,
            caps: PhantomData,
        }
    }

    fn phase<'a, 'p>(_call: Call, frontend: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        let held = frontend.held;
        ModelPhase {
            frontend,
            held,
            caps: PhantomData,
        }
    }

    fn request_uninstall<'s>(_: &mut Self::Frontend<'s>, (): ()) -> Result<(), Poisoned<ModelFault>>
    where
        Self: 's,
        Id: 's,
    {
        Ok(())
    }
    fn shutdown_bound<'s>(_: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<'s, Id, C, X>
    where
        Self: 's,
        Id: 's,
    {
        core::future::pending()
    }
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> LowerMacBackend<Id>
    for ModelBackend<Id, C, X>
{
    type Caps = C;
    const TX_QUEUE_DEPTH: core::num::NonZeroU8 = MODEL_TX_QUEUE_DEPTH;
    const PUBLICATION_BOUND: oer_time::Duration = MODEL_PUBLICATION_BOUND;
    const GROUP_COMMAND_BOUND: oer_time::Duration = MODEL_GROUP_COMMAND_BOUND;
}

impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext> ModelSlotOwner<Id>
    for ModelBackend<Id, C, X>
{
    fn end_world(&mut self, queues: Option<&mut Self::Queues>) {
        self.air.end_installation();
        self.air.driver.borrow_mut().take();
        if let Some(Some(queues)) = queues {
            queues.held.body.borrow_mut().take();
        }
    }
    fn end_vacant_world(vacant: &mut Self::Vacant) {
        vacant.air.end_installation();
        vacant.air.driver.borrow_mut().take();
    }
}
