//! The scheduler's driver side: its core, which joins the hardware-list
//! protocol (`oer_esp32s31_bluetooth_memory::scheduler`) with controller time,
//! and the finished-list worker the interrupt wakes.

#[cfg(any(target_arch = "riscv32", test))]
pub mod core;
pub(crate) mod finished_lists;

#[cfg(any(target_arch = "riscv32", test))]
pub use core::{ControllerTimeAcquisitionError, SchedulerInitialized};

pub use finished_lists::{
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerHardwareListIndex,
    SchedulerFinishedListCaptureError, SchedulerFinishedListWorker,
    SchedulerFinishedListWorkerStep,
};
