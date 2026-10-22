//! CPU route policy, interrupt classification and deferred publication.

pub(crate) mod classifier;
mod halves;
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

pub use halves::{ControllerInterruptHalf, RadioDoorbell, SchedulerDoorbell};
pub use nrt::NrtDefaultInterruptEpoch;

pub use primary::{
    PrimaryInterruptStep, PrimaryNoSchedulerWork, PrimaryPublishedInterruptStep,
    PrimarySchedulerEvent,
};

#[cfg(target_arch = "riscv32")]
pub use ports::{
    InterruptOwnerRestartStorage, InterruptOwnerStorage, SharedInterruptDispatchStorage,
};
pub use route::{CpuInterruptRoutePolicy, CpuInterruptSource, InterruptHandlerResidency};

pub use wake::{SchedulerWakeBatch, SchedulerWakeCell, SchedulerWakePublication};
