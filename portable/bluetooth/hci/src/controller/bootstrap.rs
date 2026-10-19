//! Closed bootstrap configuration, command decoding and software epoch state.

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
mod response;
pub(crate) mod state;

pub(crate) use command::BootstrapCommandDecodeError;
pub use command::{BootstrapCommand, OwnedBootstrapCommand, le_controller_supported_commands};
pub use config::{BluetoothPublicDeviceAddress, BootstrapConfigError, LeControllerBootstrapConfig};
pub(crate) use response::invalid_parameters;
pub use response::{BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BootstrapCommandCompleteEvent};
use response::{command_error, command_success};
pub use state::{BootstrapHostBuffers, BootstrapPhase, LeControllerBootstrap};

#[cfg(test)]
mod tests;
