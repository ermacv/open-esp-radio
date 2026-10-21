//! Task-side lifecycle transitions over one retained hardware bank.
use super::*;

enum Phase {
    Vacant,
    Disabled,
    DisabledClosed,
    Open,
    Closing,
    Closed,
    Disabling,
    DisablingClosing,
    DisablingClosed,
    ConfigureOpen {
        config: CoreVif,
        policy: ReceivePolicy,
    },
    ConfigureClosing {
        config: CoreVif,
        policy: ReceivePolicy,
    },
    ConfigureClosed {
        config: CoreVif,
        policy: ReceivePolicy,
    },
    Failed,
}

pub(crate) enum CoreOwnerFault {
    Capacity,
    Core(LowerMacFault),
}

/// The bank stays in its caller's storage while its small phase changes.
/// Its phase stays private; an idle command frontier lends only a core borrow.
pub(crate) struct CoreOwner<C> {
    core: Option<C>,
    phase: Phase,
}
impl<C> CoreOwner<C> {
    pub(crate) const fn vacant() -> Self {
        Self {
            core: None,
            phase: Phase::Vacant,
        }
    }
}

impl<
    'id,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const B: usize,
    Path: PoolPath,
    const TICKETS: usize,
> CoreOwner<LowerMacCore<'id, 'slot, X, P, E, T, B, Path, TICKETS>>
{
    pub(crate) fn prepare(
        &mut self,
        core: DisabledCore<LowerMacCore<'id, 'slot, X, P, E, T, B, Path, TICKETS>>,
    ) {
        assert!(self.core.is_none(), "retired core released its bank");
        self.core = Some(core.core);
        self.phase = Phase::Disabled;
    }
    fn stored(&mut self) -> &mut LowerMacCore<'id, 'slot, X, P, E, T, B, Path, TICKETS> {
        self.core.as_mut().expect("core owner retains its bank")
    }
    pub(crate) fn core(&mut self) -> &mut LowerMacCore<'id, 'slot, X, P, E, T, B, Path, TICKETS> {
        assert!(
            matches!(
                self.phase,
                Phase::Disabled
                    | Phase::DisabledClosed
                    | Phase::Open
                    | Phase::Closing
                    | Phase::Closed
                    | Phase::Disabling
                    | Phase::DisablingClosing
                    | Phase::DisablingClosed
            ),
            "only an idle command frontier lends the core"
        );
        self.stored()
    }
    pub(crate) fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        if self.configuring() {
            self.stored().reset_contention(role, category);
        } else {
            self.core().reset_contention(role, category);
        }
    }
    pub(crate) fn disabled(&self) -> bool {
        matches!(self.phase, Phase::Disabled | Phase::DisabledClosed)
    }
    pub(crate) fn take_transmitter(&mut self) -> OrdinaryTxIdleOwner<'slot, X, P, E, T, B> {
        assert!(
            self.disabled(),
            "normal uninstall requires the disabled core"
        );
        self.phase = Phase::Vacant;
        DisabledCore::<_, OpenGate>::new(self.core.take().expect("stopped bank retained"))
            .dismantle()
            .into_transmitter()
    }
    pub(crate) fn configuring(&self) -> bool {
        matches!(
            self.phase,
            Phase::ConfigureOpen { .. }
                | Phase::ConfigureClosing { .. }
                | Phase::ConfigureClosed { .. }
        )
    }
    pub(crate) fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        let core = self.core.as_ref().expect("bank retained");
        match self.phase {
            Phase::Open
            | Phase::Closing
            | Phase::Closed
            | Phase::ConfigureOpen { .. }
            | Phase::ConfigureClosing { .. }
            | Phase::ConfigureClosed { .. } => core.received(hardware, frame),
            _ => None,
        }
    }
    pub(crate) fn deadline(&mut self) -> Option<oer_time::Instant> {
        if self.configuring() || matches!(self.phase, Phase::Failed) {
            self.stored().next_deadline()
        } else {
            self.core().next_deadline()
        }
    }
    pub(crate) fn tune(&mut self, channel: WifiChannel) {
        let next = match self.phase {
            Phase::Disabled => Phase::Open,
            Phase::DisabledClosed => Phase::Closed,
            _ => return,
        };
        let core = self.stored();
        core.set_checked_channel(channel);
        core.tuned = channel;
        self.phase = next;
    }
    pub(crate) fn submit<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        queue: FreeQueue<'id, Path>,
        ticket: TxAdmission<'id, Path>,
        values: MpduValues<'slot, B>,
    ) -> Result<(), SubmitFault<'id, 'slot, B, Path>> {
        assert!(
            matches!(self.phase, Phase::Open),
            "publication requires an open enabled bank"
        );
        self.stored()
            .submit_checked(hardware, queue, ticket, values, true)
    }
    pub(crate) fn service<
        H: HtAmpduHardware + CcmpKeyHardware,
        K: LowerMacSink<'id, 'slot, B, Path>,
    >(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        if self.configuring() || matches!(self.phase, Phase::Failed) {
            self.stored().service(hardware, wake, sink)
        } else {
            self.core().service(hardware, wake, sink)
        }
    }
    pub(crate) fn configure(&mut self, config: CoreVif) -> Result<(), CoreOwnerFault> {
        if !matches!(self.phase, Phase::Open | Phase::Closing | Phase::Closed) {
            return Err(CoreOwnerFault::Capacity);
        }
        let policy =
            receive_policy(&self.stored().config, &config).map_err(|_| CoreOwnerFault::Capacity)?;
        self.phase = match self.phase {
            Phase::Open => Phase::ConfigureOpen { config, policy },
            Phase::Closing => Phase::ConfigureClosing { config, policy },
            Phase::Closed => Phase::ConfigureClosed { config, policy },
            _ => unreachable!("configuration retains its original phase"),
        };
        Ok(())
    }
    pub(crate) fn advance_configuration<
        H: LowerMacHardware,
        K: LowerMacSink<'id, 'slot, B, Path>,
    >(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> bool {
        let (config, policy, next) = match &self.phase {
            Phase::ConfigureOpen { config, policy } => (*config, *policy, Phase::Open),
            Phase::ConfigureClosing { config, policy } => (*config, *policy, Phase::Closing),
            Phase::ConfigureClosed { config, policy } => (*config, *policy, Phase::Closed),
            _ => return !self.configuring(),
        };
        if self.stored().advance_vif(hardware, sink, config, policy) {
            self.phase = next;
            true
        } else {
            false
        }
    }
    pub(crate) fn hold(&mut self) {
        if matches!(self.phase, Phase::Open) {
            self.phase = Phase::Closing;
        }
    }
    pub(crate) fn close<H: LowerMacHardware>(&mut self, hardware: &mut H) -> bool {
        if matches!(self.phase, Phase::Closing) {
            let core = self.stored();
            if core.fault.is_none() && core.published_idle() {
                hardware.set_power_save_tx_block(true);
                self.phase = Phase::Closed;
            }
        }
        matches!(self.phase, Phase::Closed)
    }
    pub(crate) fn open<H: LowerMacHardware, K: LowerMacSink<'id, 'slot, B, Path>>(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> Result<(), CoreOwnerFault> {
        if !matches!(self.phase, Phase::Closed) {
            return Err(CoreOwnerFault::Capacity);
        }
        match self.stored().open_held_gate(hardware, sink) {
            Ok(()) => {
                self.phase = Phase::Open;
                Ok(())
            }
            Err(fault) => {
                self.phase = Phase::Failed;
                Err(CoreOwnerFault::Core(fault))
            }
        }
    }
    pub(crate) fn disable<H: CcmpKeyHardware, K: LowerMacSink<'id, 'slot, B, Path>>(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) {
        let next = match self.phase {
            Phase::Open => Phase::Disabling,
            Phase::Closing => Phase::DisablingClosing,
            Phase::Closed => Phase::DisablingClosed,
            _ => return,
        };
        let core = self.stored();
        if core.fault.is_none() {
            core.withdraw_held_attempts(hardware, sink);
        }
        self.phase = next;
    }
    pub(crate) fn finish_disable<K: LowerMacSink<'id, 'slot, B, Path>>(
        &mut self,
        sink: &mut K,
    ) -> bool {
        if !matches!(
            self.phase,
            Phase::Disabling | Phase::DisablingClosing | Phase::DisablingClosed
        ) {
            return self.disabled();
        }
        let core = self.stored();
        if core.fault.is_some() || core.attempts().next().is_some() {
            return false;
        }
        self.phase = match self.phase {
            Phase::Disabling => Phase::Disabled,
            Phase::DisablingClosed => Phase::DisabledClosed,
            Phase::DisablingClosing => panic!("gate close completes before disable"),
            _ => unreachable!("only disabling ownership reaches its terminal"),
        };
        sink.disabled();
        true
    }
}
