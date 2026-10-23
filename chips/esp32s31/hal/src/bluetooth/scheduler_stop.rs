//! Finite scheduler lifecycle sequence from the reviewed common-library stop.
//!
//! Every pending result retains the sequence. The caller owns the absolute
//! deadline and waits for each typed P3 acknowledgement. Neither an
//! idle sample nor this sequence alone releases descriptor/software ownership.

#[cfg(feature = "bluetooth-engine")]
use super::engine::scheduler::interrupt::{
    SchedulerDoorbell, SchedulerInterruptError, SchedulerTaskHalf,
};
#[cfg(test)]
use crate::settle::Unsettled;
use core::task::{Context, Poll, ready};
#[cfg(feature = "bluetooth-engine")]
use oer_esp32s31_pac::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerStopped, BluetoothTaskRegisters,
};

#[derive(Debug)]
enum Phase {
    Initial,
    InitialReply,
    MaskReply,
    Preamble,
    PreambleReply,
    Requested,
    RequestedReply,
}

/// Durable progress of the common scheduler stop transaction, including each
/// outstanding P3 request. Dropping a poll borrower never resets this state.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStop {
    phase: Phase,
}
impl BluetoothSchedulerStop {
    /// A started transaction retains custody until its stopped receipt is consumed.
    pub fn is_pending(&self) -> bool {
        !matches!(self.phase, Phase::Initial)
    }
}
impl Default for BluetoothSchedulerStop {
    fn default() -> Self {
        Self {
            phase: Phase::Initial,
        }
    }
}

pub(crate) enum Progress<Stopped> {
    Pending,
    Stopped(Stopped),
}
trait Control {
    type Error;
    type Busy;
    type Stopped;
    fn begin_busy(&mut self) -> Result<(), Self::Error>;
    fn poll_busy(&mut self, cx: &mut Context<'_>) -> Poll<Result<Self::Busy, Self::Error>>;
    fn is_busy(busy: &Self::Busy) -> bool;
    fn confirm_stopped(&mut self, busy: Self::Busy) -> Option<Self::Stopped>;
    fn begin_mask(&mut self) -> Result<(), Self::Error>;
    fn poll_mask(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>>;
    fn disable_run(&mut self);
    fn commands_ready(&mut self) -> bool;
    fn request(&mut self);
}

fn poll<C: Control>(
    stop: &mut BluetoothSchedulerStop,
    hw: &mut C,
    cx: &mut Context<'_>,
) -> Poll<Result<Progress<C::Stopped>, C::Error>> {
    // At most three BUSY requests and one mask request per finite step.
    // Each pending response yields; no task-side register wait spins here.
    loop {
        match stop.phase {
            Phase::Initial => {
                hw.begin_busy()?;
                stop.phase = Phase::InitialReply;
            }
            Phase::InitialReply => {
                let busy = ready!(hw.poll_busy(cx))?;
                if let Some(stopped) = hw.confirm_stopped(busy) {
                    return Poll::Ready(Ok(Progress::Stopped(stopped)));
                }
                hw.begin_mask()?;
                stop.phase = Phase::MaskReply;
            }
            Phase::MaskReply => {
                ready!(hw.poll_mask(cx))?;
                hw.disable_run();
                stop.phase = Phase::Preamble;
            }
            Phase::Preamble => {
                hw.begin_busy()?;
                stop.phase = Phase::PreambleReply;
            }
            Phase::PreambleReply => {
                let busy = ready!(hw.poll_busy(cx))?;
                if C::is_busy(&busy) && !hw.commands_ready() {
                    stop.phase = Phase::Preamble;
                    return Poll::Ready(Ok(Progress::Pending));
                }
                hw.request();
                stop.phase = Phase::Requested;
            }
            Phase::Requested => {
                hw.begin_busy()?;
                stop.phase = Phase::RequestedReply;
            }
            Phase::RequestedReply => {
                let busy = ready!(hw.poll_busy(cx))?;
                if let Some(stopped) = hw.confirm_stopped(busy) {
                    return Poll::Ready(Ok(Progress::Stopped(stopped)));
                }
                stop.phase = Phase::Requested;
                return Poll::Ready(Ok(Progress::Pending));
            }
        }
    }
}

#[cfg(feature = "bluetooth-engine")]
struct Hardware<'a, D> {
    task: &'a mut BluetoothTaskRegisters,
    interrupts: &'a mut SchedulerTaskHalf,
    bell: &'a D,
}
#[cfg(feature = "bluetooth-engine")]
impl<D: SchedulerDoorbell> Control for Hardware<'_, D> {
    type Error = SchedulerInterruptError;
    type Busy = BluetoothSchedulerBusyObservation;
    type Stopped = BluetoothSchedulerStopped;
    fn begin_busy(&mut self) -> Result<(), Self::Error> {
        self.interrupts.begin_busy(self.bell)
    }
    fn poll_busy(&mut self, cx: &mut Context<'_>) -> Poll<Result<Self::Busy, Self::Error>> {
        self.interrupts.poll_busy(self.bell, cx)
    }
    fn is_busy(busy: &Self::Busy) -> bool {
        busy.is_busy()
    }
    fn confirm_stopped(&mut self, busy: Self::Busy) -> Option<Self::Stopped> {
        self.task.confirm_scheduler_stopped(busy)
    }
    fn begin_mask(&mut self) -> Result<(), Self::Error> {
        self.interrupts.begin_mask_run(self.bell)
    }
    fn poll_mask(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.interrupts.poll_mask_run(self.bell, cx)
    }
    fn disable_run(&mut self) {
        self.task.disable_scheduler_run_event();
    }
    fn commands_ready(&mut self) -> bool {
        self.task.scheduler_stop_commands_ready()
    }
    fn request(&mut self) {
        self.task.publish_scheduler_lifecycle_request();
    }
}

#[cfg(feature = "bluetooth-engine")]
pub(crate) fn poll_hardware<D: SchedulerDoorbell>(
    task: &mut BluetoothTaskRegisters,
    interrupts: &mut SchedulerTaskHalf,
    stop: &mut BluetoothSchedulerStop,
    bell: &D,
    cx: &mut Context<'_>,
) -> Poll<Result<Progress<BluetoothSchedulerStopped>, SchedulerInterruptError>> {
    poll(
        stop,
        &mut Hardware {
            task,
            interrupts,
            bell,
        },
        cx,
    )
}
#[cfg(test)]
mod tests;
