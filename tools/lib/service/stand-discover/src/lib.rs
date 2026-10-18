//! The stand file against the host: every Espressif USB Serial/JTAG device
//! `uhubctl` reports, mapped to its hub, port and touch button and compared
//! with the stand file — boards in place, moved, missing (with why the port
//! is empty) and new, each new one with a `[[board]]` fragment to paste. It
//! never changes the file. [`blink_target`] checks the port `cargo stand
//! discover --blink` may switch.
#![forbid(unsafe_code)]

use std::time::Duration;

use oer_stand_file::{Hub, StandFile};
use oer_stand_power::{self as power, HubStatus};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The USB vendor:product of an Espressif chip's USB Serial/JTAG port.
const ESPRESSIF_SERIAL_JTAG: &str = "303a:1001";
/// How long `--blink` keeps a port off.
pub const BLINK: Duration = Duration::from_secs(5);

/// What `uhubctl` reports now, compared with `stand`.
pub fn report(stand: &StandFile) -> Result<Vec<Finding>> {
    Ok(discover(stand, &power::report()?))
}

/// Where a stand-file hub port is, as discovery names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Place {
    /// The stand-file hub, or the hub's USB location when the stand file
    /// does not describe it.
    pub hub: String,
    pub port: u8,
    pub button: Option<u8>,
}

/// What discovery found for one board or port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Finding {
    /// A board of the stand file on its own port.
    InPlace { board: String, place: Place },
    /// A board of the stand file on another port than the file says.
    Moved {
        board: String,
        expected: Place,
        found: Place,
    },
    /// A board of the stand file no hub reports.
    Missing {
        board: String,
        expected: Place,
        why: Absence,
    },
    /// An Espressif board the stand file does not describe.
    New {
        serial: String,
        place: Place,
        switchable: bool,
    },
}

/// Why a board's port holds no board.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Absence {
    /// The port is off.
    PortOff,
    /// The port is powered but empty: its button is off, or nothing is
    /// plugged in.
    Empty,
    /// The port holds another device.
    Occupied,
    /// No hub of the report is the board's hub.
    HubAbsent,
}

fn place(stand: &StandFile, location: &str, port: u8) -> Place {
    match stand.hub.iter().find(|hub| hub.usb2 == location) {
        Some(hub) => Place {
            hub: hub.id.clone(),
            port,
            button: hub.button(port),
        },
        None => Place {
            hub: location.to_owned(),
            port,
            button: None,
        },
    }
}

