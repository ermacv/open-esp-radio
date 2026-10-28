//! Closed bootstrap configuration, command decoding, responses and the
//! event masks a Reset restores.

use core::num::NonZeroU16;

use bt_hci::{
    FromHciBytes,
    cmd::{
        Cmd, Opcode,
        controller_baseband::{
            HostBufferSize, HostBufferSizeParams, Reset, SetControllerToHostFlowControl,
            SetEventMask,
        },
        info::{ReadBdAddr, ReadLocalSupportedCmds},
        le::{
            LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures,
            LeSetEventMask, LeSetRandomAddr,
        },
    },
    param::{
        BdAddr, ControllerToHostFlowControl, Error as HciError, EventMask, LeEventMask, Status,
    },
};

use crate::HciCommandPacket;

mod command;
mod config;
mod reset;
mod response;

pub(crate) use command::BootstrapCommandDecodeError;
pub use command::{BootstrapCommand, OwnedBootstrapCommand, le_controller_supported_commands};
pub use config::{BluetoothPublicDeviceAddress, BootstrapConfigError, LeControllerBootstrapConfig};
pub use reset::{default_event_mask, default_le_event_mask};
pub(crate) use response::invalid_parameters;
pub use response::{BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BootstrapCommandCompleteEvent};

#[cfg(test)]
mod tests;
