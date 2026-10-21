use super::*;
use radio::{EnableResult, PortBackend};

pub(super) enum Bank<'id, D: Device<B>, const B: usize> {
    Disabled(DisabledCore<Core<'id, D, B>>),
    DisabledClosed(DisabledCore<Core<'id, D, B>, ClosedGate>),
    Open(EnabledCore<Core<'id, D, B>>),
    Closing(EnabledCore<Core<'id, D, B>, ClosingGate>),
    Closed(EnabledCore<Core<'id, D, B>, ClosedGate>),
    Disabling(DisablingCore<Core<'id, D, B>>),
    DisablingClosing(DisablingCore<Core<'id, D, B>, ClosingGate>),
    DisablingClosed(DisablingCore<Core<'id, D, B>, ClosedGate>),
    ConfigureOpen(Configuring<EnabledCore<Core<'id, D, B>>>),
    ConfigureClosing(Configuring<EnabledCore<Core<'id, D, B>, ClosingGate>>),
    ConfigureClosed(Configuring<EnabledCore<Core<'id, D, B>, ClosedGate>>),
    Failed(GateOpenFailed<Core<'id, D, B>>),
}
pub(super) struct CoreOwner<'id, D: Device<B>, const B: usize>(Option<Bank<'id, D, B>>);
impl<'id, D: Device<B>, const B: usize> CoreOwner<'id, D, B> {
    pub(super) fn new(core: DisabledCore<Core<'id, D, B>>) -> Self {
        Self(Some(Bank::Disabled(core)))
    }
    pub(super) fn core(&mut self) -> &mut Core<'id, D, B> {
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
        queue: FreeQueue<'id, CorePath<'id>>,
        ticket: TxAdmission<'id, CorePath<'id>>,
        values: MpduValues<'static, B>,
    ) -> Result<(), core_mac::SubmitFault<'id, 'static, B, CorePath<'id>>> {
        match self.0.as_mut().expect("enabled bank retained") {
            Bank::Open(c) => c.submit_checked(hardware, queue, ticket, values),
            _ => panic!("publication requires an open enabled bank"),
        }
    }
    pub(super) fn service(
        &mut self,
        hardware: &mut D::Hardware,
        wake: WifiTxWake,
        sink: &mut driver::Sink<'id, B>,
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
        sink: &mut driver::Sink<'id, B>,
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
        sink: &mut driver::Sink<'id, B>,
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
    pub(super) fn disable(&mut self, hardware: &mut D::Hardware, sink: &mut driver::Sink<'id, B>) {
        self.0 = Some(match self.0.take().expect("bank retained") {
            Bank::Open(c) => Bank::Disabling(c.disable_checked(hardware, sink)),
            Bank::Closing(c) => Bank::DisablingClosing(c.disable_checked(hardware, sink)),
            Bank::Closed(c) => Bank::DisablingClosed(c.disable_checked(hardware, sink)),
            other => other,
        });
    }
    pub(super) fn finish_disable(&mut self, sink: &mut driver::Sink<'id, B>) -> bool {
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

pub struct Resources<'id, D: Device<B>, const B: usize> {
    pub tickets: [Slot<'id, Tickets, Part<'id, 2>>; TX_TICKETS],
    pub terminal_access: TerminalAccess<'id, Tickets, Part<'id, 2>>,
    pub buffers: [Buffer<'id, B>; TX_TICKETS],
    pub connection_epoch: Slot<'id, ConnectionEpochKind, Part<'id, 4>>,
    pub station_association: core_mac::association::VacantStationAssociation<'id, CorePath<'id>>,
    pub access_point_association:
        core_mac::association::VacantAccessPointAssociation<'id, CorePath<'id>>,
    pub station_commands: InterfacePermissions<'id, Station, Part<'id, 8>>,
    pub access_point_commands: InterfacePermissions<'id, AccessPoint, Part<'id, 9>>,
    pub block_acks: [PhaseBlockAckSlot<'id, Types<D, B>>; 8],
    pub station_group_permission: keys::StaPermission<'id>,
    pub access_point_group_permission: keys::ApPermission<'id>,
    pub gate: GateOpen<'id, Part<'id, 1>>,
    pub arbiter_authority: ArbiterAuthority<'id, Part<'id, 1>>,
}

/// Body-side admission and interface configuration.
pub struct Frontend<D: Device<B>, const B: usize> {
    addresses: [IndividualAddress; 2],
    storage: &'static PortStorage<D, B>,
}

impl<D: Device<B>, const B: usize> PortBackend for Esp32s31Backend<D, B> {
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
    type Event<'id> = Event<'id, D, B>;
    type Terminal<'id> = Terminal<'id, D, B>;
    type Domain = Ieee80211Radio;
    type Fault = Fault;
    type Refusal = Fault;
    type Setup<'a, 'id>
        = Esp32s31Setup<'a, 'id, D, B>
    where
        Self: 'a,
        'id: 'a;
    type Phase<'a, 'id>
        = Esp32s31Phase<'a, 'id, D, B>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = Resources<'id, D, B>;
    type Held<'id> = Held<'id, D, B>;
    type Frontend<'id> = Frontend<D, B>;
    type Driver = &'static PortStorage<D, B>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = Uninstalled<D, B>;
    #[inline(always)]
    fn resources<'id>(
        &mut self,
        mint: radio::Mint<'id, Self::Pools>,
    ) -> radio::InstallParts<'id, Self> {
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
        let initial = self
            .initial
            .take()
            .expect("installation consumes initial resources once");
        let core = LowerMacCore::new(authority, initial.transmitter, initial.config, initial.coex);
        let mut memory = initial.buffers.into_iter();
        let buffers = buffers.map(|place| {
            let slot = memory.next().expect("one DMA allocation per buffer place");
            let TxSlotAccess::Free(memory) = slot.access() else {
                panic!("installation takes only idle TX allocations")
            };
            Buffer {
                place,
                memory: Some(memory),
                admission: None,
                returned: &self.storage.returned,
            }
        });
        radio::InstallParts {
            resources: Resources {
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
            },
            held: Held {
                storage: self.storage,
                state: RefCell::new(driver::Driver::new(
                    core,
                    core_tickets,
                    queues,
                    self.timer.now(),
                )),
                commands: Queue::new(),
                events: Queue::new(),
                rx_out: Cell::new(0),
                recycled: Queue::new(),
                failed_command: RefCell::new(None),
                failed_terminal: RefCell::new(None),
                gate: RefCell::new(issuer),
                enabled: Cell::new(false),
                want_enabled: Cell::new(false),
                channel: Cell::new(self.channel),
                edca: Cell::new([EdcaParameterSet::STANDARD_DEFAULT; 2]),
                dirty_edca: Cell::new(true),
                block_acks: RefCell::new([None; 8]),
                dirty_ba: Cell::new(false),
                color: Cell::new(None),
                loss: Cell::new(false),
                #[cfg(oer_debug_control)]
                publication_hold: Cell::new(0),
                #[cfg(oer_debug_control)]
                completion_hold: Cell::new(false),
            },
            frontend: Frontend {
                addresses: self.addresses,
                storage: self.storage,
            },
        }
    }
    fn setup<'a, 'id: 'a>(
        _: radio::Call,
        held: &'a Held<'id, D, B>,
        _frontend: &'a mut Self::Frontend<'id>,
    ) -> Self::Setup<'a, 'id>
    where
        Self: 'a,
    {
        Esp32s31Setup { held }
    }
    fn phase<'a, 'id: 'a>(
        _: radio::Call,
        held: &'a Held<'id, D, B>,
        frontend: &'a mut Self::Frontend<'id>,
    ) -> Self::Phase<'a, 'id>
    where
        Self: 'a,
    {
        Esp32s31Phase {
            held,
            addresses: frontend.addresses,
        }
    }
    fn enable<'a, 'id: 'a>(
        _: radio::Call,
        held: &'a Held<'id, D, B>,
        _frontend: &'a mut Self::Frontend<'id>,
    ) -> impl Future<Output = EnableResult<Fault, Fault>> + 'a {
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
    fn disable<'a, 'id: 'a>(
        _: radio::Call,
        held: &'a Held<'id, D, B>,
        _frontend: &'a mut Self::Frontend<'id>,
        sink: &'a mut dyn FnMut(Terminal<'id, D, B>) -> core::ops::ControlFlow<Terminal<'id, D, B>>,
    ) -> impl Future<Output = Result<(), radio::Poisoned<Fault>>> + 'a {
        held.want_enabled.set(false);
        held.storage.driver.wake();
        poll_fn(move |cx| {
            held.storage.body.register(cx.waker());
            while let Some(event) = held.events.pop() {
                match event {
                    LowerMacEvent::Terminal(terminal) => {
                        if let core::ops::ControlFlow::Break(terminal) = sink(terminal) {
                            *held.failed_terminal.borrow_mut() = Some(terminal);
                            held.storage.poison(Fault::Abandoned);
                            return Poll::Ready(held.storage.check());
                        }
                    }
                    LowerMacEvent::Received { frame, .. } => {
                        if let Err(frame) = held.recycled.push(frame) {
                            core::mem::forget(frame);
                            held.storage.poison(Fault::Capacity);
                        }
                        held.loss.set(true);
                        held.storage.driver.wake();
                    }
                    LowerMacEvent::Tbtt(_) => held.loss.set(true),
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
    async fn now<'a, 'id: 'a>(
        frontend: &'a Self::Frontend<'id>,
        _: radio::Call,
    ) -> radio::PortResult<Ieee80211Instant, ClockError, Fault> {
        frontend.storage.check()?;
        Ok(frontend.storage.device.borrow_mut().radio_now())
    }
    async fn drive<'p, 'id>(
        loan: radio::DriverLoan<'p, Self::Driver>,
        held: &'p Self::Held<'id>,
        retire: radio::Retire<'p>,
    ) -> radio::DriverExit<(), Fault> {
        loop {
            let deadline = held.state.borrow_mut().deadline(held);
            let timer = held.storage.device.borrow().timer().clone();
            let delay = timer.wait_until(deadline);
            let mut delay = core::pin::pin!(delay);
            let done = poll_fn(|cx| {
                held.storage.driver.register(cx.waker());
                if held.storage.poisoned.get().is_some() {
                    return Poll::Ready(true);
                }
                if retire.is_requested() || retire.is_abandoned() {
                    let result = held.storage.device.borrow_mut().poll_stop(cx);
                    match result {
                        Poll::Ready(Ok(())) => return Poll::Ready(true),
                        Poll::Ready(Err(cause)) => {
                            held.storage.poison(cause);
                            return Poll::Ready(true);
                        }
                        Poll::Pending => {
                            if held.state.borrow_mut().deadline(held) < deadline
                                || delay.as_mut().poll(cx).is_ready()
                            {
                                return Poll::Ready(false);
                            }
                            return Poll::Pending;
                        }
                    }
                }
                let mut device = held.storage.device.borrow_mut();
                if let Err(cause) = held.state.borrow_mut().poll(cx, held, &mut *device) {
                    held.storage.poison(cause);
                    return Poll::Ready(true);
                }
                drop(device);
                if held.state.borrow_mut().deadline(held) < deadline {
                    return Poll::Ready(false);
                }
                let mut retiring = core::pin::pin!(retire.wait());
                if retiring.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(true);
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
        if let Some(cause) = held.storage.poisoned.get() {
            let (_, published) = held.state.borrow_mut().poison.publish(cause);
            radio::DriverExit::Faulted {
                state: (),
                cause,
                poisoned: loan.fail_stop(published),
            }
        } else {
            retire.retired(loan, ()).await
        }
    }
    fn retire_bound(&self, _: radio::Call) -> impl Future<Output = ()> + use<D, B> {
        let timer = self.timer.clone();
        async move {
            timer
                .wait_until(timer.now().saturating_add(COMMAND_BOUND))
                .await
        }
    }
    fn detach<'id>(&mut self, _: radio::Detach<'_>, _: &Held<'id, D, B>) {
        self.storage.device.borrow_mut().detach();
    }
    #[inline(always)]
    fn uninstall<'id>(
        self,
        held: Held<'id, D, B>,
        _: Self::Frontend<'id>,
        retired: radio::Retired<'id, (Self::Driver, ())>,
        _: radio::Detached<'id>,
        _: (),
    ) -> Self::Uninstalled {
        let _ = retired.into_state();
        if self.storage.poisoned.get().is_some() {
            let _ = radio::Quarantined::new(held);
            return Uninstalled::ResetRequired;
        }
        for activity in &self.storage.key_live {
            activity.set(0);
        }
        let transmitter = held.state.borrow_mut().take_transmitter();
        let mut config = self.config;
        config.channel = held
            .channel
            .get()
            .wifi_channel()
            .expect("S31 channel retained through setup");
        Uninstalled::Stopped(StoppedPort {
            storage: self.storage,
            transmitter: Some(transmitter),
            config,
            addresses: self.addresses,
            coex: self.coex,
        })
    }
    fn abandon(&mut self, call: radio::Quarantine) -> radio::Abandon {
        self.storage.poison(Fault::Abandoned);
        if let Ok(mut device) = self.storage.device.try_borrow_mut() {
            if device.abandon() {
                return radio::Abandon::Stopped(call.stopped());
            }
        }
        radio::Abandon::Unconfirmed(call)
    }
}
