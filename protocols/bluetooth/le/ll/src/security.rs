//! Bluetooth LE ACL packet encryption with affine key and counter ownership.
//!
//! This module owns the Peripheral encryption start, pause and restart procedures, the resulting
//! session key and the independent 39-bit Peripheral-transmit and
//! Central-transmit packet counters. Its caller supplies fresh SKDp/IVp entropy,
//! transfers retained responses to a reliable radio packet owner and supplies
//! the Host-selected LTK. A retransmission reuses the ciphertext already
//! retained by that radio owner and never calls
//! [`LePeripheralAclEncryption::encrypt_new_packet`] again. Stateless packet protection
//! and zeroizing key material belong to `oer-bluetooth-le-cipher`.

use oer_bluetooth_le_wire::encryption::{
    LeEncryptionControlResponse, LeEncryptionRequest, LeLongTermKeyRequest,
};

use oer_bluetooth_le_cipher::{
    LE_PACKET_COUNTER_MAX, LeAclEncryptionError, LeAclInitializationVector, LeAclSessionKey,
    LeLongTermKey, LeSessionKeyDiversifier, decrypt_packet, decrypted_packet_length,
    encrypt_packet, encrypted_packet_length,
};

const PERIPHERAL_TO_CENTRAL_DIRECTION: bool = false;
const CENTRAL_TO_PERIPHERAL_DIRECTION: bool = true;

/// Fresh Peripheral contribution to one encryption start or restart.
pub struct LePeripheralEncryptionRandom {
    session_key_diversifier: [u8; 8],
    initialization_vector: [u8; 4],
}

impl LePeripheralEncryptionRandom {
    pub const fn new(session_key_diversifier: [u8; 8], initialization_vector: [u8; 4]) -> Self {
        Self {
            session_key_diversifier,
            initialization_vector,
        }
    }
}

/// Invalid input or ownership transition in Peripheral encryption start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralEncryptionProcedureError {
    Busy,
    InvalidState,
    MalformedEncryptionRequest,
    UnexpectedPhysicalChannelPdu,
    MicFailure,
}

/// Required interpretation of the next accepted Central packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralEncryptionReceiveMode {
    Plaintext,
    EncryptedStartResponse,
    Encrypted,
    UnencryptedPauseResponse,
    RestartEncryptionRequest,
    Blocked,
}

/// Result of authenticating one packet while connection encryption is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralEncryptedReceive {
    Plaintext { length: usize },
    PauseRequest,
}

#[derive(Clone, Copy)]
struct LePeripheralEncryptionMaterial {
    request: LeLongTermKeyRequest,
    diversifier: [u8; 16],
    initialization_vector: [u8; 8],
    restart: bool,
}

enum LePeripheralEncryptionState {
    Idle,
    EncryptionResponsePending(LePeripheralEncryptionMaterial),
    EncryptionResponseTransmitted(LePeripheralEncryptionMaterial),
    WaitingForLongTermKey(LePeripheralEncryptionMaterial),
    StartRequestPending {
        encryption: LePeripheralAclEncryption,
        initial: bool,
    },
    StartRequestTransmitted {
        encryption: LePeripheralAclEncryption,
        initial: bool,
    },
    StartResponsePending {
        encryption: LePeripheralAclEncryption,
        response: [u8; 5],
        initial: bool,
    },
    Active(LePeripheralAclEncryption),
    PauseResponsePending {
        _encryption: LePeripheralAclEncryption,
        response: [u8; 5],
    },
    PauseResponseTransmitted,
    RestartEncryptionRequestExpected,
    RejectPending,
    RejectTransmitted,
    TerminationRequired,
    Failed,
}

/// How a terminal encryption-procedure failure ends the connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralEncryptionTermination {
    /// Send `LL_TERMINATE_IND` with this reason: PIN or Key Missing when the
    /// Host rejects the key of a restart.
    Terminate(u8),
    /// Leave the Connection state at once without sending anything, and
    /// report Connection Terminated due to MIC Failure (`0x3D`). A MIC
    /// failure or an unexpected PDU during the procedure requires this (Core
    /// Vol 6 Part B, encryption procedures); the peer then loses the
    /// connection at its supervision timeout.
    Exit,
}

/// Peripheral-side encryption-start handshake with retained response ownership.
///
/// Radio code reports when each unencrypted response enters its reliable TX
/// graph and when that graph observes acknowledgment. The LTK request becomes
/// visible only after `LL_ENC_RSP` is acknowledged. The first received and sent
/// encrypted `LL_START_ENC_RSP` packets consume counter zero in their respective
/// directions.
pub struct LePeripheralEncryptionProcedure {
    state: LePeripheralEncryptionState,
    long_term_key_request_reported: bool,
    encryption_enabled_pending: bool,
    encryption_refreshed_pending: bool,
}

impl LePeripheralEncryptionProcedure {
    pub const fn new() -> Self {
        Self {
            state: LePeripheralEncryptionState::Idle,
            long_term_key_request_reported: false,
            encryption_enabled_pending: false,
            encryption_refreshed_pending: false,
        }
    }

    pub const fn receive_mode(&self) -> LePeripheralEncryptionReceiveMode {
        match self.state {
            LePeripheralEncryptionState::Idle => LePeripheralEncryptionReceiveMode::Plaintext,
            LePeripheralEncryptionState::StartRequestTransmitted { .. } => {
                LePeripheralEncryptionReceiveMode::EncryptedStartResponse
            }
            LePeripheralEncryptionState::Active(_) => LePeripheralEncryptionReceiveMode::Encrypted,
            LePeripheralEncryptionState::PauseResponseTransmitted => {
                LePeripheralEncryptionReceiveMode::UnencryptedPauseResponse
            }
            LePeripheralEncryptionState::RestartEncryptionRequestExpected => {
                LePeripheralEncryptionReceiveMode::RestartEncryptionRequest
            }
            _ => LePeripheralEncryptionReceiveMode::Blocked,
        }
    }

