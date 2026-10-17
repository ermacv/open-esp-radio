//! Executor-neutral HCI codec for the supported LE Direct Test Mode commands.
//!
//! This module stops at the semantic HCI boundary. Decoding a command does not
//! start a radio, reserve a scheduler item, prove controller readiness or imply
//! that a test reached hardware. Consuming one of the typed command tokens to
//! build Command Complete only records a status supplied by the controller
//! owner after its own state transition has finished.

use bt_hci::{
    PacketKind,
    cmd::{Opcode, OpcodeGroup},
    param::{Error as HciError, Status},
};

use crate::{HciCommandPacket, HciControllerResponse};

/// Largest complete HCI Event body emitted for the supported DTM commands.
///
/// LE Test End returns Status followed by the two-octet packet count, making
/// its Command Complete event two octets larger than the start-test responses.
pub const LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 8;

/// HCI LE Receiver Test v1 command opcode.
pub const LE_RECEIVER_TEST_V1_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001d);
/// HCI LE Transmitter Test v1 command opcode.
pub const LE_TRANSMITTER_TEST_V1_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
/// HCI LE Test End command opcode.
pub const LE_TEST_END_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);
/// HCI LE Receiver Test v2 command opcode.
pub const LE_RECEIVER_TEST_V2_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x0033);
/// HCI LE Transmitter Test v2 command opcode.
pub const LE_TRANSMITTER_TEST_V2_OPCODE: Opcode = Opcode::new(OpcodeGroup::LE, 0x0034);

/// One RF channel in the legacy LE test-channel domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDtmChannel(u8);

impl LeDtmChannel {
    const LAST: u8 = 39;

    /// Decode the HCI channel parameter without assigning it to a radio.
    pub const fn from_hci_parameter(channel: u8) -> Option<Self> {
        if channel <= Self::LAST {
            Some(Self(channel))
        } else {
            None
        }
    }

    /// Return the semantic LE test-channel index.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Payload pattern selected by a supported LE Transmitter Test command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LeDtmPayloadPattern {
    /// PRBS9.
    Prbs9 = 0,
    /// Repeated `11110000` in transmission order.
    Repeated11110000 = 1,
    /// Repeated `10101010` in transmission order.
    Repeated10101010 = 2,
    /// PRBS15.
    Prbs15 = 3,
    /// Repeated `11111111`.
    RepeatedAllOnes = 4,
    /// Repeated `00000000`.
    RepeatedAllZeros = 5,
    /// Repeated `00001111` in transmission order.
    Repeated00001111 = 6,
    /// Repeated `01010101` in transmission order.
    Repeated01010101 = 7,
}

impl LeDtmPayloadPattern {
    /// Decode the complete standard HCI selector domain.
    pub const fn from_hci_parameter(selector: u8) -> Option<Self> {
        match selector {
            0 => Some(Self::Prbs9),
            1 => Some(Self::Repeated11110000),
            2 => Some(Self::Repeated10101010),
            3 => Some(Self::Prbs15),
            4 => Some(Self::RepeatedAllOnes),
            5 => Some(Self::RepeatedAllZeros),
            6 => Some(Self::Repeated00001111),
            7 => Some(Self::Repeated01010101),
            _ => None,
        }
    }

    /// Return the HCI selector value.
    pub const fn hci_parameter(self) -> u8 {
        self as u8
    }
}

/// PHY selected by a normalized LE DTM start command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LeDtmPhy {
    /// LE 1M PHY.
    Le1M = 1,
    /// LE 2M PHY.
    Le2M = 2,
    /// Generic LE Coded receiver or S=8 transmitter.
    LeCoded = 3,
    /// LE Coded S=2 transmitter-only selection.
    LeCodedS2 = 4,
}

impl LeDtmPhy {
    const fn from_receiver_parameter(selector: u8) -> Option<Self> {
        match selector {
            1 => Some(Self::Le1M),
            2 => Some(Self::Le2M),
            3 => Some(Self::LeCoded),
            _ => None,
        }
    }

    const fn from_transmitter_parameter(selector: u8) -> Option<Self> {
        match selector {
            1 => Some(Self::Le1M),
            2 => Some(Self::Le2M),
            3 => Some(Self::LeCoded),
            4 => Some(Self::LeCodedS2),
            _ => None,
        }
    }

