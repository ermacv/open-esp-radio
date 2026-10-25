//! The one handler of the Bluetooth interrupts and its gate, P3 capture and
//! P2 classification of latched inputs.

pub(crate) mod classifier;
pub mod gate;
mod halves;
pub(crate) mod nrt;
pub(crate) mod primary;
pub(crate) mod route;
pub(crate) mod wake;

pub use classifier::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
    PrimaryControllerFault, PrimaryInterruptClassification, PrimarySchedulerTrigger,
    SchedulerReferenceAction, SchedulerReferenceGate, SchedulerWorkClassifier, SchedulerWorkerWake,
    SchedulerWorkerWakeClass,
};

pub use gate::{
    BluetoothInterruptEnds, BluetoothInterruptGate, BluetoothInterruptSignals,
    BluetoothInterruptStorage, BluetoothInterrupts, BluetoothInterruptsOf, ChipGate, DoorbellLine,
    GateError, GateOpenRefused, ModemLpTimerPort,
};
pub use halves::{RadioDoorbell, SchedulerDoorbell};
pub use nrt::NrtDefaultInterruptEpoch;

pub use primary::PrimaryInterruptCapture;

pub use route::{CpuInterruptRoutePolicy, CpuInterruptSource, InterruptHandlerResidency};

pub use wake::{
    CapturedWakeBatch, PrimaryWakeFault, SchedulerWakeBatch, SchedulerWakeCell,
    SchedulerWakePublication,
};
