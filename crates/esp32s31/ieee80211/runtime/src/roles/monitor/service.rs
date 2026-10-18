#![cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        clippy::result_large_err,
        reason = "monitor preparation retains the exact RX and IRQ owners on failure"
    )
)]

//! Finite standalone-monitor owner over RX DMA and one MAC interrupt epoch.

#![forbid(unsafe_code)]

use core::{future::Future, pin::pin};

use crate::{
    datapath::{
        irq::{
            InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochDrain,
            MacInterruptEpochQuiesceError,
        },
        rx::frontier::{RxFrontierError, RxFrontierPhase},
    },
    roles::monitor::rx::{MonitorRx, MonitorRxProgress},
};

use embassy_futures::{
    select::{Either, select},
    yield_now,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::Timer;

use oer_esp32s31_hal::types::MacInterruptMask;

use oer_esp32s31_ieee80211_mac::{
    init::{
        MAC_COLD_RX_INTERRUPT_MASK, MacRuntimeStopHardware, MacSnifferHardware,
        activate_promiscuous_receive, deactivate_promiscuous_receive,
    },
    irq::MacInterruptRoute,
    rx::{RxDma, RxPhyInfo},
};

use oer_ieee80211_softmac::{
    MonitorInjectionRequest, MonitorSink, WifiChannel, WifiStandaloneMonitorPlan,
};

/// Qualified interrupt mask retained by a standalone normalized monitor.
///
/// This is deliberately the complete recovered cold-RX mask rather than only
/// `RX_SUCCESS`. It retains acknowledgement of status which accompanies RX
/// on sustained traffic and whose independent semantics are not qualified.
pub const ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK: MacInterruptMask = MAC_COLD_RX_INTERRUPT_MASK;

/// Aggregate progress for one finite monitor run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MonitorRunReport {
    /// Fully started, independently stopped physical-channel capture epochs.
    pub channel_epochs: u32,
    /// Successful stopped-only retunes between channel epochs.
    pub channel_switches: u32,
    /// Bottom-half service epochs, including the initial handoff probe.
    pub rx_service_wakes: u32,
    /// Actual hard-IRQ RX work posts observed during this run.
    #[cfg(any(feature = "diagnostics", test))]
    pub rx_interrupt_posts: u32,
    pub receive: MonitorRxProgress,
    pub interrupt_drain: MacInterruptEpochDrain,
}

impl MonitorRunReport {
    fn record(&mut self, progress: MonitorRxProgress) {
        self.receive.completed_descriptors = self
            .receive
            .completed_descriptors
            .saturating_add(progress.completed_descriptors);
        self.receive.published_frames = self
            .receive
            .published_frames
            .saturating_add(progress.published_frames);
        self.receive.dropped_frames = self
            .receive
            .dropped_frames
            .saturating_add(progress.dropped_frames);
        self.receive.full_drops = self.receive.full_drops.saturating_add(progress.full_drops);
        self.receive.oversized_drops = self
            .receive
            .oversized_drops
            .saturating_add(progress.oversized_drops);
        self.receive.filtered_drops = self
            .receive
            .filtered_drops
            .saturating_add(progress.filtered_drops);
        self.receive.malformed_frames = self
            .receive
            .malformed_frames
            .saturating_add(progress.malformed_frames);
        self.receive.recycled_descriptors = self
            .receive
            .recycled_descriptors
            .saturating_add(progress.recycled_descriptors);
        self.receive.reload_pending = progress.reload_pending;
        self.receive.service_probe_pending = progress.service_probe_pending;
    }

    pub(crate) fn merge(&mut self, epoch: Self) {
        self.channel_epochs = self.channel_epochs.saturating_add(epoch.channel_epochs);
        self.channel_switches = self.channel_switches.saturating_add(epoch.channel_switches);
        self.rx_service_wakes = self.rx_service_wakes.saturating_add(epoch.rx_service_wakes);
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.rx_interrupt_posts = self
                .rx_interrupt_posts
                .saturating_add(epoch.rx_interrupt_posts);
        }
        self.record(epoch.receive);
        self.interrupt_drain.mac.rx |= epoch.interrupt_drain.mac.rx;
        self.interrupt_drain.mac.rx_capacity |= epoch.interrupt_drain.mac.rx_capacity;
        self.interrupt_drain.mac.tx_events |= epoch.interrupt_drain.mac.tx_events;
        self.interrupt_drain.power_events = self
            .interrupt_drain
            .power_events
            .union(epoch.interrupt_drain.power_events);
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn record_interrupt_posts(&mut self, start: u32, current: u32) {
        self.rx_interrupt_posts = current.wrapping_sub(start);
    }
}

