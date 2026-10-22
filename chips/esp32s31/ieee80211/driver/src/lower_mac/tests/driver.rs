//! Fixtures retain the same in-place owner as production between calls.
use super::*;
#[derive(Debug, Eq, PartialEq)]
pub(super) enum TestStart {
    Enabled,
    Retune(WifiChannel),
}
pub(super) struct TestDriver<'id>(pub(super) CoreOwner<DeviceCore<'id>>);
impl<'id> core::ops::Deref for TestDriver<'id> {
    type Target = DeviceCore<'id>;
    fn deref(&self) -> &Self::Target {
        self.0.core.as_ref().unwrap()
    }
}
impl core::ops::DerefMut for TestDriver<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.core.as_mut().unwrap()
    }
}
impl<'id> TestDriver<'id> {
    pub(super) fn new(core: CoreOwner<DeviceCore<'id>>) -> Self {
        Self(core)
    }
    pub(super) fn take_enabled(&mut self) -> EnabledView<'_, DeviceCore<'id>> {
        let CoreView::Open(view) = self.0.view() else {
            panic!("enabled fixture")
        };
        view
    }
    pub(super) fn idle_disabled(&mut self) -> bool {
        matches!(
            self.0.view(),
            CoreView::Disabled(_) | CoreView::DisabledClosing(_) | CoreView::DisabledClosed(_)
        )
    }
    pub(super) fn radio_tuned_to(&mut self, channel: WifiChannel) -> bool {
        match self.0.view() {
            CoreView::Disabled(mut view) => {
                view.radio_tuned_to(channel);
                true
            }
            CoreView::DisabledClosing(mut view) => {
                view.radio_tuned_to(channel);
                true
            }
            CoreView::DisabledClosed(mut view) => {
                view.radio_tuned_to(channel);
                true
            }
            _ => false,
        }
    }
    pub(super) fn set_checked_channel(&mut self, channel: WifiChannel) {
        match self.0.view() {
            CoreView::Disabled(mut view) => {
                view.set_checked_channel(channel);
            }
            CoreView::DisabledClosing(mut view) => {
                view.set_checked_channel(channel);
            }
            CoreView::DisabledClosed(mut view) => {
                view.set_checked_channel(channel);
            }
            _ => panic!("disabled fixture"),
        }
    }
    pub(super) fn enable_checked(&mut self) -> TestStart {
        match self.0.view() {
            CoreView::Disabled(view) => match view.enable_checked() {
                LifecycleStart::Enabled(_) => TestStart::Enabled,
                LifecycleStart::Retune(retuning) => TestStart::Retune(retuning.target_channel()),
            },
            CoreView::DisabledClosing(view) => match view.enable_checked() {
                LifecycleStart::Enabled(_) => TestStart::Enabled,
                LifecycleStart::Retune(retuning) => TestStart::Retune(retuning.target_channel()),
            },
            CoreView::DisabledClosed(view) => match view.enable_checked() {
                LifecycleStart::Enabled(_) => TestStart::Enabled,
                LifecycleStart::Retune(retuning) => TestStart::Retune(retuning.target_channel()),
            },
            _ => panic!("disabled fixture"),
        }
    }
    pub(super) fn finish_retune(&mut self, retuned: bool) -> bool {
        match self.0.view() {
            CoreView::Retuning(view) => view.finish_retune(retuned).is_ok(),
            CoreView::RetuningClosing(view) => view.finish_retune(retuned).is_ok(),
            CoreView::RetuningClosed(view) => view.finish_retune(retuned).is_ok(),
            _ => panic!("retuning fixture"),
        }
    }
    pub(super) fn submit_checked(
        &mut self,
        hardware: &mut Hardware,
        queue: FreeQueue<'id>,
        admission: TxAdmission<'id>,
        values: MpduValues<'static, 512>,
    ) -> Result<(), SubmitFault<'id, 'static, 512>> {
        match self.0.view() {
            CoreView::Open(mut view) => view.submit_checked(hardware, queue, admission, values),
            CoreView::Closing(mut view) => view.submit_checked(hardware, queue, admission, values),
            CoreView::Closed(mut view) => view.submit_checked(hardware, queue, admission, values),
            _ => panic!("enabled fixture"),
        }
    }
    pub(super) fn received<'frame>(
        &mut self,
        hardware: &Hardware,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        match self.0.view() {
            CoreView::Open(view) => view.received(hardware, frame),
            CoreView::Closing(view) => view.received(hardware, frame),
            CoreView::Closed(view) => view.received(hardware, frame),
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
        match self.0.view() {
            CoreView::Open(view) => {
                let _draining = view.disable_checked(hardware, events);
            }
            CoreView::Closing(view) => {
                let _draining = view.disable_checked(hardware, events);
            }
            CoreView::Closed(view) => {
                let _draining = view.disable_checked(hardware, events);
            }
            _ => panic!("enabled fixture"),
        }
        self.finish_disable(events);
        Ok(())
    }
    pub(super) fn finish_disable(&mut self, events: &mut Events<'id>) {
        match self.0.view() {
            CoreView::Disabling(view) => {
                let _finished = view.finish(events);
            }
            CoreView::DisablingClosing(view) => {
                let _finished = view.finish(events);
            }
            CoreView::DisablingClosed(view) => {
                let _finished = view.finish(events);
            }
            _ => {}
        }
    }
    pub(super) fn service(
        &mut self,
        hardware: &mut Hardware,
        wake: WifiTxWake,
        events: &mut Events<'id>,
    ) -> Result<(), LowerMacFault> {
        let result = match self.0.view() {
            CoreView::Disabled(mut view) => view.service(hardware, wake, events),
            CoreView::DisabledClosing(mut view) => view.service(hardware, wake, events),
            CoreView::DisabledClosed(mut view) => view.service(hardware, wake, events),
            CoreView::Retuning(mut view) => view.service(hardware, wake, events),
            CoreView::RetuningClosing(mut view) => view.service(hardware, wake, events),
            CoreView::RetuningClosed(mut view) => view.service(hardware, wake, events),
            CoreView::Open(mut view) => view.service(hardware, wake, events),
            CoreView::Closing(mut view) => view.service(hardware, wake, events),
            CoreView::Closed(mut view) => view.service(hardware, wake, events),
            CoreView::Disabling(mut view) => view.service(hardware, wake, events),
            CoreView::DisablingClosing(mut view) => view.service(hardware, wake, events),
            CoreView::DisablingClosed(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureDisabled(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureDisabledClosing(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureDisabledClosed(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureOpen(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureClosing(mut view) => view.service(hardware, wake, events),
            CoreView::ConfigureClosed(mut view) => view.service(hardware, wake, events),
            CoreView::Failed(mut view) => view.service(hardware, wake, events),
            _ => panic!("prepared fixture"),
        };
        self.finish_disable(events);
        result
    }
    pub(super) fn configure_checked_vif(
        &mut self,
        hardware: &mut Hardware,
        config: CoreVif,
    ) -> Result<(), UnsupportedVif> {
        match self.0.view() {
            CoreView::Disabled(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            CoreView::DisabledClosing(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            CoreView::DisabledClosed(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            CoreView::Open(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            CoreView::Closing(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            CoreView::Closed(view) => match view.configure(config) {
                Ok(command) => match command.advance(hardware, &mut Events::default()) {
                    ConfigurationProgress::Configured(_) => Ok(()),
                    ConfigurationProgress::Waiting(_) => {
                        panic!("this fixture configures an idle interface")
                    }
                },
                Err(refused) => Err(refused.reason),
            },
            _ => panic!("configurable fixture"),
        }
    }
    pub(super) fn hold_new_attempts(&mut self) {
        match self.0.view() {
            CoreView::Open(view) => {
                let _closing = view.hold_new_attempts();
            }
            _ => panic!("open fixture"),
        }
    }
    pub(super) fn block_held_queues(&mut self, hardware: &mut Hardware) {
        match self.0.view() {
            CoreView::Closing(view) => match view.close_gate(hardware) {
                GateClosure::Closed(_) => (),
                GateClosure::Closing(_) => panic!("this fixture closes an idle publication prefix"),
            },
            _ => panic!("closing fixture"),
        }
    }
    pub(super) fn open_gate(
        &mut self,
        hardware: &mut Hardware,
        events: &mut Events<'id>,
    ) -> Result<EnabledView<'_, DeviceCore<'id>>, FailedGateView<'_, DeviceCore<'id>>> {
        if matches!(self.0.view(), CoreView::Closing(_)) {
            self.block_held_queues(hardware);
        }
        let CoreView::Closed(view) = self.0.view() else {
            panic!("closed fixture")
        };
        view.open_gate(hardware, events)
    }
    #[expect(
        clippy::result_large_err,
        reason = "the fixture returns the actual hardware bank"
    )]
    pub(super) fn dismantle(
        mut self,
    ) -> Result<
        LowerMacCoreParts<
            'id,
            'static,
            oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile,
            Power,
            fn() -> u32,
            Timer,
            RadioNine,
            512,
        >,
        Self,
    > {
        match self.0.view() {
            CoreView::Disabled(view) => Ok(view.dismantle()),
            CoreView::DisabledClosing(view) => Ok(view.dismantle()),
            CoreView::DisabledClosed(view) => Ok(view.dismantle()),
            _ => Err(self),
        }
    }
}
