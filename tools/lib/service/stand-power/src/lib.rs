//! A board's hub port power through `uhubctl`, and `uhubctl`'s report, read
//! by one parser.
//!
//! Only stand code switches a hub port: the arbiter returning a board's port
//! to its working state when a lease changes hands, a reset ladder's `power`
//! rung, the power-on start of a written image and `cargo stand
//! discover --blink`. A lease runs no `uhubctl` itself.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use oer_stand_file::HubPort;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How long a board may take to leave USB once its port is off.
const POWER_LEAVE: Duration = Duration::from_secs(5);
/// How long a board may take to come back once its port is on.
const POWER_RETURN: Duration = Duration::from_secs(10);

/// The power of one switchable hub port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HubPower {
    pub port: HubPort,
}

impl HubPower {
    pub fn new(port: HubPort) -> Self {
        Self { port }
    }

    /// Whether the port is powered.
    pub fn is_on(&self, lifetime: &oer_process::IoLifetime) -> crate::Result<bool> {
        let mut command = oer_process::command("uhubctl");
        command
            .args(["--location", &self.port.location, "--ports"])
            .arg(self.port.port.to_string());
        lifetime.pin(&mut command)?;
        let output = oer_process::output(&mut command, Some(Duration::from_secs(30)))?;
        powered(
            &parse(&String::from_utf8_lossy(&output.stdout)),
            &self.port.location,
            self.port.port,
        )
        .ok_or_else(|| {
            format!(
                "uhubctl reports no port {} of hub {}",
                self.port.port, self.port.location
            )
            .into()
        })
    }

    /// Power the port on.
    pub fn on(&self, lifetime: &oer_process::IoLifetime) -> crate::Result<()> {
        self.action("on", 2, lifetime)
    }

    /// Power the port off and on again.
    pub fn cycle(&self, lifetime: &oer_process::IoLifetime) -> crate::Result<()> {
        self.action("cycle", 2, lifetime)
    }