/// Failure while closing an interrupt/RX ownership epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorStopError<E> {
    /// CPU/peripheral routing could not be quiesced. RX remains live because
    /// stopping it while a handler may still own the register bank is unsafe.
    Interrupt(MacInterruptEpochQuiesceError<E>),
    /// The interrupt route is quiesced, but the DMA walker did not confirm its
    /// stop. The drain is retained as evidence for reset/recovery policy.
    Receive {
        error: RxFrontierError,
        interrupt_drain: MacInterruptEpochDrain,
    },
}

/// Terminal reason for a failed standalone monitor transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorRunError<E> {
    Start(RxFrontierError),
    Activate(MacInterruptEpochActivateError<E>),
    ActivateStop {
        activation: MacInterruptEpochActivateError<E>,
        stop: MonitorStopError<E>,
    },
    Service {
        error: RxFrontierError,
        stop: Option<MonitorStopError<E>>,
    },
    Stop(MonitorStopError<E>),
    #[cfg(target_arch = "riscv32")]
    Channel(crate::roles::monitor::builder::MonitorChannelSwitchError),
}

/// Why role-level hardware cannot currently be borrowed for a stopped-only
/// operation such as channel switching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorStoppedAccessError {
    RoleActive,
}

/// Failed run report. The borrowed service retains every unique owner at its
/// exact current phase.
pub struct MonitorRunFailure<E> {
    pub error: MonitorRunError<E>,
    pub report: MonitorRunReport,
}

/// Complete standalone monitor owner.
///
/// The hardware value is not borrowed by another task. The capture sink may
/// retain only independent pool leases, never RX DMA storage.
pub struct MonitorService<
    'storage,
    'runtime,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    hardware: Option<H>,
    receive: Option<MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>>,
    sink: Option<S>,
    interrupts: Option<InterruptEpoch<'runtime, R, M>>,
    platform: Option<R::Platform>,
    logical_parked: bool,
    quarantined: bool,
}

