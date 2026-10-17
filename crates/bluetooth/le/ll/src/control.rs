//! Bounded peripheral responder for feature, version, LE Ping and Data Length
//! Update exchange.
//!
//! Input must already have passed the controller's CRC and duplicate filters.
//! This module does not own SN/NESN, encryption, ACL delivery or LL teardown.
//! Optional features are deliberately absent until their procedures are closed.

use crate::connection::{
    LeConnectionTiming, LeDataChannelMap, LePeripheralChannelMapUpdateError,
    LePeripheralConnectionUpdateError,
};
use crate::data_length::{LeDataLength, LeDataLengthProcedure, LeDataLengths};

/// Controller identity supplied by the product; independent of the radio vendor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeVersionInformation([u8; 5]);

impl LeVersionInformation {
    pub const fn new(version: u8, company_identifier: u16, subversion: u16) -> Self {
        let company = company_identifier.to_le_bytes();
        let subversion = subversion.to_le_bytes();
        Self([
            version,
            company[0],
            company[1],
            subversion[0],
            subversion[1],
        ])
    }
    pub const fn version(self) -> u8 {
        self.0[0]
    }
    pub const fn company_identifier(self) -> u16 {
        u16::from_le_bytes([self.0[1], self.0[2]])
    }
    pub const fn subversion(self) -> u16 {
        u16::from_le_bytes([self.0[3], self.0[4]])
    }
}

/// Payload of one response, excluding the hardware-owned data-channel header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeControlResponse {
    bytes: [u8; 9],
    len: u8,
}

/// Page-zero feature result produced by one Host-initiated procedure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRemoteFeaturesResult {
    Success([u8; 8]),
    Unsupported,
    Rejected { reason: u8 },
    ResponseTimeout,
    ConnectionClosed,
}

/// Admission result before HCI Command Status makes a local request eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRemoteFeaturesAdmission {
    Cached([u8; 8]),
    Admitted,
    Busy,
}

/// Version result produced by one Host-initiated procedure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRemoteVersionResult {
    Success(LeVersionInformation),
    Unsupported,
    Rejected { reason: u8 },
    ResponseTimeout,
    ConnectionClosed,
}

/// Admission result before HCI Command Status makes a local version request eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRemoteVersionAdmission {
    Cached(LeVersionInformation),
    Admitted,
    Busy,
    LocalIdentityUnavailable,
}

impl LeControlResponse {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralControlError {
    MalformedPdu,
    UnsupportedHeader,
    ResponseQueueFull,
    PeerTermination { reason: u8 },
    MandatoryProcedureUnavailable { opcode: u8 },
    ChannelMapUpdate(LePeripheralChannelMapUpdateError),
    ConnectionUpdate(LePeripheralConnectionUpdateError),
}

impl LePeripheralControlError {
    /// Link Layer termination reason for a peer-originated protocol failure.
    pub const fn termination_reason(self) -> Option<u8> {
        match self {
            Self::PeerTermination { .. } => None,
            Self::ChannelMapUpdate(LePeripheralChannelMapUpdateError::InstantPassed) => Some(0x28),
            Self::ChannelMapUpdate(LePeripheralChannelMapUpdateError::ProcedureAlreadyPending) => {
                Some(0x23)
            }
            Self::ChannelMapUpdate(
                LePeripheralChannelMapUpdateError::IncompatibleProcedurePending,
            ) => Some(0x2a),
            Self::ConnectionUpdate(LePeripheralConnectionUpdateError::InstantPassed) => Some(0x28),
            Self::ConnectionUpdate(LePeripheralConnectionUpdateError::ProcedureAlreadyPending) => {
                Some(0x23)
            }
            Self::ConnectionUpdate(
                LePeripheralConnectionUpdateError::IncompatibleProcedurePending,
            ) => Some(0x2a),
            Self::MalformedPdu => Some(0x1e),
            Self::UnsupportedHeader | Self::MandatoryProcedureUnavailable { .. } => Some(0x20),
            Self::ResponseQueueFull => Some(0x1f),
        }
    }
}

/// One accepted unencrypted LE-U payload borrowed from its completed RX batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePeripheralDataFragment<'a> {
    payload: &'a [u8],
    continuing: bool,
}

/// Validated Channel Map Update carried by `LL_CHANNEL_MAP_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeChannelMapUpdate {
    channel_map: LeDataChannelMap,
    instant: u16,
}

/// Validated Connection Update carried by `LL_CONNECTION_UPDATE_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionUpdate {
    timing: LeConnectionTiming,
    instant: u16,
}

impl LeConnectionUpdate {
    pub const fn timing(self) -> LeConnectionTiming {
        self.timing
    }

    pub const fn instant(self) -> u16 {
        self.instant
    }
}

impl LeChannelMapUpdate {
    pub const fn channel_map(self) -> LeDataChannelMap {
        self.channel_map
    }

    pub const fn instant(self) -> u16 {
        self.instant
    }
}

impl<'a> LePeripheralDataFragment<'a> {
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }

    pub const fn is_continuing(self) -> bool {
        self.continuing
    }
}

/// Semantic disposition of one accepted data-channel PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralReceive<'a> {
    Data(LePeripheralDataFragment<'a>),
    Control,
    ChannelMapUpdate(LeChannelMapUpdate),
    ConnectionUpdate(LeConnectionUpdate),
}

