//! The telemetry module: the program-counter profile. No current image
//! produces it; the shared scenario `[profile]` header still names it, and
//!  decides both. The event trace went with the images that served it in
//!  (tag `legacy-radio-2026-10-16`). The Wi-Fi observer properties have
//! had no producer since the Wi-Fi stack went in  (tag
//! `legacy-wifi-2026-10-16`);  deleted their definitions.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod profile;
pub use profile::*;

crate::messages! {
    /// The program-counter profile: a sampling interrupt on each hart.
    property PcProfile = "telemetry/pc-profile";
    endpoint ControlProfile = "telemetry/profile/control" => crate::telemetry::ProfileState;
    topic ProfileState = "telemetry/profile/state";
    endpoint GetProfileSamples = "telemetry/profile/samples/get" => crate::telemetry::ProfileSamples;
    topic ProfileSamples = "telemetry/profile/samples";
}

/// Arm, disarm or query the program-counter profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ControlProfile(pub crate::telemetry::ProfileControl);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileState(pub crate::telemetry::ProfileStatus);

/// Read retained profile samples of one hart from `first`; only a closed
/// window has pages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetProfileSamples {
    pub hart: u8,
    pub first: u32,
}

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileSamples(pub crate::telemetry::ProfileSamplesPage);
