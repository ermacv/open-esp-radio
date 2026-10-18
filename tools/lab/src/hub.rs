//! The hub port that powers a board, found on the host instead of written in
//! the stand file, and whether that port may be switched off.
//!
//! The board's USB location in sysfs names its parent hub and port
//! (`3-8.3.4.4` hangs on port 4 of hub `3-8.3.4`). `uhubctl` confirms that
//! the hub switches power per port and has that port. A port is switched off
//! only when the one device below it is the board itself: not a hub, nothing
//! behind it, and nothing on the port's USB 3 companion (its sysfs `peer`),
//! which `uhubctl` switches with it. A port the stand file marks `protected`,
//! one missing from a hub's `switchable` list when it has one, and one known
//! to keep its power (`keeps-power`) are refused too.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::stand::{Hub, Stand};

/// The sysfs USB device directory of this host.
pub const SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";

/// The USB device class of a hub.
const HUB_CLASS: &str = "09";

/// One hub `uhubctl` reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubStatus {
    /// Its USB location, such as `3-8.3.4`.
    pub location: String,
    /// Whether it switches power per port (`ppps`).
    pub per_port_power: bool,
    pub ports: Vec<PortStatus>,
}

/// One port of a reported hub.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortStatus {
    pub port: u32,
    pub powered: bool,
    pub connected: bool,
}

/// The hubs of `uhubctl`'s status listing, in the order it gives them.
pub fn parse_status(text: &str) -> Vec<HubStatus> {
    let mut hubs: Vec<HubStatus> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Current status for hub ") {
            let Some((location, description)) = rest.split_once(' ') else {
                continue;
            };
            hubs.push(HubStatus {
                location: location.to_owned(),
                per_port_power: description.trim_end_matches(']').ends_with(", ppps"),
                ports: Vec::new(),
            });
        } else if let Some(rest) = line.trim_start().strip_prefix("Port ")
            && let Some(hub) = hubs.last_mut()
            && let Some((port, state)) = rest.split_once(':')
            && let Ok(port) = port.trim().parse()
        {
            // The state words come before the attached device's `[…]`.
            let words = state.split('[').next().unwrap_or_default();
            let has = |word: &str| words.split_whitespace().any(|candidate| candidate == word);
            hub.ports.push(PortStatus {
                port,
                powered: has("power"),
                connected: has("connect"),
            });
        }
    }
    hubs
}

/// What `uhubctl` reports on this host.
pub fn status() -> crate::Result<Vec<HubStatus>> {
    let output = Command::new("uhubctl")
        .output()
        .map_err(|error| format!("run uhubctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "uhubctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(parse_status(&String::from_utf8_lossy(&output.stdout)))
}

/// The hub port that powers a board and may be switched off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PowerPort {
    /// The hub's USB location (`uhubctl -l`).
    pub hub: String,
    pub port: u32,
    /// The stand file's id of the hub, when it describes it.
    pub hub_id: Option<String>,
}

impl std::fmt::Display for PowerPort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.hub_id {
            Some(id) => write!(formatter, "{id} ({}) port {}", self.hub, self.port),
            None => write!(formatter, "{} port {}", self.hub, self.port),
        }
    }
}

/// Where a board at USB `location` hangs: its hub location and port.
pub fn parent(location: &str) -> Option<(&str, u32)> {
    let (hub, port) = location.rsplit_once('.')?;
    Some((hub, port.parse().ok()?))
}

