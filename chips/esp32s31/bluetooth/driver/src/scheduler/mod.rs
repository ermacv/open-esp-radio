//! The scheduler's driver side: its core, which joins the hardware-list
//! protocol (`oer_esp32s31_hal::bluetooth::engine::scheduler`) with controller time,
//! and the finished-list worker the interrupt wakes.

#[cfg(any(target_arch = "riscv32", test))]
pub mod core;
pub(crate) mod finished_lists;

#[cfg(any(target_arch = "riscv32", test))]
pub use core::{
    ControllerTimeAcquisitionError, SchedulerInitializationError, SchedulerInitializationFailure,
    SchedulerInitialized,
};

pub use finished_lists::{
    BluetoothSchedulerHardwareListIndex, EngineFinishedHardwareListObserved,
    SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};

#[cfg(target_arch = "riscv32")]
pub use core::{ControllerTimeAccess, ControllerTimeReady};
