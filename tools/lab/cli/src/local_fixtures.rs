//! The host's own fixtures the stand file uses, its Wi-Fi radios and its
//! Bluetooth adapter, probed for `cargo stand fixtures`.
use std::{path::Path, time::Duration};

use oer_lab_model::equipment::{Fixture, interfaces_or_names, radio_switch};

/// The fixtures of this host the stand file at `lab` uses: its Wi-Fi radios
/// and its Bluetooth adapter.
pub(crate) fn probe_local(lab: &Path) -> Vec<Fixture> {
    let adapter = oer_lab_model::Stand::load(lab)
        .ok()
        .and_then(|stand| stand.bluetooth)
        .map(|bluetooth| bluetooth.adapter);
    let mut fixtures = local_radios();
    fixtures.extend(bluetooth(adapter));
    fixtures
}

/// This host's Wi-Fi radios, claimed by their sysfs path.
fn local_radios() -> Vec<Fixture> {
    let Ok(entries) = std::fs::read_dir("/sys/class/ieee80211") else {
        return Vec::new();
    };
    let mut radios = entries
        .flatten()
        .map(|entry| {
            let phy = entry.file_name().to_string_lossy().into_owned();
            let key = entry
                .path()
                .canonicalize()
                .ok()
                .map(|path| format!("local-radio:{}", path.display()));
            let interfaces = std::fs::read_dir(entry.path().join("device/net"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|interface| interface.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let script = interfaces
                .iter()
                .map(|interface| {
                    format!(
                        "echo '@interface {interface}'; iw dev {interface} info | awk '$1 == \"type\" || $1 == \"channel\"'; iw dev {interface} survey dump"
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            let probed = oer_os_process::output(
                oer_os_process::command("sh").args(["-c", &script]),
                Some(Duration::from_secs(5)),
            )
            .ok()
            .map(|output| interfaces_or_names(&String::from_utf8_lossy(&output.stdout), &interfaces))
            .unwrap_or_default();
            Fixture {
                role: String::from("host Wi-Fi"),
                name: phy,
                key,
                reachable: true,
                detail: std::fs::read_to_string(entry.path().join("device/driver/module/drivers"))
                    .ok()
                    .or_else(|| {
                        std::fs::read_link(entry.path().join("device/driver"))
                            .ok()
                            .and_then(|driver| Some(format!("driver {}", driver.file_name()?.to_string_lossy())))
                    }),
                interfaces: probed,
                error: None,
                blocked: false,
            }
        })
        .collect::<Vec<_>>();
    radios.sort_by(|a, b| a.name.cmp(&b.name));
    radios
}

/// The Bluetooth adapter the lab configuration names, `hci0` by default.
fn bluetooth(adapter: Option<String>) -> Option<Fixture> {
    let adapter = adapter.unwrap_or_else(|| String::from("hci0"));
    let path = Path::new("/sys/class/bluetooth").join(&adapter);
    let key = path
        .canonicalize()
        .ok()
        .map(|path| format!("bluetooth:{}", path.display()));
    // The adapter's radio switch: a blocked adapter answers no HCI command.
    let rfkill = std::fs::read_dir(&path)
        .into_iter()
        .flatten()
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with("rfkill"))
        .map(|entry| {
            let blocked = |kind: &str| {
                std::fs::read_to_string(entry.path().join(kind))
                    .is_ok_and(|state| state.trim() == "1")
            };
            radio_switch(blocked("hard"), blocked("soft"))
        });
    Some(Fixture {
        role: String::from("host Bluetooth"),
        blocked: rfkill.is_some_and(|(_, blocked)| blocked),
        detail: rfkill.map(|(state, _)| state.to_owned()),
        reachable: key.is_some(),
        error: key
            .is_none()
            .then(|| format!("{} is absent", path.display())),
        name: adapter,
        key,
        ..Fixture::default()
    })
}
