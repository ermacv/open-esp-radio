//! Portable command classification before chip-specific session execution.

use bt_hci::cmd::Opcode;

use super::bootstrap::{BootstrapCommandDecodeError, invalid_parameters};

use super::le::data_length::LeDataLengthDecodeError;
use super::le::peripheral::{
    LeDisconnectDecodeError, LeReadRemoteFeaturesDecodeError,
    LeReadRemoteVersionInformationDecodeError,
};
use crate::{
    BootstrapCommandCompleteEvent, HciCommandPacket, LeDataLengthCommand,
    LeDataLengthCommandCompleteEvent, LeDisconnectCommand, LeDisconnectCommandStatusEvent,
    LeDtmCommand, LeDtmCommandCompleteEvent, LeLegacyAdvertisingCommand,
    LeLegacyAdvertisingCommandCompleteEvent, LeLegacyAdvertisingCommandKind,
    LeLegacyAdvertisingConfigurationCommand, LeLegacyAdvertisingEnableCommand,
    LeLegacyScanningCommand, LeLegacyScanningCommandCompleteEvent, LeLegacyScanningCommandKind,
    LeLegacyScanningConfigurationCommand, LeLegacyScanningEnableCommand,
    LeLongTermKeyCommandCompleteEvent, LeLongTermKeyCommandDecodeError,
    LeLongTermKeyRequestNegativeReplyCommand, LeLongTermKeyRequestReplyCommand,
    LeReadRemoteFeaturesCommand, LeReadRemoteFeaturesCommandStatusEvent,
    LeReadRemoteVersionInformationCommand, LeReadRemoteVersionInformationCommandStatusEvent,
    OwnedBootstrapCommand, UnknownCommandCompleteEvent,
};

/// One finite result of classifying a validated HCI command packet.
///
/// Bootstrap, DTM and software-only role configuration commands are
/// owned semantic tokens. Malformed responses are complete owned packets that
/// may be retained across Controller-to-Host backpressure. An opcode outside
/// the closed table becomes an owned Unknown Command response, so no
/// classification result borrows receive storage.
#[derive(Debug)]
#[must_use = "a classified HCI command must be routed or answered exactly once"]
pub enum LeControllerCommandClassification {
    /// Standard LE Rand, serviced by entropy the endpoint's owner retains.
    Random(crate::LeRandCommand),
    /// LE Rand had a nonempty parameter body.
    MalformedRandom(crate::LeRandCommandCompleteEvent),
    /// A validated Disconnect command awaits the active connection lifecycle.
    Disconnect(LeDisconnectCommand),
    /// Disconnect had an invalid parameter body.
    MalformedDisconnect(LeDisconnectCommandStatusEvent),
    /// A validated LE Read Remote Features command awaits a live connection.
    ReadRemoteFeatures(LeReadRemoteFeaturesCommand),
    /// LE Read Remote Features had an invalid parameter body.
    MalformedReadRemoteFeatures(LeReadRemoteFeaturesCommandStatusEvent),
    /// A validated Read Remote Version Information command awaits a live connection.
    ReadRemoteVersionInformation(LeReadRemoteVersionInformationCommand),
    /// Read Remote Version Information had an invalid parameter body.
    MalformedReadRemoteVersionInformation(LeReadRemoteVersionInformationCommandStatusEvent),
    /// A validated LE Long Term Key Request Reply awaits the live encryption procedure.
    LongTermKeyReply(LeLongTermKeyRequestReplyCommand),
    /// A validated LE Long Term Key Request Negative Reply awaits the live procedure.
    LongTermKeyNegativeReply(LeLongTermKeyRequestNegativeReplyCommand),
    /// A claimed LTK reply opcode had malformed parameters.
    MalformedLongTermKeyReply(LeLongTermKeyCommandCompleteEvent),
    /// A validated filter accept list command.
    AcceptList(crate::LeAcceptListCommand),
    /// A filter accept list command had an invalid or unsupported parameter body.
    MalformedAcceptList(crate::LeAcceptListCommandCompleteEvent),
    /// A validated Data Length Extension command.
    DataLength(LeDataLengthCommand),
    /// A Data Length Extension command had an invalid parameter body.
    MalformedDataLength(LeDataLengthCommandCompleteEvent),
    /// A decoded bootstrap command awaits session-aware software dispatch.
    Bootstrap(OwnedBootstrapCommand),
    /// A known bootstrap opcode had malformed parameters and produced this response.
    MalformedBootstrap(BootstrapCommandCompleteEvent),
    /// A validated DTM command awaits chip-specific session execution.
    Dtm(LeDtmCommand),
    /// A known DTM opcode had malformed parameters and produced this response.
    MalformedDtm(LeDtmCommandCompleteEvent),
    /// A validated software-only legacy advertising configuration command.
    LegacyAdvertisingConfiguration(LeLegacyAdvertisingConfigurationCommand),
    /// A validated Enable command awaits radio-lifecycle policy.
    LegacyAdvertisingEnable(LeLegacyAdvertisingEnableCommand),
    /// A claimed legacy advertising opcode was malformed.
    MalformedLegacyAdvertising(LeLegacyAdvertisingCommandCompleteEvent),
    /// A validated software-only legacy scanning configuration command.
    LegacyScanningConfiguration(LeLegacyScanningConfigurationCommand),
    /// A validated Set Scan Enable command awaits radio-lifecycle policy.
    LegacyScanningEnable(LeLegacyScanningEnableCommand),
    /// A claimed legacy scanning opcode was malformed or unsupported.
    MalformedLegacyScanning(LeLegacyScanningCommandCompleteEvent),
    /// This closed Controller command table did not claim the opcode.
    Unsupported(UnknownCommandCompleteEvent),
}

