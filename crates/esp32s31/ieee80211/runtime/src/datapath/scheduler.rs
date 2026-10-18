use super::*;
use oer_ieee80211_runtime::await_stack_boundary;

/// Control steps the scheduler runs again without awaiting before it yields.
///
/// A step that answers `More` is run again at once, so a step that never
/// consumes its input would otherwise hold the executor forever and starve
/// every task beside the radio loop.
pub(crate) const CONTROL_PROGRESS_BUDGET: u32 = 32;

/// Counts control steps rerun without an await and yields the executor once
/// per [`CONTROL_PROGRESS_BUDGET`].
#[derive(Default)]
pub(crate) struct ControlProgressBudget {
    unyielded: u32,
}

impl ControlProgressBudget {
    pub(crate) async fn rerun(&mut self) {
        self.unyielded += 1;
        if self.unyielded >= CONTROL_PROGRESS_BUDGET {
            self.unyielded = 0;
            yield_now().await;
        }
    }
}

use crate::diagnostics::core0_rx_performance::{
    CORE0_PERFORMANCE, Core0PerformanceSample, Core0TxPhase,
};

#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::{
    CORE0_RX_CYCLES, Core0ControlOutcome, Core0RxSchedulerCycleProfile, Core0RxSchedulerPath,
    cycle_count,
};

