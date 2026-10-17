//! Consuming lifecycle and command barriers for the S31 hardware bank.
//!
//! Only an enabled owner admits TX. Disable and configuration retain that
//! owner while their published attempts drain. A closed gate is constructed
//! only after the published queues have ended; new admissions in ClosingGate
//! and ClosedGate remain held. No runtime flag duplicates these states.
use super::*;
use core::ops::{Deref, DerefMut};

/// The disabled, idle hardware bank. Only this owner can retune or dismantle.
#[must_use]
pub struct DisabledCore<C, G = OpenGate> {
    core: C,
    gate: core::marker::PhantomData<G>,
}
/// An asynchronous enable owns its bank until the radio reports its retune.
#[must_use]
pub struct RetuningCore<C, G = OpenGate> {
    core: C,
    gate: core::marker::PhantomData<G>,
}
/// An enabled bank; the gate type determines whether a submission publishes.
#[must_use]
pub struct EnabledCore<C, G = OpenGate> {
    core: C,
    gate: core::marker::PhantomData<G>,
}
/// Disable owns every outstanding publication until its final terminal.
#[must_use]
pub struct DisablingCore<C, G = OpenGate> {
    core: C,
    gate: core::marker::PhantomData<G>,
}
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
/// Closed set of admission policies; only the core's transitions change it.
pub trait CoreGate: sealed::Gate {
    #[doc(hidden)]
    const PUBLISH: bool;
}
impl CoreGate for OpenGate {
    const PUBLISH: bool = true;
}
impl CoreGate for ClosingGate {
    const PUBLISH: bool = false;
}
impl CoreGate for ClosedGate {
    const PUBLISH: bool = false;
}

/// Enable either returns its enabled owner or retains it for an actual retune.
#[must_use]
pub enum LifecycleStart<C, G = OpenGate> {
    Enabled(EnabledCore<C, G>),
    Retune(RetuningCore<C, G>),
}
/// A gate barrier retains the closing owner until its published prefix ends.
#[must_use]
pub enum GateClosure<C> {
    Closing(EnabledCore<C, ClosingGate>),
    Closed(EnabledCore<C, ClosedGate>),
}
/// Configuration owns the bank, policy and requested interface while TX drains.
#[must_use]
pub struct Configuring<C> {
    core: C,
    config: CoreVif,
    policy: ReceivePolicy,
}
/// Configuration completes exactly once, returning its original lifecycle owner.
#[must_use]
pub enum ConfigurationProgress<C> {
    Waiting(Configuring<C>),
    Configured(C),
}
/// A refused receive policy returns the unchanged owner.
#[must_use]
pub struct ConfigureRefused<C> {
    pub core: C,
    pub reason: UnsupportedVif,
}
/// Gate publication failed; this owner retains every published predecessor.
#[must_use]
pub struct GateOpenFailed<C> {
    pub fault: LowerMacFault,
    pub(super) core: DisablingCore<C, ClosedGate>,
}

impl<C, G> DisabledCore<C, G> {
    pub(super) fn new(core: C) -> Self {
        Self {
            core,
            gate: core::marker::PhantomData,
        }
    }
}
impl<C, G> EnabledCore<C, G> {
    fn new(core: C) -> Self {
        Self {
            core,
            gate: core::marker::PhantomData,
        }
    }
}

impl<C, G> Deref for DisabledCore<C, G> {
    type Target = C;
    fn deref(&self) -> &C {
        &self.core
    }
}
impl<C, G> DerefMut for DisabledCore<C, G> {
    fn deref_mut(&mut self) -> &mut C {
        &mut self.core
    }
}

impl<C, G> Deref for RetuningCore<C, G> {
    type Target = C;
    fn deref(&self) -> &C {
        &self.core
    }
}
impl<C, G> DerefMut for RetuningCore<C, G> {
    fn deref_mut(&mut self) -> &mut C {
        &mut self.core
    }
}

impl<C, G> Deref for DisablingCore<C, G> {
    type Target = C;
    fn deref(&self) -> &C {
        &self.core
    }
}
impl<C, G> DerefMut for DisablingCore<C, G> {
    fn deref_mut(&mut self) -> &mut C {
        &mut self.core
    }
}

impl<C, G> Deref for EnabledCore<C, G> {
    type Target = C;
    fn deref(&self) -> &C {
        &self.core
    }
}
impl<C, G> DerefMut for EnabledCore<C, G> {
    fn deref_mut(&mut self) -> &mut C {
        &mut self.core
    }
}

impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> DisabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Recover construction resources. This type has no admitted publication.
    pub fn dismantle(self) -> LowerMacCoreParts<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS> {
        let core = self.core;
        LowerMacCoreParts {
            authority: core.authority,
            tx: core.tx,
            config: core.config,
            coex: core.coex,
            keys: core.keys,
        }
    }
    /// Record the actual channel after a radio start.
    pub fn radio_tuned_to(&mut self, channel: WifiChannel) {
        self.core.tuned = channel;
    }
    /// Choose the next enable's channel; an enabled owner has no such operation.
    pub fn set_checked_channel(&mut self, channel: WifiChannel) {
        self.core.set_checked_channel(channel);
    }
    /// Consume disabled ownership, retaining it across an asynchronous retune.
    pub fn enable_checked(
        self,
    ) -> LifecycleStart<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G> {
        if self.core.channel == self.core.tuned {
            LifecycleStart::Enabled(EnabledCore::new(self.core))
        } else {
            LifecycleStart::Retune(RetuningCore {
                core: self.core,
                gate: core::marker::PhantomData,
            })
        }
    }
    /// Reserve the bank for a receive policy's FIFO barrier.
    #[allow(clippy::result_large_err, reason = "refusal retains the entire bank")]
    pub fn configure(self, config: CoreVif) -> Result<Configuring<Self>, ConfigureRefused<Self>> {
        match receive_policy(&self.core.config, &config) {
            Ok(policy) => Ok(Configuring {
                core: self,
                config,
                policy,
            }),
            Err(reason) => Err(ConfigureRefused { core: self, reason }),
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> RetuningCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// The actual target of this enable's retune.
    pub fn target_channel(&self) -> WifiChannel {
        self.core.channel
    }
    /// A failed retune returns disabled ownership on the previous channel.
    #[allow(clippy::result_large_err, reason = "a failed retune retains the bank")]
    #[expect(
        clippy::type_complexity,
        reason = "the consuming transition preserves every concrete hardware owner and gate state"
    )]
    pub fn finish_retune(
        mut self,
        retuned: bool,
    ) -> Result<
        EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>,
        DisabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>,
    > {
        if retuned {
            self.core.tuned = self.core.channel;
            Ok(EnabledCore::new(self.core))
        } else {
            Err(DisabledCore::new(self.core))
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Admit an MPDU only while enabled; the gate's type selects publication.
    #[allow(
        clippy::result_large_err,
        reason = "refusal returns all admitted owners"
    )]
    pub fn submit_checked<H: LowerMacHardware>(
        &mut self,
        hardware: &mut H,
        queue: FreeQueue<'core, Path>,
        admission: TxAdmission<'core, Path>,
        values: MpduValues<'slot, BUFFER_SIZE>,
    ) -> Result<(), SubmitFault<'core, 'slot, BUFFER_SIZE, Path>> {
        self.core
            .submit_checked(hardware, queue, admission, values, G::PUBLISH)
    }
    /// An enabled receiver narrows the hardware's superset of receive rules.
    pub fn received<'frame, H: StaNoiseFloorHardware>(
        &self,
        hardware: &H,
        frame: &NormalizedRxFrame<'frame>,
    ) -> Option<(&'frame [u8], Esp32s31RxMeta)> {
        self.core.received(hardware, frame)
    }
    /// Withdraw held attempts and retain published attempts in the drain owner.
    /// A poisoned bank keeps all rights; it can never complete disable.
    pub fn disable_checked<H: CcmpKeyHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> DisablingCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G> {
        if self.core.fault.is_none() {
            self.core.withdraw_held_attempts(hardware, sink);
        }
        DisablingCore {
            core: self.core,
            gate: core::marker::PhantomData,
        }
    }
    /// Reserve the bank at this command's FIFO position. Only earlier attempts
    /// of the requested interface delay it; the other role keeps its owners.
    #[allow(clippy::result_large_err, reason = "refusal retains the entire bank")]
    pub fn configure(self, config: CoreVif) -> Result<Configuring<Self>, ConfigureRefused<Self>> {
        match receive_policy(&self.core.config, &config) {
            Ok(policy) => Ok(Configuring {
                core: self,
                config,
                policy,
            }),
            Err(reason) => Err(ConfigureRefused { core: self, reason }),
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    G: CoreGate,
> DisablingCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>
{
    /// Finish after the final TX terminal. Returning disabled ownership emits
    /// exactly one disabled terminal; a waiting or poisoned owner stays whole.
    #[allow(
        clippy::result_large_err,
        reason = "a draining bank retains all owners"
    )]
    #[expect(
        clippy::type_complexity,
        reason = "the consuming transition preserves every concrete hardware owner and gate state"
    )]
    pub fn finish<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        sink: &mut K,
    ) -> Result<
        DisabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, G>,
        Self,
    > {
        if self.core.fault.is_some() || self.core.attempts().next().is_some() {
            Err(self)
        } else {
            sink.disabled();
            Ok(DisabledCore::new(self.core))
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, OpenGate>
{
    /// Stop new publications by transferring ownership to the closing state.
    pub fn hold_new_attempts(
        self,
    ) -> EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, ClosingGate>
    {
        EnabledCore::new(self.core)
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, ClosingGate>
{
    /// Advance the barrier after a TX terminal. Hardware blocking can only be
    /// reached with no published attempt and with new attempts already held.
    pub fn close_gate<H: LowerMacHardware>(
        self,
        hardware: &mut H,
    ) -> GateClosure<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>> {
        if self.core.fault.is_some() || !self.core.published_idle() {
            GateClosure::Closing(self)
        } else {
            hardware.set_power_save_tx_block(true);
            GateClosure::Closed(EnabledCore::new(self.core))
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, ClosedGate>
{
    /// Publish the held prefix. Failure returns every unpublished attempt and
    /// retains the published prefix in an owner exposing only drain operations.
    #[allow(clippy::result_large_err, reason = "failure retains the complete bank")]
    #[expect(
        clippy::type_complexity,
        reason = "the consuming transition preserves every concrete hardware owner and gate state"
    )]
    pub fn open_gate<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> Result<
        EnabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>>,
        GateOpenFailed<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>>,
    > {
        if let Some(fault) = self.core.fault {
            return Err(GateOpenFailed {
                fault,
                core: DisablingCore {
                    core: self.core,
                    gate: core::marker::PhantomData,
                },
            });
        }
        hardware.set_power_save_tx_block(false);
        for index in 0..LOWER_MAC_TX_QUEUES {
            let Some(attempt) = self.core.queues[index].take() else {
                continue;
            };
            let Attempt {
                queue,
                role,
                admission,
                rate,
                work,
            } = attempt;
            let work = match work {
                Work::Held { slot, plan, single } => {
                    match self
                        .core
                        .tx
                        .start_queued_single_attempt(hardware, slot, plan, single)
                    {
                        Ok(queued) => Work::Published(queued),
                        Err(QueuedSingleAttemptRefused { error, slot }) => {
                            self.core.finish_attempt(
                                hardware,
                                queue,
                                admission,
                                slot,
                                PublicationEnd::Withdrawn,
                                rate,
                                sink,
                            );
                            self.core.withdraw_held_attempts(hardware, sink);
                            return Err(GateOpenFailed {
                                fault: LowerMacFault::Tx(error),
                                core: DisablingCore {
                                    core: self.core,
                                    gate: core::marker::PhantomData,
                                },
                            });
                        }
                    }
                }
                published @ (Work::Published(_) | Work::Quarantined { .. }) => published,
            };
            self.core.queues[index] = Some(Attempt {
                queue,
                role,
                admission,
                rate,
                work,
            });
        }
        Ok(EnabledCore::new(self.core))
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    C: DerefMut<Target = LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>>,
> Configuring<C>
{
    /// A pending publication's deadline bounds this FIFO barrier.
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.core.next_deadline()
    }
    /// Service earlier publications without admitting any later command.
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
    /// Withdraw the requested role's held attempts, then apply its policy once
    /// that role's published attempts end. No configuration races a submission.
    pub fn advance<H: LowerMacHardware, K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        mut self,
        hardware: &mut H,
        sink: &mut K,
    ) -> ConfigurationProgress<C> {
        if self.core.fault.is_some() {
            return ConfigurationProgress::Waiting(self);
        }
        for index in 0..LOWER_MAC_TX_QUEUES {
            if self.core.queues[index]
                .as_ref()
                .is_some_and(|attempt| attempt.role == self.config.role)
            {
                self.core.abort_held(hardware, index, sink);
            }
        }
        if self
            .core
            .attempts()
            .any(|attempt| attempt.role == self.config.role)
        {
            ConfigurationProgress::Waiting(self)
        } else {
            self.core.apply_vif(hardware, self.config, self.policy);
            ConfigurationProgress::Configured(self.core)
        }
    }
}
impl<
    'core,
    'slot,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
> GateOpenFailed<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>>
{
    /// The next deadline of the prefix published before the failure.
    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.core.next_deadline()
    }
    /// Preserve each published attempt's own terminal outcome.
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
    /// Recover disabled ownership only after all published attempts ended.
    #[allow(clippy::result_large_err, reason = "failure retains the complete bank")]
    #[expect(
        clippy::type_complexity,
        reason = "the consuming transition preserves every concrete hardware owner and gate state"
    )]
    pub fn finish<K: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>>(
        self,
        sink: &mut K,
    ) -> Result<
        DisabledCore<LowerMacCore<'core, 'slot, P, E, T, BUFFER_SIZE, Path, TICKETS>, ClosedGate>,
        Self,
    > {
        match self.core.finish(sink) {
            Ok(core) => Ok(core),
            Err(core) => Err(Self {
                fault: self.fault,
                core,
            }),
        }
    }
}