impl LeControllerCommandClassification {
    /// Opcode retained by this exact classification result.
    pub const fn opcode(&self) -> Opcode {
        match self {
            Self::Random(_) | Self::MalformedRandom(_) => crate::LeRandCommand::OPCODE,
            Self::Disconnect(_) | Self::MalformedDisconnect(_) => LeDisconnectCommand::OPCODE,
            Self::ReadRemoteFeatures(_) | Self::MalformedReadRemoteFeatures(_) => {
                LeReadRemoteFeaturesCommand::OPCODE
            }
            Self::ReadRemoteVersionInformation(_)
            | Self::MalformedReadRemoteVersionInformation(_) => {
                LeReadRemoteVersionInformationCommand::OPCODE
            }
            Self::LongTermKeyReply(_) => LeLongTermKeyRequestReplyCommand::OPCODE,
            Self::LongTermKeyNegativeReply(_) => LeLongTermKeyRequestNegativeReplyCommand::OPCODE,
            Self::MalformedLongTermKeyReply(response) => response.opcode(),
            Self::AcceptList(command) => command.opcode(),
            Self::MalformedAcceptList(response) => response.opcode(),
            Self::DataLength(command) => command.opcode(),
            Self::MalformedDataLength(response) => response.opcode(),
            Self::Bootstrap(command) => command.opcode(),
            Self::MalformedBootstrap(response) => response.opcode(),
            Self::Dtm(command) => command.kind().opcode(),
            Self::MalformedDtm(response) => response.opcode(),
            Self::LegacyAdvertisingConfiguration(command) => command.kind().opcode(),
            Self::LegacyAdvertisingEnable(_) => LeLegacyAdvertisingCommandKind::SetEnable.opcode(),
            Self::MalformedLegacyAdvertising(response) => response.opcode(),
            Self::LegacyScanningConfiguration(_) => {
                LeLegacyScanningCommandKind::SetParameters.opcode()
            }
            Self::LegacyScanningEnable(_) => LeLegacyScanningCommandKind::SetEnable.opcode(),
            Self::MalformedLegacyScanning(response) => response.opcode(),
            Self::Unsupported(response) => response.opcode(),
        }
    }
}