    /// Admit one exact Central `LL_ENC_REQ` control payload.
    pub fn begin(
        &mut self,
        request: &[u8],
        random: LePeripheralEncryptionRandom,
    ) -> Result<(), LePeripheralEncryptionProcedureError> {
        let restart = match self.state {
            LePeripheralEncryptionState::Idle => false,
            LePeripheralEncryptionState::RestartEncryptionRequestExpected => true,
            _ => return Err(LePeripheralEncryptionProcedureError::Busy),
        };
        let Some(request) = LeEncryptionRequest::decode(request) else {
            if restart {
                self.state = LePeripheralEncryptionState::Failed;
                return Err(LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu);
            }
            return Err(LePeripheralEncryptionProcedureError::MalformedEncryptionRequest);
        };
        let mut diversifier = [0; 16];
        for (slot, byte) in diversifier.iter_mut().zip(
            request
                .diversifier
                .iter()
                .chain(random.session_key_diversifier.iter()),
        ) {
            *slot = *byte;
        }
        let mut initialization_vector = [0; 8];
        for (slot, byte) in initialization_vector.iter_mut().zip(
            request
                .initialization_vector
                .iter()
                .chain(random.initialization_vector.iter()),
        ) {
            *slot = *byte;
        }
        self.state = LePeripheralEncryptionState::EncryptionResponsePending(
            LePeripheralEncryptionMaterial {
                request: request.key,
                diversifier,
                initialization_vector,
                restart,
            },
        );
        self.long_term_key_request_reported = false;
        Ok(())
    }

    /// Current response, retained until the radio graph accepts it.
    pub fn pending_response(&self) -> Option<LeEncryptionControlResponse> {
        match &self.state {
            LePeripheralEncryptionState::EncryptionResponsePending(material) => {
                Some(LeEncryptionControlResponse::encryption_response(
                    material.diversifier,
                    material.initialization_vector,
                ))
            }
            LePeripheralEncryptionState::StartRequestPending { .. } => {
                Some(LeEncryptionControlResponse::start_request())
            }
            LePeripheralEncryptionState::StartResponsePending {
                response: encrypted,
                ..
            }
            | LePeripheralEncryptionState::PauseResponsePending {
                response: encrypted,
                ..
            } => Some(LeEncryptionControlResponse::encrypted_response(*encrypted)),
            LePeripheralEncryptionState::RejectPending => {
                Some(LeEncryptionControlResponse::missing_key_reject())
            }
            _ => None,
        }
    }

    /// Transfer the current response to the reliable radio TX graph once.
    pub fn response_enqueued(&mut self) -> Result<(), LePeripheralEncryptionProcedureError> {
        let state = core::mem::replace(&mut self.state, LePeripheralEncryptionState::Failed);
        self.state = match state {
            LePeripheralEncryptionState::EncryptionResponsePending(material) => {
                LePeripheralEncryptionState::EncryptionResponseTransmitted(material)
            }
            LePeripheralEncryptionState::StartRequestPending {
                encryption,
                initial,
            } => LePeripheralEncryptionState::StartRequestTransmitted {
                encryption,
                initial,
            },
            LePeripheralEncryptionState::StartResponsePending {
                encryption,
                initial,
                ..
            } => {
                self.encryption_enabled_pending = initial;
                self.encryption_refreshed_pending = !initial;
                LePeripheralEncryptionState::Active(encryption)
            }
            LePeripheralEncryptionState::PauseResponsePending { .. } => {
                LePeripheralEncryptionState::PauseResponseTransmitted
            }
            LePeripheralEncryptionState::RejectPending => {
                LePeripheralEncryptionState::RejectTransmitted
            }
            state => {
                self.state = state;
                return Err(LePeripheralEncryptionProcedureError::InvalidState);
            }
        };
        Ok(())
    }

    /// Apply the radio ACK result to the transmitted unencrypted response.
    pub fn observe_transmission_completion(&mut self, acknowledged: bool) {
        if !acknowledged {
            return;
        }
        let state = core::mem::replace(&mut self.state, LePeripheralEncryptionState::Failed);
        self.state = match state {
            LePeripheralEncryptionState::EncryptionResponseTransmitted(material) => {
                LePeripheralEncryptionState::WaitingForLongTermKey(material)
            }
            LePeripheralEncryptionState::StartRequestTransmitted {
                encryption,
                initial,
            } => LePeripheralEncryptionState::StartRequestTransmitted {
                encryption,
                initial,
            },
            LePeripheralEncryptionState::RejectTransmitted => LePeripheralEncryptionState::Idle,
            state => state,
        };
    }

    /// Host event data available after the acknowledged `LL_ENC_RSP`.
    pub const fn long_term_key_request(&self) -> Option<LeLongTermKeyRequest> {
        match self.state {
            LePeripheralEncryptionState::WaitingForLongTermKey(material) => Some(material.request),
            _ => None,
        }
    }

    /// Transfer the Host request exactly once while retaining the waiting state.
    pub fn take_long_term_key_request(&mut self) -> Option<LeLongTermKeyRequest> {
        if self.long_term_key_request_reported {
            return None;
        }
        let request = self.long_term_key_request()?;
        self.long_term_key_request_reported = true;
        Some(request)
    }

