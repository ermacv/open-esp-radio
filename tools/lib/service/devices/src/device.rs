//! The attached development boards and what a process does with one while
//! it holds its device lock: write an image bundle, start it, reset it and
//! read its console.
//!
//! A board is found by its USB serial number, the MAC of an Espressif USB
//! Serial/JTAG port. Its chip is not visible on USB; it is known once a
//! write or a probe connected to its ROM, which records it in the user's
//! cache (`devices/<MAC>.chip`).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_chip_profile::{Profile, Start};
use oer_image_bundle::ImageBundle;
use serde::Serialize;

use crate::discovery as ports;
use crate::flash;
use crate::image;
use crate::port::Port;
use crate::reset;
use oer_device_lock as lock;
use oer_device_lock::{DeviceAccess, DeviceId, Holder};

/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a returned port may take until the user may open it.
const PORT_ACCESS: Duration = Duration::from_secs(3);

/// An attached board.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Device {
    /// Its `/dev/serial/by-id` link, else the enumerated port.
    pub port: PathBuf,
    pub mac: DeviceId,
    /// Its chip id, once a write or a probe saw it.
    pub chip: Option<String>,
    /// Who holds its device lock now.
    pub holder: Option<Holder>,
}

/// Every attached board with a MAC serial number.
pub fn devices() -> Vec<Device> {
    ports::attached()
        .into_iter()
        .filter_map(|port| {
            let mac = port.mac?;
            Some(Device {
                port: ports::port_of(&mac).unwrap_or_else(|_| PathBuf::from(&port.port)),
                chip: known_chip(&mac),
                holder: lock::holder(&mac).ok().flatten(),
                mac,
            })
        })
        .collect()
}