    /// Power the port off for `off`, then on again: long enough for a person
    /// to see which button's light goes out.
    pub fn cycle_holding(
        &self,
        off: Duration,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<()> {
        self.action("cycle", off.as_secs().max(1), lifetime)
    }

    /// Power the port off and on again while watching the board with `mac`
    /// on it. The board must leave once the port is off and come back once
    /// it is on: what the stand's own power cycles (a power-on start, the
    /// download-mode entry) require, whose outcome then shows on its own.
    pub fn cycle_observed(
        &self,
        mac: &oer_device_mac::DeviceId,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<PowerCycle> {
        self.action("off", 2, lifetime)?;
        let left = wait_until(POWER_LEAVE, &|| !oer_devices::discovery::is_attached(mac));
        // From the moment the port is told to power on.
        let started = Instant::now();
        self.action("on", 2, lifetime)?;
        let returned = wait_until(POWER_RETURN, &|| oer_devices::discovery::is_attached(mac))
            .then(|| started.elapsed());
        Ok(PowerCycle { left, returned })
    }

    /// [`Self::cycle_observed`], then `reset_cause` once the board returned:
    /// the proof that the port cuts the board's power. A port that only
    /// drops the board from the bus keeps it powered, and the board returns
    /// with its previous reset cause. Only the verification of a port takes
    /// it: the image a power cycle starts may switch the chip's JTAG off or
    /// reset again before the cause is read.
    pub fn prove_power_loss(
        &self,
        mac: &oer_device_mac::DeviceId,
        lifetime: &oer_process::IoLifetime,
        reset_cause: impl FnOnce() -> crate::Result<ResetCause>,
    ) -> crate::Result<PowerLoss> {
        let cycle = self.cycle_observed(mac, lifetime)?;
        let reset = cycle
            .returned
            .is_some()
            .then(|| reset_cause().map_err(|error| error.to_string()));
        Ok(PowerLoss { cycle, reset })
    }

    fn action(
        &self,
        action: &str,
        delay_secs: u64,
        lifetime: &oer_process::IoLifetime,
    ) -> crate::Result<()> {
        let mut command = oer_process::command("uhubctl");
        command
            .args(["--location", &self.port.location, "--ports"])
            .arg(self.port.port.to_string())
            .args(["--action", action, "--delay"])
            .arg(delay_secs.to_string());
        lifetime.pin(&mut command)?;
        let output = oer_process::output(&mut command, Some(Duration::from_secs(30 + delay_secs)))?;
        if !output.status.success() {
            return Err(format!(
                "uhubctl could not {action} {} port {}: {}",
                self.port.location,
                self.port.port,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

/// What a watched power cycle of a board's port showed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerCycle {
    /// The board's own USB device left once the port was off.
    pub left: bool,
    /// How long after the port was on the board came back, if it did.
    pub returned: Option<Duration>,
}

impl PowerCycle {
    /// Whether the board left USB and came back, or why not.
    pub fn verdict(self) -> std::result::Result<Duration, String> {
        match (self.left, self.returned) {
            (true, Some(after)) => Ok(after),
            (false, _) => Err(String::from(
                "the board stayed on USB while its port was off: the port does not cut its power",
            )),
            (true, None) => Err(format!(
                "the board left USB but did not return within {} s of its port's power",
                POWER_RETURN.as_secs()
            )),
        }
    }
}

/// What [`HubPower::prove_power_loss`] showed: the power cycle and why the
/// board last reset once it returned, or why that could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PowerLoss {
    pub cycle: PowerCycle,
    pub reset: Option<std::result::Result<ResetCause, String>>,
}

impl PowerLoss {
    /// Whether the board lost its power and came back, or why not: leaving
    /// USB and returning is not enough, the board must read a power-on reset.
    pub fn verdict(self) -> std::result::Result<Duration, String> {
        let after = self.cycle.verdict()?;
        match self.reset {
            Some(Ok(cause)) if cause.power_on() => Ok(after),
            Some(Ok(cause)) => Err(format!(
                "the board left USB and returned, but its reset cause is {cause}, not a power-on \
                 reset: the port does not cut its power"
            )),
            Some(Err(error)) => Err(format!(
                "the board left USB and returned, but its reset cause could not be read: {error}"
            )),
            None => Err(String::from(
                "the board left USB and returned, but its reset cause was not read",
            )),
        }
    }
}

/// Where a chip holds why it last reset: one field of its platform
/// publication, resolved through the binding index, and the code of a
/// power-on reset (the chip profile's `[reset-cause]`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetCauseField {
    /// The address of the word holding the field.
    pub address: u32,
    pub bit_offset: u32,
    pub bit_width: u32,
    pub power_on: u32,
}

impl ResetCauseField {
    /// The field `profile` names, from the platform binding index of `chip`
    /// in the repository at `root`.
    pub fn resolve(
        root: &std::path::Path,
        chip: &str,
        profile: &oer_chip_profile::ResetCause,
    ) -> crate::Result<Self> {
        let path = root.join(format!("registers/{chip}/published/platform.bindings.toml"));
        let index = oer_register_bindings::BindingIndex::load(&path)?;
        Self::of(&index, profile).map_err(|error| format!("{}: {error}", path.display()).into())
    }

    fn of(
        index: &oer_register_bindings::BindingIndex,
        profile: &oer_chip_profile::ResetCause,
    ) -> std::result::Result<Self, String> {
        let (register, field) = profile
            .field
            .rsplit_once('.')
            .ok_or_else(|| format!("`{}` is not PERIPHERAL.REGISTER.FIELD", profile.field))?;
        let binding = index
            .registers
            .iter()
            .find(|binding| binding.identity == register)
            .ok_or_else(|| format!("no register {register}"))?;
        let field = binding
            .fields
            .iter()
            .find(|candidate| candidate.svd_name == field)
            .ok_or_else(|| format!("register {register} has no field {field}"))?;
        Ok(Self {
            address: binding.address,
            bit_offset: field.bit_offset,
            bit_width: field.bit_width,
            power_on: profile.power_on,
        })
    }

    /// The reset cause `word`, read at [`Self::address`], holds.
    pub fn cause(&self, word: u32) -> ResetCause {
        let mask = u32::MAX >> (32 - self.bit_width);
        ResetCause {
            code: (word >> self.bit_offset) & mask,
            power_on: self.power_on,
        }
    }
}

/// A chip's reset cause code, with the code of a power-on reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCause {
    pub code: u32,
    power_on: u32,
}

impl ResetCause {
    pub fn power_on(self) -> bool {
        self.code == self.power_on
    }
}

impl std::fmt::Display for ResetCause {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#x}", self.code)
    }
}

/// Poll `condition` until it holds or `within` passes.
pub fn wait_until(within: Duration, condition: &dyn Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

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
    pub device: Option<(String, Option<oer_device_mac::DeviceId>)>,
}

/// `uhubctl`'s report of every hub it reaches.
pub fn report() -> crate::Result<Vec<HubStatus>> {
    let output = oer_process::output(
        &mut oer_process::command("uhubctl"),
        Some(Duration::from_secs(30)),
    )?;
    if !output.status.success() {
        return Err(format!(
            "uhubctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
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
                .and_then(|last| oer_device_mac::DeviceId::parse(last).ok());
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
