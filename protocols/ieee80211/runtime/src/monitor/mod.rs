//! Bounded capture and injection handoffs for monitor-mode applications.
//!
//! Capture frames retain independent caller-owned storage. Injection requests
//! retain their payload and completion capacity for one exact monitor dwell.
//! [`policy`] selects the frames a monitor keeps and the channels it visits.

mod capture;
pub mod injection;
pub mod policy;

pub use capture::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};