/// Classify one validated command without executing radio or Link-Layer work.
///
/// DTM decoding runs first because it distinguishes malformed input for its
/// known opcodes from an opcode outside that command family. The latter
/// is decoded only when it belongs to the closed bootstrap table; otherwise
/// classification builds its terminal owned Unknown Command response.
/// Classification never observes or advances a bootstrap epoch. No branch
/// waits, publishes a response, or claims hardware readiness.
pub fn classify_le_controller_command(
    command: HciCommandPacket<'_>,
) -> LeControllerCommandClassification {
    if command.opcode() == crate::LeRandCommand::OPCODE {
        return if command.parameters().is_empty() {
            LeControllerCommandClassification::Random(crate::LeRandCommand(()))
        } else {
            LeControllerCommandClassification::MalformedRandom(
                crate::LeRandCommandCompleteEvent::error(
                    bt_hci::param::Error::INVALID_HCI_PARAMETERS,
                ),
            )
        };
    }
    match LeDisconnectCommand::decode(command) {
        Ok(command) => return LeControllerCommandClassification::Disconnect(command),
        Err(LeDisconnectDecodeError::Malformed) => {
            return LeControllerCommandClassification::MalformedDisconnect(
                LeDisconnectCommandStatusEvent::invalid_parameters(),
            );
        }
        Err(LeDisconnectDecodeError::Unsupported) => {}
    }

    match crate::LeAcceptListCommand::decode(command) {
        Ok(command) => return LeControllerCommandClassification::AcceptList(command),
        Err(crate::controller::le::accept_list::LeAcceptListDecodeError::Malformed(response)) => {
            return LeControllerCommandClassification::MalformedAcceptList(response);
        }
        Err(crate::controller::le::accept_list::LeAcceptListDecodeError::Unsupported) => {}
    }

    match LeDataLengthCommand::decode(command) {
        Ok(command) => return LeControllerCommandClassification::DataLength(command),
        Err(LeDataLengthDecodeError::Malformed(response)) => {
            return LeControllerCommandClassification::MalformedDataLength(response);
        }
        Err(LeDataLengthDecodeError::Unsupported) => {}
    }

    match LeReadRemoteFeaturesCommand::decode(command) {
        Ok(command) => return LeControllerCommandClassification::ReadRemoteFeatures(command),
        Err(LeReadRemoteFeaturesDecodeError::Malformed) => {
            return LeControllerCommandClassification::MalformedReadRemoteFeatures(
                LeReadRemoteFeaturesCommandStatusEvent::invalid_parameters(),
            );
        }
        Err(LeReadRemoteFeaturesDecodeError::Unsupported) => {}
    }

    match LeReadRemoteVersionInformationCommand::decode(command) {
        Ok(command) => {
            return LeControllerCommandClassification::ReadRemoteVersionInformation(command);
        }
        Err(LeReadRemoteVersionInformationDecodeError::Malformed) => {
            return LeControllerCommandClassification::MalformedReadRemoteVersionInformation(
                LeReadRemoteVersionInformationCommandStatusEvent::invalid_parameters(),
            );
        }
        Err(LeReadRemoteVersionInformationDecodeError::Unsupported) => {}
    }

    match LeLongTermKeyRequestReplyCommand::decode(command) {
        Ok(command) => return LeControllerCommandClassification::LongTermKeyReply(command),
        Err(LeLongTermKeyCommandDecodeError::Malformed) => {
            return LeControllerCommandClassification::MalformedLongTermKeyReply(
                LeLongTermKeyCommandCompleteEvent::invalid_parameters(
                    LeLongTermKeyRequestReplyCommand::OPCODE,
                ),
            );
        }
        Err(LeLongTermKeyCommandDecodeError::Unsupported) => {}
    }

    match LeLongTermKeyRequestNegativeReplyCommand::decode(command) {
        Ok(command) => {
            return LeControllerCommandClassification::LongTermKeyNegativeReply(command);
        }
        Err(LeLongTermKeyCommandDecodeError::Malformed) => {
            return LeControllerCommandClassification::MalformedLongTermKeyReply(
                LeLongTermKeyCommandCompleteEvent::invalid_parameters(
                    LeLongTermKeyRequestNegativeReplyCommand::OPCODE,
                ),
            );
        }
        Err(LeLongTermKeyCommandDecodeError::Unsupported) => {}
    }

    if LeLegacyAdvertisingCommandKind::from_opcode(command.opcode()).is_some() {
        return match LeLegacyAdvertisingCommand::decode(command) {
            Ok(command) => match LeLegacyAdvertisingConfigurationCommand::from_command(command) {
                Ok(command) => {
                    LeControllerCommandClassification::LegacyAdvertisingConfiguration(command)
                }
                Err(command) => LeControllerCommandClassification::LegacyAdvertisingEnable(
                    LeLegacyAdvertisingEnableCommand::from_command(command)
                        .expect("the non-configuration advertising command is Enable"),
                ),
            },
            Err(error) => LeControllerCommandClassification::MalformedLegacyAdvertising(
                error
                    .into_command_complete()
                    .expect("a claimed advertising opcode must build an exact completion"),
            ),
        };
    }

    if LeLegacyScanningCommandKind::from_opcode(command.opcode()).is_some() {
        return match LeLegacyScanningCommand::decode(command) {
            Ok(command) => match LeLegacyScanningConfigurationCommand::from_command(command) {
                Ok(command) => {
                    LeControllerCommandClassification::LegacyScanningConfiguration(command)
                }
                Err(LeLegacyScanningCommand::SetEnable(command)) => {
                    LeControllerCommandClassification::LegacyScanningEnable(command)
                }
                Err(LeLegacyScanningCommand::SetParameters(_)) => {
                    unreachable!("Set Parameters refines into scanning configuration")
                }
            },
            Err(error) => LeControllerCommandClassification::MalformedLegacyScanning(
                error
                    .into_command_complete()
                    .expect("a claimed scanning opcode must build an exact completion"),
            ),
        };
    }

    match LeDtmCommand::decode(command) {
        Ok(command) => LeControllerCommandClassification::Dtm(command),
        Err(error) => match error.into_command_complete() {
            Ok(response) => LeControllerCommandClassification::MalformedDtm(response),
            Err(_) => match OwnedBootstrapCommand::decode(command) {
                Ok(command) => LeControllerCommandClassification::Bootstrap(command),
                Err(BootstrapCommandDecodeError::Malformed(kind)) => {
                    LeControllerCommandClassification::MalformedBootstrap(invalid_parameters(
                        kind.opcode(),
                    ))
                }
                Err(BootstrapCommandDecodeError::Unsupported) => {
                    LeControllerCommandClassification::Unsupported(
                        UnknownCommandCompleteEvent::new(command.opcode()),
                    )
                }
            },
        },
    }
}

#[cfg(test)]
mod tests;
