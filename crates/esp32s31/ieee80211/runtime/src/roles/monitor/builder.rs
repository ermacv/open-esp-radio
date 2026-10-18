//! One-shot materialization of a standalone ESP32-S31 monitor owner graph.

#![forbid(unsafe_code)]

use crate::{
    datapath::{
        irq::InterruptEpoch,
        rx::{dma::ReceiveDmaStorage, frontier::RxFrontierError},
    },
    roles::monitor::{
        MonitorCommandReceiver, MonitorCompletion, MonitorControlError, MonitorControlResources,
        MonitorController,
        rx::{MonitorPrepareError, MonitorRx},
        service::{
            MonitorRunError, MonitorRunFailure, MonitorRunReport, MonitorService,
            MonitorStoppedAccessError,
        },
    },
};

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::{Duration, Timer};

use oer_esp32s31_hal::{owner::RadioRuntimeOwner, types::MacInterruptEnableState};

use oer_esp32s31_hal::shared_radio::PlatformClockProvider;
use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyTargetObserver};
use oer_esp32s31_radio_runtime::RadioSystem;

use oer_esp32s31_ieee80211::{
    mac_start::WifiMacStartReport,
    runtime::{WifiRoleOwner, WifiRuntimeContext},
    switch_esp32s31_wifi_channel,
};

use oer_esp32s31_ieee80211_dma::rx_storage::RxDmaStorageError;

use oer_esp32s31_ieee80211_mac::{
    irq::MacInterruptRoute,
    rx::{RxDmaBufferAddresses, RxPhyInfo, RxRingHalted, RxRingLive},
};

use oer_ieee80211_mac::channel::WifiChannel;

use oer_ieee80211_softmac::{
    MonitorChannelPolicy, MonitorChannelSequence, MonitorSink, WifiStandaloneMonitorPlan,
};

/// Permanently located DMA arena and its once-derived address table.
///
/// Construction happens before any radio owner moves. Once successful the
/// address table becomes immutable for the rest of the firmware lifetime,
/// matching the DMA ring's real safety requirement.
#[derive(Clone, Copy)]
pub struct MonitorStorage<
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    buffer_addresses: &'static RxDmaBufferAddresses<COUNT>,
    descriptor_base: u32,
}

impl<const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
{
    pub fn new(
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        buffer_addresses: &'static mut RxDmaBufferAddresses<COUNT>,
    ) -> Result<Self, RxDmaStorageError> {
        let descriptor_base = storage.dma_layout(buffer_addresses)?;
        Ok(Self {
            storage,
            buffer_addresses,
            descriptor_base,
        })
    }

    /// Reborrow the common RX arena for another role-local stopped resource
    /// graph. A physical supervisor must still enforce exclusive activation.
    pub const fn storage(
        self,
    ) -> &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub const fn buffer_addresses(self) -> &'static RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    pub const fn descriptor_base(self) -> u32 {
        self.descriptor_base
    }
}

/// Role-neutral physical radio frontier consumed by a monitor task.
///
/// The interrupt epoch is deliberately already materialized. Logical role
/// changes never uninstall its CPU route while the MAC remains powered.
pub struct MonitorRadio<'runtime, P, Membership, R, M: RawMutex>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
{
    pub owner: WifiRoleOwner<P, Membership>,
    pub registers: RadioRuntimeOwner,
    pub interrupts: InterruptEpoch<'runtime, R, M>,
}

impl<'runtime, P, Membership, R, M: RawMutex> MonitorRadio<'runtime, P, Membership, R, M>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
{
    pub fn new(
        owner: WifiRoleOwner<P, Membership>,
        registers: RadioRuntimeOwner,
        interrupts: InterruptEpoch<'runtime, R, M>,
    ) -> Self {
        Self {
            owner,
            registers,
            interrupts,
        }
    }
}