    /// Return the standard HCI selector.
    pub const fn hci_parameter(self) -> u8 {
        self as u8
    }
}

/// Expected modulation-index class for an LE Receiver Test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmModulationIndex {
    /// The transmitter uses the standard modulation-index requirements.
    Standard,
    /// The transmitter uses the stable modulation-index requirements.
    Stable,
}

impl LeDtmModulationIndex {
    const fn from_hci_parameter(parameter: u8) -> Option<Self> {
        match parameter {
            0 => Some(Self::Standard),
            1 => Some(Self::Stable),
            _ => None,
        }
    }
}

/// A decoded command in the closed supported LE DTM HCI subset.
#[derive(Debug, Eq, PartialEq)]
pub enum LeDtmCommand {
    /// Begin a normalized receiver test.
    ReceiverTest(LeReceiverTestCommand),
    /// Begin a normalized transmitter test.
    TransmitterTest(LeTransmitterTestCommand),
    /// End the active LE test.
    TestEnd(LeTestEndCommand),
}

impl LeDtmCommand {
    /// Decode one semantic command already admitted by the HCI packet boundary.
    pub fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeDtmCommandDecodeError> {
        Self::decode_body(command.opcode(), command.parameters())
    }

    /// Decode an opcode and the complete command-parameter body.
    ///
    /// This entry point is useful for Controller owners which already split the HCI
    /// command header and for focused malformed-body tests. It applies the same
    /// closed opcode and exact-length policy as [`Self::decode`].
    pub fn decode_body(opcode: Opcode, parameters: &[u8]) -> Result<Self, LeDtmCommandDecodeError> {
        if opcode == LE_RECEIVER_TEST_V1_OPCODE {
            require_parameter_length(LeDtmCommandKind::ReceiverTestV1, parameters, 1)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::ReceiverTestV1,
                    channel: parameters[0],
                },
            )?;
            Ok(Self::ReceiverTest(LeReceiverTestCommand {
                kind: LeDtmCommandKind::ReceiverTestV1,
                channel,
                phy: LeDtmPhy::Le1M,
                modulation_index: LeDtmModulationIndex::Standard,
            }))
        } else if opcode == LE_TRANSMITTER_TEST_V1_OPCODE {
            require_parameter_length(LeDtmCommandKind::TransmitterTestV1, parameters, 3)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::TransmitterTestV1,
                    channel: parameters[0],
                },
            )?;
            let pattern = LeDtmPayloadPattern::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPayloadPattern {
                    command: LeDtmCommandKind::TransmitterTestV1,
                    selector: parameters[2],
                },
            )?;
            Ok(Self::TransmitterTest(LeTransmitterTestCommand {
                kind: LeDtmCommandKind::TransmitterTestV1,
                channel,
                payload_length: parameters[1],
                pattern,
                phy: LeDtmPhy::Le1M,
            }))
        } else if opcode == LE_RECEIVER_TEST_V2_OPCODE {
            require_parameter_length(LeDtmCommandKind::ReceiverTestV2, parameters, 3)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    channel: parameters[0],
                },
            )?;
            let phy = LeDtmPhy::from_receiver_parameter(parameters[1]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPhy {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    selector: parameters[1],
                },
            )?;
            let modulation_index = LeDtmModulationIndex::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedModulationIndex {
                    command: LeDtmCommandKind::ReceiverTestV2,
                    parameter: parameters[2],
                },
            )?;
            Ok(Self::ReceiverTest(LeReceiverTestCommand {
                kind: LeDtmCommandKind::ReceiverTestV2,
                channel,
                phy,
                modulation_index,
            }))
        } else if opcode == LE_TRANSMITTER_TEST_V2_OPCODE {
            require_parameter_length(LeDtmCommandKind::TransmitterTestV2, parameters, 4)?;
            let channel = LeDtmChannel::from_hci_parameter(parameters[0]).ok_or(
                LeDtmCommandDecodeError::ChannelOutsideTestDomain {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    channel: parameters[0],
                },
            )?;
            let pattern = LeDtmPayloadPattern::from_hci_parameter(parameters[2]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPayloadPattern {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    selector: parameters[2],
                },
            )?;
            let phy = LeDtmPhy::from_transmitter_parameter(parameters[3]).ok_or(
                LeDtmCommandDecodeError::UnsupportedPhy {
                    command: LeDtmCommandKind::TransmitterTestV2,
                    selector: parameters[3],
                },
            )?;
            Ok(Self::TransmitterTest(LeTransmitterTestCommand {
                kind: LeDtmCommandKind::TransmitterTestV2,
                channel,
                payload_length: parameters[1],
                pattern,
                phy,
            }))
        } else if opcode == LE_TEST_END_OPCODE {
            require_parameter_length(LeDtmCommandKind::TestEnd, parameters, 0)?;
            Ok(Self::TestEnd(LeTestEndCommand { private: () }))
        } else {
            Err(LeDtmCommandDecodeError::UnsupportedOpcode { opcode })
        }
    }

    /// Kind of this decoded command without inspecting its parameters.
    pub const fn kind(&self) -> LeDtmCommandKind {
        match self {
            Self::ReceiverTest(command) => command.kind(),
            Self::TransmitterTest(command) => command.kind(),
            Self::TestEnd(_) => LeDtmCommandKind::TestEnd,
        }
    }

    /// Apply the reviewed command policy for an idle DTM session.
    ///
    /// A start command remains an owned semantic request for the hardware
    /// runner. Test End succeeds without starting or stopping hardware and
    /// reports the idle session's zero packet count. The policy exposes no
    /// caller-selected HCI status.
    pub fn into_idle_session_disposition(self) -> LeDtmIdleSessionDisposition {
        match self {
            Self::ReceiverTest(command) => LeDtmIdleSessionDisposition::StartReceiver(command),
            Self::TransmitterTest(command) => {
                LeDtmIdleSessionDisposition::StartTransmitter(command)
            }
            Self::TestEnd(command) => {
                LeDtmIdleSessionDisposition::CompleteNoTest(command.into_ended_command_complete(0))
            }
        }
    }

    /// Apply the reviewed command policy for an active DTM session.
    ///
    /// Test End remains owned until the chip-specific runner has quiesced the
    /// active graph and obtained its terminal packet count. A second receiver
    /// or transmitter start is rejected as Controller Busy without replacing
    /// or mutating that active session.
    pub fn into_active_session_disposition(self) -> LeDtmActiveSessionDisposition {
        match self {
            Self::ReceiverTest(command) => LeDtmActiveSessionDisposition::RejectControllerBusy(
                LeDtmCommandCompleteEvent::without_return_parameters(
                    command.opcode(),
                    HciError::CONTROLLER_BUSY.to_status(),
                ),
            ),
            Self::TransmitterTest(command) => LeDtmActiveSessionDisposition::RejectControllerBusy(
                LeDtmCommandCompleteEvent::without_return_parameters(
                    command.opcode(),
                    HciError::CONTROLLER_BUSY.to_status(),
                ),
            ),
            Self::TestEnd(command) => LeDtmActiveSessionDisposition::End(command),
        }
    }
}

