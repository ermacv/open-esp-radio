//! Borrowed lifecycle views over one caller-owned hardware bank.
use super::*;
use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use oer_ieee80211_port::backend::BackendTxProfile;

mod bank;
use bank::Phase;
pub use bank::{CoreOwner, CoreView, VacantView};

/// New attempts publish immediately.
pub enum OpenGate {}
/// New attempts are held while the published prefix drains.
pub enum ClosingGate {}
/// Hardware queues are blocked and no attempt is published.
pub enum ClosedGate {}
mod sealed {
    pub trait Gate {}
    impl Gate for super::OpenGate {}
    impl Gate for super::ClosingGate {}
    impl Gate for super::ClosedGate {}
}
/// Closed set of admission policies; transitions alone change the gate.
pub trait CoreGate: sealed::Gate {
    #[doc(hidden)]
    const PUBLISH: bool;
    #[doc(hidden)]
    const BLOCKED: bool;
}
impl CoreGate for OpenGate {
    const PUBLISH: bool = true;
    const BLOCKED: bool = false;
}
impl CoreGate for ClosingGate {
    const PUBLISH: bool = false;
    const BLOCKED: bool = false;
}
impl CoreGate for ClosedGate {
    const PUBLISH: bool = false;
    const BLOCKED: bool = true;
}
// The engine receives this small value; it is never specialized by gate.
#[derive(Clone, Copy)]
enum GateMode {
    Open,
    Closing,
    Closed,
}
// These are compile-time gate choices; no second runtime phase is stored.
struct GatePhase<G>(PhantomData<G>);
impl<G: CoreGate> GatePhase<G> {
    const DISABLED: Phase = if G::PUBLISH {
        Phase::Disabled
    } else if G::BLOCKED {
        Phase::DisabledClosed
    } else {
        Phase::DisabledClosing
    };
    const RETUNING: Phase = if G::PUBLISH {
        Phase::Retuning
    } else if G::BLOCKED {
        Phase::RetuningClosed
    } else {
        Phase::RetuningClosing
    };
    const ENABLED: Phase = if G::PUBLISH {
        Phase::Open
    } else if G::BLOCKED {
        Phase::Closed
    } else {
        Phase::Closing
    };
    const MODE: GateMode = if G::PUBLISH {
        GateMode::Open
    } else if G::BLOCKED {
        GateMode::Closed
    } else {
        GateMode::Closing
    };
}

macro_rules! view {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[must_use]
        pub struct $name<'view, C, G = OpenGate> {
            owner: &'view mut CoreOwner<C>,
            gate: PhantomData<G>,
        }
        impl<'view, C, G> $name<'view, C, G> {
            #[inline]
            fn new(owner: &'view mut CoreOwner<C>) -> Self { Self { owner, gate: PhantomData } }
        }
        impl<C, G> Deref for $name<'_, C, G> {
            type Target = C;
            #[inline]
            fn deref(&self) -> &C { self.owner.stored() }
        }
        impl<C, G> DerefMut for $name<'_, C, G> {
            #[inline]
            fn deref_mut(&mut self) -> &mut C { self.owner.stored_mut() }
        }
    };
}
view!(/// A disabled bank; only this view can select a channel or dismantle.
    DisabledView);
view!(/// A pending enable, retained in its owner during the radio retune.
    RetuningView);
view!(/// An enabled bank with a statically selected admission policy.
    EnabledView);
view!(/// A disabling bank retaining all outstanding publications.
    DisablingView);

/// Non-lifecycle commands shared by idle phase views. No transition or TX
/// admission is available through this narrower borrowed projection.
#[must_use]
pub struct CommandView<'view, C> {
    core: &'view mut C,
}
impl<C> Deref for CommandView<'_, C> {
    type Target = C;
    #[inline]
    fn deref(&self) -> &C {
        self.core
    }
}
impl<C> DerefMut for CommandView<'_, C> {
    #[inline]
    fn deref_mut(&mut self) -> &mut C {
        self.core
    }
}