    /// Report the initial transition to AES-CCM exactly once.
    pub fn take_encryption_enabled(&mut self) -> bool {
        core::mem::take(&mut self.encryption_enabled_pending)
    }

    /// Report a completed pause/restart key refresh exactly once.
    pub fn take_encryption_refreshed(&mut self) -> bool {
        core::mem::take(&mut self.encryption_refreshed_pending)
    }

    /// Whether radio data/control unrelated to encryption must remain queued.
    pub const fn blocks_unrelated_transmission(&self) -> bool {
        !matches!(
            self.state,
            LePeripheralEncryptionState::Idle | LePeripheralEncryptionState::Active(_)
        )
    }

    /// Admit ordinary control traffic, or the required PIN or Key Missing termination.
    ///
    /// A rejected refresh keeps data and unrelated control blocked. Its
    /// `LL_TERMINATE_IND(0x06)` must still reach the reliable TX owner after
    /// encryption has paused; admitting this packet does not reopen plaintext ACL.
    pub fn permits_control_pdu(&self, payload: &[u8]) -> bool {
        !self.blocks_unrelated_transmission()
            || (matches!(self.state, LePeripheralEncryptionState::TerminationRequired)
                && payload == [0x02, 0x06])
    }

    /// Install the Host's LTK and retain a new unencrypted `LL_START_ENC_REQ`.
    pub fn provide_long_term_key(
        &mut self,
        long_term_key: LeLongTermKey,
    ) -> Result<(), (LePeripheralEncryptionProcedureError, LeLongTermKey)> {
        let LePeripheralEncryptionState::WaitingForLongTermKey(material) = self.state else {
            return Err((
                LePeripheralEncryptionProcedureError::InvalidState,
                long_term_key,
            ));
        };
        let session_key =
            long_term_key.derive_session_key(LeSessionKeyDiversifier::new(material.diversifier));
        self.state = LePeripheralEncryptionState::StartRequestPending {
            encryption: LePeripheralAclEncryption::new(
                session_key,
                LeAclInitializationVector::new(material.initialization_vector),
            ),
            initial: !material.restart,
        };
        Ok(())
    }

    /// Retain a PIN or Key Missing rejection for the Central.
    pub fn reject_long_term_key(&mut self) -> Result<(), LePeripheralEncryptionProcedureError> {
        let LePeripheralEncryptionState::WaitingForLongTermKey(material) = self.state else {
            return Err(LePeripheralEncryptionProcedureError::InvalidState);
        };
        self.state = if material.restart {
            LePeripheralEncryptionState::TerminationRequired
        } else {
            LePeripheralEncryptionState::RejectPending
        };
        Ok(())
    }

    /// Authenticate the Central's first encrypted `LL_START_ENC_RSP` and retain
    /// the Peripheral's encrypted response.
    pub fn receive_encrypted_start_response(
        &mut self,
        header: u8,
        packet: &mut [u8],
    ) -> Result<(), LePeripheralEncryptionProcedureError> {
        let state = core::mem::replace(&mut self.state, LePeripheralEncryptionState::Failed);
        let LePeripheralEncryptionState::StartRequestTransmitted {
            mut encryption,
            initial,
        } = state
        else {
            self.state = state;
            return Err(LePeripheralEncryptionProcedureError::InvalidState);
        };
        let length = encryption
            .decrypt_new_packet(header, packet)
            .map_err(map_acl_receive_error)?;
        if header & 0x03 != 0x03 || length != 1 || packet.first().copied() != Some(0x06) {
            return Err(LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu);
        }
        let mut response = [0; 5];
        response[0] = 0x06;
        encryption
            .encrypt_new_packet(0x03, &mut response, 1)
            .map_err(|_| LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu)?;
        self.state = LePeripheralEncryptionState::StartResponsePending {
            encryption,
            response,
            initial,
        };
        Ok(())
    }

    /// Authenticate a Central `LL_PAUSE_ENC_REQ` after every older outbound
    /// data packet has completed, then retain the encrypted Peripheral response.
    pub fn begin_pause(
        &mut self,
        header: u8,
        packet: &mut [u8],
    ) -> Result<(), LePeripheralEncryptionProcedureError> {
        let state = core::mem::replace(&mut self.state, LePeripheralEncryptionState::Failed);
        let LePeripheralEncryptionState::Active(mut encryption) = state else {
            self.state = state;
            return Err(LePeripheralEncryptionProcedureError::InvalidState);
        };
        let length = encryption
            .decrypt_new_packet(header, packet)
            .map_err(map_acl_receive_error)?;
        if length != 1 || packet.first().copied() != Some(0x0a) {
            return Err(LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu);
        }
        let mut response = [0; 5];
        response[0] = 0x0b;
        encryption
            .encrypt_new_packet(0x03, &mut response, 1)
            .map_err(|_| LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu)?;
        self.state = LePeripheralEncryptionState::PauseResponsePending {
            _encryption: encryption,
            response,
        };
        Ok(())
    }