/// Reviewed outcome of routing one DTM command while no test is active.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the idle DTM command must start hardware or publish its response"]
pub enum LeDtmIdleSessionDisposition {
    /// Start one receiver test through the chip-specific hardware runner.
    StartReceiver(LeReceiverTestCommand),
    /// Start one transmitter test through the chip-specific hardware runner.
    StartTransmitter(LeTransmitterTestCommand),
    /// Publish the successful zero-count response for Test End without a test.
    CompleteNoTest(LeDtmCommandCompleteEvent),
}

/// Reviewed outcome of routing one DTM command while a test is active.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the active DTM command must enter Test End or publish its rejection"]
pub enum LeDtmActiveSessionDisposition {
    /// Quiesce the active hardware owner before completing this Test End.
    End(LeTestEndCommand),
    /// Publish the fixed Controller Busy response while retaining the session.
    RejectControllerBusy(LeDtmCommandCompleteEvent),
}

/// Closed command identity used in decode diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmCommandKind {
    /// LE Receiver Test v1.
    ReceiverTestV1,
    /// LE Transmitter Test v1.
    TransmitterTestV1,
    /// LE Receiver Test v2.
    ReceiverTestV2,
    /// LE Transmitter Test v2.
    TransmitterTestV2,
    /// LE Test End.
    TestEnd,
}