/// The hub port that powers the board at USB `location` in the sysfs device
/// directory `devices`, as `report` (`uhubctl`) and `stand` describe the
/// hubs, or why it may not be switched off.
pub fn power_port(
    devices: &Path,
    report: &[HubStatus],
    stand: Option<&Stand>,
    location: &str,
) -> Result<PowerPort, String> {
    let (hub, port) =
        parent(location).ok_or_else(|| format!("USB {location} hangs on no hub port"))?;
    let status = report
        .iter()
        .find(|status| status.location == hub)
        .ok_or_else(|| format!("uhubctl reports no hub {hub}"))?;
    if !status.per_port_power {
        return Err(format!("hub {hub} does not switch power per port"));
    }
    if !status.ports.iter().any(|candidate| candidate.port == port) {
        return Err(format!("hub {hub} has no port {port}"));
    }
    let described =
        stand.and_then(|stand| stand.hubs.iter().find(|candidate| candidate.usb2 == hub));
    if let Some(described) = described {
        refused_by_stand(described, port)?;
    }
    only_the_board_below(devices, hub, port, location)?;
    Ok(PowerPort {
        hub: hub.to_owned(),
        port,
        hub_id: described.map(|hub| hub.id.clone()),
    })
}

fn refused_by_stand(hub: &Hub, port: u32) -> Result<(), String> {
    if hub.protected.contains(&port) {
        return Err(format!(
            "port {port} of hub {} is protected in the stand file",
            hub.id
        ));
    }
    if !hub.switchable.is_empty() && !hub.switchable.contains(&port) {
        return Err(format!(
            "port {port} of hub {} is not in its switchable list in the stand file",
            hub.id
        ));
    }
    if hub.keeps_power.contains(&port) {
        return Err(format!(
            "port {port} of hub {} keeps its power when switched off (stand file)",
            hub.id
        ));
    }
    Ok(())
}

/// The one device below `hub`'s `port` is the board at `location`, which is
/// no hub and carries nothing, and the port's USB 3 companion is empty.
fn only_the_board_below(
    devices: &Path,
    hub: &str,
    port: u32,
    location: &str,
) -> Result<(), String> {
    let node = port_node(devices, hub, port)
        .ok_or_else(|| format!("sysfs has no port {port} of hub {hub}"))?;
    match linked_name(&node.join("device")) {
        Some(attached) if attached == location => {}
        Some(attached) => {
            return Err(format!(
                "port {port} of hub {hub} carries USB {attached}, not the board"
            ));
        }
        None => return Err(format!("port {port} of hub {hub} carries no device")),
    }
    let device = devices.join(location);
    if read(&device.join("bDeviceClass")).as_deref() == Some(HUB_CLASS) {
        return Err(format!("USB {location} is a hub, not a board"));
    }
    let below = format!("{location}.");
    if let Some(other) = names(devices)
        .into_iter()
        .find(|name| name.starts_with(&below))
    {
        return Err(format!("USB {other} hangs behind the board's port"));
    }
    if let Some(peer) = linked_path(&node.join("peer"))
        && let Some(attached) = linked_name(&peer.join("device"))
    {
        return Err(format!(
            "the USB 3 companion of port {port} of hub {hub} carries USB {attached}"
        ));
    }
    Ok(())
}

/// The sysfs node of `hub`'s `port`: `<hub>/<hub>:<interface>/<hub>-port<port>`.
fn port_node(devices: &Path, hub: &str, port: u32) -> Option<PathBuf> {
    let name = format!("{hub}-port{port}");
    let interface_prefix = format!("{hub}:");
    std::fs::read_dir(devices.join(hub))
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&interface_prefix)
        })
        .map(|entry| entry.path().join(&name))
        .find(|node| node.is_dir())
}

fn linked_path(link: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(link).ok()?;
    let parent = link.parent()?;
    Some(parent.join(target))
}

fn linked_name(link: &Path) -> Option<String> {
    Some(
        std::fs::read_link(link)
            .ok()?
            .file_name()?
            .to_string_lossy()
            .into_owned(),
    )
}

fn names(devices: &Path) -> Vec<String> {
    std::fs::read_dir(devices)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.contains(':'))
                .collect()
        })
        .unwrap_or_default()
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

/// Switch `port` of the hub at `location` on or off.
pub fn switch(location: &str, port: u32, on: bool) -> crate::Result<()> {
    let action = if on { "on" } else { "off" };
    let output = Command::new("uhubctl")
        .args(["-l", location, "-p", &port.to_string(), "-a", action])
        .output()
        .map_err(|error| format!("run uhubctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "uhubctl -l {location} -p {port} -a {action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
