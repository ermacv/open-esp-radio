//! Finite scheduler lifecycle sequence from the reviewed common-library stop.
//!
//! Every pending result retains the sequence. The caller owns the absolute
//! deadline and waits for each typed P3 acknowledgement. Neither an
//! idle sample nor this sequence alone releases descriptor/software ownership.

use oer_esp32s31_pac::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerStopped, BluetoothTaskRegisters,
};

#[cfg(feature = "bluetooth-engine")]
use super::engine::scheduler::interrupt::{
    SchedulerDoorbell, SchedulerInterruptError, SchedulerTaskHalf,
};
#[cfg(test)]
use crate::settle::Unsettled;

#[derive(Debug)]
enum Phase {
    Initial,
    Preamble,
    Requested,
}

/// Affine progress of the common scheduler stop transaction.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStop {
    phase: Phase,
}

impl Default for BluetoothSchedulerStop {
    fn default() -> Self {
        Self {
            phase: Phase::Initial,
        }
    }
}

/// Result of one finite stop step.
#[derive(Debug)]
#[must_use]
pub enum BluetoothSchedulerStopStep {
    Pending(BluetoothSchedulerStop),
    Stopped(BluetoothSchedulerStopped),
}

enum Progress<Stopped> {
    Pending(BluetoothSchedulerStop),
    Stopped(Stopped),
}

trait Control {
    type Error;
    type Stopped;

    async fn busy(&mut self) -> Result<bool, Self::Error>;
    /// Mask dynamic run interrupts, disable RUN, then fence.
    async fn preamble(&mut self) -> Result<(), Self::Error>;
    /// Command-zero then, only if ready, command-one status.
    fn commands_ready(&mut self) -> bool;
    /// Publish the lifecycle request, then fence.
    fn request(&mut self);
    /// Observe BUSY once; when clear, fence and return the receipt.
    async fn confirm_stopped(&mut self) -> Result<Option<Self::Stopped>, Self::Error>;
}

async fn step<C: Control>(
    mut stop: BluetoothSchedulerStop,
    hw: &mut C,
) -> Result<Progress<C::Stopped>, C::Error> {
    if matches!(stop.phase, Phase::Initial) {
        if let Some(stopped) = hw.confirm_stopped().await? {
            return Ok(Progress::Stopped(stopped));
        }
        hw.preamble().await?;
        stop.phase = Phase::Preamble;
    }
    if matches!(stop.phase, Phase::Preamble) {
        // B8f: an idle scheduler bypasses command reads; while busy both
        // positional statuses are required, in this short-circuit order.
        if hw.busy().await? && !hw.commands_ready() {
            return Ok(Progress::Pending(stop));
        }
        hw.request();
        stop.phase = Phase::Requested;
    }
    Ok(match hw.confirm_stopped().await? {
        Some(stopped) => Progress::Stopped(stopped),
        None => Progress::Pending(stop),
    })
}

#[cfg(feature = "bluetooth-engine")]
struct Hardware<'a, D> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut SchedulerTaskHalf,
    bell: &'a D,
}

#[cfg(feature = "bluetooth-engine")]
impl<D: SchedulerDoorbell> Hardware<'_, D> {
    async fn sample_busy(
        &mut self,
    ) -> Result<BluetoothSchedulerBusyObservation, SchedulerInterruptError> {
        self.interrupts.busy(self.bell).await
    }
}

#[cfg(feature = "bluetooth-engine")]
impl<D: SchedulerDoorbell> Control for Hardware<'_, D> {
    type Error = SchedulerInterruptError;
    type Stopped = BluetoothSchedulerStopped;

    async fn busy(&mut self) -> Result<bool, SchedulerInterruptError> {
        Ok(self.sample_busy().await?.is_busy())
    }
    async fn preamble(&mut self) -> Result<(), Self::Error> {
        self.interrupts.mask_run(self.bell).await?;
        self.task.disable_scheduler_run_event();
        Ok(())
    }
    fn commands_ready(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    fn request(&mut self) {
        self.task.publish_scheduler_lifecycle_request();
    }
    async fn confirm_stopped(
        &mut self,
    ) -> Result<Option<BluetoothSchedulerStopped>, SchedulerInterruptError> {
        let busy = self.sample_busy().await?;
        Ok(self.task.confirm_scheduler_stopped(busy))
    }
}

/// Advance one finite common-stop step through the unique task request endpoint.
#[cfg(feature = "bluetooth-engine")]
pub(crate) async fn step_hardware<D: SchedulerDoorbell>(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut SchedulerTaskHalf,
    stop: BluetoothSchedulerStop,
    bell: &D,
) -> Result<BluetoothSchedulerStopStep, SchedulerInterruptError> {
    Ok(
        match step(
            stop,
            &mut Hardware {
                task,
                interrupts,
                bell,
            },
        )
        .await?
        {
            Progress::Pending(stop) => BluetoothSchedulerStopStep::Pending(stop),
            Progress::Stopped(stopped) => BluetoothSchedulerStopStep::Stopped(stopped),
        },
    )
}

#[cfg(test)]
mod tests;