    /// Authenticate one active encrypted packet exactly once. A pause request
    /// is consumed by this method and changes procedure state; other plaintext
    /// remains in `packet` for the ordinary LL control/ACL dispatcher.
    pub fn receive_active_packet(
        &mut self,
        header: u8,
        packet: &mut [u8],
    ) -> Result<LePeripheralEncryptedReceive, LePeripheralEncryptionProcedureError> {
        let state = core::mem::replace(&mut self.state, LePeripheralEncryptionState::Failed);
        let LePeripheralEncryptionState::Active(mut encryption) = state else {
            self.state = state;
            return Err(LePeripheralEncryptionProcedureError::InvalidState);
        };
        let length = encryption
            .decrypt_new_packet(header, packet)
            .map_err(map_acl_receive_error)?;
        if header & 0x03 == 0x03 && length == 1 && packet.first().copied() == Some(0x0a) {
            let mut response = [0; 5];
            response[0] = 0x0b;
            encryption
                .encrypt_new_packet(0x03, &mut response, 1)
                .map_err(|_| LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu)?;
            self.state = LePeripheralEncryptionState::PauseResponsePending {
                _encryption: encryption,
                response,
            };
            return Ok(LePeripheralEncryptedReceive::PauseRequest);
        }
        self.state = LePeripheralEncryptionState::Active(encryption);
        Ok(LePeripheralEncryptedReceive::Plaintext { length })
    }

    /// Consume the Central's unencrypted `LL_PAUSE_ENC_RSP`. The next accepted
    /// packet must be a fresh unencrypted `LL_ENC_REQ` passed to [`Self::begin`].
    pub fn receive_unencrypted_pause_response(
        &mut self,
        payload: &[u8],
    ) -> Result<(), LePeripheralEncryptionProcedureError> {
        if !matches!(
            self.state,
            LePeripheralEncryptionState::PauseResponseTransmitted
        ) {
            return Err(LePeripheralEncryptionProcedureError::InvalidState);
        }
        if payload != [0x0b] {
            self.state = LePeripheralEncryptionState::Failed;
            return Err(LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu);
        }
        self.state = LePeripheralEncryptionState::RestartEncryptionRequestExpected;
        Ok(())
    }

    pub fn active_encryption(&mut self) -> Option<&mut LePeripheralAclEncryption> {
        match &mut self.state {
            LePeripheralEncryptionState::Active(encryption) => Some(encryption),
            _ => None,
        }
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.state, LePeripheralEncryptionState::Idle)
    }

    pub const fn is_active(&self) -> bool {
        matches!(self.state, LePeripheralEncryptionState::Active(_))
    }

    /// How a terminal procedure failure ends the connection.
    pub const fn termination(&self) -> Option<LePeripheralEncryptionTermination> {
        match self.state {
            LePeripheralEncryptionState::TerminationRequired => {
                Some(LePeripheralEncryptionTermination::Terminate(0x06))
            }
            LePeripheralEncryptionState::Failed => Some(LePeripheralEncryptionTermination::Exit),
            _ => None,
        }
    }

    /// Seal the procedure after any packet forbidden by its current receive mode.
    pub fn fail_unexpected_physical_channel_pdu(&mut self) {
        self.state = LePeripheralEncryptionState::Failed;
    }
}

impl Default for LePeripheralEncryptionProcedure {
    fn default() -> Self {
        Self::new()
    }
}

fn map_acl_receive_error(error: LeAclEncryptionError) -> LePeripheralEncryptionProcedureError {
    match error {
        LeAclEncryptionError::MicMismatch => LePeripheralEncryptionProcedureError::MicFailure,
        LeAclEncryptionError::BufferTooSmall
        | LeAclEncryptionError::InvalidEmptyPdu
        | LeAclEncryptionError::PayloadTooLong
        | LeAclEncryptionError::PacketCounterExhausted => {
            LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu
        }
    }
}

#[derive(Clone, Copy)]
struct LeAclPacketCounter(Option<u64>);

impl LeAclPacketCounter {
    const fn new() -> Self {
        Self(Some(0))
    }

    fn current(self) -> Result<u64, LeAclEncryptionError> {
        self.0.ok_or(LeAclEncryptionError::PacketCounterExhausted)
    }

    fn commit(&mut self, counter: u64) {
        self.0 = counter
            .checked_add(1)
            .filter(|next| *next <= LE_PACKET_COUNTER_MAX);
    }
}

/// Encryption owner for one Peripheral ACL connection generation.
///
/// Constructing a new value starts both directions at packet counter zero.
/// Pause/restart must replace the complete owner with a newly derived key and
/// IV; it must not reset either counter in place under the old material.
pub struct LePeripheralAclEncryption {
    session_key: LeAclSessionKey,
    initialization_vector: LeAclInitializationVector,
    peripheral_transmit_counter: LeAclPacketCounter,
    central_transmit_counter: LeAclPacketCounter,
}

impl LePeripheralAclEncryption {
    pub const fn new(
        session_key: LeAclSessionKey,
        initialization_vector: LeAclInitializationVector,
    ) -> Self {
        Self {
            session_key,
            initialization_vector,
            peripheral_transmit_counter: LeAclPacketCounter::new(),
            central_transmit_counter: LeAclPacketCounter::new(),
        }
    }

    /// Encrypt one new Peripheral-to-Central PDU in place and append its MIC.
    ///
    /// `packet` is the complete writable payload allocation and its first
    /// `plaintext_len` octets contain plaintext. The returned length includes
    /// the MIC. A retained encrypted allocation is retransmitted directly.
    /// An Empty PDU (LLID=01, zero plaintext length) returns zero without
    /// touching storage or consuming a counter; it has no MIC (Core Vol 6,
    /// Part B, Section 2.4.1).
    pub fn encrypt_new_packet(
        &mut self,
        header: u8,
        packet: &mut [u8],
        plaintext_len: usize,
    ) -> Result<usize, LeAclEncryptionError> {
        let encrypted_len = encrypted_packet_length(header, packet.len(), plaintext_len)?;
        if encrypted_len == 0 {
            return Ok(0);
        }
        let counter = self.peripheral_transmit_counter.current()?;
        encrypt_packet(
            &self.session_key,
            &self.initialization_vector,
            counter,
            PERIPHERAL_TO_CENTRAL_DIRECTION,
            header,
            packet,
            plaintext_len,
        )?;
        self.peripheral_transmit_counter.commit(counter);
        Ok(encrypted_len)
    }

