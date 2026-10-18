//! Event-driven controller-time latch and scheduler-epoch projection.
//!
//! Every in-flight observation is a single decision. `Waiting` tells the
//! caller to return to its executor and arrange a later interrupt or bounded
//! timer recheck; no state here spins, allocates, stores a waker or depends on
//! an RTOS.

#![forbid(unsafe_code)]

/// Why controller-time ownership cannot cross the terminal task barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTimeRetirementError {
    /// A live caller still owns a hardware latch request.
    Requested,
    /// A cancelled caller's hardware request still needs its original drain.
    OrphanPending,
    /// An ownership mismatch is retained and cannot become graceful shutdown.
    Faulted,
}

#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::bluetooth::BluetoothControllerLatchedTime;
/// Controller-time to scheduler-time scale of one HAL configuration.
pub use oer_esp32s31_hal::bluetooth::BluetoothControllerTimeScale;

/// A physical duration cannot form a nonempty, unambiguous hardware delta.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerDurationError {
    /// Zero or a duration below the retained scale's whole-tick resolution.
    Empty,
    /// The projected tick delta exceeds the forward signed half-range.
    BeyondHalfRange,
}

/// One ordered controller-time sample from the always-awake latch path.
#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub struct ControllerTimeSample {
    latched_time: BluetoothControllerLatchedTime,
}

#[cfg(any(target_arch = "riscv32", test))]
impl ControllerTimeSample {
    const fn from_live_latch(latched_time: BluetoothControllerLatchedTime) -> Self {
        Self { latched_time }
    }

    #[cfg(test)]
    pub const fn for_validation(raw_ticks: u32) -> Self {
        Self::from_live_latch(BluetoothControllerLatchedTime::from_bits(raw_ticks))
    }

    /// Return the complete wrapping raw controller-tick image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn raw_ticks(&self) -> u32 {
        self.latched_time.bits()
    }

    /// Borrow the typed hardware sample for one private descriptor update.
    ///
    /// This does not expose an integer image or duplicate scheduler-time
    /// authority. The returned PAC value can only enter a lower typed codec.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn latched_time(&self) -> BluetoothControllerLatchedTime {
        self.latched_time
    }
}

#[cfg(any(target_arch = "riscv32", test))]
mod worker {

    use oer_esp32s31_hal::bluetooth::{
        BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
        BluetoothControllerTimeLatchStepError, ControllerHal,
    };

    use super::ControllerTimeSample;

    /// Crate-private hardware seam required by the controller-time worker.
    ///
    /// Keeping this boundary private prevents downstream code from manufacturing
    /// a supposedly live sample. Production has exactly one implementation: the
    /// finite borrow of the unique controller HAL owner.
    pub(crate) trait ControllerTimeHardware {
        /// Publish one fresh request without waiting for hardware.
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError>;

        /// Perform exactly one hardware observation and return immediately.
        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>;
    }

    impl ControllerTimeHardware for ControllerHal<'_> {
        fn begin_controller_time_latch(
            &mut self,
        ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
            ControllerHal::begin_controller_time_latch(self)
        }

        fn step_controller_time_latch(
            &mut self,
        ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError>
        {
            ControllerHal::step_controller_time_latch(self)
        }
    }

    /// Opaque identity of one logical controller-time request.
    ///
    /// A cancellation path must return the identity it received from `request`.
    /// A late identity cannot silently abandon a newer request: it faults the
    /// private worker before any additional hardware observation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[must_use = "the request identity must be completed or abandoned"]
    pub struct ControllerTimeRequest(u64);

    impl ControllerTimeRequest {
        #[cfg(test)]
        pub const fn for_validation(generation: u64) -> Self {
            Self(generation)
        }
    }

