//! Typed records a repetition keeps beside its result: the typed
//! observations its workload made, the restorations of its cleanup scope and
//! the host's USB events of its boards.

use serde::{Deserialize, Serialize};

/// The file a repetition records its workload's typed observations in.
pub const OBSERVATIONS_FILE: &str = "observations.json";

/// The schema of [`Observations`].
pub const OBSERVATIONS_SCHEMA: u16 = 1;

/// What a workload observed in one repetition, beside the measurements its
/// result carries: the typed evidence of each step (each boot, each cycle),
/// in the order the workload made it, and the claim it makes. A failed or
/// interrupted repetition keeps every observation made before the failure;
/// the result, not this document, says whether the repetition passed.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Observations {
    pub schema: u16,
    /// What a passing repetition establishes, and what it leaves unproven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<Claim>,
    pub observations: Vec<Observation>,
}

/// The result a passing workload establishes and the properties it does not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Claim {
    pub result: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_proven: Vec<String>,
}

/// One typed observation: its name (`boot-001`, `cycle-002`, `peer-delivery`)
/// and the serialized evidence type the workload recorded under it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Observation {
    pub name: String,
    pub value: serde_json::Value,
}

/// The file a repetition records its cleanup scope in.
pub const CLEANUP_FILE: &str = "cleanup.json";

/// One restoration a repetition's cleanup scope attempted. A failure is
/// recorded beside, never instead of, the workload's own outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CleanupRecord {
    pub operation: String,
    pub duration_millis: u128,
    pub failure: Option<String>,
}

/// The file a repetition records its boards' USB events in.
pub const USB_EVENTS_FILE: &str = "usb-events.json";

/// One kernel USB event of a watched device.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsbEvent {
    pub realtime_micros: u64,
    /// The kernel's USB device path, e.g. `3-8`.
    pub device: String,
    #[serde(flatten)]
    pub kind: UsbEventKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum UsbEventKind {
    Disconnected { device_number: u32 },
    Enumerated { device_number: u32, speed: String },
}

impl std::fmt::Display for UsbEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            UsbEventKind::Disconnected { device_number } => write!(
                f,
                "usb {} disconnected (device number {device_number})",
                self.device
            ),
            UsbEventKind::Enumerated {
                device_number,
                speed,
            } => write!(
                f,
                "usb {} enumerated as {speed} device number {device_number}",
                self.device
            ),
        }
    }
}