    /// Authenticate and decrypt one new Central-to-Peripheral PDU in place.
    ///
    /// On MIC failure the complete supplied payload is cleared and the counter
    /// is not advanced. The caller must terminate the connection with `0x3d`.
    /// An Empty PDU returns zero without consuming a counter. A MIC-only
    /// payload is malformed, not an authenticated Empty PDU.
    pub fn decrypt_new_packet(
        &mut self,
        header: u8,
        packet: &mut [u8],
    ) -> Result<usize, LeAclEncryptionError> {
        let plaintext_len = decrypted_packet_length(header, packet.len())?;
        if packet.is_empty() {
            return Ok(0);
        }
        let counter = self.central_transmit_counter.current()?;
        decrypt_packet(
            &self.session_key,
            &self.initialization_vector,
            counter,
            CENTRAL_TO_PERIPHERAL_DIRECTION,
            header,
            packet,
        )?;
        self.central_transmit_counter.commit(counter);
        Ok(plaintext_len)
    }

    pub const fn next_peripheral_transmit_counter(&self) -> Option<u64> {
        self.peripheral_transmit_counter.0
    }

    pub const fn next_central_transmit_counter(&self) -> Option<u64> {
        self.central_transmit_counter.0
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests {
    use super::*;
    use oer_bluetooth_le_cipher::{LE_ACL_MIC_BYTES, LE_ENCRYPTED_PLAINTEXT_BYTES};

    const LTK: [u8; 16] = [
        0xbf, 0x01, 0xfb, 0x9d, 0x4e, 0xf3, 0xbc, 0x36, 0xd8, 0x74, 0xf5, 0x39, 0x41, 0x38, 0x68,
        0x4c,
    ];
    const SKD: [u8; 16] = [
        0x13, 0x02, 0xf1, 0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13,
        0x02,
    ];
    const IV: [u8; 8] = [0x24, 0xab, 0xdc, 0xba, 0xbe, 0xba, 0xaf, 0xde];

    fn encryption() -> LePeripheralAclEncryption {
        let key = LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(SKD));
        LePeripheralAclEncryption::new(key, LeAclInitializationVector::new(IV))
    }

    fn encryption_request() -> [u8; 23] {
        [
            0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13, 0x02, 0xf1,
            0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
        ]
    }

    fn peripheral_random() -> LePeripheralEncryptionRandom {
        LePeripheralEncryptionRandom::new(
            [0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02],
            [0xbe, 0xba, 0xaf, 0xde],
        )
    }

    fn established_procedure() -> LePeripheralEncryptionProcedure {
        let mut procedure = LePeripheralEncryptionProcedure::new();
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        assert!(
            procedure
                .provide_long_term_key(LeLongTermKey::new(LTK))
                .is_ok()
        );
        procedure.response_enqueued().unwrap();
        let mut central_start_response = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        procedure
            .receive_encrypted_start_response(0x0f, &mut central_start_response)
            .unwrap();
        procedure.response_enqueued().unwrap();
        assert!(procedure.is_active());
        procedure
    }

    #[test]
    fn empty_acl_pdus_neither_carry_a_mic_nor_consume_counters() {
        let mut encryption = encryption();
        for header in [0x01, 0x05, 0x09, 0x1d] {
            let mut storage = [0xa5; 4];
            assert_eq!(
                encryption.encrypt_new_packet(header, &mut storage, 0),
                Ok(0)
            );
            assert_eq!(storage, [0xa5; 4]);
            assert_eq!(encryption.decrypt_new_packet(header, &mut []), Ok(0));
        }
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(0));
        assert_eq!(encryption.next_central_transmit_counter(), Some(0));
        // The first real packets must still use the published Core counter-zero samples.
        let mut sent = [0x06, 0, 0, 0, 0];
        encryption.encrypt_new_packet(0x03, &mut sent, 1).unwrap();
        assert_eq!(sent, [0xa3, 0x4c, 0x13, 0xa4, 0x15]);
        let mut received = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        assert_eq!(encryption.decrypt_new_packet(0x03, &mut received), Ok(1));
        assert_eq!(received, [0x06, 0, 0, 0, 0]);
    }

    #[test]
    fn empty_exception_rejects_control_headers_and_mic_only_packets() {
        let mut cipher = encryption();
        for header in [0x00, 0x02, 0x03] {
            assert_eq!(
                cipher.encrypt_new_packet(header, &mut [], 0),
                Err(LeAclEncryptionError::InvalidEmptyPdu)
            );
            assert_eq!(
                cipher.decrypt_new_packet(header, &mut []),
                Err(LeAclEncryptionError::InvalidEmptyPdu)
            );
        }
        let mut tag = [0; 4];
        assert_eq!(
            cipher.decrypt_new_packet(0x01, &mut tag),
            Err(LeAclEncryptionError::BufferTooSmall)
        );
        assert_eq!(cipher.next_central_transmit_counter(), Some(0));
        assert_eq!(cipher.next_peripheral_transmit_counter(), Some(0));
    }

    #[test]
    fn active_encryption_accepts_empty_ack_without_retiring_session() {
        let mut procedure = established_procedure();
        for _ in 0..8 {
            assert_eq!(
                procedure.receive_active_packet(0x01, &mut []),
                Ok(LePeripheralEncryptedReceive::Plaintext { length: 0 })
            );
        }
        assert!(procedure.is_active());
        assert_eq!(procedure.termination(), None);
        let cipher = procedure.active_encryption().unwrap();
        assert_eq!(cipher.next_central_transmit_counter(), Some(1));
        assert_eq!(cipher.next_peripheral_transmit_counter(), Some(1));
    }