impl<'irq, M: RawMutex, N, B, R, K> DatapathRunner<'irq, M, N, B, R, K>
where
    N: DatapathNetwork,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
    R: DatapathNetworkRxSet,
    K: Timer,
{
    /// Record which RX wake the loop retained when a role exits; a role exit
    /// such as beacon loss can be caused by RX starvation.
    #[cfg(feature = "diagnostics")]
    #[cold]
    #[inline(never)]
    fn log_role_exit_rx_state(&self) {
        log::info!(
            "open-radio: DATAPATH role exit rx_progress={:?} rx_signaled={} rx_capacity={} \
             moderated={} recycled_probe={:?} deficit={} rx_work={} network_tx_queue={} \
             prepared_tx={}",
            self.rx_progress,
            self.irq.rx_signaled(),
            self.irq.rx_capacity_signaled(),
            self.irq.is_rx_moderation_active(),
            self.recycled_rx_probe_deadline()
                .map(|deadline| deadline.as_micros()),
            self.fairness.deficit(),
            self.services.has_rx_work(),
            self.network_tx_queue_len(),
            self.services.has_prepared_tx(),
        );
    }

    /// Drive a finite control-only exchange while network admission stays shut.
    /// RX, IRQ and terminal TX deadlines use the ordinary owner path. The
    /// caller must retain this future through completion once TX has started;
    /// an error retains the live transaction in this runner.
    pub async fn run_control_exchange<F>(
        &mut self,
        mut step: F,
    ) -> Result<Option<B::Exit>, B::Error>
    where
        F: FnMut(&mut B) -> Result<DatapathControlProgress<B::Exit>, B::Error>,
    {
        let mut budget = ControlProgressBudget::default();
        loop {
            if self.active_tx_interface.is_some() {
                self.drain_active_tx().await?;
            }
            self.discard_stale_tx_wakes();
            match step(&mut self.services)? {
                DatapathControlProgress::TxPending => {
                    self.begin_active_tx(
                        self.reported_active_tx_interface(),
                        DatapathTxOrigin::Control,
                    );
                    self.drive_active_tx(false).await?;
                }
                DatapathControlProgress::More => budget.rerun().await,
                DatapathControlProgress::Idle => return Ok(None),
                DatapathControlProgress::Exit(exit) => return Ok(Some(exit)),
            }
        }
    }

    /// Run until role policy exits or `control` requests the sticky stop.
    /// Active TX drains through its ordinary IRQ/deadline path before the
    /// stop is acted on.
    pub async fn run_controlled<C: RawMutex>(
        &mut self,
        control: &execution::Control<C>,
    ) -> Result<DatapathRunnerExit<B::Exit>, B::Error> {
        self.run_until(control.wait_stop()).await
    }

    /// Run the production radio event loop until role policy reaches its
    /// terminal edge.
    pub async fn run(&mut self) -> Result<DatapathRunnerExit<B::Exit>, B::Error> {
        self.run_until(pending()).await
    }

    /// Run the production radio event loop until a role exit or caller stop.
    ///
    /// RX is the first future in both selects. Embassy's ordered `select`
    /// therefore preserves the recovered `wDev_ProcessFiq` priority when RX
    /// and TX become ready together. A pinned network lease stays live until
    /// `service_tx` proves that hardware ownership has ended; dropping the
    /// lease then returns that slot to `embassy-net`.
    ///
    /// `stop` is observed only at transaction boundaries. If it becomes ready
    /// during TX, the normal IRQ/deadline path first releases hardware; the
    /// next idle boundary returns [`DatapathRunnerExit::Stopped`]. This makes
    /// cancellation bounded without inventing an unsafe descriptor abort.
    pub async fn run_until<S>(&mut self, stop: S) -> Result<DatapathRunnerExit<B::Exit>, B::Error>
    where
        S: Future<Output = ()>,
    {
        await_stack_boundary!(self.run_until_boundary(stop))
    }

    async fn run_until_boundary<S>(
        &mut self,
        stop: S,
    ) -> Result<DatapathRunnerExit<B::Exit>, B::Error>
    where
        S: Future<Output = ()>,
    {
        let mut stop = core::pin::pin!(stop);
        let mut stopping = false;
        let mut budget = ControlProgressBudget::default();
        #[cfg(feature = "diagnostics")]
        let mut stop_iterations = 0_u8;
        loop {
            #[cfg(feature = "task-poll-telemetry")]
            let mut core0_scheduler_cycles = Core0RxSchedulerCycleProfile::begin();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::SchedulerLoopResumed,
                self.clock.now().as_micros(),
            );
            // Poll the caller edge before servicing control. `ready(())`
            // makes this a non-blocking ordered probe, with stop winning an
            // exact tie before another transaction can begin.
            if !stopping && matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                stopping = true;
                #[cfg(feature = "diagnostics")]
                log::info!(
                    "open-radio: DATAPATH stop observed active_tx={} prepared_tx={}",
                    self.active_tx_interface.is_some(),
                    self.prepared_tx_interface.is_some(),
                );
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.stop_completed();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::StopPollCompleted,
                self.clock.now().as_micros(),
            );
            if stopping {
                #[cfg(feature = "diagnostics")]
                {
                    if stop_iterations < 16 {
                        log::info!(
                            "open-radio: DATAPATH boundary turn={} active_tx={} prepared_tx={} control_before_stop={}",
                            stop_iterations,
                            self.active_tx_interface.is_some(),
                            self.prepared_tx_interface.is_some(),
                            self.services.control_required_before_stop(),
                        );
                    }
                    stop_iterations = stop_iterations.saturating_add(1);
                }
                // An error returned from RX-during-TX retains the exact live
                // transaction. Resume it to its terminal IRQ/deadline edge
                // before asking either paired role to acquire physical TX
                // for shutdown control.
                if self.active_tx_interface.is_some() {
                    self.drain_active_tx().await?;
                    continue;
                }
                self.cancel_prepared_network_tx()?;
                self.prepared_tx_interface = None;
                if self.services.control_required_before_stop() {
                    match self
                        .services
                        .service_control(DatapathControlContext::STOPPING)
                        .await?
                    {
                        DatapathControlProgress::More => {
                            budget.rerun().await;
                            continue;
                        }
                        DatapathControlProgress::TxPending => {
                            self.begin_active_tx(
                                self.reported_active_tx_interface(),
                                DatapathTxOrigin::Control,
                            );
                            self.drive_active_tx(false).await?;
                            continue;
                        }
                        DatapathControlProgress::Exit(exit) => {
                            self.set_scope_link_state(oer_network_interface::LinkState::Down);
                            return Ok(DatapathRunnerExit::Role(exit));
                        }
                        DatapathControlProgress::Idle => {}
                    }
                }
                match self.services.service_stop()? {
                    DatapathStopProgress::More => {
                        budget.rerun().await;
                        continue;
                    }
                    DatapathStopProgress::TxPending => {
                        self.begin_active_tx(
                            self.reported_active_tx_interface(),
                            DatapathTxOrigin::Control,
                        );
                        self.drive_active_tx(false).await?;
                        continue;
                    }
                    DatapathStopProgress::Stopped => {
                        self.set_scope_link_state(oer_network_interface::LinkState::Down);
                        return Ok(DatapathRunnerExit::Stopped);
                    }
                }
            }
            // No TX owner is live at this boundary. Drain stale transaction
            // wakes before a control or network publication can create a new
            // generation.
            self.discard_stale_tx_wakes();
            // The idle wait is edge/level notification only. The movable
            // Core0 owner services policy here, after returning to its unique
            // mutable scheduling boundary. Saturated traffic uses the same
            // finite service at the explicit completed-BA boundary below.
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.discard_wakes_completed();
            let network_tx_pending =
                self.services.has_prepared_tx() || self.network_tx_queue_len() != 0;
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.first_network_queue_completed();
            let control_ready = self.control_ready_latched
                || self.services.control_ready(self.clock.now())
                || (network_tx_pending && self.services.control_required_before_network_tx());
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.control_ready_completed();
            #[cfg(any(feature = "diagnostics", test))]
            self.services.mark_prepared_tx_scheduler_phase(
                PreparedTxSchedulerPhase::ControlReadinessChecked {
                    ready: control_ready,
                },
                self.clock.now().as_micros(),
            );
            if control_ready {
                self.control_ready_latched = false;
                let control_context = DatapathControlContext {
                    network_tx_pending,
                    stop_pending: false,
                };
                #[cfg(feature = "task-poll-telemetry")]
                let core0_control_started = cycle_count();
                let control_progress = self.services.service_control(control_context).await?;
                #[cfg(feature = "task-poll-telemetry")]
                CORE0_RX_CYCLES.record_control(
                    cycle_count().wrapping_sub(core0_control_started),
                    match &control_progress {
                        DatapathControlProgress::Idle => Core0ControlOutcome::Idle,
                        DatapathControlProgress::More => Core0ControlOutcome::More,
                        DatapathControlProgress::TxPending => Core0ControlOutcome::TxPending,
                        DatapathControlProgress::Exit(_) => Core0ControlOutcome::Exit,
                    },
                );
                match control_progress {
                    DatapathControlProgress::More => {
                        self.control_ready_latched = true;
                        budget.rerun().await;
                        continue;
                    }
                    DatapathControlProgress::TxPending => {
                        self.begin_active_tx(
                            self.reported_active_tx_interface(),
                            DatapathTxOrigin::Control,
                        );
                        self.drive_active_tx(true).await?;
                        continue;
                    }
                    DatapathControlProgress::Exit(exit) => {
                        #[cfg(feature = "diagnostics")]
                        self.log_role_exit_rx_state();
                        self.cancel_prepared_network_tx()?;
                        self.prepared_tx_interface = None;
                        self.set_scope_link_state(oer_network_interface::LinkState::Down);
                        return Ok(DatapathRunnerExit::Role(exit));
                    }
                    DatapathControlProgress::Idle => {}
                }
            }
            // The active transaction can finish with a complete standby
            // aggregate already owned by software. Once stop, control and RX
            // priority have been established above, publish it directly.
            // Re-running physical queue discovery, burst classification and
            // collection-deadline calculation here creates an avoidable air
            // gap on every saturated BA transaction.
            // A station in power save holds its network frames: they stay
            // queued, and once control has offered them the idle wait below
            // ignores their readiness until a control input admits them.
            let network_admitted = self.services.control_admits_network_tx();
            if network_admitted
                && let Some((interface, admitted)) = self.prepared_network_tx_candidate()?
            {
                self.start_prepared_network_tx(interface, admitted).await?;
                continue;
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.prepared_completed();
            let network_tx_pending = network_admitted
                && (self.services.has_prepared_tx() || self.network_tx_queue_len() != 0);
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.network_pending_completed();
            #[cfg(feature = "task-poll-telemetry")]
            core0_scheduler_cycles.tx_checks_completed();

            // A staged protocol owner or reorder timeout has no new MAC IRQ
            // edge, but it is still charged to the same frame deficit as an
            // IRQ-originated RX turn. Under saturation it must yield one TX
            // transaction at the configured quantum; otherwise this early
            // software-work branch bypasses the fairness gate below.
            if self.services.has_rx_work() && !(network_tx_pending && self.network_turn_owed()) {
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Software);
                self.service_rx().await?;
                continue;
            }

            // A delayed recycled-only continuation retains the masked RX
            // epoch without keeping this task continuously runnable. Once its
            // deadline is due it has the same priority as the software repost
            // it replaces, including the existing RX/TX fairness gate.
            if self.recycled_rx_probe_due(self.clock.now())
                && !(network_tx_pending && self.network_turn_owed())
            {
                self.clear_recycled_rx_probe_deadline();
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Software);
                self.service_rx().await?;
                continue;
            }

            // Consume a coalesced RX frontier before admitting a fresh
            // network transaction, matching the recovered FIQ priority. If a
            // previous finite RX pass reported a continuation while network
            // TX is queued, admit exactly one network transaction first; the
            // reposted RX signal remains pending for the following boundary.
            let rx_can_run = matches!(
                self.rx_progress,
                DatapathRxProgress::Drained
                    | DatapathRxProgress::ProbePending
                    | DatapathRxProgress::ProtocolBlockedByTx
                    | DatapathRxProgress::RecycledAppendPending
                    | DatapathRxProgress::BudgetExhausted
                    | DatapathRxProgress::UpperLayerBlockedButDroppable
            );
            if rx_can_run
                && self.irq.rx_signaled()
                && !(network_tx_pending && self.network_turn_owed())
            {
                self.irq.wait_rx().await;
                #[cfg(feature = "task-poll-telemetry")]
                core0_scheduler_cycles.finish(Core0RxSchedulerPath::Irq);
                self.service_rx().await?;
                continue;
            }

            if network_tx_pending {
                {
                    // A batch is formed from what is queued now; accumulation
                    // happens while the preceding PPDU is on air. A partial
                    // prepared successor absorbs the newly queued owners once.
                    if self.services.has_prepared_tx() {
                        let interface = self.retained_prepared_tx_interface();
                        let network_tx = self.network.tx_consumer(interface);
                        self.services.advance_prepared_tx(&network_tx)?;
                        if self.services.can_prepare_tx()
                            && !(self.services.pulls_tx_queues()
                                && network_tx.destination_queues().is_some())
                            && self.services.prepared_tx_start_ready()
                            && let Some(frame) = self.network.try_receive_tx(interface)
                        {
                            assert_eq!(self.tx_interface_for(&frame), interface);
                            let tx_phase_started = Core0PerformanceSample::read();
                            self.services.prepare_tx(frame, &network_tx).await?;
                            CORE0_PERFORMANCE.record_tx_phase(
                                Core0TxPhase::Prepare,
                                tx_phase_started,
                                Core0PerformanceSample::read(),
                            );
                        }
                        drop(network_tx);
                        if self.services.prepared_tx_start_ready() {
                            let admitted = self.services.prepared_tx_frame_count().max(1);
                            self.start_prepared_network_tx(interface, admitted).await?;
                            continue;
                        }
                    }

                    let Some(frame) = self.try_receive_network_tx() else {
                        continue;
                    };
                    let interface = self.tx_interface_for(&frame);
                    let network_tx = self.network.tx_consumer(interface);
                    let tx_phase_started = Core0PerformanceSample::read();
                    let progress = self.services.start_tx(frame, &network_tx).await?;
                    drop(network_tx);
                    CORE0_PERFORMANCE.record_tx_phase(
                        Core0TxPhase::Start,
                        tx_phase_started,
                        Core0PerformanceSample::read(),
                    );
                    let admitted = self.services.last_started_tx_frame_count().max(1);
                    CORE0_PERFORMANCE.record_tx_initial_network_frames(admitted);
                    self.account_tx_frames(admitted);
                    self.account_pair_tx_frames(interface, admitted);
                    if progress == WifiTxProgress::Pending {
                        self.begin_active_tx(interface, DatapathTxOrigin::Network);
                        // The loop head is the single scheduling boundary:
                        // its ordered stop, control and RX checks precede
                        // the complete successor this transaction left.
                        self.drive_active_tx(true).await?;
                    }
                    continue;
                }
            }

            let irq = self.irq;
            let rx_progress = self.rx_progress;
            let recycled_rx_probe_deadline = self.recycled_rx_probe_deadline();
            let clock = &self.clock;
            let network_rx = &mut self.network_rx;
            let wait_rx = async move {
                match rx_progress {
                    DatapathRxProgress::StageCapacityBlocked => irq.wait_rx_capacity().await,
                    // Network ownership can remain unavailable for multiple
                    // milliseconds. RX DMA is an independent lower frontier:
                    // wake on either capacity or a new completion so a role
                    // can keep copying and republishing descriptors into its
                    // bounded staging reserve.
                    DatapathRxProgress::NetworkBackpressured => {
                        let _ = select(
                            core::future::poll_fn(|context| network_rx.poll_any_ready(context)),
                            irq.wait_rx(),
                        )
                        .await;
                    }
                    DatapathRxProgress::RecycledAppendPending
                        if recycled_rx_probe_deadline.is_some() =>
                    {
                        if let Some(deadline) = recycled_rx_probe_deadline {
                            clock.wait_until(deadline).await;
                        } else {
                            irq.wait_rx().await;
                        }
                    }
                    DatapathRxProgress::ProbePending
                    | DatapathRxProgress::RecycledAppendPending => irq.wait_rx().await,
                    DatapathRxProgress::Drained
                    | DatapathRxProgress::ProtocolBlockedByTx
                    | DatapathRxProgress::BudgetExhausted
                    | DatapathRxProgress::UpperLayerBlockedButDroppable => irq.wait_rx().await,
                }
            };
            let network = &self.network;
            let interfaces = self.interfaces;
            let prepared_tx_interface = self.prepared_tx_interface;
            // Held network frames stay queued, but a frame that control has
            // not yet offered must still wake the loop: a station in power
            // save wakes for queued traffic only once control offers it.
            let network_wakes =
                network_admitted || self.services.control_required_before_network_tx();
            match select(
                stop.as_mut(),
                select3(wait_rx, self.services.wait_control_ready(), async {
                    if !network_wakes {
                        core::future::pending::<()>().await;
                    }
                    if let Some(interface) = prepared_tx_interface {
                        network.wait_tx_ready(interface).await;
                    } else {
                        match interfaces {
                            DatapathInterfaceScope::Single(interface) => {
                                network.wait_tx_ready(interface).await
                            }
                            DatapathInterfaceScope::Pair { .. } => {
                                network.wait_tx_publication().await
                            }
                        }
                    }
                }),
            )
            .await
            {
                Either::First(()) => {
                    stopping = true;
                }
                Either::Second(Either3::First(())) => {
                    #[cfg(feature = "task-poll-telemetry")]
                    core0_scheduler_cycles.finish(Core0RxSchedulerPath::Select);
                    self.service_rx().await?
                }
                Either::Second(Either3::Second(())) => {
                    self.control_ready_latched = true;
                }
                Either::Second(Either3::Third(())) => {}
            }
        }
    }
}

#[cfg(all(test, feature = "owned-network"))]
mod tests;