/// Physical RX-ring authority accepted by a monitor role.
///
/// `Halted` exists only for the first cold materialization. Every logical
/// role handoff after the walker has started carries `Live` unchanged.
pub enum MonitorRxRing<const COUNT: usize> {
    Halted(RxRingHalted<'static, COUNT>),
    Live(RxRingLive<'static, COUNT>),
}

/// Runtime-owned resources consumed together when a monitor role starts.
///
/// The frame arena and address table are board placement policy. The route,
/// wake runtimes and sink are executor integration. Grouping them prevents a
/// caller from accidentally pairing an RX ring with a different IRQ epoch.
struct MonitorResources<
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    dma: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    rx_ring: Option<MonitorRxRing<COUNT>>,
    sink: S,
}

impl<S, const COUNT: usize, const DMA_BUFFER_SIZE: usize, const DMA_STORAGE_SIZE: usize>
    MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    const fn new(
        dma: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        rx_ring: Option<MonitorRxRing<COUNT>>,
        sink: S,
    ) -> Self {
        Self { dma, rx_ring, sink }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorBuildError {
    Control(MonitorControlError),
    Receive(MonitorPrepareError),
}

/// Failed materialization retaining both the MAC owner and every runtime
/// resource for retry or explicit reset policy.
struct MonitorBuildFailure<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub radio: MonitorRadio<'runtime, P, Membership, R, M>,
    pub resources: MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorBuildReport {
    pub start: WifiMacStartReport,
    pub cold_interrupt_mask: MacInterruptEnableState,
    pub descriptor_base: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelSwitchError {
    Active(MonitorStoppedAccessError),
    Phy(ConcurrentWifiChannelError),
    Receive(RxFrontierError),
    Quarantined,
    PolicyMismatch {
        expected: WifiChannel,
        active: WifiChannel,
    },
}

/// Complete stopped standalone-monitor owner.
///
/// Calling `run_until_stopped` on the contained service creates a finite live
/// IRQ/DMA epoch. The PHY state remains here for a later stopped-only channel
/// switch; it is never duplicated in the capture consumer.
struct MonitorOwner<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    service: MonitorService<
        'static,
        'runtime,
        RadioRuntimeOwner,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    context: WifiRuntimeContext<Membership>,
    report: MonitorBuildReport,
    plan: WifiStandaloneMonitorPlan,
    memory: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    capture_channel: WifiChannel,
    quarantined: bool,
}

impl<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorOwner<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn report(&self) -> MonitorBuildReport {
        self.report
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }

    pub const fn capture_channel(&self) -> WifiChannel {
        self.capture_channel
    }

    /// Return the common live radio owner and every reusable monitor resource
    /// after the logical monitor consumer is parked.
    #[expect(
        clippy::result_large_err,
        reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
    )]
    fn try_into_stopped(
        self,
        control: &'runtime MonitorControlResources<M>,
    ) -> Result<
        MonitorStopped<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
    > {
        if self.quarantined || self.service.is_quarantined() {
            return Err(self);
        }
        let Self {
            service,
            context,
            report,
            plan,
            memory,
            capture_channel,
            quarantined,
        } = self;
        let (registers, receive, sink, interrupts, platform) = match service.try_into_parts() {
            Ok(parts) => parts,
            Err(service) => {
                return Err(Self {
                    service,
                    context,
                    report,
                    plan,
                    memory,
                    capture_channel,
                    quarantined,
                });
            }
        };
        let live_ring = receive
            .into_live()
            .unwrap_or_else(|_| unreachable!("a decomposable monitor service retains live RX"));
        let radio = MonitorRadio::new(
            WifiRoleOwner::from_runtime_parts(platform, context),
            registers,
            interrupts,
        );
        Ok(MonitorStopped {
            radio,
            plan,
            resources: MonitorStoppedExecutionResources {
                memory,
                live_ring,
                sink,
                control,
            },
        })
    }

    /// Run inside the long-lived role task until its application handle asks
    /// for shutdown. Completion is published only after IRQ and DMA are
    /// confirmed quiescent, or as `Faulted` while this task still retains every
    /// hardware-visible owner in quarantine for explicit board recovery.
    pub async fn run_controlled(
        &mut self,
        control: &mut MonitorCommandReceiver<'_, M>,
        timer: &impl Timer,
    ) -> Result<MonitorRunReport, MonitorRunFailure<R::Error>> {
        let result = self
            .service
            .run_until_stopped(control.wait_stop(), timer)
            .await;
        control.complete(if self.service.is_quiescent() {
            MonitorCompletion::Stopped
        } else {
            MonitorCompletion::Faulted
        });
        result
    }

    /// Retune only while the monitor's IRQ route and RX walker are stopped,
    /// leasing the shared radio for this one transaction.
    pub async fn switch_channel<D, O, RP, RC>(
        &mut self,
        radio: &RadioSystem<RP, RC, D>,
        channel: WifiChannel,
        observer: &mut O,
    ) -> Result<(), MonitorChannelSwitchError>
    where
        D: oer_time::Timer,
        O: PhyTargetObserver,
        RC: PlatformClockProvider,
    {
        if self.quarantined {
            return Err(MonitorChannelSwitchError::Quarantined);
        }
        self.service
            .begin_stopped_transition()
            .map_err(MonitorChannelSwitchError::Active)?;
        self.quarantined = true;
        let registers = self
            .service
            .stopped_hardware_mut()
            .map_err(MonitorChannelSwitchError::Active)?;
        let mut guard = radio.lock().await;
        let (lease, platform, _) = guard.parts();
        let switched = switch_esp32s31_wifi_channel::<oer_esp32s31_phy::RomShortDelay, _, _>(
            radio.timer(),
            lease,
            platform,
            registers,
            channel,
            observer,
        )
        .await;
        drop(guard);
        if let Err(error) = switched {
            self.quarantined = true;
            return Err(MonitorChannelSwitchError::Phy(error));
        }
        self.context.set_current_channel(channel);
        if let Err(error) = self.service.prepare_next_receive_epoch() {
            self.quarantined = true;
            return Err(MonitorChannelSwitchError::Receive(error));
        }
        if let Err(error) = self.service.begin_channel_epoch(channel) {
            self.quarantined = true;
            return Err(MonitorChannelSwitchError::Active(error));
        }
        self.capture_channel = channel;
        self.service.complete_stopped_transition();
        self.quarantined = false;
        Ok(())
    }
}

