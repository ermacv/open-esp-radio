//! Reset-scoped Host configuration and bootstrap command admission.

use super::*;

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
    pub fn new(config: LeControllerBootstrapConfig) -> Self {
        Self {
            config,
            phase: BootstrapPhase::AwaitingReset,
            event_mask: default_event_mask(),
            le_event_mask: default_le_event_mask(),
            requested_random_address: None,
            host_buffers: None,
            controller_to_host_flow_control: ControllerToHostFlowControl::Off,
        }
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
    ) -> BootstrapCommandCompleteEvent {
        let opcode = command.opcode();
        if !command.is_reset() && self.phase == BootstrapPhase::AwaitingReset {
            return command_error(opcode, HciError::CMD_DISALLOWED);
        }

        match command {
            OwnedBootstrapCommand::Reset => {
                self.reset_epoch();
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::SetEventMask(mask) => {
                self.event_mask = mask;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::SetControllerToHostFlowControl(mode) => {
                if !matches!(
                    mode,
                    ControllerToHostFlowControl::Off | ControllerToHostFlowControl::AclOnSyncOff
                ) {
                    return command_error(opcode, HciError::UNSUPPORTED);
                }
                self.controller_to_host_flow_control = mode;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::HostBufferSize {
                acl_data_packet_length,
                total_acl_data_packets,
            } => {
                self.host_buffers = Some(BootstrapHostBuffers {
                    acl_data_packet_length: acl_data_packet_length.get(),
                    total_acl_data_packets: total_acl_data_packets.get(),
                });
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::ReadBdAddr => {
                command_success(opcode, self.config.public_address.hci_wire_address().raw())
            }
            OwnedBootstrapCommand::ReadLocalSupportedCommands => {
                let mut commands = super::le_controller_supported_commands();
                if has_random_source {
                    commands[27] |= 1 << 7; // LE Rand.
                }
                if self.config.filter_accept_list_size > 0 {
                    // LE Clear, Add Device To and Remove Device From Filter
                    // Accept List.
                    commands[26] |= 1 << 7;
                    commands[27] |= (1 << 0) | (1 << 1);
                }
                command_success(opcode, &commands)
            }
            OwnedBootstrapCommand::LeSetEventMask(mask) => {
                self.le_event_mask = mask;
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadBufferSize => {
                let mut response = [0; 3];
                response[..2].copy_from_slice(&self.config.le_acl_data_packet_length.to_le_bytes());
                response[2] = self.config.total_num_le_acl_data_packets;
                command_success(opcode, &response)
            }
            OwnedBootstrapCommand::LeReadLocalSupportedFeatures => {
                // Bit 0 is LE Encryption, bit 3 is Peripheral-initiated Feature
                // Exchange, bit 4 is LE Ping, bit 5 is LE Data Packet Length
                // Extension and bit 14 is CSA #2.
                command_success(
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
                command_success(opcode, &[])
            }
            OwnedBootstrapCommand::LeReadFilterAcceptListSize => {
                command_success(opcode, &[self.config.filter_accept_list_size()])
            }
        }
    }

    fn reset_epoch(&mut self) {
        self.phase = BootstrapPhase::Configuring;
        self.event_mask = default_event_mask();
        self.le_event_mask = default_le_event_mask();
        self.requested_random_address = None;
        self.host_buffers = None;
        self.controller_to_host_flow_control = ControllerToHostFlowControl::Off;
    }
}

/// The event mask after Reset, `0x0000_1FFF_FFFF_FFFF` (Core Vol 4 Part E
/// 7.3.1): every event up to bit 44, without LE Meta.
pub(crate) fn default_event_mask() -> EventMask {
    EventMask::from_hci_bytes(&0x0000_1fff_ffff_ffff_u64.to_le_bytes())
        .expect("eight octets")
        .0
}

/// The LE event mask after Reset, `0x1F` (Core Vol 4 Part E 7.8.1):
/// connection complete, advertising report, connection update complete,
/// remote features complete and LTK request.
pub(crate) fn default_le_event_mask() -> LeEventMask {
    LeEventMask::from_hci_bytes(&0x1f_u64.to_le_bytes())
        .expect("eight octets")
        .0
}
