//! The Espressif USB Serial/JTAG boards attached to this host, from sysfs:
//! each one's USB location, its serial number (the chip's MAC) and its tty.

use std::path::{Path, PathBuf};

/// Espressif's USB vendor id and the USB Serial/JTAG's product id.
pub const VENDOR: &str = "303a";
pub const PRODUCT: &str = "1001";

/// One attached board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attached {
    /// Its USB location, such as `3-8.3.4.4`: the hub location and, after
    /// the last dot, the hub port.
    pub location: String,
    pub mac: String,
    /// `/dev/ttyACM…`, once the kernel bound its CDC interface.
    pub tty: Option<PathBuf>,
}

impl Attached {
    /// The hub location and port this board hangs on.
    pub fn hub_port(&self) -> Option<(&str, u32)> {
        let (hub, port) = self.location.rsplit_once('.')?;
        Some((hub, port.parse().ok()?))
    }
}

/// The boards below the sysfs USB device directory `devices`
/// (`/sys/bus/usb/devices`), in location order.
pub fn attached_in(devices: &Path) -> Vec<Attached> {
    let read = |path: PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_owned())
    };
    let mut boards = Vec::new();
    let Ok(entries) = std::fs::read_dir(devices) else {
        return boards;
    };
    for entry in entries.flatten() {
        let device = entry.path();
        let location = entry.file_name().to_string_lossy().into_owned();
        if location.contains(':')
            || read(device.join("idVendor")).as_deref() != Some(VENDOR)
            || read(device.join("idProduct")).as_deref() != Some(PRODUCT)
        {
            continue;
        }
        let Some(mac) = read(device.join("serial")) else {
            continue;
        };
        boards.push(Attached {
            tty: tty_of(&device, &location),
            location,
            mac,
        });
    }
    boards.sort_by(|a, b| a.location.cmp(&b.location));
    boards
}

/// The boards attached to this host.
pub fn attached() -> Vec<Attached> {
    attached_in(Path::new("/sys/bus/usb/devices"))
}

/// The attached board whose MAC is `mac`.
pub fn find(mac: &str) -> Option<Attached> {
    attached()
        .into_iter()
        .find(|board| board.mac.eq_ignore_ascii_case(mac))
}

/// The tty of the device's CDC-ACM interfaces.
fn tty_of(device: &Path, location: &str) -> Option<PathBuf> {
    let interfaces = std::fs::read_dir(device).ok()?;
    let mut ttys = interfaces
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{location}:"))
        })
        .filter_map(|interface| std::fs::read_dir(interface.path().join("tty")).ok())
        .flat_map(|ttys| ttys.flatten())
        .map(|tty| Path::new("/dev").join(tty.file_name()))
        .collect::<Vec<_>>();
    ttys.sort();
    ttys.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(root: &Path, location: &str, ids: (&str, &str), serial: &str, tty: Option<&str>) {
        let device = root.join(location);
        std::fs::create_dir_all(&device).unwrap();
        std::fs::write(device.join("idVendor"), format!("{}\n", ids.0)).unwrap();
        std::fs::write(device.join("idProduct"), format!("{}\n", ids.1)).unwrap();
        std::fs::write(device.join("serial"), format!("{serial}\n")).unwrap();
        if let Some(tty) = tty {
            std::fs::create_dir_all(device.join(format!("{location}:1.0/tty/{tty}"))).unwrap();
        }
    }

    #[test]
    fn finds_usb_serial_jtag_boards_with_their_tty_and_hub_port() {
        let root = tempfile::tempdir().unwrap();
        device(
            root.path(),
            "3-8.3.4.4",
            (VENDOR, PRODUCT),
            "02:00:00:00:01:D0",
            Some("ttyACM1"),
        );
        device(
            root.path(),
            "3-8.3.2",
            (VENDOR, PRODUCT),
            "02:00:00:00:02:64",
            None,
        );
        device(root.path(), "3-8.3", ("0bda", "5411"), "hub", None);
        std::fs::create_dir_all(root.path().join("3-8.3.2:1.0")).unwrap();
        let boards = attached_in(root.path());
        assert_eq!(boards.len(), 2);
        assert_eq!(boards[0].location, "3-8.3.2");
        assert_eq!(boards[0].tty, None);
        assert_eq!(boards[1].mac, "02:00:00:00:01:D0");
        assert_eq!(boards[1].tty.as_deref(), Some(Path::new("/dev/ttyACM1")));
        assert_eq!(boards[1].hub_port(), Some(("3-8.3.4", 4)));
    }
}
