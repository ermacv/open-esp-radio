//! No clocks, retries or traffic: HIL admission and product-event translation.
use super::*;
use core::cell::Cell;
use oer_hil_protocol::RequestIdentity;
use oer_ieee80211_runtime::station as runtime;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct ConnectStation;
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct DisconnectStation;
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct ScanStation;
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct GetStationStatus;
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationControlError {
    QueueFull,
    Stopped,
}
impl From<runtime::ControlError> for StationControlError {
    fn from(value: runtime::ControlError) -> Self {
        match value {
            runtime::ControlError::QueueFull => Self::QueueFull,
            runtime::ControlError::Stopped => Self::Stopped,
        }
    }
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationAdmission {
    Accepted,
    Refused(StationControlError),
    InFlight,
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationCommandResult {
    Refused(StationCommandRefusal),
    InFlight,
    Completed(StationCommandOutcome),
    NotExecuted(StationControlError),
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub enum StationCommandOrigin {
    Requested,
    Policy,
    Unmatched,
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct StationCommandFinished {
    pub correlation: u64,
    pub origin: StationCommandOrigin,
    pub command: StationCommandKind,
    pub result: StationCommandResult,
}
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize, postcard_schema::Schema,
)]
pub struct StationStatus {
    pub epoch: u64,
    pub linked: bool,
    pub stopped: bool,
    pub notice: Option<StationNotice>,
}
impl From<runtime::Status> for StationStatus {
    fn from(value: runtime::Status) -> Self {
        Self {
            epoch: value.link.epoch,
            linked: value.link.state == oer_network::LinkState::Up,
            stopped: value.stopped,
            notice: value.notice.map(Into::into),
        }
    }
}
/// The same product command contract for portable Control and chip facade adapters.
pub trait StationControl {
    fn try_command(
        &mut self,
        command: StationCommandKind,
        correlation: u64,
    ) -> Result<(), StationControlError>;
    fn status(&self) -> StationStatus;
}
impl<K, const N: usize> StationControl for runtime::Control<'_, K, N> {
    fn try_command(
        &mut self,
        command: StationCommandKind,
        correlation: u64,
    ) -> Result<(), StationControlError> {
        let kind = match command {
            StationCommandKind::Connect => runtime::contract::Command::Connect,
            StationCommandKind::Disconnect => runtime::contract::Command::Disconnect,
            StationCommandKind::BackgroundScan => runtime::contract::Command::BackgroundScan,
        };
        runtime::Control::try_command(
            self,
            runtime::ServiceCommand::Station(runtime::Request::new(correlation, kind)),
        )
        .map_err(|r| r.reason.into())
    }
    fn status(&self) -> StationStatus {
        runtime::Control::status(self).into()
    }
}
/// One retained request identity per product command kind, until refusal/terminal.
/// It is local to the executor polling Control and its callback, and cannot cross harts.
pub struct PendingCommands {
    connect: Cell<Option<RequestIdentity>>,
    disconnect: Cell<Option<RequestIdentity>>,
    scan: Cell<Option<RequestIdentity>>,
}
impl Default for PendingCommands {
    fn default() -> Self {
        Self::new()
    }
}
impl PendingCommands {
    pub const fn new() -> Self {
        Self {
            connect: Cell::new(None),
            disconnect: Cell::new(None),
            scan: Cell::new(None),
        }
    }
    fn slot(&self, command: StationCommandKind) -> &Cell<Option<RequestIdentity>> {
        match command {
            StationCommandKind::Connect => &self.connect,
            StationCommandKind::Disconnect => &self.disconnect,
            StationCommandKind::BackgroundScan => &self.scan,
        }
    }
    pub fn admit(
        &self,
        control: &mut impl StationControl,
        request: RequestIdentity,
        command: StationCommandKind,
    ) -> StationAdmission {
        let slot = self.slot(command);
        if slot.get().is_some() {
            return StationAdmission::InFlight;
        }
        slot.set(Some(request));
        match control.try_command(command, u64::from(request.request_id) | (1u64 << 32)) {
            Ok(()) => StationAdmission::Accepted,
            Err(reason) => {
                slot.set(None);
                StationAdmission::Refused(reason)
            }
        }
    }
    pub fn finish(
        &self,
        request: runtime::Request,
        result: StationCommandResult,
    ) -> Option<(RequestIdentity, StationCommandFinished)> {
        let command = request.command().into();
        let slot = self.slot(command);
        let identity = slot.get()?;
        if (u64::from(identity.request_id) | (1u64 << 32)) != request.correlation() {
            return None;
        }
        slot.set(None);
        Some((
            identity,
            StationCommandFinished {
                correlation: request.correlation(),
                origin: StationCommandOrigin::Requested,
                command,
                result,
            },
        ))
    }
    /// Correlate a terminal or retain its explicit policy/unmatched origin.
    pub fn finished(
        &self,
        request: runtime::Request,
        result: StationCommandResult,
        fallback: RequestIdentity,
    ) -> (RequestIdentity, StationCommandFinished) {
        self.finish(request, result).unwrap_or((
            fallback,
            StationCommandFinished {
                correlation: request.correlation(),
                origin: if request.correlation() < (1u64 << 32) {
                    StationCommandOrigin::Policy
                } else {
                    StationCommandOrigin::Unmatched
                },
                command: request.command().into(),
                result,
            },
        ))
    }
    pub fn finish_stopped(
        &self,
        status: StationStatus,
    ) -> [Option<(RequestIdentity, StationCommandFinished)>; 3] {
        if !status.stopped {
            return [None; 3];
        }
        let finish = |slot: &Cell<Option<RequestIdentity>>, command| {
            slot.take().map(|identity| {
                (
                    identity,
                    StationCommandFinished {
                        correlation: u64::from(identity.request_id) | (1u64 << 32),
                        origin: StationCommandOrigin::Requested,
                        command,
                        result: StationCommandResult::NotExecuted(StationControlError::Stopped),
                    },
                )
            })
        };
        [
            finish(&self.connect, StationCommandKind::Connect),
            finish(&self.disconnect, StationCommandKind::Disconnect),
            finish(&self.scan, StationCommandKind::BackgroundScan),
        ]
    }
}