impl LeDtmCommandKind {
    /// Exact HCI opcode represented by this semantic command kind.
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::ReceiverTestV1 => LE_RECEIVER_TEST_V1_OPCODE,
            Self::TransmitterTestV1 => LE_TRANSMITTER_TEST_V1_OPCODE,
            Self::ReceiverTestV2 => LE_RECEIVER_TEST_V2_OPCODE,
            Self::TransmitterTestV2 => LE_TRANSMITTER_TEST_V2_OPCODE,
            Self::TestEnd => LE_TEST_END_OPCODE,
        }
    }
}

/// Why a semantic LE DTM command body was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDtmCommandDecodeError {
    /// The opcode is outside the supported DTM command codec.
    UnsupportedOpcode {
        /// Unrecognized opcode.
        opcode: Opcode,
    },
    /// A known command did not contain its complete exact parameter body.
    InvalidParameterLength {
        /// Command whose body was malformed.
        command: LeDtmCommandKind,
        /// Required body length.
        expected: usize,
        /// Supplied body length.
        actual: usize,
    },
    /// The channel parameter is reserved rather than an LE test channel.
    ChannelOutsideTestDomain {
        /// Known command whose channel was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI channel value.
        channel: u8,
    },
    /// The transmitter pattern selector is outside the standard domain.
    UnsupportedPayloadPattern {
        /// Known command whose payload selector was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI selector value.
        selector: u8,
    },
    /// The PHY selector is reserved for this command version or role.
    UnsupportedPhy {
        /// Known command whose PHY selector was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI selector value.
        selector: u8,
    },
    /// The receiver modulation-index parameter is reserved.
    UnsupportedModulationIndex {
        /// Known command whose parameter was rejected.
        command: LeDtmCommandKind,
        /// Rejected HCI parameter value.
        parameter: u8,
    },
}

impl LeDtmCommandDecodeError {
    /// Convert rejected input for a known DTM opcode into its required
    /// command completion.
    ///
    /// An unsupported opcode remains owned by the error so the outer Controller
    /// router can dispatch another command family or return Unknown HCI Command.
    pub fn into_command_complete(self) -> Result<LeDtmCommandCompleteEvent, Self> {
        let (command, status) = match self {
            Self::UnsupportedOpcode { .. } => return Err(self),
            Self::InvalidParameterLength { command, .. }
            | Self::ChannelOutsideTestDomain { command, .. }
            | Self::UnsupportedPayloadPattern { command, .. }
            | Self::UnsupportedModulationIndex { command, .. } => {
                (command, HciError::INVALID_HCI_PARAMETERS.to_status())
            }
            Self::UnsupportedPhy { command, .. } => (command, HciError::UNSUPPORTED.to_status()),
        };
        Ok(LeDtmCommandCompleteEvent::without_return_parameters(
            command.opcode(),
            status,
        ))
    }
}

/// Normalized validated LE Receiver Test command awaiting controller execution.
#[derive(Debug, Eq, PartialEq)]
pub struct LeReceiverTestCommand {
    kind: LeDtmCommandKind,
    channel: LeDtmChannel,
    phy: LeDtmPhy,
    modulation_index: LeDtmModulationIndex,
}

impl LeReceiverTestCommand {
    const fn kind(&self) -> LeDtmCommandKind {
        self.kind
    }

    /// Exact HCI command version retained for response identity.
    pub const fn opcode(&self) -> Opcode {
        self.kind.opcode()
    }

    /// Requested LE test channel.
    pub const fn channel(&self) -> LeDtmChannel {
        self.channel
    }

    /// Requested receiver PHY.
    pub const fn phy(&self) -> LeDtmPhy {
        self.phy
    }

    /// Expected transmitter modulation-index class.
    pub const fn modulation_index(&self) -> LeDtmModulationIndex {
        self.modulation_index
    }

    /// Consume the command after its first receiver event reached hardware.
    pub fn into_started_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(self.kind.opcode(), Status::SUCCESS)
    }

    /// Command Complete reporting Hardware Failure.
    pub fn into_hardware_failure_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }

    /// Command Complete reporting Command Disallowed while the radio is unavailable.
    pub fn into_radio_unavailable_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::CMD_DISALLOWED.to_status(),
        )
    }
}

/// Normalized validated LE Transmitter Test command awaiting controller execution.
#[derive(Debug, Eq, PartialEq)]
pub struct LeTransmitterTestCommand {
    kind: LeDtmCommandKind,
    channel: LeDtmChannel,
    payload_length: u8,
    pattern: LeDtmPayloadPattern,
    phy: LeDtmPhy,
}

