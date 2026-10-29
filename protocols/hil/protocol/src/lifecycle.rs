//! Optional lifecycle commands, independent of the image's hardware.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

crate::messages! {
    endpoint RequestReset = "lifecycle/reset" => ResetAccepted;
    topic ResetAccepted = "lifecycle/reset-accepted";
}

/// Request a reset after the acknowledgement has been written.
/// The image owns the reset and its retained reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RequestReset;

/// The image will reset after writing this acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ResetAccepted;
