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
pub mod interrupt;
pub mod list;
pub mod time;
pub mod timing;
pub mod window;

pub use config::SchedulerSoftwareConfig;

use oer_time::settle::SettleLimit;

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
