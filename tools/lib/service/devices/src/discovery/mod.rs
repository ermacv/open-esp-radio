//! The USB serial ports attached to the host, and a board's port by its
//! MAC: the `/dev/serial/by-id` link udev names after the board, which
//! survives a power cycle that gives it another `/dev/ttyACM*`.

pub use oer_tool_data_device_mac::DeviceId;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::Serialize;

/// Where udev names serial ports by their device.
pub const BY_ID: &str = "/dev/serial/by-id";

/// A USB serial port attached now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AttachedPort {
    pub port: String,
    /// Its USB serial number as a board id, when it reports one that is a
    /// MAC.
    pub mac: Option<DeviceId>,
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
}

/// The USB serial ports attached to the host.
pub fn attached() -> Vec<AttachedPort> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(usb) => Some(AttachedPort {
                port: port.port_name,
                mac: usb
                    .serial_number
                    .and_then(|serial| DeviceId::parse(&serial).ok()),
                vid: usb.vid,
                pid: usb.pid,
                product: usb.product,
            }),
            _ => None,
        })
        .collect()
}

/// Whether the board with `mac` is attached now.
pub fn is_attached(mac: &DeviceId) -> bool {
    attached().iter().any(|port| port.mac.as_ref() == Some(mac))
}

/// The port of the attached board with `mac`: its `/dev/serial/by-id` link,
/// else the port the enumeration names.
pub fn port_of(mac: &DeviceId) -> crate::Result<PathBuf> {
    let port = attached()
        .into_iter()
        .find(|port| port.mac.as_ref() == Some(mac))
        .map(|port| PathBuf::from(port.port))
        .ok_or_else(|| format!("the board {mac} is not attached"))?;
    Ok(by_id(Path::new(BY_ID), mac).unwrap_or(port))
}

/// The port of the board with `mac` once it is attached, polling for
/// `within`: a reset or a power cycle can make its port re-enumerate.
pub fn wait_for(mac: &DeviceId, within: Duration) -> Option<PathBuf> {
    let started = Instant::now();
    loop {
        if let Ok(port) = port_of(mac) {
            return Some(port);
        }
        if started.elapsed() >= within
            || oer_tool_base_process::sleep(Duration::from_millis(250)).is_err()
        {
            return None;
        }
    }
}

/// The MAC of the board behind `path`, following symlinks such as
/// `/dev/serial/by-id`.
pub fn mac_of(path: &Path) -> Option<DeviceId> {
    let canonical = fs::canonicalize(path).ok()?;
    attached().into_iter().find_map(|port| {
        (fs::canonicalize(&port.port).ok().as_ref() == Some(&canonical))
            .then_some(port.mac)
            .flatten()
    })
}

/// The link in `directory` that names the USB Serial/JTAG port of the board
/// with `mac`.
fn by_id(directory: &Path, mac: &str) -> Option<PathBuf> {
    let mut links = fs::read_dir(directory)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_uppercase().contains(mac))
        })
        .collect::<Vec<_>>();
    links.sort();
    links.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_s_port_is_its_by_id_link() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "usb-Espressif_USB_JTAG_serial_debug_unit_02:00:00:00:25:64-if00",
            "usb-Espressif_USB_JTAG_serial_debug_unit_02:00:00:00:F6:D0-if00",
        ] {
            std::fs::write(directory.path().join(name), "").unwrap();
        }
        let link = by_id(directory.path(), "xx:xx:xx:xx:25:64").unwrap();
        assert!(link.to_string_lossy().contains("xx:xx:xx:xx:25:64"));
        assert_eq!(by_id(directory.path(), "AA:AA:AA:AA:AA:AA"), None);
        assert_eq!(
            by_id(&directory.path().join("absent"), "xx:xx:xx:xx:25:64"),
            None
        );
    }
}
