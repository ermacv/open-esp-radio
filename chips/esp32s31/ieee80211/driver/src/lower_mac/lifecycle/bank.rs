//! Storage and the only public source of phase views.
use super::*;

pub(in crate::lower_mac) enum Phase {
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
/// One caller-owned bank. Views borrow this storage; transitions never move it.
pub struct CoreOwner<C> {
    pub(in crate::lower_mac) bank: Option<Prepared<C>>,
}
/// A prepared core and the phase its views follow.
pub(in crate::lower_mac) struct Prepared<C> {
    pub(in crate::lower_mac) core: C,
    pub(in crate::lower_mac) phase: Phase,
}
/// Why the owner kept its core instead of dismantling it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreRetained {
    /// No core was prepared.
    Vacant,
    /// The bank is not disabled.
    NotDisabled,
    /// The transmitter still holds an attempt or a published slot.
    Publications,
}
impl<C> CoreOwner<C> {
    /// Empty storage before initial preparation or after dismantling.
    #[inline]
    pub const fn vacant() -> Self {
        Self { bank: None }
    }
    pub(in crate::lower_mac) fn prepared(core: C) -> Self {
        Self {
            bank: Some(Prepared {
                core,
                phase: Phase::Disabled,
            }),
        }
    }
    /// Borrow the only view corresponding to the retained phase.
    #[inline]
    pub fn view(&mut self) -> CoreView<'_, C> {
        match self.bank {
            None => CoreView::Vacant(VacantView {
                bank: &mut self.bank,
            }),
            Some(ref mut prepared) => prepared.view(),
        }
    }
}
impl<C> Prepared<C> {
    fn view(&mut self) -> CoreView<'_, C> {
        match self.phase {
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
            Phase::ConfigureDisabled { config, policy } => CoreView::ConfigureDisabled(
                ConfiguringView::new(DisabledView::new(self), config, policy),
            ),
            Phase::ConfigureDisabledClosing { config, policy } => {
                CoreView::ConfigureDisabledClosing(ConfiguringView::new(
                    DisabledView::new(self),
                    config,
                    policy,
                ))
            }
            Phase::ConfigureDisabledClosed { config, policy } => CoreView::ConfigureDisabledClosed(
                ConfiguringView::new(DisabledView::new(self), config, policy),
            ),
            Phase::ConfigureOpen { config, policy } => CoreView::ConfigureOpen(
                ConfiguringView::new(EnabledView::new(self), config, policy),
            ),
            Phase::ConfigureClosing { config, policy } => CoreView::ConfigureClosing(
                ConfiguringView::new(EnabledView::new(self), config, policy),
            ),
            Phase::ConfigureClosed { config, policy } => CoreView::ConfigureClosed(
                ConfiguringView::new(EnabledView::new(self), config, policy),
            ),
            Phase::Failed { fault } => CoreView::Failed(FailedGateView { owner: self, fault }),
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
            core: &mut owner.core,
        })
    }

    /// The published aggregates' service, in every phase that holds a
    /// core: an aggregate published before a gate closed, a configuration
    /// barrier or a disable runs to its terminal as an MPDU does.
    #[inline]
    pub fn aggregates(self) -> Option<AggregateView<'view, C>> {
        let owner = match self {
            Self::Vacant(_) => return None,
            Self::Disabled(view) => view.owner,
            Self::DisabledClosing(view) => view.owner,
            Self::DisabledClosed(view) => view.owner,
            Self::Retuning(view) => view.owner,
            Self::RetuningClosing(view) => view.owner,
            Self::RetuningClosed(view) => view.owner,
            Self::Open(view) => view.owner,
            Self::Closing(view) => view.owner,
            Self::Closed(view) => view.owner,
            Self::Disabling(view) => view.owner,
            Self::DisablingClosing(view) => view.owner,
            Self::DisablingClosed(view) => view.owner,
            Self::ConfigureDisabled(view) => view.core.owner,
            Self::ConfigureDisabledClosing(view) => view.core.owner,
            Self::ConfigureDisabledClosed(view) => view.core.owner,
            Self::ConfigureOpen(view) => view.core.owner,
            Self::ConfigureClosing(view) => view.core.owner,
            Self::ConfigureClosed(view) => view.core.owner,
            Self::Failed(view) => view.owner,
        };
        Some(AggregateView {
            core: &mut owner.core,
        })
    }
}