/// Join a checked standalone-monitor plan, common-MAC owner and runtime
/// resources without exposing intermediate DMA or interrupt setup tokens.
#[expect(
    clippy::result_large_err,
    reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
)]
#[expect(
    clippy::type_complexity,
    reason = "the return type preserves the concrete radio, sink and DMA resource dimensions"
)]
fn prepare_esp32s31_monitor<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    plan: WifiStandaloneMonitorPlan,
    mut radio: MonitorRadio<'runtime, P, Membership, R, M>,
    mut resources: MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
) -> Result<
    MonitorOwner<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    MonitorBuildFailure<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    let cold_interrupt_mask = radio.owner.transition_report().cold_interrupt_mask;
    let initial_channel = radio.owner.current_channel();
    let receive = {
        let mut registers = radio.registers.wifi_mac_hal();
        match resources.rx_ring.take() {
            Some(MonitorRxRing::Halted(ring)) => {
                match MonitorRx::prepare_halted(plan, ring, &mut registers, resources.dma.storage) {
                    Ok(receive) => Ok(receive),
                    Err((ring, error)) => {
                        resources.rx_ring = Some(MonitorRxRing::Halted(ring));
                        Err(error)
                    }
                }
            }
            Some(MonitorRxRing::Live(ring)) => {
                match MonitorRx::from_live(plan, ring, resources.dma.storage) {
                    Ok(receive) => Ok(receive),
                    Err((ring, error)) => {
                        resources.rx_ring = Some(MonitorRxRing::Live(ring));
                        Err(error)
                    }
                }
            }
            None => MonitorRx::prepare_initial(
                plan,
                &mut registers,
                resources.dma.storage,
                resources.dma.descriptor_base,
                resources.dma.buffer_addresses,
            ),
        }
    };
    let receive = match receive {
        Ok(receive) => receive,
        Err(error) => {
            return Err(MonitorBuildFailure {
                error: MonitorBuildError::Receive(error),
                plan,
                radio,
                resources,
            });
        }
    };
    let start = radio.owner.start_report();
    let (platform, context) = radio.owner.into_runtime_parts();
    let mut service = MonitorService::new(
        radio.registers,
        receive,
        resources.sink,
        radio.interrupts,
        platform,
    );
    service
        .begin_channel_epoch(initial_channel)
        .expect("a newly prepared monitor service is quiescent");
    Ok(MonitorOwner {
        service,
        context,
        report: MonitorBuildReport {
            start,
            cold_interrupt_mask,
            descriptor_base: resources.dma.descriptor_base,
        },
        plan,
        memory: resources.dma,
        capture_channel: initial_channel,
        quarantined: false,
    })
}

/// Board-owned resources for one standalone monitor task.
///
/// This is the only public monitor materialization input. DMA placement,
/// capture publication and control storage move together. Physical interrupt
/// ownership belongs to [`MonitorRadio`], not this logical role.
pub struct MonitorExecutionResources<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    runtime: MonitorResources<S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    control: &'runtime MonitorControlResources<M>,
}

