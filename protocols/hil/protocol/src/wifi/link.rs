use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

use super::NetworkCredentials;

#[derive(Debug, Serialize, Deserialize, Schema)]
pub struct StationStart {
    pub credentials: NetworkCredentials,
    pub power_save: StationPowerSave,
}

/// W4 keeps the station awake; no modem power-save mode is advertised.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationPowerSave {
    #[default]
    Off,
}

/// A station attempt within the envelope's boot. The owner advances `epoch`
/// before a new attempt so delayed events cannot authorize another attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StationLink {
    pub epoch: u32,
    pub state: StationLinkState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationLinkState {
    Associated,
    /// RSN keys have been installed and the controlled port is open.
    Authorized,
    Disconnected {
        reason: StationDisconnectReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StationDisconnectReason {
    LocalRequest,
    Peer { reason_code: u16 },
    BeaconTimeout,
    AuthenticationFailed,
    AssociationFailed,
    HandshakeFailed,
    RadioFailure,
}
