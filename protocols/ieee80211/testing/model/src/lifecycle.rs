//! P2-owned driver and caller-backed local model registration.

use super::*;
use oer_ieee80211_contracts_port::{LowerMacEvent, LowerMacTerminal};
use oer_radio_port::{DisableEnd, StopDeadline};

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
        oer_ieee80211_contracts_port::ArbiterPool,
        oer_ieee80211_contracts_port::TransmitPool<ModelTickets, MODEL_TICKETS>,
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
        self.air.receive_agreements.take().clear();
        self.air.associations.set(InterfaceAssociations::default());
        self.air.observation.set(DriverObservation::default());
        let held = ModelHeld {
            air: self.air.clone(),
            commands: Bounded::new(MODEL_COMMANDS, "command"),
            recycled: Bounded::new(MODEL_RX_EVENTS, "recycle"),
            settings: Cell::default(),
            events: Bounded::new(MODEL_EVENTS, "event"),
            quarantined: Bounded::new(1, "answer shape rejection"),
            failed_group: Bounded::new(2, "failed GTK command"),
            failed_group_terminal: Bounded::new(2, "uncertain GTK terminal"),
            rx_out: Cell::new(0),
            network: Default::default(),
            lost: Cell::new(false),
            body: Cell::default(),
            gate_issuer,
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
                    admitted_at: None,
                    admitted: None,
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    loan: None,
                    memory: ModelMemory::Large(Box::new([0; ModelLarge::CAPACITY])),
                    admitted_at: None,
                    admitted: None,
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
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), oer_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        let queues = ModelQueues::prepared(queues)?;
        Ok((
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
        ))
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
                if held.air.retirement_held.get() {
                    held.air.driver.set(Some(cx.waker().clone()));
                    return Poll::Pending;
                }
                return Poll::Ready(());
            }
            if !held.air.failed() {
                if matches!(held.air.stream.get(), ModelStream::Draining) {
                    held.draining.set(true);
                }
                self.driver_state.serve(held);
                let driver = &self.driver_state;
                held.air.observation.set(driver.observation());
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
            held.air.driver.set(Some(cx.waker().clone()));
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
            || held.network.held() != 0
        {
            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
            core::future::pending::<()>().await;
        }
        self.channel = invocation.channel.get();
        self.air.driver.take();
        held.body.take();
        retire.requested().await
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
#[cfg(any(test, feature = "test-support"))]
impl<Id: SlotIdentity, C: Ieee80211Capabilities, X: ModelContext>
    oer_radio_port::conformance::registration::MockCleanup<Id> for ModelBackend<Id, C, X>
{
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
        self.air.driver.take();
        if let Some(invocation) = invocation {
            invocation.held.body.take();
        }
        Abandon::Stopped(authority.stopped())
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
    ) -> Result<Self::Body<'p>, oer_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        let resources = ModelResources::take_from(frontend.resources)?;
        Ok(InstallParts {
            view: ModelFrontend {
                air: frontend.held.air.clone(),
                held: frontend.held,
                channel: frontend.channel,
                addresses: frontend.addresses,
                loan,
            },
            resources: resources.lend(&loan),
        })
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

    /// The model reads the image clock, which only its owner advances: a
    /// deadline is checked before the drain starts, at every poll of it and
    /// before it succeeds, and `clock_advanced` wakes the wait.
    async fn disable<'a, 'p, S>(
        _call: Call,
        frontend: &'a mut Self::View<'p>,
        deadline: StopDeadline,
        sink: &'a mut S,
    ) -> Result<(), DisableEnd<ModelFault>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(Self::Terminal<'p>) -> core::ops::ControlFlow<Self::Terminal<'p>>,
    {
        let air = frontend.air.clone();
        let mut alarm = core::pin::pin!(frontend.air.until(deadline));
        let expired = move || deadline.is_some_and(|deadline| air.monotonic_now() >= deadline);
        // A deadline that passed already starts no drain.
        if expired() {
            return Err(DisableEnd::Expired);
        }
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
                return Poll::Ready(Err(DisableEnd::Poisoned(Poisoned { cause: ModelFault })));
            }
            // Checked before the drain: a drain that completes only after
            // the deadline issues no disabled owner.
            if expired() {
                return Poll::Ready(Err(DisableEnd::Expired));
            }
            if held.draining.get() || !held.commands.is_empty() || !held.recycled.is_empty() {
                // The drain stays with the driver: at the deadline only the
                // wait ends.
                if alarm.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(DisableEnd::Expired));
                }
                held.body.set(Some(cx.waker().clone()));
                held.air.wake_driver();
                return Poll::Pending;
            }
            while let Some(event) = held.take_event() {
                match event {
                    OwnedLowerMacEvent::Terminal(terminal) => {
                        if let core::ops::ControlFlow::Break(terminal) = sink(terminal.lend(&loan))
                        {
                            held.events.items.push_front(OwnedLowerMacEvent::Terminal(
                                OwnedLowerMacTerminal::retain(terminal, &loan),
                            ));
                            held.poison();
                            return Poll::Ready(Err(DisableEnd::Poisoned(Poisoned {
                                cause: ModelFault,
                            })));
                        }
                    }
                    OwnedLowerMacEvent::Received { mut frame, .. } => {
                        frame.recycled = true;
                        held.release_rx();
                        held.lost.set(true);
                    }
                }
            }
            if !held.air.group_rx.is_empty() {
                held.air.group_rx.clear();
                held.lost.set(true);
            }
            // A sink may run past the deadline: then no disabled owner.
            if expired() {
                return Poll::Ready(Err(DisableEnd::Expired));
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
    // The model's P2 owner answers in the first driver pass unless a test
    // holds it; the model has no bound of its own, so a stop's deadline is
    // the only one.
    fn shutdown_bound<'s>(
        frontend: &Self::Frontend<'s>,
        stop: StopDeadline,
    ) -> impl Future<Output = ()> + use<'s, Id, C, X>
    where
        Self: 's,
        Id: 's,
    {
        frontend.held.air.until(stop)
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
        self.air.driver.take();
        if let Some(Some(queues)) = queues {
            queues.held.body.take();
        }
    }
    fn end_vacant_world(vacant: &mut Self::Vacant) {
        vacant.air.end_installation();
        vacant.air.driver.take();
    }
}