impl<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    pub fn new(
        memory: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        rx_ring: Option<MonitorRxRing<COUNT>>,
        sink: S,
        control: &'runtime MonitorControlResources<M>,
    ) -> Self {
        Self {
            runtime: MonitorResources::new(memory, rx_ring, sink),
            control,
        }
    }
}

/// Reusable board/executor resources returned by a stopped monitor role.
///
/// Physical interrupt ownership is returned separately in the common radio
/// frontier, so these resources remain purely role-local.
pub struct MonitorStoppedExecutionResources<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    memory: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    live_ring: RxRingLive<'static, COUNT>,
    sink: S,
    control: &'runtime MonitorControlResources<M>,
}

/// Named role-local owner set returned when monitor resources are rebound to a
/// different Wi-Fi role by a supervisor or qualification harness.
pub struct MonitorStoppedResourceParts<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    S: MonitorSink<RxPhyInfo>,
{
    pub memory: MonitorStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub live_ring: RxRingLive<'static, COUNT>,
    pub sink: S,
    pub control: &'runtime MonitorControlResources<M>,
}

impl<
    'runtime,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorStoppedExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    S: MonitorSink<RxPhyInfo>,
{
    pub fn into_parts(
        self,
    ) -> MonitorStoppedResourceParts<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        MonitorStoppedResourceParts {
            memory: self.memory,
            live_ring: self.live_ring,
            sink: self.sink,
            control: self.control,
        }
    }

    /// Rebind the exact returned role-local resources to another monitor task.
    pub fn into_task_resources(
        self,
    ) -> MonitorExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        MonitorExecutionResources::new(
            self.memory,
            Some(MonitorRxRing::Live(self.live_ring)),
            self.sink,
            self.control,
        )
    }
}

/// Fully dematerialized standalone monitor role.
///
/// This value can exist only after the task parked its logical consumer and
/// returned the live RX ring. `radio` retains the installed physical IRQ
/// epoch for the next role.
pub struct MonitorStopped<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub radio: MonitorRadio<'runtime, P, Membership, R, M>,
    pub plan: WifiStandaloneMonitorPlan,
    pub resources:
        MonitorStoppedExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

/// Failed task materialization with the complete retryable owner frontier.
pub struct MonitorTaskBuildFailure<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub error: MonitorBuildError,
    pub plan: WifiStandaloneMonitorPlan,
    pub radio: MonitorRadio<'runtime, P, Membership, R, M>,
    pub resources:
        MonitorExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
}

/// Executor-side owner of all hardware used by a standalone monitor role.
///
/// Applications retain only [`MonitorController`]. This value belongs
/// in one long-lived executor task and never crosses the application control
/// plane while its IRQ/DMA epoch is active.
pub struct MonitorTask<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    owner: MonitorOwner<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    control: MonitorCommandReceiver<'runtime, M>,
}

/// Terminal output of one consuming standalone-monitor task epoch.
///
/// A returned stopped owner proves that IRQ routing and RX DMA are both
/// inactive. `Faulted` deliberately retains the complete task at its exact
/// hardware frontier; callers may only hand it to reset policy and cannot
/// misclassify a run error as reusable stopped Wi-Fi.
pub enum MonitorTaskExit<S, T, E> {
    Stopped {
        stopped: S,
        result: Result<MonitorRunReport, MonitorRunFailure<E>>,
    },
    Faulted {
        task: T,
        result: Result<MonitorRunReport, MonitorRunFailure<E>>,
    },
}