    /// Durable logical phase retained between executor events.
    #[cfg(test)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) enum ControllerTimeWorkerPhase {
        /// No logical or hardware request belongs to this worker.
        Idle,
        /// This worker published the request and may return its completed sample.
        Requested,
        /// An abandoned request must be drained without becoming a sample for a
        /// later logical caller.
        DrainingOrphan,
        /// Hardware and logical ownership disagreed; no further MMIO is allowed.
        Faulted,
    }

    /// Why a fresh logical controller-time request was not published.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ControllerTimeRequestError {
        /// Another logical request or orphan drain is active; no MMIO occurred.
        Busy,
        /// The lower owner already held a request although the durable worker was
        /// idle. The worker entered fail-stop instead of stealing that request.
        OwnershipCollision,
        /// The non-repeating request generation space was exhausted before MMIO.
        GenerationExhausted,
        /// A prior ownership mismatch put the worker into fail-stop.
        Faulted,
    }

    /// Result of exactly one controller-time recheck event.
    #[derive(Debug, Eq, PartialEq)]
    #[must_use = "the worker event outcome must drive the next controller action"]
    pub enum ControllerTimeEventStep {
        /// No transaction was active; no MMIO occurred.
        Idle,
        /// Hardware still owns the request; arrange one later recheck event.
        Waiting,
        /// The request owned by this worker produced one ordered live sample.
        Sample {
            /// Identity of the logical request which owns this result.
            request: ControllerTimeRequest,
            /// Ordered sample read by the live HAL path.
            sample: ControllerTimeSample,
        },
        /// An abandoned request was drained without relabelling its latched word
        /// as a new logical sample.
        OrphanDrained,
    }

    /// Fail-stop result of a controller-time recheck event.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ControllerTimeEventError {
        /// The caller did not present the identity owned by the active request.
        RequestMismatch,
        /// The lower sticky owner disappeared while the worker was active.
        OwnershipLost,
        /// A previous ownership mismatch already stopped this worker.
        Faulted,
    }

    /// Executor-neutral logical owner of controller-time sampling.
    ///
    /// The sole controller runner must retain this state beside its task resources,
    /// while every method receives only a short HAL borrow. Each recheck performs
    /// at most one hardware observation and contains no loop, await, waker, timer,
    /// allocator or RTOS binding.
    pub struct ControllerTimeWorker {
        state: ControllerTimeWorkerState,
        last_generation: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ControllerTimeWorkerState {
        Idle,
        Requested(ControllerTimeRequest),
        DrainingOrphan(ControllerTimeRequest),
        Faulted,
    }

    impl ControllerTimeWorker {
        /// Construct the sole worker while splitting a proven-cold task owner.
        ///
        /// This constructor is intentionally private to the crate ownership
        /// transition. Moving later lifecycle typestates carries this value; they
        /// never reconstruct it from a borrowed HAL or clear a fault by dropping
        /// the worker.
        pub(crate) const fn new_idle() -> Self {
            Self {
                state: ControllerTimeWorkerState::Idle,
                last_generation: 0,
            }
        }

        /// Current durable logical phase.
        #[cfg(test)]
        pub(crate) const fn phase(&self) -> ControllerTimeWorkerPhase {
            match self.state {
                ControllerTimeWorkerState::Idle => ControllerTimeWorkerPhase::Idle,
                ControllerTimeWorkerState::Requested(_) => ControllerTimeWorkerPhase::Requested,
                ControllerTimeWorkerState::DrainingOrphan(_) => {
                    ControllerTimeWorkerPhase::DrainingOrphan
                }
                ControllerTimeWorkerState::Faulted => ControllerTimeWorkerPhase::Faulted,
            }
        }

        /// Whether an external durable wake or bounded timer must recheck hardware.
        #[cfg(test)]
        pub(crate) const fn needs_recheck(&self) -> bool {
            matches!(
                self.state,
                ControllerTimeWorkerState::Requested(_)
                    | ControllerTimeWorkerState::DrainingOrphan(_)
            )
        }

        /// Whether the complete task owner may return to the cold ownership state.
        #[cfg(test)]
        pub(crate) const fn is_reunitable(&self) -> bool {
            self.retirement_ready().is_ok()
        }

        /// Inspect durable ownership without consuming a request or accessing MMIO.
        pub(crate) const fn retirement_ready(
            &self,
        ) -> Result<(), super::ControllerTimeRetirementError> {
            use super::ControllerTimeRetirementError as Error;
            match self.state {
                ControllerTimeWorkerState::Idle => Ok(()),
                ControllerTimeWorkerState::Requested(_) => Err(Error::Requested),
                ControllerTimeWorkerState::DrainingOrphan(_) => Err(Error::OrphanPending),
                ControllerTimeWorkerState::Faulted => Err(Error::Faulted),
            }
        }

        #[cfg(test)]
        pub(crate) fn exhaust_generation_for_validation(&mut self) {
            self.last_generation = u64::MAX;
        }

        /// Publish one fresh logical sample request.
        ///
        /// Non-idle phases return without MMIO. A lower-layer collision proves
        /// desynchronized ownership and permanently faults this worker.
        pub(crate) fn request(
            &mut self,
            hardware: &mut impl ControllerTimeHardware,
        ) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
            match self.state {
                ControllerTimeWorkerState::Idle => {}
                ControllerTimeWorkerState::Requested(_)
                | ControllerTimeWorkerState::DrainingOrphan(_) => {
                    return Err(ControllerTimeRequestError::Busy);
                }
                ControllerTimeWorkerState::Faulted => {
                    return Err(ControllerTimeRequestError::Faulted);
                }
            }

            let Some(generation) = self.last_generation.checked_add(1) else {
                self.state = ControllerTimeWorkerState::Faulted;
                return Err(ControllerTimeRequestError::GenerationExhausted);
            };
            let request = ControllerTimeRequest(generation);

            match hardware.begin_controller_time_latch() {
                Ok(()) => {
                    self.last_generation = generation;
                    self.state = ControllerTimeWorkerState::Requested(request);
                    Ok(request)
                }
                Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    Err(ControllerTimeRequestError::OwnershipCollision)
                }
            }
        }

        /// Abandon the current logical caller without clearing hardware ownership.
        ///
        /// A later event must still drain the request, but its latched value is no
        /// longer allowed to satisfy any logical sample request.
        pub(crate) fn cancel_owned(
            &mut self,
            request: ControllerTimeRequest,
        ) -> Result<(), ControllerTimeEventError> {
            match self.state {
                ControllerTimeWorkerState::Requested(owned) if owned == request => {
                    self.state = ControllerTimeWorkerState::DrainingOrphan(request);
                    Ok(())
                }
                ControllerTimeWorkerState::Faulted => Err(ControllerTimeEventError::Faulted),
                ControllerTimeWorkerState::Idle
                | ControllerTimeWorkerState::Requested(_)
                | ControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    Err(ControllerTimeEventError::RequestMismatch)
                }
            }
        }

        /// Recheck exactly the request whose affine owner is presented.
        pub(crate) fn recheck_owned(
            &mut self,
            request: ControllerTimeRequest,
            hardware: &mut impl ControllerTimeHardware,
        ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
            match self.state {
                ControllerTimeWorkerState::Requested(owned) if owned == request => {}
                ControllerTimeWorkerState::Faulted => {
                    return Err(ControllerTimeEventError::Faulted);
                }
                ControllerTimeWorkerState::Idle
                | ControllerTimeWorkerState::Requested(_)
                | ControllerTimeWorkerState::DrainingOrphan(_) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    return Err(ControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(ControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(latched_time)) => {
                    self.state = ControllerTimeWorkerState::Idle;
                    Ok(ControllerTimeEventStep::Sample {
                        request,
                        sample: ControllerTimeSample::from_live_latch(latched_time),
                    })
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    Err(ControllerTimeEventError::OwnershipLost)
                }
            }
        }

        /// Drain one abandoned hardware request without creating a sample.
        pub(crate) fn drain_orphan(
            &mut self,
            hardware: &mut impl ControllerTimeHardware,
        ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
            match self.state {
                ControllerTimeWorkerState::Idle => {
                    return Ok(ControllerTimeEventStep::Idle);
                }
                ControllerTimeWorkerState::DrainingOrphan(_) => {}
                ControllerTimeWorkerState::Faulted => {
                    return Err(ControllerTimeEventError::Faulted);
                }
                ControllerTimeWorkerState::Requested(_) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    return Err(ControllerTimeEventError::RequestMismatch);
                }
            }

            match hardware.step_controller_time_latch() {
                Ok(BluetoothControllerTimeLatchStep::Waiting) => {
                    Ok(ControllerTimeEventStep::Waiting)
                }
                Ok(BluetoothControllerTimeLatchStep::Ready(_)) => {
                    self.state = ControllerTimeWorkerState::Idle;
                    Ok(ControllerTimeEventStep::OrphanDrained)
                }
                Err(BluetoothControllerTimeLatchStepError::NotInFlight) => {
                    self.state = ControllerTimeWorkerState::Faulted;
                    Err(ControllerTimeEventError::OwnershipLost)
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) use worker::ControllerTimeHardware;
#[cfg(any(target_arch = "riscv32", test))]
pub use worker::ControllerTimeRequest;
#[cfg(test)]
pub(crate) use worker::ControllerTimeWorkerPhase;
#[cfg(any(target_arch = "riscv32", test))]
pub use worker::{
    ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequestError,
    ControllerTimeWorker,
};

#[cfg(any(target_arch = "riscv32", test))]
pub trait ControllerTimePendingOwner {
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError>;

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError>;

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError>;
}

#[cfg(any(target_arch = "riscv32", test))]
impl<T> ControllerTimePendingOwner for &mut T
where
    T: ControllerTimePendingOwner + ?Sized,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        T::recheck_owned_controller_time(self, request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        T::cancel_owned_controller_time(self, request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        T::drain_orphan_controller_time(self)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub enum ControllerTimePendingOwnerStep {
    Waiting,
    Ready(ControllerTimeSample),
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerTimePendingOrphanStep {
    Idle,
    Waiting,
    Drained,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub struct ControllerTimePendingCore<O>
where
    O: ControllerTimePendingOwner,
{
    owner: Option<O>,
    request: Option<ControllerTimeRequest>,
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub enum ControllerTimePendingCoreStep<O>
where
    O: ControllerTimePendingOwner,
{
    Waiting(ControllerTimePendingCore<O>),
    Ready {
        owner: O,
        sample: ControllerTimeSample,
    },
}

#[cfg(any(target_arch = "riscv32", test))]
#[derive(Debug)]
pub struct ControllerTimePendingCoreFailure<O>
where
    O: ControllerTimePendingOwner,
{
    owner: O,
    error: ControllerTimeEventError,
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> ControllerTimePendingCoreFailure<O>
where
    O: ControllerTimePendingOwner,
{
    pub fn into_parts(self) -> (O, ControllerTimeEventError) {
        (self.owner, self.error)
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> ControllerTimePendingCore<O>
where
    O: ControllerTimePendingOwner,
{
    pub const fn new(owner: O, request: ControllerTimeRequest) -> Self {
        Self {
            owner: Some(owner),
            request: Some(request),
        }
    }

    pub fn recheck(
        mut self,
    ) -> Result<ControllerTimePendingCoreStep<O>, ControllerTimePendingCoreFailure<O>> {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.recheck_owned_controller_time(request) {
            Ok(ControllerTimePendingOwnerStep::Waiting) => Ok(
                ControllerTimePendingCoreStep::Waiting(Self::new(owner, request)),
            ),
            Ok(ControllerTimePendingOwnerStep::Ready(sample)) => {
                Ok(ControllerTimePendingCoreStep::Ready { owner, sample })
            }
            Err(error) => Err(ControllerTimePendingCoreFailure { owner, error }),
        }
    }

    pub fn cancel(mut self) -> Result<O, ControllerTimePendingCoreFailure<O>> {
        let mut owner = self
            .owner
            .take()
            .expect("private pending-time core retains the exact owner");
        let request = self
            .request
            .take()
            .expect("private pending-time core retains the exact request");

        match owner.cancel_owned_controller_time(request) {
            Ok(()) => Ok(owner),
            Err(error) => Err(ControllerTimePendingCoreFailure { owner, error }),
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
impl<O> Drop for ControllerTimePendingCore<O>
where
    O: ControllerTimePendingOwner,
{
    fn drop(&mut self) {
        if let (Some(mut owner), Some(request)) = (self.owner.take(), self.request.take()) {
            let _result = owner.cancel_owned_controller_time(request);
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
pub fn drain_controller_time_orphan(
    owner: &mut impl ControllerTimePendingOwner,
) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
    owner.drain_orphan_controller_time()
}

/// Raw-tick anchor paired with the BLE scheduler's microsecond epoch.
///
/// The projection exactly retains the current `r_sched_timer_convertTimeToUs`
/// branch geometry, including rounding earlier fractional microseconds down.
/// Re-anchoring retains raw ticks not consumed by the microsecond projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(target_arch = "riscv32", test))]
pub struct ControllerSchedulerEpoch {
    raw_tick_anchor: u32,
    micros_anchor: u32,
    scale: BluetoothControllerTimeScale,
}

#[cfg(any(target_arch = "riscv32", test))]
impl ControllerSchedulerEpoch {
    /// Establish the source-owned epoch from its first live raw-tick update.
    ///
    /// Scheduler initialization starts both reference images at zero. The raw
    /// anchor excludes the conversion remainder so both anchors denote the same
    /// whole microsecond. The constructor only borrows the sample so the caller
    /// can consume that same affine value into [`ControllerSchedulerNow`].
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn from_first_live_update(
        sample: &ControllerTimeSample,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        let projection = scale.project_raw_ticks(sample.raw_ticks());
        Self {
            raw_tick_anchor: sample
                .raw_ticks()
                .wrapping_sub(projection.remainder_ticks as u32),
            micros_anchor: projection.whole_micros,
            scale,
        }
    }

    /// Bind arbitrary validation anchors without publishing that authority in
    /// production code.
    #[cfg(test)]
    pub const fn new(
        sample: ControllerTimeSample,
        micros_anchor: u32,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            raw_tick_anchor: sample.raw_ticks(),
            micros_anchor,
            scale,
        }
    }

    /// Project a later or earlier wrapping raw sample into the scheduler epoch.
    #[cfg(test)]
    pub(crate) const fn project(self, sample: ControllerTimeSample) -> u32 {
        self.project_without_reanchor(&sample)
    }

    /// Project one completed live sample while retaining the exact epoch.
    ///
    /// The post-enable timing observation is a separate scheduler-time read in
    /// the reviewed standalone flow. It must use the retained epoch, but unlike
    /// a task-run current-time update it does not advance either epoch anchor.
    pub const fn project_without_reanchor(self, sample: &ControllerTimeSample) -> u32 {
        self.project_raw_ticks(sample.raw_ticks())
    }

    /// Project the raw timestamp captured beside one completed LE RX packet.
    ///
    /// Packet capture is not a fresh controller-time sample and therefore
    /// cannot re-anchor the scheduler epoch. The opaque memory-layer value is
    /// consumed only by this conversion before PHY calibration.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn project_le_packet_capture(
        self,
        captured: oer_esp32s31_bluetooth_memory::LePacketCapturedTime,
    ) -> u32 {
        self.project_raw_ticks(captured.wrapping_controller_ticks())
    }

    /// Project the hardware capture retained by one completed connection event.
    ///
    /// Like an RX-packet capture, this is not a fresh time sample and cannot
    /// advance the persistent scheduler epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn project_peripheral_connection_capture(
        self,
        captured: oer_esp32s31_bluetooth_memory::PeripheralConnectionCapturedAnchorTime,
    ) -> u32 {
        self.project_raw_ticks(captured.wrapping_controller_ticks())
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn project_peripheral_event_start(
        self,
        window: crate::scheduler::SchedulerRawWindow,
    ) -> u32 {
        self.project_raw_ticks(window.start())
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn project_peripheral_event_end(
        self,
        window: crate::scheduler::SchedulerRawWindow,
    ) -> u32 {
        self.project_raw_ticks(window.end())
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn project_peripheral_receive_time(
        self,
        time: oer_esp32s31_bluetooth_memory::PeripheralConnectionReceiveTime,
    ) -> u32 {
        self.project_raw_ticks(time.wrapping_controller_ticks())
    }

    /// Project a raw controller-tick value that hardware captured, such as a
    /// packet or anchor capture. A capture is not a fresh time sample and
    /// cannot re-anchor the epoch.
    pub const fn project_capture(self, raw_ticks: u32) -> u32 {
        self.project_raw_ticks(raw_ticks)
    }

    /// Advance the raw anchor while preserving this sample's scheduler image.
    ///
    /// The Controller does this after every live task-run reference update.
    /// The raw anchor excludes any unconsumed fractional microsecond. Updating
    /// on successive odd raw samples must not discard half a microsecond on
    /// each call. The aligned anchor also preserves inverse wrap selection.
    pub const fn reanchor(self, sample: &ControllerTimeSample) -> Self {
        let raw_ticks = sample.raw_ticks();
        let projection = self
            .scale
            .project_raw_ticks(raw_ticks.wrapping_sub(self.raw_tick_anchor));
        Self {
            raw_tick_anchor: raw_ticks.wrapping_sub(projection.remainder_ticks as u32),
            micros_anchor: self.project_raw_ticks(raw_ticks),
            scale: self.scale,
        }
    }

    const fn project_raw_ticks(self, raw_ticks: u32) -> u32 {
        let delta = raw_ticks.wrapping_sub(self.raw_tick_anchor);
        if delta as i32 >= 0 {
            self.micros_anchor
                .wrapping_add(self.scale.micros_from_raw_ticks(delta))
        } else {
            let projection = self.scale.project_raw_ticks(delta.wrapping_neg());
            self.micros_anchor
                .wrapping_sub(projection.whole_micros)
                .wrapping_sub((projection.remainder_ticks != 0) as u32)
        }
    }

    /// Project one scheduler microsecond position back into raw controller ticks.
    ///
    /// This retains the complete `r_sched_timer_convertTimeToTicks` branch
    /// geometry. Discarded low scheduler bits are truncated toward the epoch
    /// anchor on both sides, matching the reviewed inverse helper.
    pub const fn raw_ticks_for_micros(self, micros: u32) -> u32 {
        let delta = micros.wrapping_sub(self.micros_anchor);
        if delta as i32 >= 0 {
            self.raw_tick_anchor
                .wrapping_add(self.scale.raw_ticks_from_micros(delta).whole_ticks)
        } else {
            self.raw_tick_anchor.wrapping_sub(
                self.scale
                    .raw_ticks_from_micros(delta.wrapping_neg())
                    .whole_ticks,
            )
        }
    }

    /// Convert a duration without applying either scheduler epoch anchor.
    ///
    /// Descriptor intervals are deltas, not absolute scheduler positions.
    /// Keeping this operation on the retained epoch prevents callers from
    /// reconstructing a duration by subtracting two independently truncated
    /// absolute projections.
    pub fn raw_duration_ticks(
        self,
        duration: oer_time::RadioDuration<oer_bluetooth_radio::LeRadio>,
    ) -> Result<u32, ControllerDurationError> {
        let projection = self
            .scale
            .checked_raw_ticks_from_micros(duration.as_micros())
            .ok_or(ControllerDurationError::BeyondHalfRange)?;
        // Retain the reviewed inverse helper's truncation of fractional
        // microseconds. The resulting whole-tick duration must still be nonzero.
        if projection.whole_ticks == 0 {
            return Err(ControllerDurationError::Empty);
        }
        if projection.whole_ticks > i32::MAX as u32 {
            return Err(ControllerDurationError::BeyondHalfRange);
        }
        Ok(projection.whole_ticks)
    }
}

/// One exact live sample bound to the retained Controller scheduler epoch.
///
/// This aggregate is deliberately affine: it cannot duplicate or detach the
/// sample from the epoch used to project it. Hardware ownership is not implied,
/// and the projected image is not RF-ready authority.
#[must_use = "the epoch-bound live scheduler sample must be consumed by scheduling"]
#[cfg(any(target_arch = "riscv32", test))]
pub struct ControllerSchedulerNow {
    epoch: ControllerSchedulerEpoch,
    sample: ControllerTimeSample,
}

#[cfg(any(target_arch = "riscv32", test))]
impl ControllerSchedulerNow {
    /// Bind one exact sample to an already retained source-owned epoch.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn from_retained_epoch(
        epoch: ControllerSchedulerEpoch,
        sample: ControllerTimeSample,
    ) -> Self {
        Self { epoch, sample }
    }

    /// Retained epoch used for this exact projection.
    pub const fn epoch(&self) -> ControllerSchedulerEpoch {
        self.epoch
    }

    /// Exact raw sample retained by this projection.
    #[cfg(test)]
    pub(crate) const fn sample(&self) -> &ControllerTimeSample {
        &self.sample
    }

    /// Wrapping microsecond image projected from the retained sample and epoch.
    pub const fn micros(&self) -> u32 {
        self.epoch.project_raw_ticks(self.sample.raw_ticks())
    }
}

#[cfg(test)]
mod tests;
