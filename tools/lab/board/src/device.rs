//! The attached development boards and what a process does with one while
//! it holds its device lock: write an image bundle, start it, reset it and
//! read its console.
//!
//! A board is found by its USB serial number, the MAC of an Espressif USB
//! Serial/JTAG port. Its chip is not visible on USB; it is known once a
//! write or a probe connected to its ROM, which records it in the user's
//! cache (`devices/<MAC>.chip`).

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::discovery as ports;

use oer_lab_model::device_id::DeviceId;

/// An attached board.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Device {
    /// Its `/dev/serial/by-id` link, else the enumerated port.
    pub port: PathBuf,
    pub mac: DeviceId,
    /// Its chip id, once a write or a probe saw it.
    pub chip: Option<String>,
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
        oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Cache, "devices")?
            .join(format!("{}.chip", mac.compact())),
    )
}

pub fn remember_chip(mac: &DeviceId, chip: &str) -> crate::Result<()> {
    let path = chip_record(mac)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, chip)?;
    Ok(())
}
