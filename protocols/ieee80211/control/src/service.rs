//! The end of a station service that did not stop on command, and the
//! answer to role commands after it.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use oer_hil_protocol::base::RejectReason;

use super::StopStation;

/// The station service ended without a stop command. A startup refusal may
/// have no link notice; this event ends the station role of the boot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationServiceEnded {
    pub epoch: u32,
    pub end: StationServiceEnd,
}

/// How the port was left after the fault.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationServiceEnd {
    /// The port is disabled and uninstalled.
    Failed(StationServiceFault),
    /// The port poisoned during an owner transition (enable or disable);
    /// it fail-stopped.
    Poisoned(StationServiceFault),
    /// Admission ended without another disable; the port fail-stopped.
    Quarantined(StationServiceFault),
    /// The radio front end never registered the station port, so no port
    /// was installed and no service ran.
    Unavailable,
}

/// Exact, schema-identified product failure; the binding has no second error dictionary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationServiceFault {
    pub schema: oer_hil_protocol::Key,
    bytes: heapless::Vec<u8, 128>,
}
impl StationServiceFault {
    pub fn encode<T: serde::Serialize + Schema>(
        fault: &T,
    ) -> Result<Self, oer_hil_protocol::EncodeError> {
        let mut bytes = [0; 128];
        let encoded = postcard::to_slice(fault, &mut bytes)
            .map_err(|_| oer_hil_protocol::EncodeError::Serialize)?;
        Ok(Self {
            schema: const { oer_hil_protocol::Key::of::<T>("wifi/station/failure") },
            bytes: heapless::Vec::from_slice(encoded)
                .map_err(|_| oer_hil_protocol::EncodeError::Serialize)?,
        })
    }
    pub fn decode<'a, T: serde::Deserialize<'a> + Schema>(
        &'a self,
    ) -> Result<T, oer_hil_protocol::EncodeError> {
        if self.schema != const { oer_hil_protocol::Key::of::<T>("wifi/station/failure") } {
            return Err(oer_hil_protocol::EncodeError::Serialize);
        }
        postcard::from_bytes(&self.bytes).map_err(|_| oer_hil_protocol::EncodeError::Serialize)
    }
}

/// The station role as a role command finds it. It is not a wire type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationRole {
    /// No station was started.
    Idle,
    Running,
    /// A stop was accepted and has not completed.
    Stopping,
    /// The service ended: `StationServiceEnded` or `StationStopped` was
    /// published. Nothing would answer a later command.
    Ended,
}

impl StopStation {
    /// The refusal of a stop in `role`, before its admission to a full
    /// queue (`Busy`). Only a running station accepts a stop, so every
    /// stop is answered.
    pub const fn refusal(role: StationRole) -> Option<RejectReason> {
        match role {
            StationRole::Running => None,
            StationRole::Idle | StationRole::Stopping | StationRole::Ended => {
                Some(RejectReason::InvalidState)
            }
        }
    }
}
