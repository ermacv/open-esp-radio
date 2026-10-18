//! Raw HCI commands to the Controller of the `bluetooth_hci` image.
use crate::Result;
use oer_hil_link::SerialCapture;
use oer_hil_protocol::{bluetooth::BluetoothHciRequest, bluetooth::BluetoothHciResponse};
use std::time::Duration;

pub const RESET: u16 = 0x0c03;
pub const READ_BD_ADDR: u16 = 0x1009;
pub const LE_SET_ADVERTISING_PARAMETERS: u16 = 0x2006;
pub const LE_SET_ADVERTISING_DATA: u16 = 0x2008;
pub const LE_SET_SCAN_RESPONSE_DATA: u16 = 0x2009;
pub const LE_SET_ADVERTISING_ENABLE: u16 = 0x200a;
pub const DISCONNECT: u16 = 0x0406;
pub const SET_EVENT_MASK: u16 = 0x0c01;

/// The default event mask plus LE Meta.
pub const EVENT_MASK_WITH_LE_META: [u8; 8] = [0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20];

/// Wait for the image's hello and require raw HCI exchanges.
pub fn require(capture: &SerialCapture) -> Result<()> {
    let capabilities = capture.request_image_keys(Duration::from_secs(10))?;
    if !capabilities.has::<oer_hil_protocol::bluetooth::Hci>() {
        return Err("firmware lacks raw Bluetooth HCI exchanges".into());
    }
    Ok(())
}

/// Send one command and return the return parameters of its successful
/// Command Complete.
pub fn command(capture: &SerialCapture, opcode: u16, parameters: &[u8]) -> Result<Vec<u8>> {
    let request = BluetoothHciRequest::Command {
        opcode,
        parameters: heapless::Vec::from_slice(parameters)
            .map_err(|_| format!("HCI command {opcode:#06x} parameters exceed the request"))?,
    };
    match crate::link::hci(capture, request)? {
        // Code, length, packets, opcode, status, return parameters.
        BluetoothHciResponse::Completed(packet)
            if packet[0] == 0x0e && packet.len() >= 6 && packet[5] == 0 =>
        {
            Ok(packet[6..].to_vec())
        }
        response => Err(format!("HCI command {opcode:#06x} failed: {response:?}").into()),
    }
}

/// Legacy advertising or scan-response data padded to the command's 31
/// octets.
pub fn data(bytes: &[u8]) -> Result<[u8; 32]> {
    let mut parameters = [0; 32];
    if bytes.len() > 31 {
        return Err("legacy advertising data exceeds 31 octets".into());
    }
    parameters[0] = bytes.len() as u8;
    parameters[1..=bytes.len()].copy_from_slice(bytes);
    Ok(parameters)
}

/// The next LE Meta event of `subevent`, skipping other events, within
/// `timeout`.
pub fn le_meta_event(capture: &SerialCapture, subevent: u8, timeout: Duration) -> Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!("no LE Meta subevent {subevent:#04x} within {timeout:?}").into());
        }
        let wait_ms = left.as_millis().min(1_000) as u16;
        match crate::link::hci(capture, BluetoothHciRequest::NextEvent { wait_ms })? {
            BluetoothHciResponse::Event { packet, .. }
                if packet[0] == 0x3e && packet.get(2) == Some(&subevent) =>
            {
                return Ok(packet.to_vec());
            }
            BluetoothHciResponse::Event { .. } | BluetoothHciResponse::NoEvent => {}
            response => return Err(format!("HCI event wait failed: {response:?}").into()),
        }
    }
}

/// Send one command and return its Command Status.
pub fn command_status(capture: &SerialCapture, opcode: u16, parameters: &[u8]) -> Result<u8> {
    let request = BluetoothHciRequest::Command {
        opcode,
        parameters: heapless::Vec::from_slice(parameters)
            .map_err(|_| format!("HCI command {opcode:#06x} parameters exceed the request"))?,
    };
    match crate::link::hci(capture, request)? {
        BluetoothHciResponse::Completed(packet) if packet[0] == 0x0f && packet.len() >= 3 => {
            Ok(packet[2])
        }
        response => Err(format!("HCI command {opcode:#06x} failed: {response:?}").into()),
    }
}
