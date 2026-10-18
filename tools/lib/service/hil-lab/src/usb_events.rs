//! Host USB events of the boards a repetition uses, read from the kernel log.
//!
//! A board whose USB serial bridge drops off the bus mid-session looks, from
//! the runner's side, like a silent target. The kernel log says whether the
//! host saw the device disconnect and enumerate again; a repetition records
//! those events next to its other artifacts as `usb-events.json`.

use std::path::Path;

use oer_process::CommandExt as _;

use crate::Result;

pub use oer_hil_run_bundle_format::run::USB_EVENTS_FILE;
pub use oer_hil_run_bundle_format::run::UsbEvent;
pub use oer_hil_run_bundle_format::run::UsbEventKind;

/// The USB devices of a repetition's boards, resolved while they are
/// attached: after a disconnect their sysfs entries are gone.
pub struct UsbWatch {
    devices: Vec<String>,
    since_micros: u64,
}

impl UsbWatch {
    pub fn start<'a>(serials: impl IntoIterator<Item = &'a Path>, since_unix_millis: u64) -> Self {
        let mut devices = serials
            .into_iter()
            .filter_map(usb_device_of)
            .collect::<Vec<_>>();
        devices.sort();
        devices.dedup();
        Self {
            devices,
            since_micros: since_unix_millis.saturating_mul(1000),
        }
    }

    /// Write the watched devices' events since the start into `output`,
    /// when there are any. The kernel log is diagnostic evidence: when it
    /// cannot be read, the repetition's own result stands and the reason is
    /// printed.
    pub fn record(&self, output: &Path) {
        if self.devices.is_empty() {
            return;
        }
        match read_kernel_log(self.since_micros)
            .map(|events| watched(events, &self.devices))
            .and_then(|events| {
                if !events.is_empty() {
                    oer_durable::atomic_json(&output.join(USB_EVENTS_FILE), &events)?;
                }
                Ok(())
            }) {
            Ok(()) => {}
            Err(error) => eprintln!("hil: host USB events were not recorded: {error}"),
        }
    }
}

/// The kernel USB device path (`3-8`, `1-2.4`) of the device behind a
/// serial port, from sysfs.
pub fn usb_device_of(serial: &Path) -> Option<String> {
    let tty = serial.canonicalize().ok()?;
    let name = tty.file_name()?.to_str()?;
    let device = Path::new("/sys/class/tty")
        .join(name)
        .join("device")
        .canonicalize()
        .ok()?;
    device_of_sysfs_path(&device)
}

/// The USB device of the first interface directory (`3-8:1.0`) among the
/// ancestors of a sysfs device path.
fn device_of_sysfs_path(path: &Path) -> Option<String> {
    path.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?.to_str()?;
        let (device, interface) = name.split_once(':')?;
        let usb_path = |text: &str| {
            text.split_once('-').is_some_and(|(bus, ports)| {
                !bus.is_empty()
                    && bus.bytes().all(|b| b.is_ascii_digit())
                    && !ports.is_empty()
                    && ports.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            })
        };
        (usb_path(device) && interface.contains('.')).then(|| device.to_owned())
    })
}

fn read_kernel_log(since_micros: u64) -> Result<Vec<UsbEvent>> {
    let output = oer_process::command("journalctl")
        .args(["--dmesg", "--output=json", "--no-pager", "--quiet"])
        .arg(format!(
            "--since=@{}.{:06}",
            since_micros / 1_000_000,
            since_micros % 1_000_000
        ))
        .supervised_output()?;
    if !output.status.success() {
        return Err(format!(
            "journalctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(parse_journal(&String::from_utf8_lossy(&output.stdout)))
}

/// USB events in `journalctl --output=json` lines; other lines are skipped.
fn parse_journal(text: &str) -> Vec<UsbEvent> {
    text.lines()
        .filter_map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).ok()?;
            let message = entry.get("MESSAGE")?.as_str()?;
            let realtime_micros = entry.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
            let (device, kind) = parse_message(message)?;
            Some(UsbEvent {
                realtime_micros,
                device,
                kind,
            })
        })
        .collect()
}