enum LePeripheralPdu<'a> {
    Data(LePeripheralDataFragment<'a>),
    Control(&'a [u8]),
}

fn decode_peripheral_pdu(pdu: &[u8]) -> Result<LePeripheralPdu<'_>, LePeripheralControlError> {
    use LePeripheralControlError as Error;
    if pdu.len() < 2 || pdu.len() != usize::from(pdu[1]) + 2 {
        return Err(Error::MalformedPdu);
    }
    // CTE is outside this profile. RFU bits are ignored as required by LL.
    if pdu[0] & 0x20 != 0 {
        return Err(Error::UnsupportedHeader);
    }
    match pdu[0] & 3 {
        llid @ (1 | 2) => Ok(LePeripheralPdu::Data(LePeripheralDataFragment {
            payload: &pdu[2..],
            continuing: llid == 1,
        })),
        3 if pdu.len() >= 3 => Ok(LePeripheralPdu::Control(&pdu[2..])),
        _ => Err(Error::MalformedPdu),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalTermination {
    None,
    Queued { reason: u8, host: bool },
    Transmitted { reason: u8, host: bool },
    Acknowledged { reason: u8, host: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalFeatureRequest {
    None,
    Admitted,
    Queued,
    Transmitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocalVersionRequest {
    None,
    Admitted,
    Queued(LeControlResponse),
    Transmitted,
}

/// LE feature page zero supported by this bounded Peripheral controller.
///
/// Bit 0 advertises LE Encryption; bits 3 and 4 advertise
/// Peripheral-initiated Feature Exchange and LE Ping; bit 5 advertises the
/// LE Data Packet Length Extension; bit 14 advertises Channel Selection
/// Algorithm #2.
pub const fn le_peripheral_supported_features() -> [u8; 8] {
    [1 | (1 << 3) | (1 << 4) | (1 << 5), 1 << 6, 0, 0, 0, 0, 0, 0]
}

/// Two waiting responses plus the independently retained controller TX packet.
/// Removing a response means the memory owner accepted it, not that it was ACKed.
pub struct LePeripheralControl {
    responses: [Option<LeControlResponse>; 2],
    termination: Option<LeControlResponse>,
    version_queued: bool,
    peer_version: Option<LeVersionInformation>,
    local_termination: LocalTermination,
    peer_features: Option<[u8; 8]>,
    local_feature_request: LocalFeatureRequest,
    remote_features_result: Option<LeRemoteFeaturesResult>,
    local_version: Option<LeVersionInformation>,
    local_version_request: LocalVersionRequest,
    remote_version_result: Option<LeRemoteVersionResult>,
    data_length: LeDataLengthProcedure,
    data_length_request: Option<LeControlResponse>,
}

impl LePeripheralControl {
    pub const fn new() -> Self {
        Self {
            responses: [None; 2],
            termination: None,
            version_queued: false,
            peer_version: None,
            local_termination: LocalTermination::None,
            peer_features: None,
            local_feature_request: LocalFeatureRequest::None,
            remote_features_result: None,
            local_version: None,
            local_version_request: LocalVersionRequest::None,
            remote_version_result: None,
            data_length: LeDataLengthProcedure::new(LeDataLength::MINIMUM),
            data_length_request: None,
        }
    }

    /// Send data PDUs of up to `transmit` once the peer agrees; a longer
    /// length than the minimum starts the Data Length Update procedure.
    pub fn with_data_length(mut self, transmit: LeDataLength) -> Self {
        self.data_length = LeDataLengthProcedure::new(transmit);
        self.data_length.start_if_extended();
        self.queue_data_length_request();
        self
    }

    /// The effective lengths of both directions.
    pub const fn data_lengths(&self) -> LeDataLengths {
        self.data_length.effective()
    }

    /// Request a new local transmit length, as HCI LE Set Data Length does.
    pub fn set_data_length(&mut self, transmit: LeDataLength) {
        self.data_length.set_transmit(transmit);
        self.queue_data_length_request();
    }

    /// The effective lengths after a change, once.
    pub fn take_data_length_change(&mut self) -> Option<LeDataLengths> {
        self.data_length.take_change()
    }

    fn queue_data_length_request(&mut self) {
        if self.data_length_request.is_none()
            && let Some(pdu) = self.data_length.queued_request()
        {
            self.data_length_request = Some(LeControlResponse { bytes: pdu, len: 9 });
        }
    }

    /// Bind the Controller identity used for local and peer-requested version exchange.
    pub const fn with_local_version(mut self, version: Option<LeVersionInformation>) -> Self {
        self.local_version = version;
        self
    }

    /// Whether accepting this exact PDU requires one Controller ACL queue slot.
    ///
    /// Empty data-channel PDUs acknowledge Link Layer traffic but carry no Host
    /// ACL packet. Control PDUs remain independently processable when every ACL
    /// slot is occupied.
    pub fn receive_requires_acl_slot(pdu: &[u8]) -> Result<bool, LePeripheralControlError> {
        match decode_peripheral_pdu(pdu)? {
            LePeripheralPdu::Data(fragment) => Ok(!fragment.payload().is_empty()),
            LePeripheralPdu::Control(_) => Ok(false),
        }
    }

    /// Dispatch one accepted unencrypted LE1M PDU in receive-list order.
    pub fn receive<'pdu>(
        &mut self,
        pdu: &'pdu [u8],
        local_version: Option<LeVersionInformation>,
    ) -> Result<LePeripheralReceive<'pdu>, LePeripheralControlError> {
        use LePeripheralControlError as Error;
        let payload = match decode_peripheral_pdu(pdu)? {
            LePeripheralPdu::Data(fragment) => return Ok(LePeripheralReceive::Data(fragment)),
            LePeripheralPdu::Control(payload) => payload,
        };
        let opcode = payload[0];
        let mut response = LeControlResponse {
            bytes: [0; 9],
            len: 2,
        };
        match opcode {
            0x08 => {
                if payload.len() != 9 {
                    return Err(Error::MalformedPdu);
                }
                let mut peer = [0; 8];
                peer.copy_from_slice(&payload[1..]);
                self.peer_features = Some(peer);
                let local = le_peripheral_supported_features();
                response.bytes[0] = 0x09;
                for index in 0..local.len() {
                    response.bytes[index + 1] = peer[index] & local[index];
                }
                response.len = 9;
            }
            0x0c => {
                if payload.len() != 6 {
                    return Err(Error::MalformedPdu);
                }
                self.peer_version = Some(LeVersionInformation(
                    payload[1..].try_into().expect("validated version length"),
                ));
                if matches!(self.local_version_request, LocalVersionRequest::Transmitted) {
                    self.local_version_request = LocalVersionRequest::None;
                    self.remote_version_result =
                        self.peer_version.map(LeRemoteVersionResult::Success);
                    return Ok(LePeripheralReceive::Control);
                }
                if self.version_queued {
                    return Ok(LePeripheralReceive::Control);
                }
                let version =
                    local_version.ok_or(Error::MandatoryProcedureUnavailable { opcode })?;
                response.bytes[0] = 0x0c;
                response.bytes[1..6].copy_from_slice(&version.0);
                response.len = 6;
            }
            0x02 => {
                if payload.len() != 2 {
                    return Err(Error::MalformedPdu);
                }
                return Err(Error::PeerTermination { reason: payload[1] });
            }
            0x01 => {
                if payload.len() != 8 {
                    return Err(Error::MalformedPdu);
                }
                let channel_map = LeDataChannelMap::new([
                    payload[1], payload[2], payload[3], payload[4], payload[5],
                ])
                .map_err(|_| Error::MalformedPdu)?;
                return Ok(LePeripheralReceive::ChannelMapUpdate(LeChannelMapUpdate {
                    channel_map,
                    instant: u16::from_le_bytes([payload[6], payload[7]]),
                }));
            }
            // Do not turn a response into another request/response loop.
            0x07 => {
                if payload.len() != 2 {
                    return Err(Error::MalformedPdu);
                }
                if payload[1] == 0x0e
                    && matches!(self.local_feature_request, LocalFeatureRequest::Transmitted)
                {
                    self.local_feature_request = LocalFeatureRequest::None;
                    self.remote_features_result = Some(LeRemoteFeaturesResult::Unsupported);
                } else if payload[1] == 0x0c
                    && matches!(self.local_version_request, LocalVersionRequest::Transmitted)
                {
                    self.local_version_request = LocalVersionRequest::None;
                    self.remote_version_result = Some(LeRemoteVersionResult::Unsupported);
                } else if payload[1] == 0x14 {
                    self.data_length.abandon();
                }
                return Ok(LePeripheralReceive::Control);
            }
            0x14 => {
                let pdu = self
                    .data_length
                    .receive_request(&payload[1..])
                    .map_err(|_| Error::MalformedPdu)?;
                response.bytes = pdu;
                response.len = 9;
            }
            0x15 => {
                self.data_length
                    .receive_response(&payload[1..])
                    .map_err(|_| Error::MalformedPdu)?;
                return Ok(LePeripheralReceive::Control);
            }
            0x0d => {
                if payload.len() != 2 {
                    return Err(Error::MalformedPdu);
                }
                if matches!(self.local_feature_request, LocalFeatureRequest::Transmitted) {
                    self.local_feature_request = LocalFeatureRequest::None;
                    self.remote_features_result =
                        Some(LeRemoteFeaturesResult::Rejected { reason: payload[1] });
                } else if matches!(self.local_version_request, LocalVersionRequest::Transmitted) {
                    self.local_version_request = LocalVersionRequest::None;
                    self.remote_version_result =
                        Some(LeRemoteVersionResult::Rejected { reason: payload[1] });
                }
                return Ok(LePeripheralReceive::Control);
            }
            0x11 => {
                if payload.len() != 3 {
                    return Err(Error::MalformedPdu);
                }
                if payload[1] == 0x0e
                    && matches!(self.local_feature_request, LocalFeatureRequest::Transmitted)
                {
                    self.local_feature_request = LocalFeatureRequest::None;
                    self.remote_features_result =
                        Some(LeRemoteFeaturesResult::Rejected { reason: payload[2] });
                } else if payload[1] == 0x0c
                    && matches!(self.local_version_request, LocalVersionRequest::Transmitted)
                {
                    self.local_version_request = LocalVersionRequest::None;
                    self.remote_version_result =
                        Some(LeRemoteVersionResult::Rejected { reason: payload[2] });
                } else if payload[1] == 0x14 {
                    self.data_length.abandon();
                }
                return Ok(LePeripheralReceive::Control);
            }
            0x09 => {
                if payload.len() != 9 {
                    return Err(Error::MalformedPdu);
                }
                if matches!(self.local_feature_request, LocalFeatureRequest::Transmitted) {
                    let mut peer = [0; 8];
                    peer.copy_from_slice(&payload[1..]);
                    self.peer_features = Some(peer);
                    self.local_feature_request = LocalFeatureRequest::None;
                    self.remote_features_result = Some(LeRemoteFeaturesResult::Success(peer));
                }
                return Ok(LePeripheralReceive::Control);
            }
            0x12 => {
                if payload.len() != 1 {
                    return Err(Error::MalformedPdu);
                }
                response.bytes[0] = 0x13;
                response.len = 1;
            }
            // A response never starts another Ping exchange.
            0x13 => {
                if payload.len() != 1 {
                    return Err(Error::MalformedPdu);
                }
                return Ok(LePeripheralReceive::Control);
            }
            // Never claim an unsupported mandatory connection transition ran.
            0x00 => {
                if payload.len() != 12 {
                    return Err(Error::MalformedPdu);
                }
                let timing = LeConnectionTiming::new(
                    payload[1],
                    u16::from_le_bytes([payload[2], payload[3]]),
                    u16::from_le_bytes([payload[4], payload[5]]),
                    u16::from_le_bytes([payload[6], payload[7]]),
                    u16::from_le_bytes([payload[8], payload[9]]),
                )
                .map_err(|_| Error::MalformedPdu)?;
                return Ok(LePeripheralReceive::ConnectionUpdate(LeConnectionUpdate {
                    timing,
                    instant: u16::from_le_bytes([payload[10], payload[11]]),
                }));
            }
            _ => {
                response.bytes[0] = 0x07; // LL_UNKNOWN_RSP for unsupported LLCP.
                response.bytes[1] = opcode;
            }
        }
        let slot = self
            .responses
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Error::ResponseQueueFull)?;
        *slot = Some(response);
        if opcode == 0x0c {
            self.version_queued = true;
        }
        Ok(LePeripheralReceive::Control)
    }

    pub const fn peer_version(&self) -> Option<LeVersionInformation> {
        self.peer_version
    }

    /// Whether either locally initiated control procedure owns the connection.
    pub const fn local_procedure_pending(&self) -> bool {
        self.remote_feature_request_pending()
            || !matches!(self.local_version_request, LocalVersionRequest::None)
            || self.data_length.local_request_pending()
    }

    pub const fn remote_version_request_available(&self) -> bool {
        self.peer_version.is_some() || (self.local_version.is_some() && !self.version_queued)
    }

    /// Whether one Host-initiated feature read already owns the LL procedure.
    pub const fn remote_feature_request_pending(&self) -> bool {
        !matches!(self.local_feature_request, LocalFeatureRequest::None)
    }

    /// Reserve one local procedure before its successful HCI Command Status.
    pub fn admit_remote_feature_request(&mut self) -> LeRemoteFeaturesAdmission {
        if let Some(features) = self.peer_features {
            return LeRemoteFeaturesAdmission::Cached(features);
        }
        if self.local_procedure_pending() {
            return LeRemoteFeaturesAdmission::Busy;
        }
        self.local_feature_request = LocalFeatureRequest::Admitted;
        LeRemoteFeaturesAdmission::Admitted
    }

    /// Make an admitted request eligible only after Command Status publication.
    pub fn activate_remote_feature_request(&mut self) -> Option<[u8; 8]> {
        if !matches!(self.local_feature_request, LocalFeatureRequest::Admitted) {
            return self.peer_features;
        }
        if let Some(features) = self.peer_features {
            self.local_feature_request = LocalFeatureRequest::None;
            return Some(features);
        }
        self.local_feature_request = LocalFeatureRequest::Queued;
        None
    }

    /// Whether the first local request still needs a transmit allocation.
    pub const fn local_feature_request_queued(&self) -> bool {
        matches!(self.local_feature_request, LocalFeatureRequest::Queued)
    }

    /// Whether the local request has entered the radio-owned TX graph.
    pub const fn local_feature_request_transmitted(&self) -> bool {
        matches!(self.local_feature_request, LocalFeatureRequest::Transmitted)
    }

    /// End a transmitted procedure at the standard response timeout.
    pub fn expire_remote_feature_request(&mut self) {
        if matches!(self.local_feature_request, LocalFeatureRequest::Transmitted) {
            self.local_feature_request = LocalFeatureRequest::None;
            self.remote_features_result = Some(LeRemoteFeaturesResult::ResponseTimeout);
        }
    }

    /// Complete any accepted Host request when the connection closes.
    pub fn close_remote_feature_request(&mut self) {
        if self.remote_feature_request_pending() {
            self.local_feature_request = LocalFeatureRequest::None;
            self.remote_features_result = Some(LeRemoteFeaturesResult::ConnectionClosed);
        }
    }

    /// Take one Host-visible completion exactly once.
    pub fn take_remote_features_result(&mut self) -> Option<LeRemoteFeaturesResult> {
        self.remote_features_result.take()
    }

    pub fn admit_remote_version_request(&mut self) -> LeRemoteVersionAdmission {
        if let Some(version) = self.peer_version {
            return LeRemoteVersionAdmission::Cached(version);
        }
        if self.local_version.is_none() {
            return LeRemoteVersionAdmission::LocalIdentityUnavailable;
        }
        if self.version_queued || self.local_procedure_pending() {
            return LeRemoteVersionAdmission::Busy;
        }
        self.local_version_request = LocalVersionRequest::Admitted;
        LeRemoteVersionAdmission::Admitted
    }

    pub fn activate_remote_version_request(&mut self) -> Option<LeVersionInformation> {
        if !matches!(self.local_version_request, LocalVersionRequest::Admitted) {
            return self.peer_version;
        }
        if let Some(version) = self.peer_version {
            self.local_version_request = LocalVersionRequest::None;
            return Some(version);
        }
        let version = self
            .local_version
            .expect("an admitted local version request retains its identity");
        let mut request = LeControlResponse {
            bytes: [0; 9],
            len: 6,
        };
        request.bytes[0] = 0x0c;
        request.bytes[1..6].copy_from_slice(&version.0);
        self.local_version_request = LocalVersionRequest::Queued(request);
        None
    }

    pub const fn local_version_request_queued(&self) -> bool {
        matches!(self.local_version_request, LocalVersionRequest::Queued(_))
    }

    pub const fn local_version_request_transmitted(&self) -> bool {
        matches!(self.local_version_request, LocalVersionRequest::Transmitted)
    }

    /// Whether the local `LL_LENGTH_REQ` awaits the peer's response.
    pub const fn local_data_length_request_transmitted(&self) -> bool {
        self.data_length.local_request_transmitted()
    }

    pub fn expire_local_procedure(&mut self) {
        if matches!(self.local_feature_request, LocalFeatureRequest::Transmitted) {
            self.local_feature_request = LocalFeatureRequest::None;
            self.remote_features_result = Some(LeRemoteFeaturesResult::ResponseTimeout);
        } else if matches!(self.local_version_request, LocalVersionRequest::Transmitted) {
            self.local_version_request = LocalVersionRequest::None;
            self.remote_version_result = Some(LeRemoteVersionResult::ResponseTimeout);
        } else {
            self.data_length.abandon();
        }
    }

    pub fn close_remote_version_request(&mut self) {
        if !matches!(self.local_version_request, LocalVersionRequest::None) {
            self.local_version_request = LocalVersionRequest::None;
            self.remote_version_result = Some(LeRemoteVersionResult::ConnectionClosed);
        }
    }

    pub fn take_remote_version_result(&mut self) -> Option<LeRemoteVersionResult> {
        self.remote_version_result.take()
    }

    pub const fn pending_response(&self) -> Option<&LeControlResponse> {
        match self.termination.as_ref() {
            Some(termination) => Some(termination),
            None => match self.responses[0].as_ref() {
                Some(response) => Some(response),
                None if matches!(self.local_feature_request, LocalFeatureRequest::Queued) => {
                    const FEATURES: [u8; 8] = le_peripheral_supported_features();
                    const REQUEST: LeControlResponse = LeControlResponse {
                        bytes: [
                            0x0e,
                            FEATURES[0],
                            FEATURES[1],
                            FEATURES[2],
                            FEATURES[3],
                            FEATURES[4],
                            FEATURES[5],
                            FEATURES[6],
                            FEATURES[7],
                        ],
                        len: 9,
                    };
                    Some(&REQUEST)
                }
                None => match &self.local_version_request {
                    LocalVersionRequest::Queued(request) => Some(request),
                    _ => self.data_length_request.as_ref(),
                },
            },
        }
    }

    /// Queue one local termination ahead of any response already accepted from the peer.
    ///
    /// Repeated calls for the same connection are idempotent. The caller must
    /// retain the connection until [`Self::local_termination_acknowledged`].
    pub fn request_local_termination(&mut self, reason: u8) {
        self.request_termination(reason, false);
    }

    /// Queue termination requested by the Host Disconnect command.
    pub fn request_host_termination(&mut self, reason: u8) {
        self.request_termination(reason, true);
    }

    fn request_termination(&mut self, reason: u8, host: bool) {
        if !matches!(self.local_termination, LocalTermination::None) {
            return;
        }
        let mut response = LeControlResponse {
            bytes: [0; 9],
            len: 2,
        };
        response.bytes[0] = 0x02;
        response.bytes[1] = reason;
        self.termination = Some(response);
        self.local_termination = LocalTermination::Queued { reason, host };
    }

    /// Record whether the retained hardware TX packet was acknowledged.
    pub fn observe_transmission_completion(&mut self, acknowledged: bool) {
        if acknowledged
            && let LocalTermination::Transmitted { reason, host } = self.local_termination
        {
            self.local_termination = LocalTermination::Acknowledged { reason, host };
        }
    }

    /// Whether the peer acknowledged the locally initiated termination PDU.
    pub const fn local_termination_acknowledged(&self) -> bool {
        matches!(
            self.local_termination,
            LocalTermination::Acknowledged { .. }
        )
    }

    /// Reason carried by an acknowledged locally initiated termination PDU.
    pub const fn local_termination_acknowledged_reason(&self) -> Option<u8> {
        match self.local_termination {
            LocalTermination::Acknowledged { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// Whether the first termination PDU still needs a transmit allocation.
    pub const fn local_termination_queued(&self) -> bool {
        matches!(self.local_termination, LocalTermination::Queued { .. })
    }

    /// Reason retained for the whole locally initiated procedure.
    pub const fn local_termination_reason(&self) -> Option<u8> {
        match self.local_termination {
            LocalTermination::None => None,
            LocalTermination::Queued { reason, .. }
            | LocalTermination::Transmitted { reason, .. }
            | LocalTermination::Acknowledged { reason, .. } => Some(reason),
        }
    }

    /// Reason published after locally initiated termination completes.
    ///
    /// HCI reports `0x16` when Disconnect came from the local Host while the
    /// requested reason remains the value carried in `LL_TERMINATE_IND`.
    pub const fn local_termination_completion_reason(&self) -> Option<u8> {
        match self.local_termination {
            LocalTermination::None => None,
            LocalTermination::Queued { reason, host }
            | LocalTermination::Transmitted { reason, host }
            | LocalTermination::Acknowledged { reason, host } => {
                Some(if host { 0x16 } else { reason })
            }
        }
    }

    /// Commit only after a CPU-owned TX graph accepted this exact response.
    pub fn response_enqueued(&mut self) {
        if let LocalTermination::Queued { reason, host } = self.local_termination
            && self.termination.is_some()
        {
            self.termination = None;
            self.local_termination = LocalTermination::Transmitted { reason, host };
        } else if self.responses[0].is_some() {
            self.responses[0] = self.responses[1].take();
        } else if matches!(self.local_feature_request, LocalFeatureRequest::Queued) {
            self.local_feature_request = LocalFeatureRequest::Transmitted;
        } else if matches!(self.local_version_request, LocalVersionRequest::Queued(_)) {
            self.local_version_request = LocalVersionRequest::Transmitted;
            self.version_queued = true;
        } else if self.data_length_request.take().is_some() {
            self.data_length.request_enqueued();
        }
    }
}

impl Default for LePeripheralControl {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURE_REQ: [u8; 11] = [3, 9, 8, 255, 255, 255, 255, 255, 255, 255, 255];

    #[test]
    fn feature_response_intersects_the_used_octet_and_advertises_supported_upper_octets() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [
                9,
                1 | (1 << 3) | (1 << 4) | (1 << 5),
                1 << 6,
                0,
                0,
                0,
                0,
                0,
                0
            ]
        );
        // Pending survives arbitrary reads while the controller queue is busy.
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn feature_response_intersects_every_octet_with_the_peer_mask() {
        let mut ll = LePeripheralControl::new();
        let request = [3, 9, 0x08, 1 << 4, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        ll.receive(&request, None).unwrap();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [0x09, 1 << 4, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn ping_request_queues_one_response_and_ping_response_does_not_loop() {
        let mut ll = LePeripheralControl::new();

        assert_eq!(
            ll.receive(&[3, 1, 0x12], None),
            Ok(LePeripheralReceive::Control)
        );
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [0x13]);
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());

        assert_eq!(
            ll.receive(&[3, 1, 0x13], None),
            Ok(LePeripheralReceive::Control)
        );
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn ping_pdus_require_empty_payloads() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(
            ll.receive(&[3, 2, 0x12, 0], None),
            Err(LePeripheralControlError::MalformedPdu)
        );
        assert_eq!(
            ll.receive(&[3, 2, 0x13, 0], None),
            Err(LePeripheralControlError::MalformedPdu)
        );
    }

    #[test]
    fn local_feature_request_starts_after_status_and_completes_from_feature_response() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Admitted
        );
        assert!(ll.pending_response().is_none());
        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Busy
        );

        assert_eq!(ll.activate_remote_feature_request(), None);
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [
                0x0e,
                1 | (1 << 3) | (1 << 4) | (1 << 5),
                1 << 6,
                0,
                0,
                0,
                0,
                0,
                0
            ]
        );
        ll.response_enqueued();
        assert!(ll.local_feature_request_transmitted());
        assert!(ll.pending_response().is_none());

        let response = [3, 9, 9, 0x08, 2, 3, 4, 5, 6, 7, 8];
        ll.receive(&response, None).unwrap();
        assert_eq!(
            ll.take_remote_features_result(),
            Some(LeRemoteFeaturesResult::Success([0x08, 2, 3, 4, 5, 6, 7, 8]))
        );
        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Cached([0x08, 2, 3, 4, 5, 6, 7, 8])
        );
    }

    #[test]
    fn unknown_response_and_timeout_finish_the_local_feature_procedure_once() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Admitted
        );
        ll.activate_remote_feature_request();
        ll.response_enqueued();
        ll.receive(&[3, 2, 7, 0x0e], None).unwrap();
        assert_eq!(
            ll.take_remote_features_result(),
            Some(LeRemoteFeaturesResult::Unsupported)
        );
        assert_eq!(ll.take_remote_features_result(), None);

        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Admitted
        );
        ll.activate_remote_feature_request();
        ll.response_enqueued();
        ll.receive(&[3, 3, 0x11, 0x0e, 0x23], None).unwrap();
        assert_eq!(
            ll.take_remote_features_result(),
            Some(LeRemoteFeaturesResult::Rejected { reason: 0x23 })
        );

        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Admitted
        );
        ll.activate_remote_feature_request();
        ll.response_enqueued();
        ll.expire_remote_feature_request();
        ll.expire_remote_feature_request();
        assert_eq!(
            ll.take_remote_features_result(),
            Some(LeRemoteFeaturesResult::ResponseTimeout)
        );
    }

    #[test]
    fn local_version_request_is_status_gated_completed_and_cached() {
        let local = LeVersionInformation::new(0x0d, 0xffff, 1);
        let peer = LeVersionInformation::new(0x0c, 0x1234, 0x5678);
        let mut ll = LePeripheralControl::new().with_local_version(Some(local));
        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Admitted
        );
        assert!(ll.pending_response().is_none());
        assert_eq!(ll.activate_remote_version_request(), None);
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [0x0c, 0x0d, 0xff, 0xff, 1, 0]
        );
        ll.response_enqueued();
        assert!(ll.local_version_request_transmitted());

        ll.receive(&[3, 6, 0x0c, 0x0c, 0x34, 0x12, 0x78, 0x56], Some(local))
            .unwrap();
        assert_eq!(
            ll.take_remote_version_result(),
            Some(LeRemoteVersionResult::Success(peer))
        );
        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Cached(peer)
        );
    }

    #[test]
    fn local_version_request_requires_identity_and_shares_local_procedure_owner() {
        let mut unavailable = LePeripheralControl::new();
        assert_eq!(
            unavailable.admit_remote_version_request(),
            LeRemoteVersionAdmission::LocalIdentityUnavailable
        );

        let mut ll = LePeripheralControl::new()
            .with_local_version(Some(LeVersionInformation::new(0x0d, 0xffff, 1)));
        assert_eq!(
            ll.admit_remote_feature_request(),
            LeRemoteFeaturesAdmission::Admitted
        );
        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Busy
        );
    }

    #[test]
    fn local_version_unknown_reject_and_timeout_finish_once() {
        let local = LeVersionInformation::new(0x0d, 0xffff, 1);
        let mut ll = LePeripheralControl::new().with_local_version(Some(local));

        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Admitted
        );
        ll.activate_remote_version_request();
        ll.response_enqueued();
        ll.receive(&[3, 2, 0x07, 0x0c], Some(local)).unwrap();
        assert_eq!(
            ll.take_remote_version_result(),
            Some(LeRemoteVersionResult::Unsupported)
        );

        let mut ll = LePeripheralControl::new().with_local_version(Some(local));
        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Admitted
        );
        ll.activate_remote_version_request();
        ll.response_enqueued();
        ll.receive(&[3, 3, 0x11, 0x0c, 0x23], Some(local)).unwrap();
        assert_eq!(
            ll.take_remote_version_result(),
            Some(LeRemoteVersionResult::Rejected { reason: 0x23 })
        );

        let mut ll = LePeripheralControl::new().with_local_version(Some(local));
        assert_eq!(
            ll.admit_remote_version_request(),
            LeRemoteVersionAdmission::Admitted
        );
        ll.activate_remote_version_request();
        ll.response_enqueued();
        ll.expire_local_procedure();
        assert_eq!(
            ll.take_remote_version_result(),
            Some(LeRemoteVersionResult::ResponseTimeout)
        );
        assert_eq!(ll.take_remote_version_result(), None);
    }

    #[test]
    fn queued_responses_preserve_order_and_overflow_preserves_both() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        ll.receive(&[3, 1, 0xf1], None).unwrap();
        assert_eq!(
            ll.receive(&FEATURE_REQ, None),
            Err(LePeripheralControlError::ResponseQueueFull)
        );
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
        ll.response_enqueued();
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [7, 0xf1]);
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn local_termination_preempts_responses_and_waits_for_peer_ack() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        ll.receive(&[3, 1, 0xf1], None).unwrap();

        ll.request_local_termination(0x13);
        ll.request_local_termination(0x15);
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [0x02, 0x13]);
        assert!(ll.local_termination_queued());
        assert_eq!(ll.local_termination_reason(), Some(0x13));
        assert!(!ll.local_termination_acknowledged());

        ll.response_enqueued();
        assert!(!ll.local_termination_queued());
        assert_eq!(ll.local_termination_reason(), Some(0x13));
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 0x09);
        ll.observe_transmission_completion(false);
        assert!(!ll.local_termination_acknowledged());
        ll.observe_transmission_completion(true);
        assert!(ll.local_termination_acknowledged());
        assert_eq!(ll.local_termination_reason(), Some(0x13));
        assert_eq!(ll.local_termination_completion_reason(), Some(0x13));

        ll.response_enqueued();
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [0x07, 0xf1]);
    }

    #[test]
    fn host_disconnect_keeps_the_air_reason_and_reports_local_host_completion() {
        let mut ll = LePeripheralControl::new();
        ll.request_host_termination(0x13);
        assert_eq!(ll.pending_response().unwrap().as_bytes(), [0x02, 0x13]);
        assert_eq!(ll.local_termination_reason(), Some(0x13));
        assert_eq!(ll.local_termination_completion_reason(), Some(0x16));
        ll.response_enqueued();
        ll.observe_transmission_completion(true);
        assert_eq!(ll.local_termination_acknowledged_reason(), Some(0x13));
        assert_eq!(ll.local_termination_completion_reason(), Some(0x16));
    }

    #[test]
    fn malformed_feature_request_does_not_replace_a_waiting_response() {
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        for pdu in [&[3, 0][..], &[3, 2, 8, 0], &[3, 9, 8]] {
            assert_eq!(
                ll.receive(pdu, None),
                Err(LePeripheralControlError::MalformedPdu)
            );
        }
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
    }

    #[test]
    fn empty_and_response_pdus_do_not_create_control_loops() {
        let mut ll = LePeripheralControl::new();
        for pdu in [
            &[1, 0][..],
            &[3, 2, 7, 8],
            &[3, 2, 13, 0x1a],
            &[3, 3, 17, 8, 0x1a],
        ] {
            ll.receive(pdu, None).unwrap();
        }
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn only_nonempty_data_requires_controller_acl_storage() {
        assert_eq!(
            LePeripheralControl::receive_requires_acl_slot(&[2, 3, 1, 2, 3]),
            Ok(true)
        );
        assert_eq!(
            LePeripheralControl::receive_requires_acl_slot(&[1, 0]),
            Ok(false)
        );
        assert_eq!(
            LePeripheralControl::receive_requires_acl_slot(&FEATURE_REQ),
            Ok(false)
        );
        assert_eq!(
            LePeripheralControl::receive_requires_acl_slot(&[2, 2, 1]),
            Err(LePeripheralControlError::MalformedPdu)
        );
    }

    #[test]
    fn unsupported_mandatory_transitions_and_termination_are_explicit() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(
            ll.receive(&[3, 2, 2, 0x13], None),
            Err(LePeripheralControlError::PeerTermination { reason: 0x13 })
        );
        assert_eq!(
            ll.receive(&[3, 6, 0x0c, 13, 2, 0, 1, 0], None),
            Err(LePeripheralControlError::MandatoryProcedureUnavailable { opcode: 0x0c })
        );
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn channel_map_update_is_validated_and_returned_without_a_response() {
        let mut ll = LePeripheralControl::new();
        let received = ll
            .receive(&[3, 8, 1, 0x03, 0, 0, 0, 0, 0x34, 0x12], None)
            .unwrap();
        let LePeripheralReceive::ChannelMapUpdate(update) = received else {
            panic!("channel-map indication must retain its semantic value");
        };
        assert_eq!(update.channel_map().wire_bytes(), [0x03, 0, 0, 0, 0]);
        assert_eq!(update.instant(), 0x1234);
        assert!(ll.pending_response().is_none());

        assert_eq!(
            ll.receive(&[3, 8, 1, 1, 0, 0, 0, 0, 1, 0], None),
            Err(LePeripheralControlError::MalformedPdu)
        );
    }

    #[test]
    fn connection_update_is_validated_and_returned_without_a_response() {
        let mut ll = LePeripheralControl::new();
        let received = ll
            .receive(&[3, 12, 0, 2, 1, 0, 40, 0, 3, 0, 200, 0, 0x34, 0x12], None)
            .unwrap();
        let LePeripheralReceive::ConnectionUpdate(update) = received else {
            panic!("connection indication must retain its semantic value");
        };
        assert_eq!(update.timing().window_size_units(), 2);
        assert_eq!(update.timing().window_offset_units(), 1);
        assert_eq!(update.timing().interval_units(), 40);
        assert_eq!(update.timing().peripheral_latency(), 3);
        assert_eq!(update.timing().supervision_timeout_units(), 200);
        assert_eq!(update.instant(), 0x1234);
        assert!(ll.pending_response().is_none());

        assert_eq!(
            ll.receive(&[3, 12, 0, 0, 1, 0, 40, 0, 3, 0, 200, 0, 1, 0], None,),
            Err(LePeripheralControlError::MalformedPdu)
        );
    }

    #[test]
    fn peer_control_failures_have_protocol_termination_reasons() {
        assert_eq!(
            LePeripheralControlError::ChannelMapUpdate(
                LePeripheralChannelMapUpdateError::InstantPassed
            )
            .termination_reason(),
            Some(0x28)
        );
        assert_eq!(
            LePeripheralControlError::ChannelMapUpdate(
                LePeripheralChannelMapUpdateError::ProcedureAlreadyPending
            )
            .termination_reason(),
            Some(0x23)
        );
        assert_eq!(
            LePeripheralControlError::ConnectionUpdate(
                LePeripheralConnectionUpdateError::IncompatibleProcedurePending
            )
            .termination_reason(),
            Some(0x2a)
        );
        assert_eq!(
            LePeripheralControlError::MalformedPdu.termination_reason(),
            Some(0x1e)
        );
        assert_eq!(
            LePeripheralControlError::PeerTermination { reason: 0x13 }.termination_reason(),
            None
        );
    }

    #[test]
    fn version_is_queued_once_and_preserves_the_product_identity() {
        let version = LeVersionInformation::new(13, 0xffff, 0x1234);
        let request = [3, 6, 12, 12, 2, 0, 0x78, 0x56];
        let mut ll = LePeripheralControl::new();
        ll.receive(&request, Some(version)).unwrap();
        ll.receive(&request, Some(version)).unwrap();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [12, 13, 255, 255, 0x34, 0x12]
        );
        assert_eq!(
            ll.peer_version(),
            Some(LeVersionInformation::new(12, 2, 0x5678))
        );
        ll.response_enqueued();
        ll.receive(&request, Some(version)).unwrap();
        assert!(ll.pending_response().is_none());
    }

    #[test]
    fn a_full_queue_or_malformed_version_does_not_consume_the_once_flag() {
        let version = Some(LeVersionInformation::new(13, 0xffff, 1));
        let request = [3, 6, 12, 12, 2, 0, 1, 0];
        let mut ll = LePeripheralControl::new();
        ll.receive(&FEATURE_REQ, None).unwrap();
        ll.receive(&FEATURE_REQ, None).unwrap();
        assert_eq!(
            ll.receive(&request, version),
            Err(LePeripheralControlError::ResponseQueueFull)
        );
        ll.response_enqueued();
        assert_eq!(
            ll.receive(&[3, 1, 12], version),
            Err(LePeripheralControlError::MalformedPdu)
        );
        ll.receive(&request, version).unwrap();
        ll.response_enqueued();
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 12);
    }

    const LENGTH_REQ: [u8; 11] = [3, 9, 0x14, 251, 0, 0x48, 8, 251, 0, 0x48, 8];

    #[test]
    fn a_peer_length_request_is_answered_and_changes_the_receive_length() {
        let mut ll = LePeripheralControl::new();
        assert_eq!(ll.data_lengths(), LeDataLengths::MINIMUM);
        assert_eq!(
            ll.receive(&LENGTH_REQ, None),
            Ok(LePeripheralReceive::Control)
        );
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [0x15, 251, 0, 0x48, 8, 27, 0, 0x48, 1]
        );
        let change = ll.take_data_length_change().unwrap();
        assert_eq!(change.receive, LeDataLength::SUPPORTED_MAXIMUM);
        assert_eq!(change.transmit, LeDataLength::MINIMUM);
        assert!(!ll.local_procedure_pending());
        // A truncated request ends the connection as malformed.
        assert_eq!(
            ll.receive(&[3, 8, 0x14, 251, 0, 0x48, 8, 251, 0, 0x48], None),
            Err(LePeripheralControlError::MalformedPdu)
        );
    }

    #[test]
    fn an_extended_connection_requests_its_length_after_other_responses() {
        let mut ll = LePeripheralControl::new().with_data_length(LeDataLength::SUPPORTED_MAXIMUM);
        assert!(ll.local_procedure_pending());
        ll.receive(&FEATURE_REQ, None).unwrap();
        assert_eq!(ll.pending_response().unwrap().as_bytes()[0], 9);
        ll.response_enqueued();
        assert_eq!(
            ll.pending_response().unwrap().as_bytes(),
            [0x14, 251, 0, 0x48, 8, 251, 0, 0x48, 8]
        );
        ll.response_enqueued();
        assert!(ll.pending_response().is_none());
        assert!(ll.local_data_length_request_transmitted());
        ll.receive(&[3, 9, 0x15, 200, 0, 0x48, 8, 100, 0, 0x48, 8], None)
            .unwrap();
        assert!(!ll.local_procedure_pending());
        let lengths = ll.data_lengths();
        assert_eq!(lengths.transmit.octets(), 200);
        assert_eq!(lengths.receive.octets(), 100);
    }

    #[test]
    fn a_peer_without_the_procedure_ends_the_local_request() {
        for refusal in [&[3, 2, 0x07, 0x14][..], &[3, 3, 0x11, 0x14, 0x1a][..]] {
            let mut ll =
                LePeripheralControl::new().with_data_length(LeDataLength::SUPPORTED_MAXIMUM);
            ll.response_enqueued();
            assert!(ll.local_data_length_request_transmitted());
            ll.receive(refusal, None).unwrap();
            assert!(!ll.local_procedure_pending());
            assert_eq!(ll.data_lengths(), LeDataLengths::MINIMUM);
            assert_eq!(ll.take_data_length_change(), None);
        }
    }
}
