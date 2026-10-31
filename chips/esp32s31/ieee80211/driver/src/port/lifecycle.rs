use super::*;
use radio::{EnableResult, PortBackend, SlotRetire, SlotStopped, SlotWakes};

pub(super) type PortCoreOwner<'id, Id, D, const B: usize> =
    crate::lower_mac::CoreOwner<Core<'id, Id, D, B>>;

pub struct Resources<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub tickets: [Slot<'p, Tickets, Part<Id, 2>>; TX_TICKETS],
    pub terminal_access: TerminalAccess<'p, Tickets, Part<Id, 2>>,
    pub buffers: [Buffer<'p, Id, D, B, O>; TX_TICKETS],
    pub connection_epoch: Slot<'p, ConnectionEpochKind, Part<Id, 4>>,
    pub station_association: core_mac::association::VacantStationAssociation<'p, CorePath<Id>>,
    pub access_point_association:
        core_mac::association::VacantAccessPointAssociation<'p, CorePath<Id>>,
    pub station_commands: InterfacePermissions<'p, Station, Part<Id, 8>>,
    pub access_point_commands: InterfacePermissions<'p, AccessPoint, Part<Id, 9>>,
    pub block_acks: [association::BlockAckSlot<'p, BlockAcks, 8, Part<Id, 7>>; 8],
    pub station_group_permission: keys::StaPermission<'p, Id>,
    pub access_point_group_permission: keys::ApPermission<'p, Id>,
    pub gate: GateOpen<'p, Part<Id, 1>>,
    pub arbiter_authority: ArbiterAuthority<'p, Part<Id, 1>>,
}
pub struct Queues<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    resources: Option<Resources<'static, Id, D, B, O>>,
    held: HeldBacking<'static, Id, D, B, O>,
    driver: driver::Driver<'static, Id, D, B, O>,
    addresses: Option<[IndividualAddress; 2]>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Queues<Id, D, B, O> {
    const fn vacant() -> Self {
        Self {
            resources: None,
            held: HeldBacking::vacant(),
            driver: driver::Driver::vacant(),
            addresses: None,
        }
    }
}
pub struct RawFrontend<'s, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    resources: &'s mut Option<Resources<'static, Id, D, B, O>>,
    storage: &'static PortSharedStorage<B>,
    commands: oer_memory::AffineSpscSender<
        's,
        driver::Command<'static, Id, D, B, O>,
        COMMANDS,
        oer_memory::CompactSpscCursor,
    >,
    events: oer_memory::AffineSpscReceiver<
        's,
        owned::OwnedEvent<'static, Id, D, B, O>,
        { TERMINAL_CAPACITY + TX_TICKETS },
        oer_memory::CompactSpscCursor,
    >,
    recycled: oer_memory::AffineSpscSender<
        's,
        ReceiveBuffer<'static, Id, D::Receive>,
        TX_TICKETS,
        oer_memory::CompactSpscCursor,
    >,
    failed_command: &'s oer_memory::OnceValue<driver::Command<'static, Id, D, B, O>>,
    failed_terminal: &'s oer_memory::OnceValue<owned::OwnedTerminal<'static, Id, D, B, O>>,
    gate: &'s Prepared<GateIssuer<'static, Part<Id, 1>>>,
    identity: &'s mut Cell<u64>,
    timer: D::Timer,
    clock_replies:
        oer_memory::AffineSpscReceiver<'s, handoff::ClockAnswer, 1, oer_memory::CompactSpscCursor>,
    clock_use: &'s mut Cell<handoff::ClockUse>,
    addresses: [IndividualAddress; 2],
}
pub struct Invocation<'s, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    held: Held<'s, 'static, Id, D, B, O>,
    driver: &'s mut driver::Driver<'static, Id, D, B, O>,
}
pub struct Frontend<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    held: FrontHeld<'p, Id, D, B, O>,
    addresses: [IndividualAddress; 2],
    loan: BodyLoan<'p, Id, Esp32s31Backend<Id, D, B, O>>,
}
pub struct BodyViews<'p, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    view: Frontend<'p, Id, D, B, O>,
    resources: Resources<'p, Id, D, B, O>,
}
// Keep queue progress and terminal conversion shared across concrete sinks.
// A terminal leaves this helper before its sink runs, so the two working
// frames do not overlap on the call stack.
impl<'p, Id: SlotIdentity + 'p, D: Device<B> + 'p, const B: usize, O: Observations>
    Frontend<'p, Id, D, B, O>
{
    #[inline(never)]
    fn poll_disable(
        &self,
    ) -> Poll<Result<Option<Terminal<'p, Id, D, B, O>>, radio::Poisoned<Fault>>> {
        let held = &self.held;
        while let Some(event) = held.events.try_receive().ok() {
            match event {
                owned::OwnedEvent::Terminal(terminal) => {
                    return Poll::Ready(Ok(Some(terminal.lend(&self.loan))));
                }
                owned::OwnedEvent::Received { frame, .. } => {
                    if let Err(frame) = held.recycled.try_send(frame).map_err(|error| error.0) {
                        core::mem::forget(frame);
                        held.storage.poison(Fault::Capacity);
                    }
                    held.loss.set(true);
                    held.storage.driver.wake();
                }
                owned::OwnedEvent::Tbtt(_) => held.loss.set(true),
            }
        }
        if let Err(error) = held.storage.check() {
            return Poll::Ready(Err(error));
        }
        if !held.enabled.get() && held.commands.is_empty() && held.recycled.is_empty() {
            Poll::Ready(Ok(None))
        } else {
            Poll::Pending
        }
    }

    #[inline(never)]
    fn reject_disable(
        &self,
        terminal: Terminal<'p, Id, D, B, O>,
    ) -> Result<(), radio::Poisoned<Fault>> {
        handoff::retain_terminal(
            self.held.failed_terminal,
            owned::OwnedTerminal::retain(terminal, &self.loan),
        );
        self.held.storage.poison(Fault::Abandoned);
        self.held.storage.check()
    }
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> SlotOwner<Id>
    for Esp32s31Backend<Id, D, B, O>
{
    type Vacant = StoppedPort<Id, D, B, O>;
    type Pools = (
        CorePools<TX_TICKETS>,
        ArbiterPool,
        TransmitPool<Tickets, TX_TICKETS>,
        radio::Slots<Buffers, TX_TICKETS>,
        radio::Slots<ConnectionEpochKind, 1>,
        radio::Slots<GroupPermission<Station>, 1>,
        radio::Slots<GroupPermission<AccessPoint>, 1>,
        association::BlockAckSlots<BlockAcks, 8>,
        InterfaceCommandPool<Station>,
        InterfaceCommandPool<AccessPoint>,
    );
    type Queues = Queues<Id, D, B, O>;
    const VACANT_QUEUES: Self::Queues = Queues::vacant();
    type Frontend<'s>
        = RawFrontend<'s, Id, D, B, O>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Invocation<'s, Id, D, B, O>
    where
        Self: 's,
        Id: 's;
    #[inline(never)]
    fn prepare_queues(
        &mut self,
        backing: &mut Self::Queues,
        mint: radio::Mint<'static, Self::Pools, Id>,
    ) {
        // Refuse before taking any resource: a second installation from this
        // backend, outstanding DMA or a joined installation's leftovers. The
        // mint drops unissued, and split reports the missing resources.
        backing.resources = None;
        backing.addresses = None;
        if !matches!(self.state, BackendState::Initial(Some(_)))
            || !self.storage.shared.returned.all_returned()
            || !backing.driver.retired()
            || !backing.held.drained()
        {
            return;
        }
        let BackendState::Initial(initial) = &mut self.state else {
            return;
        };
        let Some(initial) = initial.take() else {
            return;
        };
        self.storage.expired.set(0);
        let (
            core,
            (gate, issuer, arbiter_authority),
            (tickets, terminal_access),
            buffers,
            [connection_epoch],
            [station_group_permission],
            [access_point_group_permission],
            block_acks,
            station_commands,
            access_point_commands,
        ) = mint.into_pools();
        let CoreResources {
            authority,
            tickets: core_tickets,
            station_association,
            access_point_association,
            queues,
        } = core;
        let core = LowerMacCore::new(authority, initial.transmitter, initial.config, initial.coex);
        // Every allocation was returned (checked above): each place takes its own.
        let buffers = buffers.map(|place| {
            let (memory, returned) = self
                .storage
                .shared
                .returned
                .take(usize::from(place.index()))
                .unzip();
            Buffer {
                place,
                dma: buffer::ReturnableBuffer { memory, returned },
                loan: None,
                admission: None,
            }
        });
        backing.driver.prepare(
            core,
            core_tickets,
            queues,
            initial.observations,
            self.timer.now(),
        );
        backing.held.prepare(self.storage, issuer, self.channel);
        backing.resources = Some(Resources {
            tickets,
            terminal_access,
            buffers,
            connection_epoch,
            station_association,
            access_point_association,
            station_commands,
            access_point_commands,
            block_acks,
            station_group_permission,
            access_point_group_permission,
            gate,
            arbiter_authority,
        });
        backing.addresses = Some(self.addresses);
        self.state = BackendState::Running;
    }
    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), oer_radio_port::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        if queues.resources.is_none() {
            return Err(oer_radio_port::SlotInitError::MissingResource(
                "Wi-Fi installation resources",
            ));
        }
        let storage = *queues.held.storage;
        let (commands, driver_commands) = queues.held.commands.split();
        let (driver_events, events) = queues.held.events.split();
        let (recycled, driver_recycled) = queues.held.recycled.split();
        let (driver_clock_replies, clock_replies) = queues.held.clock_replies.split();
        Ok((
            RawFrontend {
                resources: &mut queues.resources,
                storage: &storage.shared,
                commands,
                events,
                recycled,
                failed_command: &queues.held.failed_command,
                failed_terminal: &queues.held.failed_terminal,
                gate: &queues.held.gate,
                identity: &mut queues.held.frontend_identity,
                timer: storage.device.borrow().timer().clone(),
                clock_replies,
                clock_use: &mut queues.held.clock_use,
                addresses: queues.addresses.ok_or(
                    oer_radio_port::SlotInitError::MissingResource("Wi-Fi addresses"),
                )?,
            },
            Invocation {
                held: Held {
                    storage,
                    commands: Incoming(driver_commands),
                    events: Outgoing(driver_events),
                    recycled: Incoming(driver_recycled),
                    rx_out: &queues.held.rx_out,
                    gate: &queues.held.gate,
                    failed_terminal: &queues.held.failed_terminal,
                    clock_replies: Outgoing(driver_clock_replies),
                },
                driver: &mut queues.driver,
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
        let held = &invocation.held;
        let storage = self.storage;
        if !matches!(self.state, BackendState::Running) {
            // Only a prepared installation registers; nothing to drive.
            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
            match core::future::pending::<core::convert::Infallible>().await {}
        }
        let state = &mut *invocation.driver;
        loop {
            poll_fn(|cx| {
                storage.shared.driver.register(cx.waker());
                if held.publishing.get() {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            let deadline = state.deadline(held);
            let timer = self.timer.clone();
            let mut delay = core::pin::pin!(timer.wait_until(deadline));
            let done = poll_fn(|cx| {
                storage.shared.driver.register(cx.waker());
                if storage.shared.poisoned.is_requested() {
                    retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
                    return Poll::Pending;
                }
                if held.publishing.get() {
                    return Poll::Pending;
                }
                if held.clock_replies.0.free_capacity() != 0 && held.clock_requested.replace(false)
                {
                    let answer = storage.device.borrow_mut().radio_now();
                    // Room was checked above: one outstanding clock latch.
                    if held.clock_replies.push(answer).is_err() {
                        storage.poison(Fault::Capacity);
                        retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
                        return Poll::Pending;
                    }
                    storage.shared.wake_body();
                }
                if retire.is_requested() {
                    match storage.device.borrow_mut().poll_stop(cx) {
                        Poll::Ready(Ok(())) => return Poll::Ready(true),
                        Poll::Ready(Err(cause)) => {
                            storage.poison(cause);
                            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
                            return Poll::Pending;
                        }
                        Poll::Pending => {
                            if delay.as_mut().poll(cx).is_ready() {
                                return Poll::Ready(false);
                            }
                            return Poll::Pending;
                        }
                    }
                }
                let mut device = storage.device.borrow_mut();
                if let Err(cause) = state.poll(cx, held, &mut *device) {
                    storage.poison(cause);
                    retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
                    return Poll::Pending;
                }
                drop(device);
                if state.deadline(held) < deadline {
                    return Poll::Ready(false);
                }
                let mut retiring = core::pin::pin!(retire.wait());
                if retiring.as_mut().poll(cx).is_ready() {
                    cx.waker().wake_by_ref();
                }
                if delay.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false);
                }
                Poll::Pending
            })
            .await;
            if done {
                break;
            }
        }
        if !held.commands.empty()
            || !held.events.empty()
            || !held.recycled.empty()
            || held.failed_terminal.is_requested()
            || held.rx_out.get() != 0
            || !storage.shared.returned.all_returned()
        {
            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
            core::future::pending::<()>().await;
        }
        self.channel = held.channel.get();
        for activity in &storage.shared.key_live {
            activity.set(0);
        }
        storage.device.borrow_mut().detach();
        retire.requested().await
    }

    fn finish<'s>(&mut self, invocation: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        // A stopped port without its transmitter, observation receiver or
        // channel refuses reinstall; release still takes its device.
        let mut reusable = match self.state {
            BackendState::Running => invocation.driver.retire(),
            _ => None,
        };
        let mut config = self.config;
        match self.channel.wifi_channel() {
            Some(channel) => config.channel = channel,
            None => reusable = None,
        }
        StoppedPort {
            storage: self.storage,
            reusable,
            config,
            addresses: self.addresses,
            coex: self.coex,
            identity: PhantomData,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> SlotBodyOwner<Id>
    for Esp32s31Backend<Id, D, B, O>
{
    type Body<'p>
        = BodyViews<'p, Id, D, B, O>
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
        let resources =
            frontend
                .resources
                .take()
                .ok_or(oer_radio_port::SlotInitError::MissingResource(
                    "Wi-Fi resources",
                ))?;
        Ok(BodyViews {
            view: Frontend {
                held: FrontHeld {
                    storage: frontend.storage,
                    commands: frontend.commands.borrow_local(),
                    events: frontend.events.borrow_local(),
                    recycled: frontend.recycled.borrow_local(),
                    failed_command: frontend.failed_command,
                    failed_terminal: frontend.failed_terminal,
                    gate: frontend.gate,
                    identity: frontend.identity,
                    timer: frontend.timer.clone(),
                    clock_replies: frontend.clock_replies.borrow_local(),
                    clock_use: frontend.clock_use,
                },
                addresses: frontend.addresses,
                loan,
            },
            resources: Resources {
                tickets: resources.tickets.map(|s| loan.lend_slot(s)),
                terminal_access: resources.terminal_access.lend(&loan),
                buffers: resources.buffers.map(|b| b.lend(&loan)),
                connection_epoch: loan.lend_slot(resources.connection_epoch),
                station_association: resources.station_association.lend(&loan),
                access_point_association: resources.access_point_association.lend(&loan),
                station_commands: resources.station_commands.lend(&loan),
                access_point_commands: resources.access_point_commands.lend(&loan),
                block_acks: resources.block_acks.map(|s| s.lend(&loan)),
                station_group_permission: loan.lend_slot(resources.station_group_permission),
                access_point_group_permission: loan
                    .lend_slot(resources.access_point_group_permission),
                gate: resources.gate.lend(&loan),
                arbiter_authority: resources.arbiter_authority.lend(&loan),
            },
        })
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> PortBackend<Id>
    for Esp32s31Backend<Id, D, B, O>
{
    type Domain = Ieee80211Radio;
    type Fault = Fault;
    type Refusal = Fault;
    type Uninstall = ();
    type Event<'p>
        = Event<'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = Terminal<'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = Frontend<'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = Resources<'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = Esp32s31Setup<'a, 'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = Esp32s31Phase<'a, 'p, Id, D, B, O>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    fn body_parts<'p>(body: Self::Body<'p>) -> radio::InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        radio::InstallParts {
            view: body.view,
            resources: body.resources,
        }
    }
    fn setup<'a, 'p>(_: radio::Call, view: &'a mut Self::View<'p>) -> Self::Setup<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Esp32s31Setup {
            held: &view.held,
            loan: view.loan,
        }
    }
    fn phase<'a, 'p>(_: radio::Call, view: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Esp32s31Phase {
            held: &view.held,
            addresses: view.addresses,
            loan: view.loan,
        }
    }
    fn enable<'a, 'p>(
        _: radio::Call,
        view: &'a mut Self::View<'p>,
    ) -> impl Future<Output = EnableResult<Fault, Fault>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        let held = &view.held;
        held.want_enabled.set(true);
        held.storage.driver.wake();
        poll_fn(move |cx| {
            held.storage.body.register(cx.waker());
            if let Err(error) = held.storage.check() {
                return Poll::Ready(Err(error));
            }
            if held.enabled.get() {
                Poll::Ready(Ok(Ok(())))
            } else {
                Poll::Pending
            }
        })
    }
    fn disable<'a, 'p, S>(
        _: radio::Call,
        view: &'a mut Self::View<'p>,
        sink: &'a mut S,
    ) -> impl Future<Output = Result<(), radio::Poisoned<Fault>>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(Terminal<'p, Id, D, B, O>) -> core::ops::ControlFlow<Terminal<'p, Id, D, B, O>>,
    {
        view.held.want_enabled.set(false);
        view.held.storage.driver.wake();
        poll_fn(move |cx| {
            view.held.storage.body.register(cx.waker());
            loop {
                match view.poll_disable() {
                    Poll::Ready(Ok(Some(terminal))) => {
                        if let core::ops::ControlFlow::Break(terminal) = sink(terminal) {
                            return Poll::Ready(view.reject_disable(terminal));
                        }
                    }
                    Poll::Ready(Ok(None)) => return Poll::Ready(Ok(())),
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        })
    }
    async fn now<'a, 'p>(
        _: radio::Call,
        view: &'a Self::View<'p>,
    ) -> radio::PortResult<Ieee80211Instant, ClockError, Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        view.held.now().await
    }
    fn request_uninstall<'s>(
        frontend: &mut Self::Frontend<'s>,
        _: (),
    ) -> Result<(), radio::Poisoned<Fault>>
    where
        Self: 's,
        Id: 's,
    {
        frontend.storage.driver.wake();
        frontend.storage.check()
    }
    #[allow(refining_impl_trait)]
    fn shutdown_bound<'s>(
        frontend: &Self::Frontend<'s>,
    ) -> impl Future<Output = ()> + use<Id, D, B, O>
    where
        Self: 's,
        Id: 's,
    {
        let timer = frontend.timer.clone();
        async move {
            timer
                .wait_until(timer.now().saturating_add(SHUTDOWN_BOUND))
                .await
        }
    }
}
