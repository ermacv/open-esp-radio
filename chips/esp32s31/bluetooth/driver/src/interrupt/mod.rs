//! CPU route policy, interrupt classification and deferred publication.

pub(crate) mod classifier;
pub(crate) mod nrt;
#[cfg(target_arch = "riscv32")]
mod ports;
pub(crate) mod primary;
pub(crate) mod route;
pub(crate) mod wake;

pub use classifier::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
    PrimaryControllerFault, PrimaryInterruptClassification, PrimarySchedulerTrigger,
    SchedulerReferenceAction, SchedulerReferenceGate, SchedulerWorkClassifier, SchedulerWorkerWake,
    SchedulerWorkerWakeClass,
};

pub use nrt::{NrtDefaultInterruptEpoch, step_nrt_default_interrupt};

pub use primary::{
    PrimaryInterruptStep, PrimaryNoSchedulerWork, PrimaryPublishedInterruptStep,
    PrimarySchedulerEvent, step_primary_interrupt,
};

#[cfg(target_arch = "riscv32")]
pub use ports::{
    InterruptOwnerRestartStorage, InterruptOwnerStorage, SharedInterruptDispatchStorage,
};
pub use route::{CpuInterruptRoutePolicy, CpuInterruptSource, InterruptHandlerResidency};

pub use wake::{SchedulerWakeBatch, SchedulerWakeCell, SchedulerWakePublication};
