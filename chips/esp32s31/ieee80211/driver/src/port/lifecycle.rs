use super::*;
use radio::{EnableResult, PortBackend, SlotRetire, SlotStopped, SlotWakes};

pub(super) enum Bank<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    Disabled(DisabledCore<Core<'id, Id, D, B>>),
    DisabledClosed(DisabledCore<Core<'id, Id, D, B>, ClosedGate>),
    Open(EnabledCore<Core<'id, Id, D, B>>),
    Closing(EnabledCore<Core<'id, Id, D, B>, ClosingGate>),
    Closed(EnabledCore<Core<'id, Id, D, B>, ClosedGate>),
    Disabling(DisablingCore<Core<'id, Id, D, B>>),
    DisablingClosing(DisablingCore<Core<'id, Id, D, B>, ClosingGate>),
    DisablingClosed(DisablingCore<Core<'id, Id, D, B>, ClosedGate>),
    ConfigureOpen(Configuring<EnabledCore<Core<'id, Id, D, B>>>),
    ConfigureClosing(Configuring<EnabledCore<Core<'id, Id, D, B>, ClosingGate>>),
    ConfigureClosed(Configuring<EnabledCore<Core<'id, Id, D, B>, ClosedGate>>),
    Failed(GateOpenFailed<Core<'id, Id, D, B>>),
}
pub(super) struct CoreOwner<'id, Id: SlotIdentity, D: Device<B>, const B: usize>(
    Option<Bank<'id, Id, D, B>>,
);
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> CoreOwner<'id, Id, D, B> {
    pub(super) const fn vacant() -> Self {
        Self(None)
    }
    pub(super) fn prepare(&mut self, core: DisabledCore<Core<'id, Id, D, B>>) {
        assert!(self.0.is_none(), "retired core released its bank");
        self.0 = Some(Bank::Disabled(core));
    }
    pub(super) fn core(&mut self) -> &mut Core<'id, Id, D, B> {
        match self.0.as_mut().expect("core owner retains its bank") {
            Bank::Disabled(c) => c,
            Bank::DisabledClosed(c) => c,
            Bank::Open(c) => c,
            Bank::Closing(c) => c,
            Bank::Closed(c) => c,
            Bank::Disabling(c) => c,
            Bank::DisablingClosing(c) => c,
            Bank::DisablingClosed(c) => c,
            _ => panic!("only an idle command frontier lends the core"),
        }
    }
    pub(super) fn reset_contention(
        &mut self,
        role: VifRole,
        category: oer_ieee80211_wire::qos::WmmAccessCategory,
    ) {
        match self.0.as_mut().expect("bank retained") {
            Bank::ConfigureOpen(c) => c.reset_contention(role, category),
            Bank::ConfigureClosing(c) => c.reset_contention(role, category),
            Bank::ConfigureClosed(c) => c.reset_contention(role, category),
            _ => self.core().reset_contention(role, category),
        }
    }
    pub(super) fn disabled(&self) -> bool {
        matches!(self.0, Some(Bank::Disabled(_) | Bank::DisabledClosed(_)))
    }
    pub(super) fn take_transmitter(
        &mut self,
    ) -> OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B> {
        match self.0.take().expect("stopped bank retained") {
            Bank::Disabled(core) => core.dismantle().into_transmitter(),
            Bank::DisabledClosed(core) => core.dismantle().into_transmitter(),
            _ => panic!("normal uninstall requires the disabled core"),
        }
    }
    pub(super) fn configuring(&self) -> bool {
        matches!(
            self.0,
            Some(Bank::ConfigureOpen(_) | Bank::ConfigureClosing(_) | Bank::ConfigureClosed(_))
        )
    }
    pub(super) fn received<'frame>(
        &self,
        hardware: &D::Hardware,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        match self.0.as_ref().expect("bank retained") {
            Bank::Open(c) => c.received(hardware, frame),
            Bank::Closing(c) => c.received(hardware, frame),
            Bank::Closed(c) => c.received(hardware, frame),
            Bank::ConfigureOpen(c) => c.received(hardware, frame),
            Bank::ConfigureClosing(c) => c.received(hardware, frame),
            Bank::ConfigureClosed(c) => c.received(hardware, frame),
            _ => None,
        }
    }
    pub(super) fn deadline(&mut self) -> Option<Instant> {
        match self.0.as_ref().expect("bank retained") {
            Bank::ConfigureOpen(c) => c.next_deadline(),
            Bank::ConfigureClosing(c) => c.next_deadline(),
            Bank::ConfigureClosed(c) => c.next_deadline(),
            Bank::Failed(c) => c.next_deadline(),
            _ => self.core().next_deadline(),
        }
    }
    pub(super) fn tune(&mut self, channel: oer_ieee80211_wire::channel::WifiChannel) {
        macro_rules! tune {
            ($c:ident, $variant:ident) => {{
                $c.set_checked_channel(channel);
                $c.radio_tuned_to(channel);
                match $c.enable_checked() {
                    LifecycleStart::Enabled(c) => self.0 = Some(Bank::$variant(c)),
                    LifecycleStart::Retune(_) => unreachable!("device completed this exact retune"),
                }
            }};
        }
        match self.0.take().expect("disabled bank retained") {
            Bank::Disabled(mut c) => tune!(c, Open),
            Bank::DisabledClosed(mut c) => tune!(c, Closed),
            other => self.0 = Some(other),
        }
    }
    pub(super) fn submit(
        &mut self,
        hardware: &mut D::Hardware,
        queue: FreeQueue<'id, CorePath<Id>>,
        ticket: TxAdmission<'id, CorePath<Id>>,
        values: MpduValues<'static, B>,
    ) -> Result<(), core_mac::SubmitFault<'id, 'static, B, CorePath<Id>>> {
        match self.0.as_mut().expect("enabled bank retained") {
            Bank::Open(c) => c.submit_checked(hardware, queue, ticket, values),
            _ => panic!("publication requires an open enabled bank"),
        }
    }
    pub(super) fn service(
        &mut self,
        hardware: &mut D::Hardware,
        wake: WifiTxWake,
        sink: &mut driver::Sink<'id, Id, B>,
    ) -> Result<(), Fault> {
        match self.0.as_mut().expect("bank retained") {
            Bank::ConfigureOpen(c) => c.service(hardware, wake, sink),
            Bank::ConfigureClosing(c) => c.service(hardware, wake, sink),
            Bank::ConfigureClosed(c) => c.service(hardware, wake, sink),
            Bank::Failed(c) => c.service(hardware, wake, sink),
            _ => self.core().service(hardware, wake, sink),
        }
        .map_err(Fault::from)
    }
    pub(super) fn configure(&mut self, config: CoreVif) -> Result<(), Fault> {
        macro_rules! configure {
            ($c:ident, $variant:ident, $original:ident) => {
                match $c.configure(config) {
                    Ok(command) => {
                        self.0 = Some(Bank::$variant(command));
                        Ok(())
                    }
                    Err(refused) => {
                        self.0 = Some(Bank::$original(refused.core));
                        Err(Fault::Capacity)
                    }
                }
            };
        }
        match self.0.take().expect("bank retained") {
            Bank::Open(c) => configure!(c, ConfigureOpen, Open),
            Bank::Closing(c) => configure!(c, ConfigureClosing, Closing),
            Bank::Closed(c) => configure!(c, ConfigureClosed, Closed),
            other => {
                self.0 = Some(other);
                Err(Fault::Capacity)
            }
        }
    }
    pub(super) fn advance_configuration(
        &mut self,
        hardware: &mut D::Hardware,
        sink: &mut driver::Sink<'id, Id, B>,
    ) -> bool {
        macro_rules! advance {
            ($c:ident, $waiting:ident, $done:ident) => {
                match $c.advance(hardware, sink) {
                    ConfigurationProgress::Configured(c) => Bank::$done(c),
                    ConfigurationProgress::Waiting(c) => Bank::$waiting(c),
                }
            };
        }
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::ConfigureOpen(c) => advance!(c, ConfigureOpen, Open),
            Bank::ConfigureClosing(c) => advance!(c, ConfigureClosing, Closing),
            Bank::ConfigureClosed(c) => advance!(c, ConfigureClosed, Closed),
            other => other,
        });
        !self.configuring()
    }
    pub(super) fn hold(&mut self) {
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::Open(c) => Bank::Closing(c.hold_new_attempts()),
            other => other,
        });
    }
    pub(super) fn close(&mut self, hardware: &mut D::Hardware) -> bool {
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::Closing(c) => match c.close_gate(hardware) {
                GateClosure::Closed(c) => Bank::Closed(c),
                GateClosure::Closing(c) => Bank::Closing(c),
            },
            other => other,
        });
        matches!(self.0, Some(Bank::Closed(_)))
    }
    pub(super) fn open(
        &mut self,
        hardware: &mut D::Hardware,
        sink: &mut driver::Sink<'id, Id, B>,
    ) -> Result<(), Fault> {
        match self.0.take().expect("closed bank retained") {
            Bank::Closed(c) => match c.open_gate(hardware, sink) {
                Ok(c) => {
                    self.0 = Some(Bank::Open(c));
                    Ok(())
                }
                Err(c) => {
                    let fault = c.fault;
                    self.0 = Some(Bank::Failed(c));
                    Err(fault.into())
                }
            },
            other => {
                self.0 = Some(other);
                Err(Fault::Capacity)
            }
        }
    }
    pub(super) fn disable(
        &mut self,
        hardware: &mut D::Hardware,
        sink: &mut driver::Sink<'id, Id, B>,
    ) {
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::Open(c) => Bank::Disabling(c.disable_checked(hardware, sink)),
            Bank::Closing(c) => Bank::DisablingClosing(c.disable_checked(hardware, sink)),
            Bank::Closed(c) => Bank::DisablingClosed(c.disable_checked(hardware, sink)),
            other => other,
        });
    }
    pub(super) fn finish_disable(&mut self, sink: &mut driver::Sink<'id, Id, B>) -> bool {
        macro_rules! finish {
            ($c:ident, $waiting:ident, $done:ident) => {
                match $c.finish(sink) {
                    Ok(c) => Bank::$done(c),
                    Err(c) => Bank::$waiting(c),
                }
            };
        }
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::Disabling(c) => finish!(c, Disabling, Disabled),
            Bank::DisablingClosing(c) => match c.finish(sink) {
                Ok(_c) => panic!("gate close completes before disable"),
                Err(c) => Bank::DisablingClosing(c),
            },
            Bank::DisablingClosed(c) => finish!(c, DisablingClosed, DisabledClosed),
            other => other,
        });
        self.disabled()
    }
}