impl<
    'storage,
    'runtime,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorService<'storage, 'runtime, H, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    pub const fn new(
        hardware: H,
        receive: MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        sink: S,
        interrupts: InterruptEpoch<'runtime, R, M>,
        platform: R::Platform,
    ) -> Self {
        Self {
            hardware: Some(hardware),
            receive: Some(receive),
            sink: Some(sink),
            interrupts: Some(interrupts),
            platform: Some(platform),
            logical_parked: true,
            quarantined: false,
        }
    }

    pub fn receive_phase(&self) -> RxFrontierPhase {
        self.receive
            .as_ref()
            .expect("monitor owner was already extracted")
            .phase()
    }

    pub fn interrupt_active(&self) -> bool {
        self.interrupts
            .as_ref()
            .expect("monitor owner was already extracted")
            .is_active()
    }

    /// Whether this logical role has returned its service frontier.
    ///
    /// The physical interrupt route deliberately remains installed.
    pub(crate) fn is_quiescent(&self) -> bool {
        self.logical_parked
    }

    pub(crate) const fn is_quarantined(&self) -> bool {
        self.quarantined
    }

    /// Make cancellation of a stopped-only PHY transition retain every owner
    /// until the transition commits successfully.
    pub(crate) fn begin_stopped_transition(&mut self) -> Result<(), MonitorStoppedAccessError> {
        if !self.logical_parked {
            return Err(MonitorStoppedAccessError::RoleActive);
        }
        self.quarantined = true;
        Ok(())
    }

    pub(crate) fn complete_stopped_transition(&mut self) {
        debug_assert!(self.is_quiescent());
        self.quarantined = false;
    }

    pub(crate) fn force_quarantine(&mut self) {
        self.quarantined = true;
    }

    /// Borrow the radio registers only after both asynchronous actors have
    /// released them.
    pub fn stopped_hardware_mut(&mut self) -> Result<&mut H, MonitorStoppedAccessError> {
        if !self.logical_parked {
            return Err(MonitorStoppedAccessError::RoleActive);
        }
        Ok(self
            .hardware
            .as_mut()
            .expect("monitor hardware owner exists"))
    }

    /// Prepare the cold ring for its first epoch. A running physical ring is
    /// deliberately preserved across later logical channel epochs.
    pub(crate) fn prepare_next_receive_epoch(&mut self) -> Result<(), RxFrontierError> {
        debug_assert!(self.is_quiescent());
        let receive = self.receive.as_mut().expect("monitor RX owner exists");
        if receive.phase() == RxFrontierPhase::Live {
            return Ok(());
        }
        let hardware = self
            .hardware
            .as_mut()
            .expect("monitor hardware owner exists");
        receive.prepare_next(hardware)
    }

    /// Bind retained capture metadata to a fresh, fully stopped channel
    /// epoch before RX or the interrupt route is restarted.
    pub(crate) fn begin_channel_epoch(
        &mut self,
        channel: WifiChannel,
    ) -> Result<(), MonitorStoppedAccessError> {
        if !self.logical_parked {
            return Err(MonitorStoppedAccessError::RoleActive);
        }
        self.sink
            .as_mut()
            .expect("monitor sink owner exists")
            .begin_channel_epoch(channel);
        Ok(())
    }

    /// Reach the exact S31 injection frontier through this task's real dwell
    /// owner. The current backend always returns `UnassignedMacInterface`
    /// after binding and finite-buffer validation, before any TX mutation.
    pub(crate) fn admit_injection<const TX_BUFFER_SIZE: usize>(
        &self,
        plan: WifiStandaloneMonitorPlan,
        request: MonitorInjectionRequest<'_>,
    ) -> Result<
        oer_esp32s31_ieee80211::monitor_injection::MonitorInjectionAdmission,
        oer_esp32s31_ieee80211::monitor_injection::MonitorInjectionAdmissionError,
    > {
        oer_esp32s31_ieee80211::monitor_injection::admit_esp32s31_monitor_injection::<
            RxPhyInfo,
            S,
            TX_BUFFER_SIZE,
        >(
            plan,
            self.sink.as_ref().expect("monitor sink owner exists"),
            request,
        )
    }

    /// Decompose the service only after every hardware actor acknowledged its
    /// stopped edge. Role composition uses this to return the common Wi-Fi
    /// owner; active and faulted services remain intact.
    #[allow(clippy::type_complexity)]
    pub(crate) fn try_into_parts(
        mut self,
    ) -> Result<
        (
            H,
            MonitorRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
            S,
            InterruptEpoch<'runtime, R, M>,
            R::Platform,
        ),
        Self,
    > {
        if !self.is_quiescent() || self.quarantined {
            return Err(self);
        }
        Ok((
            self.hardware
                .take()
                .expect("checked monitor hardware owner"),
            self.receive.take().expect("checked monitor RX owner"),
            self.sink.take().expect("checked monitor sink owner"),
            self.interrupts
                .take()
                .expect("checked monitor interrupt owner"),
            self.platform
                .take()
                .expect("checked monitor platform owner"),
        ))
    }
}