impl<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorTask<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    fn policy_mismatch(&mut self, expected: WifiChannel) -> MonitorRunFailure<R::Error> {
        let active = self.owner.capture_channel();
        self.control.complete(MonitorCompletion::Stopped);
        MonitorRunFailure {
            error: MonitorRunError::Channel(MonitorChannelSwitchError::PolicyMismatch {
                expected,
                active,
            }),
            report: MonitorRunReport::default(),
        }
    }

    async fn run_hopping<D, O, RP, RC>(
        &mut self,
        radio: &RadioSystem<RP, RC, D>,
        sequence: MonitorChannelSequence,
        observer: &mut O,
    ) -> Result<MonitorRunReport, MonitorRunFailure<R::Error>>
    where
        D: oer_time::Timer,
        O: PhyTargetObserver,
        RC: PlatformClockProvider,
    {
        if self.owner.current_channel() != sequence.first()
            || self.owner.capture_channel() != sequence.first()
        {
            return Err(self.policy_mismatch(sequence.first()));
        }

        let mut report = MonitorRunReport::default();
        let channels = sequence.channels();
        let mut channel_index = 0_usize;
        loop {
            if self.control.stop_requested() {
                self.control.complete(MonitorCompletion::Stopped);
                return Ok(report);
            }

            let epoch = {
                let owner = &mut self.owner;
                let control = &mut self.control;
                owner
                    .service
                    .run_until_boundary(
                        select(
                            control.wait_stop(),
                            crate::time::wait_for(
                                radio.timer(),
                                Duration::from_millis(u32::from(sequence.dwell_millis())),
                            ),
                        ),
                        radio.timer(),
                    )
                    .await
            };
            let (epoch, boundary) = match epoch {
                Ok(completed) => completed,
                Err(failure) => {
                    report.merge(failure.report);
                    self.control.complete(
                        if self.owner.service.is_quiescent()
                            && !self.owner.quarantined
                            && !self.owner.service.is_quarantined()
                        {
                            MonitorCompletion::Stopped
                        } else {
                            MonitorCompletion::Faulted
                        },
                    );
                    return Err(MonitorRunFailure {
                        error: failure.error,
                        report,
                    });
                }
            };
            report.merge(epoch);
            match boundary {
                Either::First(()) => {
                    self.control.complete(MonitorCompletion::Stopped);
                    return Ok(report);
                }
                Either::Second(_) => {}
            }

            // A stop published while the dwell boundary was being closed wins
            // before another stopped-only retune or RX epoch begins.
            if self.control.stop_requested() {
                self.control.complete(MonitorCompletion::Stopped);
                return Ok(report);
            }
            channel_index = (channel_index + 1) % channels.len();
            let next_channel = channels[channel_index];
            if let Err(error) = self
                .owner
                .switch_channel::<D, O, RP, RC>(radio, next_channel, observer)
                .await
            {
                self.owner.quarantined = true;
                self.owner.service.force_quarantine();
                self.control.complete(MonitorCompletion::Faulted);
                return Err(MonitorRunFailure {
                    error: MonitorRunError::Channel(error),
                    report,
                });
            }
            report.channel_switches = report.channel_switches.saturating_add(1);
        }
    }
}