    #[test]
    fn peripheral_and_central_samples_keep_independent_packet_counters() {
        let mut encryption = encryption();

        let mut peripheral_start = [0x06, 0, 0, 0, 0];
        assert_eq!(
            encryption
                .encrypt_new_packet(0x07, &mut peripheral_start, 1)
                .unwrap(),
            5
        );
        assert_eq!(peripheral_start, [0xa3, 0x4c, 0x13, 0xa4, 0x15]);
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(1));
        assert_eq!(encryption.next_central_transmit_counter(), Some(0));

        let mut central_start = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        assert_eq!(
            encryption
                .decrypt_new_packet(0x0f, &mut central_start)
                .unwrap(),
            1
        );
        assert_eq!(central_start, [0x06, 0, 0, 0, 0]);
        assert_eq!(encryption.next_central_transmit_counter(), Some(1));

        let mut peripheral_data = [0; 31];
        peripheral_data[..27].copy_from_slice(b"\x17\x0076543210ABCDEFGHIJKLMNOPQ");
        assert_eq!(
            encryption
                .encrypt_new_packet(0x06, &mut peripheral_data, 27)
                .unwrap(),
            31
        );
        assert_eq!(
            peripheral_data,
            [
                0xf3, 0x88, 0x81, 0xe7, 0xbd, 0x94, 0xc9, 0xc3, 0x69, 0xb9, 0xa6, 0x68, 0x46, 0xdd,
                0x47, 0x86, 0xaa, 0x8c, 0x39, 0xce, 0x54, 0x0d, 0x0d, 0xae, 0x3a, 0xdc, 0xdf, 0x89,
                0xb9, 0x60, 0x88,
            ]
        );

