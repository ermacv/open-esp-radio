//! Semantic HCI boundary for legacy passive scanning.
//!
//! Standard `bt-hci` parameter types are decoded into owned Controller intent.
//! This module does not retain configuration, start a Link Layer window,
//! publish advertising reports, or claim radio progress.

use bt_hci::{
    FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{LeSetScanEnable, LeSetScanEnableParams, LeSetScanParams, LeSetScanParamsParams},
    },
    event::{EventKind, le::LeAdvertisingReport, le::LeEventParams},
    param::{
        AddrKind, BdAddr, Error as HciError, LeAdvEventKind, LeScanKind, ScanningFilterPolicy,
        Status,
    },
};

use crate::{HciCommandPacket, HciControllerResponse};

/// Complete Command Complete event size for this command family.
pub const LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;
/// Largest complete LE Advertising Report event emitted by this profile.
pub const LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY: usize = 45;

const LEGACY_SCAN_MIN_UNITS: u16 = 0x0004;
const LEGACY_SCAN_MAX_UNITS: u16 = 0x4000;
const LEGACY_ADVERTISING_REPORT_DATA_CAPACITY: usize = 31;

/// Closed identity of the standard legacy scanning commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningCommandKind {
    /// LE Set Scan Parameters.
    SetParameters,
    /// LE Set Scan Enable.
    SetEnable,
}

impl LeLegacyScanningCommandKind {
    /// Classify an opcode without decoding its parameters.
    pub const fn from_opcode(opcode: Opcode) -> Option<Self> {
        let raw = opcode.to_raw();
        if raw == LeSetScanParams::OPCODE.to_raw() {
            Some(Self::SetParameters)
        } else if raw == LeSetScanEnable::OPCODE.to_raw() {
            Some(Self::SetEnable)
        } else {
            None
        }
    }

    /// Exact standard HCI opcode.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::SetParameters => LeSetScanParams::OPCODE,
            Self::SetEnable => LeSetScanEnable::OPCODE,
        }
    }
}

/// Validated LE 1M scan type and timing supplied by the Host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanParameters {
    active: bool,
    accept_list_only: bool,
    interval_units_625_us: u16,
    window_units_625_us: u16,
}

impl LeLegacyScanParameters {
    /// Whether the scanner sends scan requests.
    pub const fn is_active(self) -> bool {
        self.active
    }

    /// Whether only filter accept list devices are received (filter policy 1).
    pub const fn is_accept_list_only(self) -> bool {
        self.accept_list_only
    }

    /// Start-to-start scan interval in 0.625 ms units.
    pub const fn interval_units_625_us(self) -> u16 {
        self.interval_units_625_us
    }

    /// Receive-window duration in 0.625 ms units.
    pub const fn window_units_625_us(self) -> u16 {
        self.window_units_625_us
    }
}

/// Host-selected report duplicate policy for one enabled scan session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningDuplicatePolicy {
    /// Publish every accepted report.
    ReportAll,
    /// Suppress exact duplicates within the enabled scan session.
    FilterDuplicates,
}

/// One decoded Set Scan Enable command awaiting lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningEnableCommand {
    enable: bool,
    duplicate_policy: LeLegacyScanningDuplicatePolicy,
}

impl LeLegacyScanningEnableCommand {
    /// Whether the Host requested entry into the enabled lifecycle.
    pub const fn enable(self) -> bool {
        self.enable
    }

    /// Duplicate policy selected for this Enable transaction.
    pub const fn duplicate_policy(self) -> LeLegacyScanningDuplicatePolicy {
        self.duplicate_policy
    }
}

/// One fully decoded standard command awaiting Controller policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningCommand {
    /// Replace passive scan parameters while the scanner is disabled.
    SetParameters(LeLegacyScanParameters),
    /// Start or stop passive scanning.
    SetEnable(LeLegacyScanningEnableCommand),
}

/// Configuration-only command which can complete without starting hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningConfigurationCommand {
    parameters: LeLegacyScanParameters,
}

impl LeLegacyScanningConfigurationCommand {
    /// Extract Set Parameters from the full scanning command family.
    pub fn from_command(command: LeLegacyScanningCommand) -> Result<Self, LeLegacyScanningCommand> {
        match command {
            LeLegacyScanningCommand::SetParameters(parameters) => Ok(Self { parameters }),
            command => Err(command),
        }
    }

    /// Validated parameter value retained by this command.
    pub const fn parameters(self) -> LeLegacyScanParameters {
        self.parameters
    }
}