impl LeTransmitterTestCommand {
    const fn kind(&self) -> LeDtmCommandKind {
        self.kind
    }

    /// Exact HCI command version retained for response identity.
    pub const fn opcode(&self) -> Opcode {
        self.kind.opcode()
    }

    /// Requested LE test channel.
    pub const fn channel(&self) -> LeDtmChannel {
        self.channel
    }

    /// Complete eight-bit HCI payload-length parameter.
    pub const fn payload_length(&self) -> u8 {
        self.payload_length
    }

    /// Requested standard payload pattern.
    pub const fn payload_pattern(&self) -> LeDtmPayloadPattern {
        self.pattern
    }

    /// Requested transmitter PHY.
    pub const fn phy(&self) -> LeDtmPhy {
        self.phy
    }

    /// Consume the command after its first transmitter event reached hardware.
    pub fn into_started_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(self.kind.opcode(), Status::SUCCESS)
    }

    /// Command Complete reporting Hardware Failure.
    pub fn into_hardware_failure_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::HARDWARE_FAILURE.to_status(),
        )
    }

    /// Command Complete reporting Command Disallowed while the radio is unavailable.
    pub fn into_radio_unavailable_command_complete(self) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::without_return_parameters(
            self.kind.opcode(),
            HciError::CMD_DISALLOWED.to_status(),
        )
    }
}

/// Validated LE Test End command awaiting the controller's terminal report.
#[derive(Debug, Eq, PartialEq)]
pub struct LeTestEndCommand {
    private: (),
}

impl LeTestEndCommand {
    /// Consume the pending command after the controller has ended the test.
    ///
    /// `packet_count` is deliberately not inferred here. The chip-specific
    /// terminal owner supplies zero for a transmitter test or the accumulated
    /// accepted count for a receiver test. This codec only serializes that
    /// semantic report in HCI little-endian order.
    pub fn into_ended_command_complete(self, packet_count: u16) -> LeDtmCommandCompleteEvent {
        LeDtmCommandCompleteEvent::with_packet_count(
            LE_TEST_END_OPCODE,
            Status::SUCCESS,
            packet_count,
        )
    }
}

/// Complete Command Complete HCI Event built after a DTM state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDtmCommandCompleteEvent {
    bytes: [u8; LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY],
    length: usize,
    opcode: Opcode,
    status: Status,
    packet_count: Option<u16>,
}

impl LeDtmCommandCompleteEvent {
    /// Command Complete carrying only `status`, as a test start or a refused
    /// command returns.
    pub fn without_return_parameters(opcode: Opcode, status: Status) -> Self {
        let mut event = Self::header(opcode, status, 0);
        event.length = 6;
        event
    }

    fn with_packet_count(opcode: Opcode, status: Status, packet_count: u16) -> Self {
        let mut event = Self::header(opcode, status, 2);
        event.bytes[6..8].copy_from_slice(&packet_count.to_le_bytes());
        event.length = 8;
        event.packet_count = Some(packet_count);
        event
    }

    fn header(opcode: Opcode, status: Status, return_parameter_length: u8) -> Self {
        let mut bytes = [0; LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY];
        bytes[0] = 0x0e;
        bytes[1] = 4 + return_parameter_length;
        bytes[2] = 1;
        bytes[3..5].copy_from_slice(&opcode.to_raw().to_le_bytes());
        bytes[5] = status.into_inner();
        Self {
            bytes,
            length: 0,
            opcode,
            status,
            packet_count: None,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Opcode copied from the consumed typed command token.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Controller-supplied completion status.
    pub const fn status(&self) -> Status {
        self.status
    }

    /// Test End packet count, absent for Receiver/Transmitter Test start.
    pub const fn packet_count(&self) -> Option<u16> {
        self.packet_count
    }
}

impl HciControllerResponse for LeDtmCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

fn require_parameter_length(
    command: LeDtmCommandKind,
    parameters: &[u8],
    expected: usize,
) -> Result<(), LeDtmCommandDecodeError> {
    if parameters.len() == expected {
        Ok(())
    } else {
        Err(LeDtmCommandDecodeError::InvalidParameterLength {
            command,
            expected,
            actual: parameters.len(),
        })
    }
}

#[cfg(test)]
mod tests;