/// The service of aggregates already published: no admission, no lifecycle
/// transition.
#[must_use]
pub struct AggregateView<'view, C> {
    core: &'view mut C,
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
> AggregateView<'view, LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
{
    /// [`LowerMacCore::service_ampdu`].
    ///
    /// # Errors
    ///
    /// The step failed; [`PublishedAmpdu::into_progress`] retains the
    /// publication in quarantine.
    #[inline]
    pub fn service<H: HtAmpduHardware, L: PlacedBacking, const SLOTS: usize>(
        &mut self,
        hardware: &mut H,
        published: &mut PublishedAmpdu<'core, 'slot, L, SLOTS, Path>,
        wake: WifiTxWake,
        returned: impl FnMut(L),
    ) -> Result<AmpduServed, crate::mac::tx::ampdu::HtAmpduTxError> {
        self.core.service_ampdu(hardware, published, wake, returned)
    }

    /// [`LowerMacCore::complete_ampdu`].
    #[allow(
        clippy::result_large_err,
        reason = "a poisoned core retains the full aggregate and key right"
    )]
    #[inline]
    pub fn complete<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
        L,
        const SLOTS: usize,
    >(
        &mut self,
        hardware: &mut H,
        completed: ampdu::CompletedAmpdu<'core, 'slot, L, SLOTS, Path>,
        sink: &mut K,
    ) -> Result<
        (
            ampdu::Esp32s31AmpduOwner<'slot, L, SLOTS>,
            [oer_time::Instant; SLOTS],
        ),
        ampdu::AmpduFault<'core, 'slot, L, SLOTS, Path>,
    > {
        self.core.complete_ampdu(hardware, completed, sink)
    }

    /// [`LowerMacCore::complete_expired_ampdu`].
    #[allow(
        clippy::result_large_err,
        reason = "a poisoned core retains every unpublished owner"
    )]
    #[inline]
    pub fn complete_expired<
        H: CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
        L,
        const SLOTS: usize,
    >(
        &mut self,
        hardware: &mut H,
        expired: ampdu::ExpiredAmpdu<'core, 'slot, L, SLOTS, Path>,
        sink: &mut K,
    ) -> Result<
        Esp32s31AmpduBuffer<'slot, L, SLOTS>,
        ampdu::ExpiredAmpdu<'core, 'slot, L, SLOTS, Path>,
    > {
        self.core.complete_expired_ampdu(hardware, expired, sink)
    }

    /// Whether an abort, an MPDU's or an aggregate's, forces CCA now.
    #[inline]
    pub fn abort_settling(&self) -> bool {
        self.core.abort_settling()
    }
}

/// A vacant slot can be prepared once before any lifecycle transition.
#[must_use]
pub struct VacantView<'view, C> {
    bank: &'view mut Option<Prepared<C>>,
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
        DisabledView::new(self.bank.insert(Prepared {
            core: LowerMacCore::assemble(authority, tx, config, coex),
            phase: Phase::Disabled,
        }))
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
    /// Recover a disabled bank's construction resources. Any other bank,
    /// or one whose transmitter retains a publication, stays in place.
    #[allow(
        clippy::result_large_err,
        reason = "the refusal is a small reason; the retained core stays in place"
    )]
    pub fn dismantle(
        &mut self,
    ) -> Result<LowerMacCoreParts<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>, CoreRetained>
    {
        let Some(prepared) = self.bank.take() else {
            return Err(CoreRetained::Vacant);
        };
        if !matches!(
            prepared.phase,
            Phase::Disabled | Phase::DisabledClosing | Phase::DisabledClosed
        ) {
            self.bank = Some(prepared);
            return Err(CoreRetained::NotDisabled);
        }
        let Prepared { mut core, phase } = prepared;
        match core.tx.into_idle() {
            Ok(tx) => Ok(LowerMacCoreParts {
                authority: core.authority,
                tx,
                config: core.config,
                coex: core.coex,
                keys: core.keys,
            }),
            Err(tx) => {
                core.tx = tx;
                self.bank = Some(Prepared { core, phase });
                Err(CoreRetained::Publications)
            }
        }
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
> Prepared<LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
{
    // Each heavy step has one body regardless of the caller's gate marker.
    #[inline(never)]
    pub(super) fn configure_in_place(
        &mut self,
        config: CoreVif,
        enabled: bool,
        gate: GateMode,
    ) -> Result<ReceivePolicy, UnsupportedVif> {
        let policy = receive_policy(&self.core.config, &config)?;
        self.phase = match (enabled, gate) {
            (false, GateMode::Open) => Phase::ConfigureDisabled { config, policy },
            (false, GateMode::Closing) => Phase::ConfigureDisabledClosing { config, policy },
            (false, GateMode::Closed) => Phase::ConfigureDisabledClosed { config, policy },
            (true, GateMode::Open) => Phase::ConfigureOpen { config, policy },
            (true, GateMode::Closing) => Phase::ConfigureClosing { config, policy },
            (true, GateMode::Closed) => Phase::ConfigureClosed { config, policy },
        };
        Ok(policy)
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
        let core = &mut self.core;
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
        config: CoreVif,
        policy: ReceivePolicy,
        frontier: Phase,
    ) -> bool {
        if !self.core.advance_vif(hardware, sink, config, policy) {
            return false;
        }
        self.phase = frontier;
        true
    }
    #[inline(never)]
    pub(super) fn finish_disable_in_place<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        sink: &mut K,
        gate: GateMode,
    ) -> bool {
        let core = &self.core;
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
