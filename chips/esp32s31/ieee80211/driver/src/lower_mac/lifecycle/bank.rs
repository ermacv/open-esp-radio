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
    /// Borrow the only view corresponding to the retained phase.
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
/// A vacant slot can be prepared once before any lifecycle transition.
#[must_use]
pub struct VacantView<'view, C> {
    owner: &'view mut CoreOwner<C>,
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> VacantView<'view, LowerMacCore<'core, 'slot, X, P, E, T, BUFFER_SIZE, Path, TICKETS>>
{
    /// Place newly acquired construction resources in this caller's bank.
    pub fn prepare(
        self,
        authority: CoreAuthority<'core, Path, TICKETS>,
        tx: OrdinaryTxIdleOwner<'slot, X, P, E, T, BUFFER_SIZE>,
        config: LowerMacConfig,
        coex: &'slot dyn RadioCoexPriorities,
    ) -> DisabledView<'view, LowerMacCore<'core, 'slot, X, P, E, T, BUFFER_SIZE, Path, TICKETS>>
    {
        self.owner.core = LowerMacCore::new(authority, tx, config, coex).core;
        self.owner.phase = Phase::Disabled;
        DisabledView::new(self.owner)
    }
}