/// Immutable Host configuration snapshot retained from Enable until radio start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningEnableRequest {
    parameters: LeLegacyScanParameters,
    duplicate_policy: LeLegacyScanningDuplicatePolicy,
}

impl LeLegacyScanningEnableRequest {
    /// Exact accepted passive scan timing.
    pub const fn parameters(self) -> LeLegacyScanParameters {
        self.parameters
    }

    /// Duplicate policy selected by the accepted Enable command.
    pub const fn duplicate_policy(self) -> LeLegacyScanningDuplicatePolicy {
        self.duplicate_policy
    }
}

/// Enable found no Set Scan Parameters since Reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningParametersMissing;

/// Reset-scoped software configuration for the passive scanner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LeLegacyScanningConfiguration {
    parameters: Option<LeLegacyScanParameters>,
}

impl LeLegacyScanningConfiguration {
    /// No parameters set.
    pub const fn new() -> Self {
        Self { parameters: None }
    }

    /// Apply Set Scan Parameters.
    pub fn configure(&mut self, command: LeLegacyScanningConfigurationCommand) {
        self.parameters = Some(command.parameters());
    }

    /// Snapshot the configuration for an enabling Set Scan Enable.
    pub fn enable_request(
        &self,
        command: LeLegacyScanningEnableCommand,
    ) -> Result<LeLegacyScanningEnableRequest, LeLegacyScanningParametersMissing> {
        let parameters = self.parameters.ok_or(LeLegacyScanningParametersMissing)?;
        Ok(LeLegacyScanningEnableRequest {
            parameters,
            duplicate_policy: command.duplicate_policy(),
        })
    }

    /// Current parameters.
    pub const fn parameters(&self) -> Option<LeLegacyScanParameters> {
        self.parameters
    }
}

impl LeLegacyScanningCommand {
    /// Decode one command without mutating HCI, Link Layer, or hardware state.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeLegacyScanningDecodeError> {
        let Some(kind) = LeLegacyScanningCommandKind::from_opcode(command.opcode()) else {
            return Err(LeLegacyScanningDecodeError::UnsupportedOpcode {
                opcode: command.opcode(),
            });
        };

        match kind {
            LeLegacyScanningCommandKind::SetParameters => {
                let parameters = LeSetScanParamsParams::from_hci_bytes_complete(
                    command.parameters(),
                )
                .map_err(|_| LeLegacyScanningDecodeError::MalformedParameters { command: kind })?;
                Self::decode_parameters(parameters)
            }
            LeLegacyScanningCommandKind::SetEnable => {
                let parameters = LeSetScanEnableParams::from_hci_bytes_complete(
                    command.parameters(),
                )
                .map_err(|_| LeLegacyScanningDecodeError::MalformedParameters { command: kind })?;
                Ok(Self::SetEnable(LeLegacyScanningEnableCommand {
                    enable: parameters.enable,
                    duplicate_policy: if parameters.filter_duplicates {
                        LeLegacyScanningDuplicatePolicy::FilterDuplicates
                    } else {
                        LeLegacyScanningDuplicatePolicy::ReportAll
                    },
                }))
            }
        }
    }

    fn decode_parameters(
        parameters: LeSetScanParamsParams,
    ) -> Result<Self, LeLegacyScanningDecodeError> {
        let command = LeLegacyScanningCommandKind::SetParameters;
        let LeSetScanParamsParams {
            le_scan_kind,
            le_scan_interval,
            le_scan_window,
            own_addr_kind,
            scanning_filter_policy,
        } = parameters;

        let accept_list_only = if scanning_filter_policy == ScanningFilterPolicy::BasicUnfiltered {
            false
        } else if scanning_filter_policy == ScanningFilterPolicy::BasicFiltered {
            true
        } else {
            return Err(LeLegacyScanningDecodeError::UnsupportedFeature { command });
        };
        if own_addr_kind != AddrKind::PUBLIC {
            return Err(LeLegacyScanningDecodeError::UnsupportedFeature { command });
        }

        let interval_units_625_us = le_scan_interval.as_u16();
        let window_units_625_us = le_scan_window.as_u16();
        if !(LEGACY_SCAN_MIN_UNITS..=LEGACY_SCAN_MAX_UNITS).contains(&interval_units_625_us)
            || !(LEGACY_SCAN_MIN_UNITS..=LEGACY_SCAN_MAX_UNITS).contains(&window_units_625_us)
            || window_units_625_us > interval_units_625_us
        {
            return Err(LeLegacyScanningDecodeError::InvalidParameters { command });
        }

        Ok(Self::SetParameters(LeLegacyScanParameters {
            active: le_scan_kind == LeScanKind::Active,
            accept_list_only,
            interval_units_625_us,
            window_units_625_us,
        }))
    }

    /// Exact command identity retained by this semantic token.
    pub const fn kind(&self) -> LeLegacyScanningCommandKind {
        match self {
            Self::SetParameters(_) => LeLegacyScanningCommandKind::SetParameters,
            Self::SetEnable(_) => LeLegacyScanningCommandKind::SetEnable,
        }
    }
}

