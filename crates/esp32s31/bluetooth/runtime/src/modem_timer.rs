//! Driver of the source-127 modem low-power timer task.
//!
//! The interrupt service moves the timer owner into software-pending state
//! when register work needs the task. This driver acquires it, advances the
//! software queue and returns the rearmed owner to interrupt storage. The
//! radio role schedules no timer entries, so an expiration has no consumer
//! and is discarded.

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use oer_esp32s31_bluetooth::modem_timer::{
    ControllerModemTimerBegin, ControllerModemTimerReadinessClass, ControllerModemTimerRearm,
    ControllerModemTimerStep, ControllerModemTimerTask, ModemLpTimerSoftwareOwnerStorage,
};
use oer_time::Timer;

use crate::{HARDWARE_RECHECK, runtime::wait_for};

/// Why the timer task stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemTimerFault<TakeError, RestoreError> {
    /// Interrupt storage refused the software-pending owner.
    Take(TakeError),
    /// Interrupt storage refused the rearmed owner, which the task retains.
    Restore(RestoreError),
    /// The timer cannot express a recheck deadline.
    Deadline,
}

/// Drive the source-127 task until interrupt storage refuses an exchange.
///
/// `wake` is signalled by the platform's source-127 service whenever it opens
/// a new worker wake epoch.
pub async fn run_modem_timer<
    M: RawMutex,
    S: ModemLpTimerSoftwareOwnerStorage,
    const CAPACITY: usize,
>(
    task: &mut ControllerModemTimerTask<'_, S, CAPACITY>,
    wake: &Signal<M, ()>,
    timer: &impl Timer,
) -> ModemTimerFault<S::TakeError, S::RestoreError> {
    loop {
        match task.readiness().class() {
            ControllerModemTimerReadinessClass::Interrupt => {
                if !task.readiness().is_ready() {
                    wake.wait().await;
                    continue;
                }
                if let ControllerModemTimerBegin::StorageRejected(error) = task.begin() {
                    return ModemTimerFault::Take(error);
                }
            }
            ControllerModemTimerReadinessClass::Step => {
                if let ControllerModemTimerStep::Recheck = task.step()
                    && wait_for(timer, HARDWARE_RECHECK).await.is_err()
                {
                    return ModemTimerFault::Deadline;
                }
            }
            ControllerModemTimerReadinessClass::EventCapacity => {
                let _ = task.take_expiration();
            }
            ControllerModemTimerReadinessClass::Rearm => {
                if let ControllerModemTimerRearm::StorageRejected(error) = task.rearm() {
                    return ModemTimerFault::Restore(error);
                }
            }
        }
    }
}

/// Advance the task until it holds no software work, without waiting for an
/// interrupt: a published wake is taken, a step or rearm in progress
/// completes and an expiration is discarded.
///
/// The Controller shutdown calls this after the runtime stopped and before
/// the CPU routes are removed, so the timer owner returns to stable storage.
///
/// # Errors
///
/// Stable storage refused the owner.
pub async fn settle_modem_timer<S: ModemLpTimerSoftwareOwnerStorage, const CAPACITY: usize>(
    task: &mut ControllerModemTimerTask<'_, S, CAPACITY>,
    timer: &impl Timer,
) -> Result<(), ModemTimerFault<S::TakeError, S::RestoreError>> {
    loop {
        match task.readiness().class() {
            ControllerModemTimerReadinessClass::Interrupt => {
                if !task.readiness().is_ready() {
                    return Ok(());
                }
                if let ControllerModemTimerBegin::StorageRejected(error) = task.begin() {
                    return Err(ModemTimerFault::Take(error));
                }
            }
            ControllerModemTimerReadinessClass::Step => {
                if let ControllerModemTimerStep::Recheck = task.step()
                    && wait_for(timer, HARDWARE_RECHECK).await.is_err()
                {
                    return Err(ModemTimerFault::Deadline);
                }
            }
            ControllerModemTimerReadinessClass::EventCapacity => {
                let _ = task.take_expiration();
            }
            ControllerModemTimerReadinessClass::Rearm => {
                if let ControllerModemTimerRearm::StorageRejected(error) = task.rearm() {
                    return Err(ModemTimerFault::Restore(error));
                }
            }
        }
    }
}