pub struct Resources<'p, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub tickets: [Slot<'p, Tickets, Part<Id, 2>>; TX_TICKETS],
    pub terminal_access: TerminalAccess<'p, Tickets, Part<Id, 2>>,
    pub buffers: [Buffer<'p, Id, D, B>; TX_TICKETS],
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
pub struct Queues<Id: SlotIdentity, D: Device<B>, const B: usize> {
    resources: Option<Resources<'static, Id, D, B>>,
    held: Held<'static, Id, D, B>,
    driver: driver::Driver<'static, Id, D, B>,
    addresses: Option<[IndividualAddress; 2]>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Queues<Id, D, B> {
    const fn vacant() -> Self {
        Self {
            resources: None,
            held: Held::vacant(),
            driver: driver::Driver::vacant(),
            addresses: None,
        }
    }
}
pub struct RawFrontend<'s, Id: SlotIdentity, D: Device<B>, const B: usize> {
    resources: &'s mut Option<Resources<'static, Id, D, B>>,
    held: &'s Held<'static, Id, D, B>,
    addresses: [IndividualAddress; 2],
}
pub struct Invocation<'s, Id: SlotIdentity, D: Device<B>, const B: usize> {
    held: &'s Held<'static, Id, D, B>,
    driver: &'s mut driver::Driver<'static, Id, D, B>,
}
pub struct Frontend<'p, Id: SlotIdentity, D: Device<B>, const B: usize> {
    held: &'p Held<'static, Id, D, B>,
    addresses: [IndividualAddress; 2],
    loan: BodyLoan<'p, Id, Esp32s31Backend<Id, D, B>>,
}
pub struct BodyViews<'p, Id: SlotIdentity, D: Device<B>, const B: usize> {
    view: Frontend<'p, Id, D, B>,
    resources: Resources<'p, Id, D, B>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> SlotOwner<Id> for Esp32s31Backend<Id, D, B> {
    type Vacant = StoppedPort<Id, D, B>;
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
    type Queues = Queues<Id, D, B>;
    const VACANT_QUEUES: Self::Queues = Queues::vacant();
    type Frontend<'s>
        = RawFrontend<'s, Id, D, B>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Invocation<'s, Id, D, B>
    where
        Self: 's,
        Id: 's;
    #[inline(never)]
    fn prepare_queues(
        &mut self,
        backing: &mut Self::Queues,
        mint: radio::Mint<'static, Self::Pools, Id>,
    ) {
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
        let BackendState::Initial(initial) = &mut self.state else {
            panic!("installation consumes initial resources once");
        };
        let initial = initial
            .take()
            .expect("installation retains initial resources");
        let core = LowerMacCore::new(authority, initial.transmitter, initial.config, initial.coex);
        let mut memory = initial.buffers.into_iter();
        let buffers = buffers.map(|place| {
            let slot = memory.next().expect("one DMA allocation per buffer place");
            let TxSlotAccess::Free(memory) = slot.access() else {
                panic!("installation takes only idle TX allocations")
            };
            Buffer {
                place,
                dma: buffer::ReturnableBuffer {
                    memory: Some(memory),
                    returned: &self.storage.returned,
                },
                loan: None,
                admission: None,
            }
        });
        backing
            .driver
            .prepare(core, core_tickets, queues, self.timer.now());
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
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        (
            RawFrontend {
                resources: &mut queues.resources,
                held: &queues.held,
                addresses: queues.addresses.expect("prepared slot addresses"),
            },
            Invocation {
                held: &queues.held,
                driver: &mut queues.driver,
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
        let storage = self.storage;
        assert!(
            matches!(self.state, BackendState::Running),
            "registered driver retains its state"
        );
        let state = &mut *invocation.driver;
        loop {
            let deadline = state.deadline(held);
            let timer = self.timer.clone();
            let mut delay = core::pin::pin!(timer.wait_until(deadline));
            let done = poll_fn(|cx| {
                storage.driver.register(cx.waker());
                if storage.poisoned.get().is_some() {
                    retire.request_quarantine();
                    return Poll::Pending;
                }
                if retire.is_requested() {
                    match storage.device.borrow_mut().poll_stop(cx) {
                        Poll::Ready(Ok(())) => return Poll::Ready(true),
                        Poll::Ready(Err(cause)) => {
                            storage.poison(cause);
                            retire.request_quarantine();
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
                    retire.request_quarantine();
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
            || held.failed_command.borrow().is_some()
            || held.failed_terminal.borrow().is_some()
            || held.rx_out.get() != 0
            || storage.returned.borrow().len() != TX_TICKETS
        {
            retire.request_quarantine();
            core::future::pending::<()>().await;
        }
        self.channel = held.channel.get();
        for activity in &storage.key_live {
            activity.set(0);
        }
        storage.device.borrow_mut().detach();
        retire.requested().await
    }
    fn abandon<'s>(
        &mut self,
        _: Option<&mut Self::Invocation<'s>>,
        call: radio::Quarantine,
    ) -> radio::Abandon
    where
        Self: 's,
        Id: 's,
    {
        self.storage.poison(Fault::Abandoned);
        if let Ok(mut device) = self.storage.device.try_borrow_mut() {
            if device.abandon() {
                return radio::Abandon::Stopped(call.stopped());
            }
        }
        radio::Abandon::Unconfirmed(call)
    }
    fn finish<'s>(&mut self, invocation: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        assert!(
            matches!(self.state, BackendState::Running),
            "stopped slot retains its driver"
        );
        let transmitter = invocation.driver.retire();
        let mut config = self.config;
        config.channel = self
            .channel
            .wifi_channel()
            .expect("S31 channel retained through setup");
        StoppedPort {
            storage: self.storage,
            transmitter: Some(transmitter),
            config,
            addresses: self.addresses,
            coex: self.coex,
            identity: PhantomData,
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> SlotBodyOwner<Id>
    for Esp32s31Backend<Id, D, B>
{
    type Body<'p>
        = BodyViews<'p, Id, D, B>
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
        let resources = frontend
            .resources
            .take()
            .expect("one body receives its resource loan");
        BodyViews {
            view: Frontend {
                held: frontend.held,
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
        }
    }
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize> PortBackend<Id> for Esp32s31Backend<Id, D, B> {
    type Domain = Ieee80211Radio;
    type Fault = Fault;
    type Refusal = Fault;
    type Uninstall = ();
    type Event<'p>
        = Event<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = Terminal<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = Frontend<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = Resources<'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = Esp32s31Setup<'a, 'p, Id, D, B>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = Esp32s31Phase<'a, 'p, Id, D, B>
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
            held: view.held,
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
            held: view.held,
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
        let held = view.held;
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
    fn disable<'a, 'p>(
        _: radio::Call,
        view: &'a mut Self::View<'p>,
        sink: &'a mut dyn FnMut(
            Terminal<'p, Id, D, B>,
        ) -> core::ops::ControlFlow<Terminal<'p, Id, D, B>>,
    ) -> impl Future<Output = Result<(), radio::Poisoned<Fault>>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        let held = view.held;
        let loan = view.loan;
        held.want_enabled.set(false);
        held.storage.driver.wake();
        poll_fn(move |cx| {
            held.storage.body.register(cx.waker());
            while let Some(event) = held.events.pop() {
                match event {
                    owned::OwnedEvent::Terminal(t) => {
                        if let core::ops::ControlFlow::Break(t) = sink(t.lend(&loan)) {
                            *held.failed_terminal.borrow_mut() =
                                Some(owned::OwnedTerminal::retain(t, &loan));
                            held.storage.poison(Fault::Abandoned);
                            return Poll::Ready(held.storage.check());
                        }
                    }
                    owned::OwnedEvent::Received { frame, .. } => {
                        if let Err(frame) = held.recycled.push(frame) {
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
            if !held.enabled.get() && held.commands.empty() && held.recycled.empty() {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
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
        view.held.storage.check()?;
        Ok(view.held.storage.device.borrow_mut().radio_now())
    }
    fn request_uninstall<'s>(
        frontend: &mut Self::Frontend<'s>,
        _: (),
    ) -> Result<(), radio::Poisoned<Fault>>
    where
        Self: 's,
        Id: 's,
    {
        frontend.held.storage.driver.wake();
        frontend.held.storage.check()
    }
    #[allow(refining_impl_trait)]
    fn shutdown_bound<'s>(frontend: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<Id, D, B>
    where
        Self: 's,
        Id: 's,
    {
        let timer = frontend.held.storage.device.borrow().timer().clone();
        async move {
            timer
                .wait_until(timer.now().saturating_add(SHUTDOWN_BOUND))
                .await
        }
    }
}