impl<B: ModelComposition> ModelQueues<B> {
    fn prepared(backing: &mut Option<Self>) -> Result<&mut Self, oer_radio_port::SlotInitError> {
        backing
            .as_mut()
            .ok_or(oer_radio_port::SlotInitError::QueuesUnprepared)
    }
}
impl<B: ModelComposition> ModelResources<'static, B> {
    fn take_from(backing: &mut Option<Self>) -> Result<Self, oer_radio_port::SlotInitError> {
        backing
            .take()
            .ok_or(oer_radio_port::SlotInitError::MissingResource(
                "model resources",
            ))
    }
}
#[cfg(test)]
mod construction_tests {
    use super::*;
    enum Declaration {}
    type Backend = ModelBackend<oer_radio_port::StaticId<Declaration>>;
    #[test]
    fn unprepared_backing_returns_a_typed_error_without_minting_endpoints() {
        let mut backing: Option<ModelQueues<Backend>> = None;
        assert!(matches!(
            ModelQueues::prepared(&mut backing),
            Err(oer_radio_port::SlotInitError::QueuesUnprepared)
        ));
        assert!(backing.is_none());
    }
    #[test]
    fn absent_body_resources_return_a_typed_error_without_minting_capacity() {
        let mut backing: Option<ModelResources<'static, Backend>> = None;
        assert!(matches!(
            ModelResources::take_from(&mut backing),
            Err(oer_radio_port::SlotInitError::MissingResource(
                "model resources"
            ))
        ));
        assert!(backing.is_none());
    }
}
