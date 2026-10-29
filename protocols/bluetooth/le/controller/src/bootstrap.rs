//! Reset-scoped Host configuration and bootstrap command admission over the
//! HCI bootstrap codecs.

use oer_bluetooth_hci::{
    BdAddr, ControllerToHostFlowControl, Error as HciError, EventMask, LeEventMask,
};
use oer_bluetooth_hci::{
    BootstrapCommandCompleteEvent, HciBuildError, LeControllerBootstrapConfig,
    OwnedBootstrapCommand, default_event_mask, default_le_event_mask,
    le_controller_supported_commands,
};

/// HCI bootstrap lifecycle relative to the mandatory Reset command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapPhase {
    /// No valid Reset has established a fresh Host configuration epoch.
    AwaitingReset,
    /// Bootstrap commands may configure the current software HCI epoch.
    Configuring,
}

/// Host buffer declaration accepted for the LE-only initial profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapHostBuffers {
    /// Maximum Controller-to-Host ACL packet length offered by the Host.
    pub acl_data_packet_length: u16,
    /// Number of Controller-to-Host ACL packet slots offered by the Host.
    pub total_acl_data_packets: u16,
}

/// Pure software state for the conservative initial LE HCI command subset.
///
/// Successful setters update requested Host policy only. No field in this type
/// means that a mask, address, buffer or flow-control mode has reached the
/// ESP32-S31 Controller, Link Layer or radio.
pub struct LeControllerBootstrap {
    config: LeControllerBootstrapConfig,
    phase: BootstrapPhase,
    event_mask: EventMask,
    le_event_mask: LeEventMask,
    requested_random_address: Option<BdAddr>,
    host_buffers: Option<BootstrapHostBuffers>,
    controller_to_host_flow_control: ControllerToHostFlowControl,
}

impl LeControllerBootstrap {
    /// Construct cold bootstrap state which accepts only a valid Reset first.
    pub fn new(config: LeControllerBootstrapConfig) -> Result<Self, HciBuildError> {
        Ok(Self {
            config,
            phase: BootstrapPhase::AwaitingReset,
            event_mask: default_event_mask()?,
            le_event_mask: default_le_event_mask()?,
            requested_random_address: None,
            host_buffers: None,
            controller_to_host_flow_control: ControllerToHostFlowControl::Off,
        })
    }

    /// Immutable values reported to the Host.
    pub const fn config(&self) -> LeControllerBootstrapConfig {
        self.config
    }

    /// Current Reset/configuration phase.
    pub const fn phase(&self) -> BootstrapPhase {
        self.phase
    }

    /// Whether no successful Reset has opened this bootstrap epoch.
    ///
    /// Every mutating command is rejected before Reset, so this phase also
    /// proves that no event mask, random address, Host buffer declaration or
    /// flow-control request has been accepted.
    pub const fn is_pristine(&self) -> bool {
        matches!(self.phase, BootstrapPhase::AwaitingReset)
    }

    /// Requested base HCI event mask in the current epoch.
    pub const fn event_mask(&self) -> EventMask {
        self.event_mask
    }

    /// Requested LE Meta event mask in the current epoch.
    pub const fn le_event_mask(&self) -> LeEventMask {
        self.le_event_mask
    }

    /// Requested random address, not a hardware-applied address.
    pub const fn requested_random_address(&self) -> Option<BdAddr> {
        self.requested_random_address
    }

    /// Host buffers declared for future Controller-to-Host ACL flow control.
    pub const fn host_buffers(&self) -> Option<BootstrapHostBuffers> {
        self.host_buffers
    }

    /// Requested Controller-to-Host flow-control mode.
    pub const fn controller_to_host_flow_control(&self) -> ControllerToHostFlowControl {
        self.controller_to_host_flow_control
    }