        let mut central_data = [
            0x7a, 0x70, 0xd6, 0x64, 0x15, 0x22, 0x6d, 0xf2, 0x6b, 0x17, 0x83, 0x9a, 0x06, 0x04,
            0x05, 0x59, 0x6b, 0xd6, 0x56, 0x4f, 0x79, 0x6b, 0x5b, 0x9c, 0xe6, 0xff, 0x32, 0xf7,
            0x5a, 0x6d, 0x33,
        ];
        assert_eq!(
            encryption
                .decrypt_new_packet(0x0e, &mut central_data)
                .unwrap(),
            27
        );
        assert_eq!(&central_data[..27], b"\x17\x00cdefghijklmnopq1234567890");
        assert!(central_data[27..].iter().all(|byte| *byte == 0));
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(2));
        assert_eq!(encryption.next_central_transmit_counter(), Some(2));
    }

    #[test]
    fn sequence_bits_do_not_enter_aad_and_retention_does_not_advance_a_counter() {
        let mut first = encryption();
        let mut second = encryption();
        let mut first_packet = [0x06, 0, 0, 0, 0];
        let mut second_packet = first_packet;
        first
            .encrypt_new_packet(0x03, &mut first_packet, 1)
            .unwrap();
        second
            .encrypt_new_packet(0x1f, &mut second_packet, 1)
            .unwrap();
        assert_eq!(first_packet, second_packet);
        let retained_retransmission = first_packet;
        assert_eq!(retained_retransmission, first_packet);
        assert_eq!(first.next_peripheral_transmit_counter(), Some(1));
    }

    #[test]
    fn wrong_mic_clears_plaintext_and_does_not_advance() {
        let mut encryption = encryption();
        let mut packet = [0x9f, 0xcd, 0xa7, 0xf4, 0x49];
        assert_eq!(
            encryption.decrypt_new_packet(0x0f, &mut packet),
            Err(LeAclEncryptionError::MicMismatch)
        );
        assert_eq!(packet, [0; 5]);
        assert_eq!(encryption.next_central_transmit_counter(), Some(0));
    }

    #[test]
    fn bounds_fail_before_consuming_a_packet_counter() {
        let mut encryption = encryption();
        assert_eq!(
            encryption.encrypt_new_packet(0x01, &mut [0; 4], 1),
            Err(LeAclEncryptionError::BufferTooSmall)
        );
        assert_eq!(
            encryption.encrypt_new_packet(
                0x01,
                &mut [0; LE_ENCRYPTED_PLAINTEXT_BYTES + LE_ACL_MIC_BYTES + 1],
                LE_ENCRYPTED_PLAINTEXT_BYTES + 1,
            ),
            Err(LeAclEncryptionError::PayloadTooLong)
        );
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(0));

        encryption.peripheral_transmit_counter = LeAclPacketCounter(Some(LE_PACKET_COUNTER_MAX));
        let mut final_packet = [0; 1 + LE_ACL_MIC_BYTES];
        encryption
            .encrypt_new_packet(0x01, &mut final_packet, 1)
            .unwrap();
        assert_eq!(encryption.next_peripheral_transmit_counter(), None);
        assert_eq!(
            encryption.encrypt_new_packet(0x01, &mut final_packet, 1),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
    }

    #[test]
    fn exhausted_counters_preserve_empty_and_extent_validation_precedence() {
        let mut encryption = encryption();
        encryption.peripheral_transmit_counter = LeAclPacketCounter(None);
        encryption.central_transmit_counter = LeAclPacketCounter(None);
        let mut storage = [0xa5; 5];
        assert_eq!(encryption.encrypt_new_packet(0x01, &mut storage, 0), Ok(0));
        assert_eq!(encryption.decrypt_new_packet(0x01, &mut []), Ok(0));
        assert_eq!(
            encryption.encrypt_new_packet(0x03, &mut storage, 0),
            Err(LeAclEncryptionError::InvalidEmptyPdu)
        );
        assert_eq!(
            encryption.decrypt_new_packet(0x03, &mut []),
            Err(LeAclEncryptionError::InvalidEmptyPdu)
        );
        assert_eq!(
            encryption.encrypt_new_packet(0x01, &mut storage[..4], 1),
            Err(LeAclEncryptionError::BufferTooSmall)
        );
        assert_eq!(
            encryption.decrypt_new_packet(0x01, &mut storage[..4]),
            Err(LeAclEncryptionError::BufferTooSmall)
        );
        assert_eq!(
            encryption.encrypt_new_packet(0x01, &mut storage, usize::MAX),
            Err(LeAclEncryptionError::PayloadTooLong)
        );
        assert_eq!(
            encryption.encrypt_new_packet(0x01, &mut storage, 1),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
        assert_eq!(
            encryption.decrypt_new_packet(0x01, &mut storage),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
        assert_eq!(storage, [0xa5; 5]);
        assert_eq!(encryption.next_peripheral_transmit_counter(), None);
        assert_eq!(encryption.next_central_transmit_counter(), None);
    }

    #[test]
    fn peripheral_start_procedure_retains_each_response_and_uses_counter_zero() {
        let mut procedure = LePeripheralEncryptionProcedure::new();
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        let encryption_response = procedure.pending_response().unwrap();
        assert_eq!(
            encryption_response.as_bytes(),
            &[
                0x04, 0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02, 0xbe, 0xba, 0xaf, 0xde,
            ]
        );
        assert!(!encryption_response.is_encrypted());

        procedure.response_enqueued().unwrap();
        assert_eq!(procedure.pending_response(), None);
        procedure.observe_transmission_completion(false);
        assert_eq!(procedure.long_term_key_request(), None);
        procedure.observe_transmission_completion(true);
        assert_eq!(
            procedure.long_term_key_request(),
            Some(LeLongTermKeyRequest::new(
                [0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab],
                0x2474
            ))
        );
        assert!(procedure.take_long_term_key_request().is_some());
        assert_eq!(procedure.take_long_term_key_request(), None);

        assert!(
            procedure
                .provide_long_term_key(LeLongTermKey::new(LTK))
                .is_ok()
        );
        let start_request = procedure.pending_response().unwrap();
        assert_eq!(start_request.as_bytes(), &[0x05]);
        assert!(!start_request.is_encrypted());
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);

        let mut central_start_response = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        procedure
            .receive_encrypted_start_response(0x0f, &mut central_start_response)
            .unwrap();
        assert_eq!(central_start_response, [0x06, 0, 0, 0, 0]);
        let peripheral_start_response = procedure.pending_response().unwrap();
        assert_eq!(
            peripheral_start_response.as_bytes(),
            &[0xa3, 0x4c, 0x13, 0xa4, 0x15]
        );
        assert!(peripheral_start_response.is_encrypted());
        procedure.response_enqueued().unwrap();
        assert!(procedure.is_active());
        assert!(procedure.take_encryption_enabled());
        assert!(!procedure.take_encryption_enabled());

        let encryption = procedure.active_encryption().unwrap();
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(1));
        assert_eq!(encryption.next_central_transmit_counter(), Some(1));
    }

    #[test]
    fn authenticated_data_cannot_substitute_for_start_control_response() {
        let mut procedure = LePeripheralEncryptionProcedure::new();
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        assert!(
            procedure
                .provide_long_term_key(LeLongTermKey::new(LTK))
                .is_ok()
        );
        procedure.response_enqueued().unwrap();
        // A valid MIC authenticates the LLID too. A data PDU containing 0x06
        // still cannot complete the three-way control handshake.
        let mut packet = [0x06, 0, 0, 0, 0];
        encrypt_packet(
            &LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(SKD)),
            &LeAclInitializationVector::new(IV),
            0,
            CENTRAL_TO_PERIPHERAL_DIRECTION,
            0x02,
            &mut packet,
            1,
        )
        .unwrap();
        assert_eq!(
            procedure.receive_encrypted_start_response(0x02, &mut packet),
            Err(LePeripheralEncryptionProcedureError::UnexpectedPhysicalChannelPdu)
        );
        assert_eq!(
            procedure.termination(),
            Some(LePeripheralEncryptionTermination::Exit)
        );
        assert!(!procedure.take_encryption_enabled());
    }

    #[test]
    fn peripheral_start_procedure_rejects_a_missing_key_reliably() {
        let mut procedure = LePeripheralEncryptionProcedure::new();
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        procedure.reject_long_term_key().unwrap();

        let rejection = procedure.pending_response().unwrap();
        assert_eq!(rejection.as_bytes(), &[0x0d, 0x06]);
        assert!(!rejection.is_encrypted());
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(false);
        assert!(!procedure.is_idle());
        assert_eq!(procedure.pending_response(), None);
        procedure.observe_transmission_completion(true);
        assert!(procedure.is_idle());
    }

    #[test]
    fn peripheral_start_procedure_fails_closed_on_invalid_transitions_and_mic() {
        let mut procedure = LePeripheralEncryptionProcedure::new();
        assert_eq!(
            procedure.begin(&[0x03], peripheral_random()),
            Err(LePeripheralEncryptionProcedureError::MalformedEncryptionRequest)
        );
        assert!(procedure.is_idle());
        assert_eq!(procedure.termination(), None);
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        assert_eq!(
            procedure.begin(&encryption_request(), peripheral_random()),
            Err(LePeripheralEncryptionProcedureError::Busy)
        );
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        assert!(
            procedure
                .provide_long_term_key(LeLongTermKey::new(LTK))
                .is_ok()
        );
        procedure.response_enqueued().unwrap();

        let mut bad_mic = [0x9f, 0xcd, 0xa7, 0xf4, 0x49];
        assert_eq!(
            procedure.receive_encrypted_start_response(0x0f, &mut bad_mic),
            Err(LePeripheralEncryptionProcedureError::MicFailure)
        );
        assert_eq!(bad_mic, [0; 5]);
        assert!(!procedure.is_active());
        assert_eq!(procedure.pending_response(), None);
        assert_eq!(
            procedure.termination(),
            Some(LePeripheralEncryptionTermination::Exit)
        );
    }

    #[test]
    fn pause_restarts_with_new_material_and_missing_restart_key_requires_termination() {
        let mut procedure = established_procedure();
        let mut pause_request = [0x0a, 0, 0, 0, 0];
        encrypt_packet(
            &LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(SKD)),
            &LeAclInitializationVector::new(IV),
            1,
            CENTRAL_TO_PERIPHERAL_DIRECTION,
            0x0f,
            &mut pause_request,
            1,
        )
        .unwrap();
        procedure.begin_pause(0x0f, &mut pause_request).unwrap();
        assert_eq!(pause_request, [0x0a, 0, 0, 0, 0]);

        let mut expected_pause_response = [0x0b, 0, 0, 0, 0];
        encrypt_packet(
            &LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(SKD)),
            &LeAclInitializationVector::new(IV),
            1,
            PERIPHERAL_TO_CENTRAL_DIRECTION,
            0x03,
            &mut expected_pause_response,
            1,
        )
        .unwrap();
        let pause_response = procedure.pending_response().unwrap();
        assert!(pause_response.is_encrypted());
        assert_eq!(pause_response.as_bytes(), &expected_pause_response);
        procedure.response_enqueued().unwrap();
        assert!(!procedure.is_active());
        procedure
            .receive_unencrypted_pause_response(&[0x0b])
            .unwrap();

        let restart_skdp = [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe];
        let restart_ivp = [0x55, 0xaa, 0x33, 0xcc];
        procedure
            .begin(
                &encryption_request(),
                LePeripheralEncryptionRandom::new(restart_skdp, restart_ivp),
            )
            .unwrap();
        assert_eq!(
            procedure.pending_response().unwrap().as_bytes(),
            &[
                0x04, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x55, 0xaa, 0x33, 0xcc,
            ]
        );
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        assert!(
            procedure
                .provide_long_term_key(LeLongTermKey::new(LTK))
                .is_ok()
        );
        procedure.response_enqueued().unwrap();

        let mut restart_skd = SKD;
        restart_skd[8..].copy_from_slice(&restart_skdp);
        let mut restart_iv = IV;
        restart_iv[4..].copy_from_slice(&restart_ivp);
        let restart_session_key =
            LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(restart_skd));
        let mut central_restart_response = [0x06, 0, 0, 0, 0];
        encrypt_packet(
            &restart_session_key,
            &LeAclInitializationVector::new(restart_iv),
            0,
            CENTRAL_TO_PERIPHERAL_DIRECTION,
            0x0f,
            &mut central_restart_response,
            1,
        )
        .unwrap();
        procedure
            .receive_encrypted_start_response(0x0f, &mut central_restart_response)
            .unwrap();
        procedure.response_enqueued().unwrap();
        assert!(procedure.take_encryption_refreshed());
        assert!(!procedure.take_encryption_refreshed());
        let encryption = procedure.active_encryption().unwrap();
        assert_eq!(encryption.next_peripheral_transmit_counter(), Some(1));
        assert_eq!(encryption.next_central_transmit_counter(), Some(1));

        let mut second_pause_request = [0x0a, 0, 0, 0, 0];
        encrypt_packet(
            &restart_session_key,
            &LeAclInitializationVector::new(restart_iv),
            1,
            CENTRAL_TO_PERIPHERAL_DIRECTION,
            0x0f,
            &mut second_pause_request,
            1,
        )
        .unwrap();
        procedure
            .begin_pause(0x0f, &mut second_pause_request)
            .unwrap();
        procedure.response_enqueued().unwrap();
        procedure
            .receive_unencrypted_pause_response(&[0x0b])
            .unwrap();
        procedure
            .begin(&encryption_request(), peripheral_random())
            .unwrap();
        procedure.response_enqueued().unwrap();
        procedure.observe_transmission_completion(true);
        procedure.reject_long_term_key().unwrap();
        assert_eq!(
            procedure.termination(),
            Some(LePeripheralEncryptionTermination::Terminate(0x06))
        );
        assert_eq!(procedure.pending_response(), None);
        assert!(procedure.blocks_unrelated_transmission());
        assert!(procedure.permits_control_pdu(&[0x02, 0x06]));
        for payload in [
            &[0x0d, 0x06][..],
            &[0x02, 0x13],
            &[0x0c],
            &[0x02, 0x06, 0],
            &[],
        ] {
            assert!(!procedure.permits_control_pdu(payload));
        }
        procedure.observe_transmission_completion(true);
        assert_eq!(
            procedure.termination(),
            Some(LePeripheralEncryptionTermination::Terminate(0x06))
        );
        assert!(!procedure.is_idle());
        assert!(procedure.active_encryption().is_none());
        assert!(!procedure.take_encryption_enabled());
        assert!(!procedure.take_encryption_refreshed());
    }
}