/// Compare the report with the stand file.
pub fn discover(stand: &StandFile, report: &[HubStatus]) -> Vec<Finding> {
    let attached = report
        .iter()
        .flat_map(|hub| hub.ports.iter().map(move |port| (hub, port)))
        .filter_map(|(hub, port)| match &port.device {
            Some((id, Some(serial))) if id == ESPRESSIF_SERIAL_JTAG => {
                Some((serial.to_string(), hub.location.as_str(), port.port))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut findings = Vec::new();
    for board in &stand.board {
        // A board on no described hub port has no place to compare.
        let Some(hub_port) = &board.port else {
            continue;
        };
        let hub = stand.hub(&hub_port.hub);
        let expected = Place {
            hub: hub_port.hub.clone(),
            port: hub_port.port,
            button: hub.and_then(|hub| hub.button(hub_port.port)),
        };
        match attached.iter().find(|(serial, ..)| board.is_serial(serial)) {
            Some((_, location, port)) => {
                let found = place(stand, location, *port);
                findings.push(
                    if found.hub == expected.hub && found.port == expected.port {
                        Finding::InPlace {
                            board: board.id.clone(),
                            place: found,
                        }
                    } else {
                        Finding::Moved {
                            board: board.id.clone(),
                            expected,
                            found,
                        }
                    },
                );
            }
            None => {
                let status = hub.and_then(|hub| {
                    report
                        .iter()
                        .find(|status| status.location == hub.usb2)?
                        .ports
                        .iter()
                        .find(|port| port.port == hub_port.port)
                });
                let why = match (hub, status) {
                    (Some(_), Some(port)) if !port.powered => Absence::PortOff,
                    (Some(_), Some(port)) if port.connected => Absence::Occupied,
                    (Some(_), Some(_)) => Absence::Empty,
                    _ => Absence::HubAbsent,
                };
                findings.push(Finding::Missing {
                    board: board.id.clone(),
                    expected,
                    why,
                });
            }
        }
    }
    for (serial, location, port) in attached {
        if stand.board_by_serial(&serial).is_none() {
            let switchable = stand
                .hub
                .iter()
                .find(|hub| hub.usb2 == location)
                .is_some_and(|hub| hub.is_switchable(port));
            findings.push(Finding::New {
                serial,
                place: place(stand, location, port),
                switchable,
            });
        }
    }
    findings
}

fn at(place: &Place) -> String {
    match place.button {
        Some(button) => format!("{}:{} (button {button})", place.hub, place.port),
        None => format!("{}:{}", place.hub, place.port),
    }
}

/// The findings as discovery prints them.
pub fn describe(stand: &StandFile, findings: &[Finding]) -> String {
    let mut out = String::new();
    for finding in findings {
        out.push_str(&match finding {
            Finding::InPlace { board, place } => format!("ok      {board} on {}\n", at(place)),
            Finding::Moved {
                board,
                expected,
                found,
            } => format!(
                "moved   {board} on {}; the stand file says {}: update its `port`\n",
                at(found),
                at(expected)
            ),
            Finding::Missing {
                board,
                expected,
                why,
            } => format!(
                "missing {board} on {}: {}\n",
                at(expected),
                match (why, expected.button) {
                    (Absence::PortOff, _) => String::from("the port is off"),
                    (Absence::Empty, Some(button)) => {
                        format!("button {button} is off or the board is not plugged in")
                    }
                    (Absence::Empty, None) => String::from("the port is empty"),
                    (Absence::Occupied, _) => String::from("another device is on the port"),
                    (Absence::HubAbsent, _) => String::from("its hub is not attached"),
                }
            ),
            Finding::New {
                serial,
                place,
                switchable,
            } => {
                let port = if stand.hub(&place.hub).is_some() {
                    format!("{{ hub = \"{}\", port = {} }}", place.hub, place.port)
                } else {
                    format!(
                        "{{ hub = \"?\", port = {} }}  # hub {} is not in the stand file",
                        place.port, place.hub
                    )
                };
                format!(
                    "new     {serial} on {}; add it to the stand file:\n\n\
                     [[board]]\n\
                     id = \"?\"  # the name written on the board\n\
                     usb-serial = \"{serial}\"\n\
                     chip = \"?\"\n\
                     radios = []\n\
                     roles = [\"dut\", \"peer\"]\n\
                     port = {port}\n\
                     reset = [{}]\n\n",
                    at(place),
                    if *switchable {
                        "\"jtag\", \"power\""
                    } else {
                        "\"jtag\""
                    }
                )
            }
        });
    }
    out
}

/// The stand-file hub and port `--blink HUB:PORT` names: a switchable port,
/// never one a cascaded hub hangs on.
pub fn blink_target<'a>(stand: &'a StandFile, target: &str) -> Result<(&'a Hub, u8)> {
    let (hub, port) = target
        .split_once(':')
        .ok_or_else(|| format!("`{target}` is not HUB:PORT"))?;
    let port: u8 = port
        .parse()
        .map_err(|_| format!("`{port}` is not a port number"))?;
    let hub = stand
        .hub(hub)
        .ok_or_else(|| format!("hub `{hub}` is not in the stand file"))?;
    if hub.is_protected(port) {
        return Err(format!(
            "{}:{port} is protected: a cascaded hub hangs on it, and switching it drops every board behind it",
            hub.id
        )
        .into());
    }
    if !hub.is_switchable(port) {
        return Err(format!(
            "{}:{port} is not switchable: `uhubctl` does not cut its power",
            hub.id
        )
        .into());
    }
    Ok((hub, port))
}

#[cfg(test)]
mod tests;
