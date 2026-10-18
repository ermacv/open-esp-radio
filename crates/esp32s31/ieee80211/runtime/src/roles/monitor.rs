//! Public standalone-monitor composition.

#[cfg(target_arch = "riscv32")]
mod builder;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod control;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub(crate) mod rx;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod service;

#[cfg(target_arch = "riscv32")]
pub(crate) use control::MonitorCommandReceiver;

pub use control::{
    MonitorCompletion, MonitorControlError, MonitorControlResources, MonitorController,
};

#[cfg(target_arch = "riscv32")]
pub use crate::roles::monitor::builder::{
    MonitorBuildError, MonitorBuildReport, MonitorChannelSwitchError, MonitorExecutionResources,
    MonitorRadio, MonitorRxRing, MonitorStopped, MonitorStoppedExecutionResources,
    MonitorStoppedResourceParts, MonitorStorage, MonitorTask, MonitorTaskBuildFailure,
    MonitorTaskExit, prepare_esp32s31_monitor_task,
};

pub use crate::roles::monitor::{
    rx::{MonitorConfigError, MonitorPrepareError, MonitorRxProgress},
    service::{
        ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK, MonitorRunError, MonitorRunFailure,
        MonitorRunReport, MonitorStopError, MonitorStoppedAccessError,
    },
};

pub use oer_esp32s31_ieee80211::monitor_injection::{
    MonitorInjectionAdmission, MonitorInjectionAdmissionError, MonitorInjectionUnsupported,
};
