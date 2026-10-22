//! The scheduler's hardware-list protocol: configuration, time policy, the
//! event executor and its hardware execution. The lists live in controller
//! SRAM and the controller walks them, so the executor's order is the proof
//! the HAL publications need.
//!
//! Portable policies remain available on hosts; hardware execution retains
//! its target-or-test availability.

pub mod config;
pub(crate) mod executor;
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod hardware;
pub mod list;
pub mod time;
pub mod timing;
pub mod window;

pub use config::SchedulerSoftwareConfig;

use crate::settle::SettleLimit;

/// The named settle of one scheduler diagnostic-pair sample.
///
/// The vendor repeats the pair until both reads agree; they disagree only
/// while the multiplexed value crosses from the MAC clock domain, and one
/// attempt takes well under a microsecond. When the pair still disagrees
/// after the longest settle a driver may busy-wait, the Controller stops on
/// a scheduler fault instead of spinning, in the interrupt handler as in the
/// task.
pub const DIAGNOSTIC_SETTLE: SettleLimit =
    SettleLimit::new(SettleLimit::MAX).expect("the longest settle is a valid limit");

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
    ) -> Result<crate::bluetooth::BluetoothSchedulerRunInterruptsPrepared, Self::Error>;

    /// Run one finite operation on the stored interrupt owner, serialized
    /// with interrupt service.
    ///
    /// When storage does not hold the owner, `value` returns unchanged.
    fn with_interrupt_registers<T, R>(
        &self,
        value: T,
        operation: impl FnOnce(&mut crate::bluetooth::InterruptRegistersOwner, T) -> R,
    ) -> Result<R, T>;
}

#[cfg(any(target_arch = "riscv32", test))]
pub use hardware::{SchedulerHardwareError, SchedulerStartError, SchedulerStopError};
pub use timing::SchedulerTimingPolicy;

pub use executor::{
    SchedulerCancelError, SchedulerCompletion, SchedulerIdleInsertion, SchedulerNext,
    SchedulerNotStopped, SchedulerObservation, SchedulerReleased, SchedulerStep,
    SchedulerStopRejected, SchedulerSubmitError, SchedulerTransactionActive,
    SchedulerTransactionFault, SchedulerWait,
};
pub use list::{
    SchedulerList, SchedulerListCompletionScan, SchedulerListInsertError, SchedulerListPlacement,
    SchedulerListRemoval,
};
pub use window::SchedulerRawWindow;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) use executor::SchedulerAction;

/// Fresh scheduler state whose hardware-written head was resolved inside HAL.
#[derive(Debug)]
pub struct SchedulerHardwareSnapshot {
    pub busy: crate::bluetooth::BluetoothSchedulerBusyObservation,
    pub head: Option<super::SchedulerItemId>,
}

pub use executor::SchedulerExecutor;