/// The attached board `query` names: a MAC, a port path, or nothing when
/// exactly one board is attached.
pub fn find(query: Option<&str>) -> crate::Result<Device> {
    let devices = devices();
    let found = match query {
        None => match devices.as_slice() {
            [one] => Some(one.clone()),
            [] => return Err("no board is attached".into()),
            _ => {
                return Err(format!(
                    "{} boards are attached; name one with --device MAC|PORT: {}",
                    devices.len(),
                    devices
                        .iter()
                        .map(|device| device.mac.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .into());
            }
        },
        Some(query) => match DeviceId::parse(query) {
            Ok(mac) => devices.into_iter().find(|device| device.mac == mac),
            Err(_) => {
                let mac = ports::mac_of(Path::new(query));
                devices
                    .into_iter()
                    .find(|device| Some(&device.mac) == mac.as_ref())
            }
        },
    };
    found.ok_or_else(|| format!("no attached board is `{}`", query.unwrap_or_default()).into())
}

/// The chip a write or a probe last saw on the board with `mac`.
pub fn known_chip(mac: &DeviceId) -> Option<String> {
    std::fs::read_to_string(chip_record(mac).ok()?)
        .ok()
        .map(|chip| chip.trim().to_owned())
        .filter(|chip| !chip.is_empty())
}

fn chip_record(mac: &DeviceId) -> crate::Result<PathBuf> {
    Ok(
        oer_durable::xdg::path(oer_durable::xdg::Base::Cache, "devices")?
            .join(format!("{}.chip", mac.compact())),
    )
}

fn remember_chip(mac: &DeviceId, chip: &str) -> crate::Result<()> {
    let path = chip_record(mac)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, chip)?;
    Ok(())
}

impl Device {
    /// Take the board's device lock for `command`: at once, or waiting for
    /// its holder when `wait`. Inside a lease that holds the board, the
    /// lease's delegation.
    pub fn open(self, command: &str, wait: bool) -> crate::Result<Opened> {
        let access = if wait {
            DeviceAccess::wait(&self.mac, command)?
        } else {
            DeviceAccess::acquire(&self.mac, command)?
        };
        Ok(Opened {
            device: self,
            access,
            command: command.to_owned(),
        })
    }
}

/// A board this process holds (or an ancestor's lease delegated to it): the
/// session every operation on it goes through.
#[derive(Debug)]
pub struct Opened {
    device: Device,
    access: DeviceAccess,
    /// What holds it, for the receipts of its writes.
    command: String,
}

impl Opened {
    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn access(&self) -> &DeviceAccess {
        &self.access
    }

    /// The board's port now: where it was while that exists, else its port
    /// once it is attached again.
    pub fn port(&self) -> crate::Result<PathBuf> {
        if self.device.port.exists() {
            return Ok(self.device.port.clone());
        }
        ports::wait_for(&self.device.mac, REATTACH)
            .ok_or_else(|| format!("board {} is gone", self.device.mac).into())
    }

    /// Write `bundle`, known as `image`, through espflash for the chip of
    /// `profile` by the one write operation (`oer_devices::image`, which
    /// publishes its receipt), then start it as the profile's `[flash]
    /// start` says (a board without hub power starts a power-on image by an
    /// RTS reset) and confirm the start.
    pub fn write(
        &mut self,
        bundle: &ImageBundle,
        image: &str,
        profile: &Profile,
    ) -> crate::Result<image::Receipt> {
        let _operation = self.access.operation()?;
        if bundle.chip != profile.id {
            return Err(format!(
                "the bundle is an {} image; the profile is {}'s",
                bundle.chip, profile.id
            )
            .into());
        }
        if let Some(chip) = &self.device.chip
            && *chip != bundle.chip
        {
            return Err(format!(
                "board {} is an {chip}; the bundle is an {} image",
                self.device.mac, bundle.chip
            )
            .into());
        }
        let port = self.port()?;
        let store = image::Store::open()?;
        let written = image::write(
            &store,
            &self.access,
            bundle,
            image,
            image::Transport::Usb {
                port: &port,
                espflash_chip: &profile.espflash_chip,
            },
            &self.command,
        )?;
        remember_chip(&self.device.mac, &profile.id)?;
        self.device.chip = Some(profile.id.clone());
        match written.pending_start() {
            Start::Reset => {}
            Start::PowerOn => drop(self.reset_port()?),
        }
        written.started()
    }

    fn reset_port(&self) -> crate::Result<Port> {
        let _operation = self.access.operation()?;
        let port = self.port()?;
        crate::port::retrying(PORT_ACCESS, || reset::reset_into_application(&port))
            .map_err(Into::into)
    }

    /// Reset the board into its flashed application; its console session,
    /// which keeps the device lock for as long as the port is open.
    pub fn reset(self) -> crate::Result<Console> {
        let operation = self.access.operation()?;
        let port = self.reset_port()?;
        Ok(Console {
            opened: self,
            port,
            operation,
        })
    }

    /// The board's console session without a reset; it keeps the device
    /// lock for as long as the port is open.
    pub fn console(self) -> crate::Result<Console> {
        let _operation = self.access.operation()?;
        let port = self.port()?;
        let port = crate::port::retrying(PORT_ACCESS, || reset::open_without_reset(&port))?;
        Ok(Console {
            opened: self,
            port,
            operation: _operation,
        })
    }

    /// Connect to the board's ROM, record its chip among `chips` and reset it
    /// into its application.
    pub fn probe(&mut self, chips: &[Profile]) -> crate::Result<String> {
        let _operation = self.access.operation()?;
        let port = self.port()?;
        let name = flash::detect(&port)?;
        let profile = chips
            .iter()
            .find(|profile| profile.espflash_chip == name)
            .ok_or_else(|| {
                format!(
                    "board {} is an {name}, which no chip profile names",
                    self.device.mac
                )
            })?;
        remember_chip(&self.device.mac, &profile.id)?;
        self.device.chip = Some(profile.id.clone());
        Ok(profile.id.clone())
    }
}

/// A board's open console under its device lock: the port and the lock end
/// together, so no port outlives the lock that guards it.
///
/// It reads and writes the port ([`std::io::Read`], [`std::io::Write`]) but
/// never lends it out: a `&mut Port` could be swapped for another port and
/// outlive the lock. The port leaves only with the lock, through
/// [`Console::capture`].
#[derive(Debug)]
pub struct Console {
    // Declared first: the port closes before the lock is released.
    port: Port,
    opened: Opened,
    operation: oer_device_lock::DeviceOperation,
}

impl std::io::Read for Console {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.port, buffer)
    }
}

impl std::io::Write for Console {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut self.port, bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.port)
    }
}

impl Console {
    pub fn device(&self) -> &Device {
        self.opened.device()
    }

    /// Copy the console's lines into `log` for `duration` or until a line
    /// contains `until`, holding the lock throughout; whether `until`
    /// appeared.
    pub fn capture(
        self,
        duration: std::time::Duration,
        until: Option<&str>,
        log: &std::path::Path,
    ) -> crate::Result<bool> {
        let Self {
            port,
            opened,
            operation,
        } = self;
        let lines = crate::console::lines(port);
        let seen = crate::console::capture(&lines, duration, until, log)?;
        // The reader closes the port before the lock is released.
        drop(lines);
        drop(opened);
        drop(operation);
        Ok(seen)
    }
}
