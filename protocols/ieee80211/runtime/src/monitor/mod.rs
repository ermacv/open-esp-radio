//! Bounded capture and injection handoffs for monitor-mode applications.
//!
//! Capture frames retain independent caller-owned storage. Injection requests
//! retain their payload and completion capacity for one exact monitor dwell.
//! Both hand frames over lock-free SPSC queues; nothing waits for a lock.
//! The frames a monitor keeps and the channels it visits are
//! `oer-ieee80211-upper-mac`'s `monitor` policy.

mod capture;
pub mod injection;

pub use capture::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};
