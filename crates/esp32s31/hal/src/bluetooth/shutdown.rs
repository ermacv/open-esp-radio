//! Final Bluetooth reset and timer reunion before the clocks are released.
//!
//! Caller admission retains the idle task, released output and drained
//! source-127 owner, and Bluetooth has left the shared BTBB baseband. Reset
//! uses the same reviewed S31 `modem_clock_module_mac_reset(PERIPH_BT_MODULE)`
//! domain transaction as boot and `btdm_lp_shutdown`; no synthetic
//! runtime-counter stop register is used.

use oer_esp32s31_pac::{BluetoothInterruptSetup, BluetoothModemLpTimerRegisters};

use super::{ClockedOwner, Partition, TaskOwner};
use crate::shared_radio::{RadioClient, SharedRadioLease};

/// Proof that the Bluetooth Controller domains were reset.
///
/// Only the verified shutdown issues it. After the reset no Controller
/// hardware follows a pointer into controller memory, so the memory owners
/// of the finished epoch may return their allocations to the initial image.
/// It cannot be copied or constructed elsewhere:
///
/// ```compile_fail
/// let _forged = oer_esp32s31_hal::bluetooth::BluetoothControllerReset { _private: () };
/// ```
#[must_use = "the reset proof returns the finished epoch's controller memory"]
pub struct BluetoothControllerReset {
    _private: (),
}

impl BluetoothControllerReset {
    /// Mint a proof for a host model of controller memory.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

/// Controller shutdown rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothShutdownError {
    /// A controller-time request still belongs to the task-side worker.
    ControllerTimePending,
    /// The task owner still holds the modem LP-timer partition, so the
    /// supplied drained timer cannot be the Controller's.
    TimerOwnerPresent,
    /// Bluetooth still holds the shared BTBB baseband.
    BtbbHeld,
    /// A controller domain reset did not read back released.
    ControllerReset,
}

/// Failed shutdown keeps every hardware partition private.
#[must_use = "failed Bluetooth shutdown retains the complete partition"]
pub struct BluetoothShutdownFailure {
    error: BluetoothShutdownError,
    _task: TaskOwner,
    _output: BluetoothInterruptSetup,
    _timer: BluetoothModemLpTimerRegisters,
}

impl BluetoothShutdownFailure {
    pub const fn error(&self) -> BluetoothShutdownError {
        self.error
    }
}

impl core::fmt::Debug for BluetoothShutdownFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothShutdownFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

trait Control {
    fn time_pending(&self) -> bool;
    fn timer_separated(&self) -> bool;
    fn btbb_held(&self) -> bool;
    fn disable_compare(&mut self);
    fn reset_controller(&mut self);
    fn reset_released(&self) -> bool;
}

fn prepare(control: &mut impl Control) -> Result<(), BluetoothShutdownError> {
    if control.time_pending() {
        return Err(BluetoothShutdownError::ControllerTimePending);
    }
    if !control.timer_separated() {
        return Err(BluetoothShutdownError::TimerOwnerPresent);
    }
    if control.btbb_held() {
        return Err(BluetoothShutdownError::BtbbHeld);
    }
    control.disable_compare();
    control.reset_controller();
    if !control.reset_released() {
        return Err(BluetoothShutdownError::ControllerReset);
    }
    Ok(())
}

struct Hardware<'a, 'radio, T> {
    task: &'a mut TaskOwner,
    lease: &'a mut SharedRadioLease<'radio, T>,
    timer: &'a mut BluetoothModemLpTimerRegisters,
}

impl<T> Control for Hardware<'_, '_, T> {
    fn time_pending(&self) -> bool {
        self.task.time_latch.in_flight()
    }
    fn timer_separated(&self) -> bool {
        self.task.modem_lp_timer.is_none()
    }
    fn btbb_held(&self) -> bool {
        self.lease.holds_btbb(RadioClient::Bluetooth)
    }
    fn disable_compare(&mut self) {
        // The outer retired Controller owns inactive CPU routes and a drained
        // software queue. Compare is disabled before the timer domain reset;
        // physical counter shutdown follows from reset and clock release.
        self.timer.disable_compare();
    }
    fn reset_controller(&mut self) {
        self.task
            .registers
            .reset_controller_domains(self.lease.registers_mut());
    }
    fn reset_released(&self) -> bool {
        self.task
            .registers
            .controller_resets_released(self.lease.registers())
    }
}

/// Reset the Controller, reunite the drained timer and return to the clocked
/// owner with the reset proof.
///
/// All memory stays retained until this operation completes. Failure has no
/// hardware escape.
pub(super) fn shut_down<T>(
    mut task: TaskOwner,
    lease: &mut SharedRadioLease<'_, T>,
    output: BluetoothInterruptSetup,
    mut timer: BluetoothModemLpTimerRegisters,
) -> Result<(ClockedOwner, BluetoothControllerReset), BluetoothShutdownFailure> {
    if let Err(error) = prepare(&mut Hardware {
        task: &mut task,
        lease,
        timer: &mut timer,
    }) {
        return Err(BluetoothShutdownFailure {
            error,
            _task: task,
            _output: output,
            _timer: timer,
        });
    }
    let TaskOwner {
        registers,
        modem_lp_timer: _,
        time_latch: _,
        reunitable: _,
    } = task;
    Ok((
        ClockedOwner {
            partition: Partition {
                task: registers,
                modem_lp_timer: timer,
                interrupts: output,
            },
        },
        BluetoothControllerReset { _private: () },
    ))
}

#[cfg(test)]
mod tests;