/// Why a packet could not become an owned semantic scanning command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyScanningDecodeError {
    /// The opcode does not belong to this command family.
    UnsupportedOpcode {
        /// Unclaimed opcode.
        opcode: Opcode,
    },
    /// `bt-hci` rejected the standard parameter encoding.
    MalformedParameters {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
    /// A standard value violated the Core scan timing constraints.
    InvalidParameters {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
    /// A valid standard command selected a role not closed by this Controller.
    UnsupportedFeature {
        /// Claimed command kind.
        command: LeLegacyScanningCommandKind,
    },
}

impl LeLegacyScanningDecodeError {
    /// Convert a rejection for a known opcode into the required completion.
    pub fn into_command_complete(self) -> Result<LeLegacyScanningCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::MalformedParameters { command } | Self::InvalidParameters { command } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedFeature { command } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeLegacyScanningCommandCompleteEvent::new(
            command.opcode(),
            status,
        ))
    }
}

/// Owned Command Complete for one legacy scanning command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyScanningCommandCompleteEvent {
    bytes: [u8; LE_LEGACY_SCANNING_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl LeLegacyScanningCommandCompleteEvent {
    /// Command Complete for `opcode` with `status` and no return parameters.
    pub fn new(opcode: Opcode, status: Status) -> Self {
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                0x0e,
                0x04,
                0x01,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
            ],
            opcode,
            status,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Exact consumed command opcode.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Result selected by the semantic/controller boundary.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for LeLegacyScanningCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Why one portable report cannot be represented by the legacy HCI event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyAdvertisingReportEventError {
    /// Legacy advertising data cannot exceed 31 octets.
    DataTooLong {
        /// Supplied advertising data length.
        length: usize,
    },
    /// The legacy report supports only public and random advertiser addresses.
    UnsupportedAddressKind(AddrKind),
}

/// One owned standard LE Advertising Report event containing exactly one report.
///
/// `bt-hci` 0.10 exposes parsing but not construction for Controller events.
/// This bounded owner therefore accepts only `bt-hci` field-domain types and
/// is regression-decoded through its standard event model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyAdvertisingReportEvent {
    bytes: [u8; LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY],
    length: u8,
}

impl LeLegacyAdvertisingReportEvent {
    /// Build one complete LE Meta event without an H4 packet indicator.
    pub fn new(
        event_kind: LeAdvEventKind,
        address_kind: AddrKind,
        address: BdAddr,
        data: &[u8],
        rssi_dbm: i8,
    ) -> Result<Self, LeLegacyAdvertisingReportEventError> {
        if data.len() > LEGACY_ADVERTISING_REPORT_DATA_CAPACITY {
            return Err(LeLegacyAdvertisingReportEventError::DataTooLong { length: data.len() });
        }
        if address_kind != AddrKind::PUBLIC && address_kind != AddrKind::RANDOM {
            return Err(LeLegacyAdvertisingReportEventError::UnsupportedAddressKind(
                address_kind,
            ));
        }

        let length = 14 + data.len();
        let parameter_length = length - 2;
        let mut bytes = [0; LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY];
        bytes[0] = EventKind::Le.0;
        bytes[1] = parameter_length as u8;
        bytes[2] = LeAdvertisingReport::SUBEVENT_CODE;
        bytes[3] = 1;
        bytes[4] = event_kind as u8;
        bytes[5] = address_kind.as_raw();
        bytes[6..12].copy_from_slice(address.raw());
        bytes[12] = data.len() as u8;
        bytes[13..13 + data.len()].copy_from_slice(data);
        bytes[13 + data.len()] = rssi_dbm.to_le_bytes()[0];
        Ok(Self {
            bytes,
            length: length as u8,
        })
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

impl HciControllerResponse for LeLegacyAdvertisingReportEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(test)]
mod tests;
