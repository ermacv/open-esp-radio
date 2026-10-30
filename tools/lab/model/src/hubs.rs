//! `uhubctl`'s report of the stand's hubs, read by one parser: per hub its
//! ports' power and connection state and the device each holds. Only the LAB
//! service runs `uhubctl`; its clients read the report it returns.
#![forbid(unsafe_code)]

/// One hub section of `uhubctl`'s report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubStatus {
    pub location: String,
    pub ports: Vec<PortStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortStatus {
    pub port: u8,
    pub powered: bool,
    pub connected: bool,
    /// The attached device's `vid:pid` and, when its description ends in
    /// one, its MAC (an Espressif USB Serial/JTAG port's serial number).
    pub device: Option<(String, Option<crate::device_id::DeviceId>)>,
}

/// `uhubctl`'s report: per hub its ports' power and connection state and
/// the device each holds.
pub fn parse(report: &str) -> Vec<HubStatus> {
    let mut hubs: Vec<HubStatus> = Vec::new();
    for line in report.lines() {
        if let Some(rest) = line.strip_prefix("Current status for hub ") {
            if let Some(location) = rest.split_whitespace().next() {
                hubs.push(HubStatus {
                    location: location.to_owned(),
                    ports: Vec::new(),
                });
            }
            continue;
        }
        let Some(hub) = hubs.last_mut() else {
            continue;
        };
        let Some(rest) = line.trim_start().strip_prefix("Port ") else {
            continue;
        };
        let Some((number, state)) = rest.split_once(':') else {
            continue;
        };
        let Ok(port) = number.trim().parse::<u8>() else {
            continue;
        };
        let (flags, device) = match state.split_once('[') {
            Some((flags, device)) => (flags, Some(device.trim_end().trim_end_matches(']'))),
            None => (state, None),
        };
        let words = flags.split_whitespace().collect::<Vec<_>>();
        let device = device.and_then(|device| {
            let id = device.split_whitespace().next()?.to_owned();
            // An Espressif USB Serial/JTAG port reports its MAC as the last
            // word; other devices' descriptions end otherwise.
            let serial = device
                .split_whitespace()
                .last()
                .and_then(|last| crate::device_id::DeviceId::parse(last).ok());
            Some((id, serial))
        });
        hub.ports.push(PortStatus {
            port,
            powered: words.contains(&"power"),
            connected: words.contains(&"connect"),
            device,
        });
    }
    hubs
}

/// Whether port `port` of hub `location` is powered in `report`: that
/// hub's own section, not its USB 3 companion's.
pub fn powered(report: &[HubStatus], location: &str, port: u8) -> Option<bool> {
    report
        .iter()
        .find(|hub| hub.location == location)?
        .ports
        .iter()
        .find(|status| status.port == port)
        .map(|status| status.powered)
}

#[cfg(test)]
mod tests;