impl<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorTask<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn report(&self) -> MonitorBuildReport {
        self.owner.report()
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.owner.current_channel()
    }

    /// Reach the current S31 injection frontier through this task's actual
    /// capture dwell. The present backend fails at its unassigned monitor TX
    /// interface before borrowing sequence, DMA or IRQ state.
    pub fn admit_injection_frontier<const TX_BUFFER_SIZE: usize>(
        &self,
        request: oer_ieee80211_softmac::MonitorInjectionRequest<'_>,
    ) -> Result<
        oer_esp32s31_ieee80211::monitor_injection::MonitorInjectionAdmission,
        oer_esp32s31_ieee80211::monitor_injection::MonitorInjectionAdmissionError,
    > {
        self.owner
            .service
            .admit_injection::<TX_BUFFER_SIZE>(self.owner.plan, request)
    }

    /// Run the finite role epoch. The task-side command receiver is private,
    /// so only the paired controller can request the terminal edge.
    pub async fn run(
        &mut self,
        timer: &impl Timer,
    ) -> Result<MonitorRunReport, MonitorRunFailure<R::Error>> {
        self.owner.run_controlled(&mut self.control, timer).await
    }

    /// Run and consume one complete role epoch for a supervisor task wrapper.
    ///
    /// The run result and hardware ownership are intentionally independent:
    /// an operational error can still leave a safely stopped owner, while an
    /// apparently completed control exchange must not permit reuse if the
    /// IRQ/DMA owners did not return.
    #[allow(clippy::type_complexity)]
    pub async fn run_to_exit(
        mut self,
        timer: &impl Timer,
    ) -> MonitorTaskExit<
        MonitorStopped<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
        R::Error,
    > {
        let result = self.run(timer).await;
        match self.try_into_stopped() {
            Ok(stopped) => MonitorTaskExit::Stopped { stopped, result },
            Err(task) => MonitorTaskExit::Faulted { task, result },
        }
    }

    /// Run either the compatible fixed-channel epoch or one bounded hopping
    /// cycle until the paired controller requests stop.
    #[allow(clippy::type_complexity)]
    pub async fn run_channel_policy_to_exit<D, O, RP, RC>(
        mut self,
        radio: &RadioSystem<RP, RC, D>,
        policy: MonitorChannelPolicy,
        observer: &mut O,
    ) -> MonitorTaskExit<
        MonitorStopped<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
        R::Error,
    >
    where
        D: oer_time::Timer,
        O: PhyTargetObserver,
        RC: PlatformClockProvider,
    {
        let result = match policy {
            MonitorChannelPolicy::Fixed(channel)
                if self.owner.current_channel() != channel
                    || self.owner.capture_channel() != channel =>
            {
                Err(self.policy_mismatch(channel))
            }
            MonitorChannelPolicy::Fixed(_) => self.run(radio.timer()).await,
            MonitorChannelPolicy::Hopping(sequence) => {
                self.run_hopping::<D, O, RP, RC>(radio, sequence, observer)
                    .await
            }
        };
        match self.try_into_stopped() {
            Ok(stopped) => MonitorTaskExit::Stopped { stopped, result },
            Err(task) => MonitorTaskExit::Faulted { task, result },
        }
    }

    /// Consume a task only after its hardware epoch is fully stopped.
    ///
    /// Success acknowledges `Stopped` to the paired controller and returns
    /// both the common Wi-Fi owner and role-local reusable resources. If IRQ
    /// routing or RX DMA is still active, the complete task is returned.
    #[expect(
        clippy::result_large_err,
        reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
    )]
    pub fn try_into_stopped(
        self,
    ) -> Result<
        MonitorStopped<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        Self,
    > {
        let Self { owner, mut control } = self;
        let control_resources = control.resources();
        match owner.try_into_stopped(control_resources) {
            Ok(stopped) => {
                control.complete(MonitorCompletion::Stopped);
                drop(control);
                Ok(stopped)
            }
            Err(owner) => Err(Self { owner, control }),
        }
    }

    /// Retune only while the contained IRQ/DMA service is stopped.
    pub async fn switch_channel<D, O, RP, RC>(
        &mut self,
        radio: &RadioSystem<RP, RC, D>,
        channel: WifiChannel,
        observer: &mut O,
    ) -> Result<(), MonitorChannelSwitchError>
    where
        D: oer_time::Timer,
        O: PhyTargetObserver,
        RC: PlatformClockProvider,
    {
        self.owner
            .switch_channel::<D, O, RP, RC>(radio, channel, observer)
            .await
    }
}

/// Materialize one executor-owned task and its hardware-free application
/// controller in a single transaction.
#[allow(clippy::type_complexity)]
#[expect(
    clippy::result_large_err,
    reason = "failure retains the exact radio, IRQ and DMA owners without heap allocation"
)]
pub fn prepare_esp32s31_monitor_task<
    'runtime,
    P,
    Membership,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    plan: WifiStandaloneMonitorPlan,
    radio: MonitorRadio<'runtime, P, Membership, R, M>,
    resources: MonitorExecutionResources<'runtime, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
) -> Result<
    (
        MonitorController<'runtime, M>,
        MonitorTask<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ),
    MonitorTaskBuildFailure<
        'runtime,
        P,
        Membership,
        R,
        M,
        S,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
>
where
    R: MacInterruptRoute<Platform = P>,
    P: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    let MonitorExecutionResources { runtime, control } = resources;
    let (controller, mut receiver) = match control.split() {
        Ok(endpoints) => endpoints,
        Err(error) => {
            return Err(MonitorTaskBuildFailure {
                error: MonitorBuildError::Control(error),
                plan,
                radio,
                resources: MonitorExecutionResources { runtime, control },
            });
        }
    };
    match prepare_esp32s31_monitor(plan, radio, runtime) {
        Ok(owner) => Ok((
            controller,
            MonitorTask {
                owner,
                control: receiver,
            },
        )),
        Err(failure) => {
            // No task or live IRQ/DMA epoch was created. Return the control
            // lease cleanly so the exact build resources remain retryable.
            receiver.complete(MonitorCompletion::Stopped);
            drop(receiver);
            drop(controller);
            Err(MonitorTaskBuildFailure {
                error: failure.error,
                plan: failure.plan,
                radio: failure.radio,
                resources: MonitorExecutionResources {
                    runtime: failure.resources,
                    control,
                },
            })
        }
    }
}
