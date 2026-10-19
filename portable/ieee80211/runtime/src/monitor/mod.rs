//! Bounded capture and injection handoffs for monitor-mode applications.
//!
//! Capture frames retain independent caller-owned storage. Injection requests
//! retain their payload and completion capacity for one exact monitor dwell.

mod capture;
pub mod injection;

pub use capture::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};
