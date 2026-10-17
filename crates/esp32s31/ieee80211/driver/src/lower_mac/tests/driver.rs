//! Test loop retaining the actual consuming production owners between calls.
use super::*;

#[derive(Debug, Eq, PartialEq)]
pub(super) enum TestStart {
    Enabled,
    Retune(WifiChannel),
}
enum State<'id> {
    Disabled(DisabledCore<DeviceCore<'id>>),
    DisabledClosing(DisabledCore<DeviceCore<'id>, ClosingGate>),
    DisabledClosed(DisabledCore<DeviceCore<'id>, ClosedGate>),
    Retuning(RetuningCore<DeviceCore<'id>>),
    RetuningClosing(RetuningCore<DeviceCore<'id>, ClosingGate>),
    RetuningClosed(RetuningCore<DeviceCore<'id>, ClosedGate>),
    Open(EnabledCore<DeviceCore<'id>>),
    Closing(EnabledCore<DeviceCore<'id>, ClosingGate>),
    Closed(EnabledCore<DeviceCore<'id>, ClosedGate>),
    Disabling(DisablingCore<DeviceCore<'id>>),
    DisablingClosing(DisablingCore<DeviceCore<'id>, ClosingGate>),
    DisablingClosed(DisablingCore<DeviceCore<'id>, ClosedGate>),
}
pub(super) struct TestDriver<'id>(Option<State<'id>>);
impl<'id> core::ops::Deref for TestDriver<'id> {
    type Target = DeviceCore<'id>;
    fn deref(&self) -> &Self::Target {
        match self.0.as_ref().unwrap() {
            State::Disabled(core) => core,
            State::DisabledClosing(core) => core,
            State::DisabledClosed(core) => core,
            State::Retuning(core) => core,
            State::RetuningClosing(core) => core,
            State::RetuningClosed(core) => core,
            State::Open(core) => core,
            State::Closing(core) => core,
            State::Closed(core) => core,
            State::Disabling(core) => core,
            State::DisablingClosing(core) => core,
            State::DisablingClosed(core) => core,
        }
    }
}
impl core::ops::DerefMut for TestDriver<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self.0.as_mut().unwrap() {
            State::Disabled(core) => core,
            State::DisabledClosing(core) => core,
            State::DisabledClosed(core) => core,
            State::Retuning(core) => core,
            State::RetuningClosing(core) => core,
            State::RetuningClosed(core) => core,
            State::Open(core) => core,
            State::Closing(core) => core,
            State::Closed(core) => core,
            State::Disabling(core) => core,
            State::DisablingClosing(core) => core,
            State::DisablingClosed(core) => core,
        }
    }
}
impl<'id> TestDriver<'id> {
    pub(super) fn new(core: DisabledCore<DeviceCore<'id>>) -> Self {
        Self(Some(State::Disabled(core)))
    }
    pub(super) fn enabled(core: EnabledCore<DeviceCore<'id>>) -> Self {
        Self(Some(State::Open(core)))
    }
    pub(super) fn take_enabled(self) -> EnabledCore<DeviceCore<'id>> {
        let Some(State::Open(core)) = self.0 else {
            panic!("enabled fixture")
        };
        core
    }
    pub(super) fn idle_disabled(&self) -> bool {
        matches!(
            self.0,
            Some(State::Disabled(_) | State::DisabledClosing(_) | State::DisabledClosed(_))
        )
    }
    pub(super) fn radio_tuned_to(&mut self, channel: WifiChannel) -> bool {
        match self.0.as_mut().unwrap() {
            State::Disabled(core) => core.radio_tuned_to(channel),
            State::DisabledClosing(core) => core.radio_tuned_to(channel),
            State::DisabledClosed(core) => core.radio_tuned_to(channel),
            _ => return false,
        }
        true
    }
    pub(super) fn set_checked_channel(&mut self, channel: WifiChannel) {
        match self.0.as_mut().unwrap() {
            State::Disabled(core) => core.set_checked_channel(channel),
            State::DisabledClosing(core) => core.set_checked_channel(channel),
            State::DisabledClosed(core) => core.set_checked_channel(channel),
            _ => panic!("disabled fixture"),
        }
    }
    pub(super) fn enable_checked(&mut self) -> TestStart {
        macro_rules! enable {
            ($core:ident, $enabled:ident, $retuning:ident) => {
                match $core.enable_checked() {
                    LifecycleStart::Enabled(core) => {
                        self.0 = Some(State::$enabled(core));
                        TestStart::Enabled
                    }
                    LifecycleStart::Retune(core) => {
                        let channel = core.target_channel();
                        self.0 = Some(State::$retuning(core));
                        TestStart::Retune(channel)
                    }
                }
            };
        }
        match self.0.take().unwrap() {
            State::Disabled(core) => enable!(core, Open, Retuning),
            State::DisabledClosing(core) => enable!(core, Closing, RetuningClosing),
            State::DisabledClosed(core) => enable!(core, Closed, RetuningClosed),
            _ => panic!("disabled fixture"),
        }
    }
    pub(super) fn finish_retune(&mut self, retuned: bool) -> bool {
        macro_rules! finish {
            ($core:ident, $enabled:ident, $disabled:ident) => {
                match $core.finish_retune(retuned) {
                    Ok(core) => State::$enabled(core),
                    Err(core) => State::$disabled(core),
                }
            };
        }
        self.0 = Some(match self.0.take().unwrap() {
            State::Retuning(core) => finish!(core, Open, Disabled),
            State::RetuningClosing(core) => finish!(core, Closing, DisabledClosing),
            State::RetuningClosed(core) => finish!(core, Closed, DisabledClosed),
            _ => panic!("retuning fixture"),
        });
        retuned
    }
    pub(super) fn submit_checked(
        &mut self,
        hardware: &mut Hardware,
        queue: FreeQueue<'id>,
        admission: TxAdmission<'id>,
        values: MpduValues<'static, 512>,
    ) -> Result<(), SubmitFault<'id, 'static, 512>> {
        match self.0.as_mut().unwrap() {
            State::Open(core) => core.submit_checked(hardware, queue, admission, values),
            State::Closing(core) => core.submit_checked(hardware, queue, admission, values),
            State::Closed(core) => core.submit_checked(hardware, queue, admission, values),
            _ => panic!("enabled fixture"),
        }
    }
    pub(super) fn received<'frame>(
        &self,
        hardware: &Hardware,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        match self.0.as_ref().unwrap() {
            State::Open(core) => core.received(hardware, frame),
            State::Closing(core) => core.received(hardware, frame),
            State::Closed(core) => core.received(hardware, frame),
            _ => None,
        }
    }
    pub(super) fn disable_checked(
        &mut self,
        hardware: &mut Hardware,
        events: &mut Events<'id>,
    ) -> Result<(), LowerMacFault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        self.0 = Some(match self.0.take().unwrap() {
            State::Open(core) => State::Disabling(core.disable_checked(hardware, events)),
            State::Closing(core) => State::DisablingClosing(core.disable_checked(hardware, events)),
            State::Closed(core) => State::DisablingClosed(core.disable_checked(hardware, events)),
            _ => panic!("enabled fixture"),
        });
        self.finish_disable(events);
        Ok(())
    }
    fn finish_disable(&mut self, events: &mut Events<'id>) {
        macro_rules! finish {
            ($core:ident, $disabled:ident, $draining:ident) => {
                match $core.finish(events) {
                    Ok(core) => State::$disabled(core),
                    Err(core) => State::$draining(core),
                }
            };
        }
        self.0 = Some(match self.0.take().unwrap() {
            State::Disabling(core) => finish!(core, Disabled, Disabling),
            State::DisablingClosing(core) => finish!(core, DisabledClosing, DisablingClosing),
            State::DisablingClosed(core) => finish!(core, DisabledClosed, DisablingClosed),
            state => state,
        });
    }
    pub(super) fn service(
        &mut self,
        hardware: &mut Hardware,
        wake: WifiTxWake,
        events: &mut Events<'id>,
    ) -> Result<(), LowerMacFault> {
        let result = core::ops::DerefMut::deref_mut(self).service(hardware, wake, events);
        self.finish_disable(events);
        result
    }
    pub(super) fn configure_checked_vif(
        &mut self,
        hardware: &mut Hardware,
        config: CoreVif,
    ) -> Result<(), UnsupportedVif> {
        let mut events = Events::default();
        macro_rules! configure {
            ($core:ident, $variant:ident) => {
                match $core.configure(config) {
                    Ok(command) => match command.advance(hardware, &mut events) {
                        ConfigurationProgress::Configured(core) => {
                            self.0 = Some(State::$variant(core));
                            Ok(())
                        }
                        ConfigurationProgress::Waiting(_) => {
                            panic!("this fixture configures an idle interface")
                        }
                    },
                    Err(refused) => {
                        self.0 = Some(State::$variant(refused.core));
                        Err(refused.reason)
                    }
                }
            };
        }
        match self.0.take().unwrap() {
            State::Disabled(core) => configure!(core, Disabled),
            State::DisabledClosing(core) => configure!(core, DisabledClosing),
            State::DisabledClosed(core) => configure!(core, DisabledClosed),
            State::Open(core) => configure!(core, Open),
            State::Closing(core) => configure!(core, Closing),
            State::Closed(core) => configure!(core, Closed),
            _ => panic!("configurable fixture"),
        }
    }
    pub(super) fn hold_new_attempts(&mut self) {
        let State::Open(core) = self.0.take().unwrap() else {
            panic!("open fixture")
        };
        self.0 = Some(State::Closing(core.hold_new_attempts()));
    }
    pub(super) fn block_held_queues(&mut self, hardware: &mut Hardware) {
        let State::Closing(core) = self.0.take().unwrap() else {
            panic!("closing fixture")
        };
        match core.close_gate(hardware) {
            GateClosure::Closed(core) => self.0 = Some(State::Closed(core)),
            GateClosure::Closing(_) => panic!("this fixture closes an idle publication prefix"),
        }
    }
    #[expect(clippy::result_large_err, reason = "test preserves the complete bank")]
    pub(super) fn open_gate(
        mut self,
        hardware: &mut Hardware,
        events: &mut Events<'id>,
    ) -> Result<Self, GateOpenFailed<DeviceCore<'id>>> {
        if matches!(self.0, Some(State::Closing(_))) {
            self.block_held_queues(hardware);
        }
        let State::Closed(core) = self.0.take().unwrap() else {
            panic!("closed fixture")
        };
        core.open_gate(hardware, events).map(Self::enabled)
    }
    #[expect(clippy::result_large_err, reason = "test preserves the complete bank")]
    #[expect(
        clippy::type_complexity,
        reason = "the fixture returns the actual hardware bank"
    )]
    pub(super) fn dismantle(
        self,
    ) -> Result<LowerMacCoreParts<'id, 'static, Power, fn() -> u32, Timer, 512>, Self> {
        match self.0 {
            Some(State::Disabled(core)) => Ok(core.dismantle()),
            Some(State::DisabledClosing(core)) => Ok(core.dismantle()),
            Some(State::DisabledClosed(core)) => Ok(core.dismantle()),
            state => Err(Self(state)),
        }
    }
}
