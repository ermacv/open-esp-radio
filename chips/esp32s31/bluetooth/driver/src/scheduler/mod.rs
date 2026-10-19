//! Scheduler configuration, time policy, the event executor and its hardware
//! execution.
//!
//! Portable policies remain available on hosts; hardware execution retains
//! its target-or-test availability.

pub(crate) mod config;
#[cfg(any(target_arch = "riscv32", test))]
pub mod core;
pub mod executor;
pub(crate) mod finished_lists;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod hardware;
pub mod list;
pub mod time;
pub mod timing;
pub mod window;

pub use config::SchedulerSoftwareConfig;

use oer_esp32s31_hal::bluetooth::BluetoothDiagnosticReadBudget;

/// Attempt budget of one scheduler diagnostic-pair sample.
///
/// The vendor repeats the pair until both reads agree; they disagree only
/// while the multiplexed value crosses from the MAC clock domain, and one
/// attempt takes well under a microsecond. After this many disagreeing pairs
/// the Controller stops on a scheduler fault instead of spinning, in the
/// interrupt handler as in the task.
pub const DIAGNOSTIC_READ_BUDGET: BluetoothDiagnosticReadBudget =
    BluetoothDiagnosticReadBudget::new(64).expect("a nonzero attempt budget");

/// Stable task-side access to the shared interrupt owner.
///
/// This is deliberately separate from hard-handler dispatch. Implementations
/// execute each operation synchronously while the owner stays in the same
/// stable slot, serialized with live interrupt service.
#[cfg(target_arch = "riscv32")]
pub trait SchedulerRunInterruptStorage {
    /// Exact reason the stable owner could not prepare scheduler interrupts.
    type Error: Copy + ::core::fmt::Debug;

    /// Clear stale dynamic sources and enable the scheduler-run groups.
    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<oer_esp32s31_hal::bluetooth::BluetoothSchedulerRunInterruptsPrepared, Self::Error>;

    /// Run one finite operation on the stored interrupt owner, serialized
    /// with interrupt service.
    ///
    /// When storage does not hold the owner, `value` returns unchanged.
    fn with_interrupt_registers<T, R>(
        &self,
        value: T,
        operation: impl FnOnce(&mut oer_esp32s31_hal::bluetooth::InterruptRegistersOwner, T) -> R,
    ) -> Result<R, T>;
}

#[cfg(any(target_arch = "riscv32", test))]
pub use core::{ControllerTimeAcquisitionError, SchedulerInitialized};
#[cfg(any(target_arch = "riscv32", test))]
pub use hardware::{SchedulerHardwareError, SchedulerStartError, SchedulerStopError};
pub use timing::SchedulerTimingPolicy;

pub use executor::{
    SchedulerAction, SchedulerCancelError, SchedulerCompletion, SchedulerExecutor,
    SchedulerHardwareView, SchedulerIdleInsertion, SchedulerItemAccess, SchedulerNext,
    SchedulerNotStopped, SchedulerObservation, SchedulerReleased, SchedulerStep,
    SchedulerStopRejected, SchedulerSubmitError, SchedulerTransactionActive,
    SchedulerTransactionFault, SchedulerWait,
};
pub use list::{
    SchedulerList, SchedulerListCompletionScan, SchedulerListInsertError, SchedulerListPlacement,
    SchedulerListRemoval,
};
pub use window::SchedulerRawWindow;

pub use finished_lists::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};
