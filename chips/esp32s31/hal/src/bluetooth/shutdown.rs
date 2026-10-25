//! Final Bluetooth reset and timer reunion before the clocks are released.
//!
//! Caller admission retains the idle task and released output, the timer's
//! handler disabled its compare and still holds the source-127 owner, and
//! Bluetooth has left the shared BTBB baseband. Reset uses the same reviewed
//! S31 `modem_clock_module_mac_reset(PERIPH_BT_MODULE)` domain transaction as
//! boot and `btdm_lp_shutdown`; no synthetic runtime-counter stop register is
//! used. The timer leaves its handler only after the reset, when no source of
//! the reset domain raises it, and the reunion takes it then.

use crate::power::PlatformClockBackend;

use crate::bluetooth::registers::{BluetoothInterruptSetup, BluetoothTaskRegisters};

use super::{
    ClockedOwner, ModemLpTimerCompareDisabled, ModemLpTimerInterruptReadyOwner, Partition,
    TaskOwner,
};
use crate::domain::{RadioClient, RadioDomain};

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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
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

/// Failed shutdown keeps every hardware partition private; the timer stays
/// with its handler.
#[must_use = "failed Bluetooth shutdown retains the complete partition"]
pub struct BluetoothShutdownFailure {
    error: BluetoothShutdownError,
    _task: TaskOwner,
    _output: BluetoothInterruptSetup,
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
    fn reset_controller(&mut self);
    fn reset_released(&self) -> bool;
}

/// Admit the reset and reset the Controller domains. The timer's handler
/// disabled its compare before (the caller's proof), as the physical
/// counter shutdown follows from the reset and the clock release.
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
    control.reset_controller();
    if !control.reset_released() {
        return Err(BluetoothShutdownError::ControllerReset);
    }
    Ok(())
}

struct Hardware<'a, B: PlatformClockBackend, T> {
    task: &'a mut TaskOwner,
    radio: &'a mut RadioDomain<B, T>,
}

impl<B: PlatformClockBackend, T> Control for Hardware<'_, B, T> {
    fn time_pending(&self) -> bool {
        self.task.time_latch.in_flight()
    }
    fn timer_separated(&self) -> bool {
        self.task.modem_lp_timer.is_none()
    }
    fn btbb_held(&self) -> bool {
        self.radio.holds_btbb(RadioClient::Bluetooth)
    }
    fn reset_controller(&mut self) {
        self.task
            .registers
            .reset_controller_domains(self.radio.registers_mut());
    }
    fn reset_released(&self) -> bool {
        self.task
            .registers
            .controller_resets_released(self.radio.registers())
    }
}

/// The reset Controller, awaiting its timer from the timer's handler.
#[must_use = "the reset Controller returns to the clocked owner with its timer"]
pub struct BluetoothControllerResetTimerPending {
    task: BluetoothTaskRegisters,
    output: BluetoothInterruptSetup,
}

impl BluetoothControllerResetTimerPending {
    /// Reunite the timer its handler gave up after the reset and return to
    /// the clocked owner with the proof of the reset. Performs no MMIO.
    pub fn reunite(
        self,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> (ClockedOwner, BluetoothControllerReset) {
        (
            ClockedOwner {
                partition: Partition {
                    task: self.task,
                    modem_lp_timer: timer.timer,
                    interrupts: self.output,
                },
            },
            BluetoothControllerReset { _private: () },
        )
    }
}

/// Reset the Controller, whose timer's handler disabled its compare, and
/// await the timer.
///
/// All memory stays retained until the reunion. Failure has no hardware
/// escape.
pub(super) fn shut_down<B: PlatformClockBackend, T>(
    mut task: TaskOwner,
    radio: &mut RadioDomain<B, T>,
    output: BluetoothInterruptSetup,
    _compare: ModemLpTimerCompareDisabled,
) -> Result<BluetoothControllerResetTimerPending, BluetoothShutdownFailure> {
    if let Err(error) = prepare(&mut Hardware {
        task: &mut task,
        radio,
    }) {
        return Err(BluetoothShutdownFailure {
            error,
            _task: task,
            _output: output,
        });
    }
    let TaskOwner {
        registers,
        modem_lp_timer: _,
        time_latch: _,
        reunitable: _,
        epoch: _,
    } = task;
    Ok(BluetoothControllerResetTimerPending {
        task: registers,
        output,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) fn failure_for_host_test(
    task: TaskOwner,
    output: BluetoothInterruptSetup,
    error: BluetoothShutdownError,
) -> BluetoothShutdownFailure {
    BluetoothShutdownFailure {
        error,
        _task: task,
        _output: output,
    }
}

#[cfg(test)]
pub(super) fn reset_for_host_test(
    task: TaskOwner,
    output: BluetoothInterruptSetup,
) -> BluetoothControllerResetTimerPending {
    BluetoothControllerResetTimerPending {
        task: task.registers,
        output,
    }
}