/// `usb 3-8: USB disconnect, device number 19` and
/// `usb 3-8: new full-speed USB device number 20 using xhci_hcd`.
fn parse_message(message: &str) -> Option<(String, UsbEventKind)> {
    let (device, text) = message.strip_prefix("usb ")?.split_once(": ")?;
    if let Some(number) = text.strip_prefix("USB disconnect, device number ") {
        return Some((
            device.to_owned(),
            UsbEventKind::Disconnected {
                device_number: number.trim().parse().ok()?,
            },
        ));
    }
    let (speed, rest) = text
        .strip_prefix("new ")?
        .split_once(" USB device number ")?;
    let number = rest.split_whitespace().next()?.parse().ok()?;
    Some((
        device.to_owned(),
        UsbEventKind::Enumerated {
            device_number: number,
            speed: speed.to_owned(),
        },
    ))
}

fn watched(events: Vec<UsbEvent>, devices: &[String]) -> Vec<UsbEvent> {
    events
        .into_iter()
        .filter(|event| devices.contains(&event.device))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_usb_messages_parse_into_typed_events() {
        assert_eq!(
            parse_message("usb 3-8: USB disconnect, device number 19"),
            Some((
                "3-8".into(),
                UsbEventKind::Disconnected { device_number: 19 }
            ))
        );
        assert_eq!(
            parse_message("usb 1-2.4: new full-speed USB device number 20 using xhci_hcd"),
            Some((
                "1-2.4".into(),
                UsbEventKind::Enumerated {
                    device_number: 20,
                    speed: "full-speed".into()
                }
            ))
        );
        assert_eq!(
            parse_message("cdc_acm 3-8:1.0: ttyACM0: USB ACM device"),
            None
        );
        assert_eq!(parse_message("usb 3-8: New USB device found"), None);
    }

    #[test]
    fn journal_lines_keep_only_usb_events_of_watched_devices() {
        let journal = [
            r#"{"MESSAGE":"usb 3-8: USB disconnect, device number 19","__REALTIME_TIMESTAMP":"100"}"#,
            r#"{"MESSAGE":"usb 3-9: USB disconnect, device number 4","__REALTIME_TIMESTAMP":"101"}"#,
            r#"{"MESSAGE":"cdc_acm 3-8:1.0: ttyACM0: USB ACM device","__REALTIME_TIMESTAMP":"102"}"#,
            r#"{"MESSAGE":[1,2],"__REALTIME_TIMESTAMP":"103"}"#,
            "not json",
            r#"{"MESSAGE":"usb 3-8: new full-speed USB device number 20 using xhci_hcd","__REALTIME_TIMESTAMP":"104"}"#,
        ]
        .join("\n");
        let events = watched(parse_journal(&journal), &["3-8".to_owned()]);
        assert_eq!(
            events
                .iter()
                .map(|event| (event.realtime_micros, event.to_string()))
                .collect::<Vec<_>>(),
            [
                (100, "usb 3-8 disconnected (device number 19)".to_owned()),
                (
                    104,
                    "usb 3-8 enumerated as full-speed device number 20".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn the_usb_device_is_the_interface_ancestor_of_the_tty() {
        for (path, device) in [
            (
                "/sys/devices/pci0000:00/0000:00:14.0/usb3/3-8/3-8:1.0",
                Some("3-8"),
            ),
            (
                "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2.4/1-2.4:1.0/ttyUSB0",
                Some("1-2.4"),
            ),
            ("/sys/devices/pnp0/00:04/tty/ttyS0", None),
        ] {
            assert_eq!(
                device_of_sysfs_path(Path::new(path)).as_deref(),
                device,
                "{path}"
            );
        }
    }
}