/// Enable either completes or waits for an actual radio retune.
#[must_use]
pub enum LifecycleStart<V, R> {
    Enabled(V),
    Retune(R),
}
/// The closing view persists until its published prefix ends.
#[must_use]
pub enum GateClosure<V, C> {
    Closing(V),
    Closed(C),
}
/// A FIFO configuration barrier, borrowing its original lifecycle view.
#[must_use]
pub struct ConfiguringView<V> {
    core: V,
}
/// Configuration returns its original lifecycle view exactly once.
#[must_use]
pub enum ConfigurationProgress<V> {
    Waiting(ConfiguringView<V>),
    Configured(V),
}
/// A refused receive policy returns the unchanged view.
#[must_use]
pub struct ConfigureRefused<V> {
    pub core: V,
    pub reason: UnsupportedVif,
}
/// Failed gate publication retains its prefix and admits only draining.
#[must_use]
pub struct FailedGateView<'view, C> {
    pub(super) owner: &'view mut CoreOwner<C>,
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> DisabledView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Reset this role's contention after the final exchange terminal.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.deref_mut().reset_contention(role, category);
    }

    /// Recover construction resources after the final terminal.
    #[inline]
    pub fn dismantle(
        self,
    ) -> LowerMacCoreParts<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS> {
        self.owner.phase = Phase::Vacant;
        let core = self
            .owner
            .core
            .take()
            .expect("disabled view retains its core");
        LowerMacCoreParts {
            authority: core.authority,
            tx: core
                .tx
                .into_idle()
                .ok()
                .expect("disabled core has retired every queued publication"),
            config: core.config,
            coex: core.coex,
            keys: core.keys,
        }
    }
    /// Record the actual channel after a radio start.
    #[inline]
    pub fn radio_tuned_to(&mut self, channel: WifiChannel) {
        self.tuned = channel;
    }
    /// Choose the next enable's channel.
    #[inline]
    pub fn set_checked_channel(&mut self, channel: WifiChannel) {
        self.deref_mut().set_checked_channel(channel);
    }
    /// Consume this view, updating its owner before returning the next view.
    #[inline]
    pub fn enable_checked(
        self,
    ) -> LifecycleStart<
        EnabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
        RetuningView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    > {
        if self.channel == self.tuned {
            self.owner.phase = GatePhase::<G>::ENABLED;
            LifecycleStart::Enabled(EnabledView::new(self.owner))
        } else {
            self.owner.phase = GatePhase::<G>::RETUNING;
            LifecycleStart::Retune(RetuningView::new(self.owner))
        }
    }

    /// Reserve the bank at this command's FIFO position.
    #[inline]
    pub fn configure(
        self,
        config: CoreVif,
    ) -> Result<ConfiguringView<Self>, ConfigureRefused<Self>> {
        match self
            .owner
            .configure_in_place(config, false, GatePhase::<G>::MODE)
        {
            Ok(()) => Ok(ConfiguringView { core: self }),
            Err(reason) => Err(ConfigureRefused { core: self, reason }),
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> RetuningView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Reset this role's contention after the final exchange terminal.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.deref_mut().reset_contention(role, category);
    }

    /// The actual target of this enable's retune.
    #[inline]
    pub fn target_channel(&self) -> WifiChannel {
        self.channel
    }
    /// A failed retune returns disabled ownership on the previous channel.
    #[inline]
    pub fn finish_retune(
        mut self,
        retuned: bool,
    ) -> Result<
        EnabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
        DisabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    > {
        if retuned {
            self.tuned = self.channel;
            self.owner.phase = GatePhase::<G>::ENABLED;
            Ok(EnabledView::new(self.owner))
        } else {
            self.owner.phase = GatePhase::<G>::DISABLED;
            Err(DisabledView::new(self.owner))
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> EnabledView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Reset this role's contention after the final exchange terminal.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.deref_mut().reset_contention(role, category);
    }

    /// Admit an MPDU; only the gate's type selects publication.
    #[allow(
        clippy::result_large_err,
        reason = "refusal returns all admitted owners"
    )]
    #[inline]
    pub fn submit_checked<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        queue: FreeQueue<'core, Path>,
        admission: TxAdmission<'core, Path>,
        values: MpduValues<'slot, BUFFER_SIZE>,
    ) -> Result<(), SubmitFault<'core, 'slot, BUFFER_SIZE, Path>> {
        self.deref_mut()
            .submit_checked(hardware, queue, admission, values, G::PUBLISH)
    }
    /// Narrow the hardware's superset of receive rules.
    #[inline]
    pub fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        self.deref().received(hardware, frame)
    }
    /// Withdraw held attempts, retaining published attempts until their terminals.
    #[inline]
    pub fn disable_checked<H: CcmpKeyHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        hardware: &mut H,
        sink: &mut K,
    ) -> DisablingView<
        'view,
        LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        G,
    > {
        self.owner
            .disable_in_place(hardware, sink, GatePhase::<G>::MODE);
        DisablingView::new(self.owner)
    }

    /// Reserve the bank at this command's FIFO position.
    #[inline]
    pub fn configure(
        self,
        config: CoreVif,
    ) -> Result<ConfiguringView<Self>, ConfigureRefused<Self>> {
        match self
            .owner
            .configure_in_place(config, true, GatePhase::<G>::MODE)
        {
            Ok(()) => Ok(ConfiguringView { core: self }),
            Err(reason) => Err(ConfigureRefused { core: self, reason }),
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> DisablingView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Reset this role's contention after the final exchange terminal.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.deref_mut().reset_contention(role, category);
    }

    /// Emit one Disabled terminal once the entire prefix is retired.
    #[inline]
    pub fn finish<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        sink: &mut K,
    ) -> Result<
        DisabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
        Self,
    > {
        if self
            .owner
            .finish_disable_in_place(sink, GatePhase::<G>::MODE)
        {
            Ok(DisabledView::new(self.owner))
        } else {
            Err(self)
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
>
    EnabledView<
        'view,
        LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        OpenGate,
    >
{
    /// Hold new attempts while the published prefix drains.
    #[inline]
    pub fn hold_new_attempts(
        self,
    ) -> EnabledView<
        'view,
        LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        ClosingGate,
    > {
        self.owner.phase = Phase::Closing;
        EnabledView::new(self.owner)
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
>
    EnabledView<
        'view,
        LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        ClosingGate,
    >
{
    /// Block hardware queues only after every published predecessor ended.
    #[inline]
    pub fn close_gate<H: LowerMacHardware>(
        self,
        hardware: &mut H,
    ) -> GateClosure<
        Self,
        EnabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            ClosedGate,
        >,
    > {
        if self.fault.is_some() || !self.published_idle() {
            GateClosure::Closing(self)
        } else {
            hardware.set_power_save_tx_block(true);
            self.owner.phase = Phase::Closed;
            GateClosure::Closed(EnabledView::new(self.owner))
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
>
    EnabledView<
        'view,
        LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        ClosedGate,
    >
{
    /// Publish the held prefix, retaining published predecessors on failure.
    #[inline]
    pub fn open_gate<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> Result<
        EnabledView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>,
        FailedGateView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        >,
    > {
        match self.open_held_gate(hardware, sink) {
            Ok(()) => {
                self.owner.phase = Phase::Open;
                Ok(EnabledView::new(self.owner))
            }
            Err(fault) => {
                self.owner.phase = Phase::Failed { fault };
                Err(FailedGateView { owner: self.owner })
            }
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
>
    ConfiguringView<
        DisabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    >
{
    /// The published prefix's next deadline.
    #[inline]
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.core.next_deadline()
    }
    /// Reset contention when an earlier publication ends.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.core.reset_contention(role, category);
    }
    /// Service predecessors without admitting any later command.
    #[inline]
    pub fn service<
        H: HtAmpduHardware + CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        self.core.service(hardware, wake, sink)
    }
    /// Apply the receive policy after the requested role's prefix ends.
    #[inline]
    pub fn advance<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        hardware: &mut H,
        sink: &mut K,
    ) -> ConfigurationProgress<
        DisabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    > {
        if self
            .core
            .owner
            .advance_configuration_in_place(hardware, sink)
        {
            ConfigurationProgress::Configured(self.core)
        } else {
            ConfigurationProgress::Waiting(self)
        }
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
>
    ConfiguringView<
        EnabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    >
{
    /// The published prefix's next deadline.
    #[inline]
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.core.next_deadline()
    }
    /// Reset contention when an earlier publication ends.
    #[inline]
    pub fn reset_contention(&mut self, role: VifRole, category: WmmAccessCategory) {
        self.core.reset_contention(role, category);
    }
    /// Service predecessors without admitting any later command.
    #[inline]
    pub fn service<
        H: HtAmpduHardware + CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        self.core.service(hardware, wake, sink)
    }
    /// Apply the receive policy after the requested role's prefix ends.
    #[inline]
    pub fn advance<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        hardware: &mut H,
        sink: &mut K,
    ) -> ConfigurationProgress<
        EnabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            G,
        >,
    > {
        if self
            .core
            .owner
            .advance_configuration_in_place(hardware, sink)
        {
            ConfigurationProgress::Configured(self.core)
        } else {
            ConfigurationProgress::Waiting(self)
        }
    }

    /// Receive with the original interface policy while its barrier drains.
    #[inline]
    pub fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        self.core.received(hardware, frame)
    }
}

impl<
    'view,
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> FailedGateView<'view, LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>>
{
    /// The publication failure that retained this prefix.
    #[inline]
    pub fn fault(&self) -> LowerMacFault {
        match self.owner.phase {
            Phase::Failed { fault } => fault,
            _ => panic!("failed view retains its publication fault"),
        }
    }
    /// The next deadline of the prefix published before failure.
    #[inline]
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.owner.stored().next_deadline()
    }
    /// Preserve each published predecessor's own terminal outcome.
    #[inline]
    pub fn service<
        H: HtAmpduHardware + CcmpKeyHardware,
        K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    >(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        self.owner.stored_mut().service(hardware, wake, sink)
    }
    /// Recover disabled ownership only after every predecessor ended.
    #[inline]
    pub fn finish<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        sink: &mut K,
    ) -> Result<
        DisabledView<
            'view,
            LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
            ClosedGate,
        >,
        Self,
    > {
        if self.owner.finish_disable_in_place(sink, GateMode::Closed) {
            Ok(DisabledView::new(self.owner))
        } else {
            Err(self)
        }
    }
}

impl<
    'core,
    'slot,
    X: BackendTxProfile,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    C: RadioCoexPriorities,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> LowerMacCore<'core, 'slot, X, P, E, T, C, BUFFER_SIZE, Path, TICKETS>
{
    // Every view transition uses these in-place hardware steps.
    #[inline(never)]
    fn open_held_gate<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> Result<(), LowerMacFault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        hardware.set_power_save_tx_block(false);
        for index in 0..LOWER_MAC_TX_QUEUES {
            let Some(attempt) = self.queues[index].take() else {
                continue;
            };
            let Attempt {
                queue,
                role,
                admission,
                rate,
                admitted_at,
                work,
            } = attempt;
            let work = match work {
                Work::Held { slot, .. }
                    if self.msdu_expired(role, queue.category(), admitted_at) =>
                {
                    self.finish_attempt(
                        hardware,
                        role,
                        queue,
                        admission,
                        slot,
                        PublicationEnd::Expired { msdus: 1 },
                        rate,
                        sink,
                    );
                    continue;
                }
                Work::Held { slot, plan, single } => {
                    match self
                        .tx
                        .start_queued_single_attempt(hardware, slot, plan, single)
                    {
                        Ok(queued) => Work::Published(queued),
                        Err(QueuedSingleAttemptRefused { error, slot }) => {
                            self.finish_attempt(
                                hardware,
                                role,
                                queue,
                                admission,
                                slot,
                                PublicationEnd::Withdrawn,
                                rate,
                                sink,
                            );
                            self.withdraw_held_attempts(hardware, sink);
                            return Err(LowerMacFault::Tx(error));
                        }
                    }
                }
                published @ (Work::Published(_) | Work::Quarantined { .. }) => published,
            };
            self.queues[index] = Some(Attempt {
                queue,
                role,
                admission,
                rate,
                admitted_at,
                work,
            });
        }
        Ok(())
    }
    #[inline(never)]
    fn advance_vif<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        &mut self,
        hardware: &mut H,
        sink: &mut K,
        config: CoreVif,
        policy: ReceivePolicy,
    ) -> bool {
        if self.fault.is_some() {
            return false;
        }
        for index in 0..LOWER_MAC_TX_QUEUES {
            if self.queues[index]
                .as_ref()
                .is_some_and(|attempt| attempt.role == config.role)
            {
                self.abort_held(hardware, index, sink);
            }
        }
        if self.attempts().any(|attempt| attempt.role == config.role) {
            false
        } else {
            self.apply_vif(hardware, config, policy);
            true
        }
    }
}
