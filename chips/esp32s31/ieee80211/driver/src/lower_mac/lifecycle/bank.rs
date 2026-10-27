//! Storage and the only public source of phase views.
use super::*;

pub(in crate::lower_mac) enum Phase {
    Vacant,
    Disabled,
    DisabledClosing,
    DisabledClosed,
    Retuning,
    RetuningClosing,
    RetuningClosed,
    Open,
    Closing,
    Closed,
    Disabling,
    DisablingClosing,
    DisablingClosed,
    ConfigureDisabled {
        config: CoreVif,
        policy: ReceivePolicy,
    },
    ConfigureDisabledClosing {
        config: CoreVif,
        policy: ReceivePolicy,
    },
    ConfigureDisabledClosed {
        config: CoreVif,
        policy: ReceivePolicy,
    },
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
    Failed {
        fault: LowerMacFault,
    },
}
impl Phase {
    pub(super) fn configuration(&self) -> (CoreVif, ReceivePolicy) {
        match self {
            Self::ConfigureDisabled { config, policy }
            | Self::ConfigureDisabledClosing { config, policy }
            | Self::ConfigureDisabledClosed { config, policy }
            | Self::ConfigureOpen { config, policy }
            | Self::ConfigureClosing { config, policy }
            | Self::ConfigureClosed { config, policy } => (*config, *policy),
            _ => panic!("configuration view retains its command"),
        }
    }
}