impl<
    'storage,
    'runtime,
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> MonitorService<'storage, 'runtime, H, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    /// Run until `stop` resolves, leaving IRQ/policy stopped while the
    /// physical RX ring remains continuously live.
    pub async fn run_until_stopped<F>(
        &mut self,
        stop: F,
        timer: &impl Timer,
    ) -> Result<MonitorRunReport, MonitorRunFailure<R::Error>>
    where
        F: Future<Output = ()>,
    {
        self.run_until_boundary(stop, timer)
            .await
            .map(|(report, ())| report)
    }

    /// Run one fresh RX/IRQ epoch until an arbitrary control boundary resolves.
    ///
    /// The future only borrows `self`. Cancelling it cannot drop the hardware
    /// owner, and dropping the service itself performs the same fail-closed
    /// shutdown before any owner field can be destroyed.
    pub(crate) async fn run_until_boundary<F, T>(
        &mut self,
        boundary: F,
        timer: &impl Timer,
    ) -> Result<(MonitorRunReport, T), MonitorRunFailure<R::Error>>
    where
        F: Future<Output = T>,
    {
        let mut report = MonitorRunReport::default();
        #[cfg(any(feature = "diagnostics", test))]
        let interrupt_posts_at_start = self.interrupts().mac_runtime().rx_post_count();
        activate_promiscuous_receive(
            self.hardware
                .as_mut()
                .expect("monitor hardware owner exists"),
        );
        let start = {
            let receive = self.receive.as_mut().expect("monitor RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("monitor hardware owner exists");
            receive.start(hardware)
        };
        if let Err(error) = start {
            deactivate_promiscuous_receive(
                self.hardware
                    .as_mut()
                    .expect("monitor hardware owner exists"),
            );
            #[cfg(any(feature = "diagnostics", test))]
            report.record_interrupt_posts(
                interrupt_posts_at_start,
                self.interrupts().mac_runtime().rx_post_count(),
            );
            return Err(MonitorRunFailure {
                error: MonitorRunError::Start(error),
                report,
            });
        }
        self.logical_parked = false;
        self.hardware
            .as_mut()
            .expect("monitor hardware owner exists")
            .resume_mac_runtime();
        let activation = {
            let platform = self
                .platform
                .as_ref()
                .expect("monitor platform owner exists");
            let interrupts = self
                .interrupts
                .as_mut()
                .expect("monitor interrupt owner exists");
            interrupts.activate_or_resume_rx_moderated(
                platform,
                ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK,
            )
        };
        if let Err(activation) = activation {
            let error = match self.stop(timer).await {
                Ok(drain) => {
                    report.interrupt_drain = drain;
                    MonitorRunError::Activate(activation)
                }
                Err(stop) => MonitorRunError::ActivateStop { activation, stop },
            };
            #[cfg(any(feature = "diagnostics", test))]
            report.record_interrupt_posts(
                interrupt_posts_at_start,
                self.interrupts().mac_runtime().rx_post_count(),
            );
            return Err(MonitorRunFailure { error, report });
        }
        report.channel_epochs = 1;

        // A descriptor may have completed before route activation. The ring,
        // not this synthetic wake, remains the source of multiplicity.
        self.interrupts().mac_runtime().notify_rx_handoff();
        let mut boundary = pin!(boundary);
        let boundary = loop {
            match select(boundary.as_mut(), self.interrupts().mac_runtime().wait_rx()).await {
                Either::First(boundary) => break boundary,
                Either::Second(()) => {
                    report.rx_service_wakes = report.rx_service_wakes.saturating_add(1);
                    let service = {
                        let receive = self.receive.as_mut().expect("monitor RX owner exists");
                        let hardware = self
                            .hardware
                            .as_mut()
                            .expect("monitor hardware owner exists");
                        let sink = self.sink.as_mut().expect("monitor sink owner exists");
                        receive.service(hardware, sink)
                    };
                    match service {
                        Ok(progress) => {
                            let service_probe_pending =
                                progress.reload_pending || progress.service_probe_pending;
                            report.record(progress);
                            if service_probe_pending {
                                // A completed descriptor may have published
                                // RX_DONE/LAST before the walker latched its
                                // old link. That ownership edge need not
                                // produce another interrupt, so keep this
                                // finite bottom half runnable cooperatively.
                                yield_now().await;
                                self.interrupts().mac_runtime().notify_rx_handoff();
                            } else {
                                // The complete durable frontier has been
                                // recycled. Restore RX delivery only after
                                // that proof, matching connected STA/AP.
                                let _ = self.interrupts().mac_runtime().unmask_rx_after_drain();
                            }
                        }
                        Err(error) => {
                            let stop = self.stop(timer).await.err();
                            #[cfg(any(feature = "diagnostics", test))]
                            report.record_interrupt_posts(
                                interrupt_posts_at_start,
                                self.interrupts().mac_runtime().rx_post_count(),
                            );
                            return Err(MonitorRunFailure {
                                error: MonitorRunError::Service { error, stop },
                                report,
                            });
                        }
                    }
                }
            }
        };

        match self.stop(timer).await {
            Ok(drain) => {
                report.interrupt_drain = drain;
                #[cfg(any(feature = "diagnostics", test))]
                report.record_interrupt_posts(
                    interrupt_posts_at_start,
                    self.interrupts().mac_runtime().rx_post_count(),
                );
                Ok((report, boundary))
            }
            Err(error) => {
                #[cfg(any(feature = "diagnostics", test))]
                report.record_interrupt_posts(
                    interrupt_posts_at_start,
                    self.interrupts().mac_runtime().rx_post_count(),
                );
                Err(MonitorRunFailure {
                    error: MonitorRunError::Stop(error),
                    report,
                })
            }
        }
    }

    /// Close the logical monitor epoch without stopping the physical walker.
    ///
    /// Once MAC activity reaches zero, recycle the finite completion frontier.
    /// The next role receives the exact live ring and installed interrupt
    /// epoch, with no interval lacking either ownership frontier.
    pub async fn stop(
        &mut self,
        timer: &impl Timer,
    ) -> Result<MacInterruptEpochDrain, MonitorStopError<R::Error>> {
        // Stop wins over injection admission. Revocation happens before IRQ
        // park/RX-walker waits, and a failed stop never republishes this
        // dwell as usable.
        self.sink
            .as_mut()
            .expect("monitor sink owner exists")
            .end_channel_epoch();
        deactivate_promiscuous_receive(
            self.hardware
                .as_mut()
                .expect("monitor hardware owner exists"),
        );
        self.hardware
            .as_mut()
            .expect("monitor hardware owner exists")
            .request_mac_runtime_stop();
        crate::time::wait_for(timer, crate::time::MAC_STOP_SETTLE).await;
        while self
            .hardware
            .as_mut()
            .expect("monitor hardware owner exists")
            .mac_runtime_active_state()
            != 0
        {
            crate::time::wait_for(timer, crate::time::MAC_STOP_POLL).await;
        }
        let interrupt_drain = if self.interrupt_active() {
            let interrupts = self
                .interrupts
                .as_mut()
                .expect("monitor interrupt owner exists");
            interrupts.park().map_err(MonitorStopError::Interrupt)?
        } else {
            MacInterruptEpochDrain::default()
        };
        if self.receive_phase() == RxFrontierPhase::Live {
            loop {
                let progress = {
                    let receive = self.receive.as_mut().expect("monitor RX owner exists");
                    let hardware = self
                        .hardware
                        .as_mut()
                        .expect("monitor hardware owner exists");
                    let sink = self.sink.as_mut().expect("monitor sink owner exists");
                    receive
                        .service(hardware, sink)
                        .map_err(|error| MonitorStopError::Receive {
                            error,
                            interrupt_drain,
                        })?
                };
                // A stopped MAC may leave a freshly republished BASE waiting
                // for the next runtime resume. That durable continuation is
                // part of `RxRingLive`; it must cross the role boundary rather
                // than making logical stop wait for inactive hardware.
                if progress.completed_descriptors == 0 {
                    break;
                }
                yield_now().await;
            }
        }
        self.logical_parked = true;
        Ok(interrupt_drain)
    }

    fn interrupts(&self) -> &InterruptEpoch<'runtime, R, M> {
        self.interrupts
            .as_ref()
            .expect("monitor interrupt owner exists")
    }

    /// Preserve every owner which may still be observed by hardware.
    ///
    /// This path is used only when the platform could not prove IRQ/DMA
    /// quiescence. Intentionally retaining this finite owner set keeps the
    /// process fail-closed until board reset, which is the only valid recovery
    /// after the ownership edge could not be closed.
    fn retain_active_owners_for_reset(&mut self) {
        if let Some(receive) = self.receive.as_mut() {
            receive.require_reset();
        }
        core::mem::forget(self.hardware.take());
        core::mem::forget(self.receive.take());
        core::mem::forget(self.sink.take());
        core::mem::forget(self.interrupts.take());
        core::mem::forget(self.platform.take());
    }

    /// Cancellation fallback for Rust's synchronous `Drop` boundary.
    ///
    /// Ordinary callers use [`Self::stop`] and return the live ring to the
    /// supervisor. `Drop` cannot complete that ownership handoff, so a live
    /// ring is retained for board reset instead of being silently poisoned.
    fn stop_on_drop(&mut self) {
        self.sink
            .as_mut()
            .expect("monitor sink owner exists")
            .end_channel_epoch();
        deactivate_promiscuous_receive(
            self.hardware
                .as_mut()
                .expect("monitor hardware owner exists"),
        );
        if self.interrupt_active() || self.receive_phase() == RxFrontierPhase::Live {
            self.retain_active_owners_for_reset();
        }
    }
}

impl<
    H,
    R,
    M: RawMutex,
    S,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> Drop for MonitorService<'_, '_, H, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>
where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    S: MonitorSink<RxPhyInfo>,
{
    fn drop(&mut self) {
        if self.hardware.is_none() {
            return;
        }
        if self.quarantined {
            self.retain_active_owners_for_reset();
            return;
        }
        if self.interrupt_active() || self.receive_phase() == RxFrontierPhase::Live {
            self.stop_on_drop();
        }
    }
}

#[cfg(test)]
mod tests;