    /// Consume one classified bootstrap command.
    ///
    /// The Controller decides when this runs: Reset waits until its radio
    /// work has quiesced, and the random address is refused while advertising
    /// or scanning is enabled. `has_random_source` reports LE Rand in the
    /// supported-commands bitmap.
    pub fn dispatch(
        &mut self,
        command: OwnedBootstrapCommand,
        has_random_source: bool,
    ) -> Result<BootstrapCommandCompleteEvent, HciBuildError> {
        let opcode = command.opcode();
        if !command.is_reset() && self.phase == BootstrapPhase::AwaitingReset {
            return Ok(BootstrapCommandCompleteEvent::error(
                opcode,
                HciError::CMD_DISALLOWED,
            ));
        }

        Ok(match command {
            OwnedBootstrapCommand::Reset => {
                self.reset_epoch()?;
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::SetEventMask(mask) => {
                self.event_mask = mask;
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::SetControllerToHostFlowControl(mode) => {
                if !matches!(
                    mode,
                    ControllerToHostFlowControl::Off | ControllerToHostFlowControl::AclOnSyncOff
                ) {
                    return Ok(BootstrapCommandCompleteEvent::error(
                        opcode,
                        HciError::UNSUPPORTED,
                    ));
                }
                self.controller_to_host_flow_control = mode;
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::HostBufferSize {
                acl_data_packet_length,
                total_acl_data_packets,
            } => {
                self.host_buffers = Some(BootstrapHostBuffers {
                    acl_data_packet_length: acl_data_packet_length.get(),
                    total_acl_data_packets: total_acl_data_packets.get(),
                });
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::ReadBdAddr => BootstrapCommandCompleteEvent::success(
                opcode,
                &self.config.public_address().hci_wire_address().into_inner(),
            ),
            OwnedBootstrapCommand::ReadLocalSupportedCommands => {
                let mut commands = le_controller_supported_commands();
                if has_random_source {
                    commands[27] |= 1 << 7; // LE Rand.
                }
                if self.config.filter_accept_list_size() > 0 {
                    // LE Clear, Add Device To and Remove Device From Filter
                    // Accept List.
                    commands[26] |= 1 << 7;
                    commands[27] |= (1 << 0) | (1 << 1);
                }
                BootstrapCommandCompleteEvent::success(opcode, &commands)
            }
            OwnedBootstrapCommand::LeSetEventMask(mask) => {
                self.le_event_mask = mask;
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadBufferSize => {
                let [low, high] = self.config.le_acl_data_packet_length().to_le_bytes();
                let response = [low, high, self.config.total_num_le_acl_data_packets()];
                BootstrapCommandCompleteEvent::success(opcode, &response)
            }
            OwnedBootstrapCommand::LeReadLocalSupportedFeatures => {
                // Bit 0 is LE Encryption, bit 3 is Peripheral-initiated Feature
                // Exchange, bit 4 is LE Ping, bit 5 is LE Data Packet Length
                // Extension and bit 14 is CSA #2.
                BootstrapCommandCompleteEvent::success(
                    opcode,
                    &[
                        (1 << 0) | (1 << 3) | (1 << 4) | (1 << 5),
                        1 << 6,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ],
                )
            }
            OwnedBootstrapCommand::LeSetRandomAddress(address) => {
                self.requested_random_address = Some(address);
                BootstrapCommandCompleteEvent::success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadFilterAcceptListSize => {
                BootstrapCommandCompleteEvent::success(
                    opcode,
                    &[self.config.filter_accept_list_size()],
                )
            }
        })
    }

    fn reset_epoch(&mut self) -> Result<(), HciBuildError> {
        // Validate both defaults before changing any reset-scoped state.
        let event_mask = default_event_mask()?;
        let le_event_mask = default_le_event_mask()?;
        self.phase = BootstrapPhase::Configuring;
        self.event_mask = event_mask;
        self.le_event_mask = le_event_mask;
        self.requested_random_address = None;
        self.host_buffers = None;
        self.controller_to_host_flow_control = ControllerToHostFlowControl::Off;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