/// One caller-owned bank. Views borrow this storage; transitions never move it.
pub struct CoreOwner<C> {
    pub(in crate::lower_mac) core: Option<C>,
    pub(in crate::lower_mac) phase: Phase,
}
impl<C> CoreOwner<C> {
    /// Empty storage before initial preparation or after dismantling.
    #[inline]
    pub const fn vacant() -> Self {
        Self {
            core: None,
            phase: Phase::Vacant,
        }
    }
    pub(in crate::lower_mac) fn prepared(core: C) -> Self {
        Self {
            core: Some(core),
            phase: Phase::Disabled,
        }
    }
    #[inline]
    pub(super) fn stored(&self) -> &C {
        self.core.as_ref().expect("view retains its core")
    }
    #[inline]
    pub(super) fn stored_mut(&mut self) -> &mut C {
        self.core.as_mut().expect("view retains its core")
    }
    /// Borrow the only view corresponding to the retained phase.
    #[inline]
    pub fn view(&mut self) -> CoreView<'_, C> {
        match self.phase {
            Phase::Vacant => CoreView::Vacant(VacantView { owner: self }),
            Phase::Disabled => CoreView::Disabled(DisabledView::new(self)),
            Phase::DisabledClosing => CoreView::DisabledClosing(DisabledView::new(self)),
            Phase::DisabledClosed => CoreView::DisabledClosed(DisabledView::new(self)),
            Phase::Retuning => CoreView::Retuning(RetuningView::new(self)),
            Phase::RetuningClosing => CoreView::RetuningClosing(RetuningView::new(self)),
            Phase::RetuningClosed => CoreView::RetuningClosed(RetuningView::new(self)),
            Phase::Open => CoreView::Open(EnabledView::new(self)),
            Phase::Closing => CoreView::Closing(EnabledView::new(self)),
            Phase::Closed => CoreView::Closed(EnabledView::new(self)),
            Phase::Disabling => CoreView::Disabling(DisablingView::new(self)),
            Phase::DisablingClosing => CoreView::DisablingClosing(DisablingView::new(self)),
            Phase::DisablingClosed => CoreView::DisablingClosed(DisablingView::new(self)),
            Phase::ConfigureDisabled { .. } => CoreView::ConfigureDisabled(ConfiguringView {
                core: DisabledView::new(self),
            }),
            Phase::ConfigureDisabledClosing { .. } => {
                CoreView::ConfigureDisabledClosing(ConfiguringView {
                    core: DisabledView::new(self),
                })
            }
            Phase::ConfigureDisabledClosed { .. } => {
                CoreView::ConfigureDisabledClosed(ConfiguringView {
                    core: DisabledView::new(self),
                })
            }
            Phase::ConfigureOpen { .. } => CoreView::ConfigureOpen(ConfiguringView {
                core: EnabledView::new(self),
            }),
            Phase::ConfigureClosing { .. } => CoreView::ConfigureClosing(ConfiguringView {
                core: EnabledView::new(self),
            }),
            Phase::ConfigureClosed { .. } => CoreView::ConfigureClosed(ConfiguringView {
                core: EnabledView::new(self),
            }),
            Phase::Failed { .. } => CoreView::Failed(FailedGateView { owner: self }),
        }
    }
}
/// Exhaustive borrowed views, including pending configuration and failed gates.
#[must_use]
pub enum CoreView<'view, C> {
    Vacant(VacantView<'view, C>),
    Disabled(DisabledView<'view, C, OpenGate>),
    DisabledClosing(DisabledView<'view, C, ClosingGate>),
    DisabledClosed(DisabledView<'view, C, ClosedGate>),
    Retuning(RetuningView<'view, C, OpenGate>),
    RetuningClosing(RetuningView<'view, C, ClosingGate>),
    RetuningClosed(RetuningView<'view, C, ClosedGate>),
    Open(EnabledView<'view, C, OpenGate>),
    Closing(EnabledView<'view, C, ClosingGate>),
    Closed(EnabledView<'view, C, ClosedGate>),
    Disabling(DisablingView<'view, C, OpenGate>),
    DisablingClosing(DisablingView<'view, C, ClosingGate>),
    DisablingClosed(DisablingView<'view, C, ClosedGate>),
    ConfigureDisabled(ConfiguringView<DisabledView<'view, C, OpenGate>>),
    ConfigureDisabledClosing(ConfiguringView<DisabledView<'view, C, ClosingGate>>),
    ConfigureDisabledClosed(ConfiguringView<DisabledView<'view, C, ClosedGate>>),
    ConfigureOpen(ConfiguringView<EnabledView<'view, C, OpenGate>>),
    ConfigureClosing(ConfiguringView<EnabledView<'view, C, ClosingGate>>),
    ConfigureClosed(ConfiguringView<EnabledView<'view, C, ClosedGate>>),
    Failed(FailedGateView<'view, C>),
}
impl<'view, C> CoreView<'view, C> {
    /// Narrow an idle phase to its shared non-lifecycle command set.
    /// Configuration, retuning and failed gates do not lend these commands.
    #[inline]
    pub fn commands(self) -> Option<CommandView<'view, C>> {
        let owner = match self {
            Self::Disabled(view) => view.owner,
            Self::DisabledClosing(view) => view.owner,
            Self::DisabledClosed(view) => view.owner,
            Self::Open(view) => view.owner,
            Self::Closing(view) => view.owner,
            Self::Closed(view) => view.owner,
            Self::Disabling(view) => view.owner,
            Self::DisablingClosing(view) => view.owner,
            Self::DisablingClosed(view) => view.owner,
            _ => return None,
        };
        Some(CommandView {
            core: owner.stored_mut(),
        })
    }
}

/// A vacant slot can be prepared once before any lifecycle transition.
#[must_use]
pub struct VacantView<'view, C> {
    owner: &'view mut CoreOwner<C>,
}

impl<
    'view,
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> VacantView<'view, LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
{
    /// Place newly acquired construction resources in this caller's bank.
    pub fn prepare(
        self,
        authority: CoreAuthority<'core, Path, TICKETS>,
        tx: OrdinaryTxIdleOwner<'slot, P, E, T, BUFFER_SIZE>,
        config: LowerMacConfig,
        coex: &'slot C,
    ) -> DisabledView<'view, LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
    {
        self.owner.core = LowerMacCore::new(authority, tx, config, coex).core;
        self.owner.phase = Phase::Disabled;
        DisabledView::new(self.owner)
    }
}

impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> CoreOwner<LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
{
    // Each heavy step has one body regardless of the caller's gate marker.
    #[inline(never)]
    pub(super) fn configure_in_place(
        &mut self,
        config: CoreVif,
        enabled: bool,
        gate: GateMode,
    ) -> Result<(), UnsupportedVif> {
        let policy = receive_policy(&self.stored().config, &config)?;
        self.phase = match (enabled, gate) {
            (false, GateMode::Open) => Phase::ConfigureDisabled { config, policy },
            (false, GateMode::Closing) => Phase::ConfigureDisabledClosing { config, policy },
            (false, GateMode::Closed) => Phase::ConfigureDisabledClosed { config, policy },
            (true, GateMode::Open) => Phase::ConfigureOpen { config, policy },
            (true, GateMode::Closing) => Phase::ConfigureClosing { config, policy },
            (true, GateMode::Closed) => Phase::ConfigureClosed { config, policy },
        };
        Ok(())
    }
    #[inline(never)]
    pub(super) fn disable_in_place<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
        gate: GateMode,
    ) {
        let core = self.stored_mut();
        if core.fault.is_none() {
            core.withdraw_held_attempts(hardware, sink);
        }
        self.phase = match gate {
            GateMode::Open => Phase::Disabling,
            GateMode::Closing => Phase::DisablingClosing,
            GateMode::Closed => Phase::DisablingClosed,
        };
    }
    #[inline(never)]
    pub(super) fn advance_configuration_in_place<
        H: LowerMacHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> bool {
        let (config, policy) = self.phase.configuration();
        if !self
            .stored_mut()
            .advance_vif(hardware, sink, config, policy)
        {
            return false;
        }
        self.phase = match self.phase {
            Phase::ConfigureDisabled { .. } => Phase::Disabled,
            Phase::ConfigureDisabledClosing { .. } => Phase::DisabledClosing,
            Phase::ConfigureDisabledClosed { .. } => Phase::DisabledClosed,
            Phase::ConfigureOpen { .. } => Phase::Open,
            Phase::ConfigureClosing { .. } => Phase::Closing,
            Phase::ConfigureClosed { .. } => Phase::Closed,
            _ => panic!("configuration view retains its original frontier"),
        };
        true
    }
    #[inline(never)]
    pub(super) fn finish_disable_in_place<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        sink: &mut K,
        gate: GateMode,
    ) -> bool {
        let core = self.stored();
        if core.fault.is_some() || core.attempts().next().is_some() {
            return false;
        }
        self.phase = match gate {
            GateMode::Open => Phase::Disabled,
            GateMode::Closing => Phase::DisabledClosing,
            GateMode::Closed => Phase::DisabledClosed,
        };
        sink.disabled();
        true
    }
}
