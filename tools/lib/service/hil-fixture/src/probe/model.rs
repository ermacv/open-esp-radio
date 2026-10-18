//! Finite discovery workload and its evidence contract; no hardware operations.
use serde::{Deserialize, Serialize};

pub const READY: &str = "probe-source-v1 ready";

pub const REQUESTS: u16 = 401;
pub const MAX_LATENESS_US: u64 = 4_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ssid: String,
    pub channel: u8,
}

impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.ssid.is_empty() || self.ssid.len() > 32 || self.ssid.chars().any(char::is_control) {
            return Err("probe load requires a 1..32-byte SSID without controls");
        }
        if !(1..=13).contains(&self.channel) {
            return Err("probe load requires a 2.4-GHz channel 1..13");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Request {
    pub offset_us: u64,
    pub sequence: u16,
    pub source: [u8; 6],
}

pub fn request(index: u16) -> Option<Request> {
    let (offset_us, suffix) = match index {
        0 => (1_000_000, 0),
        1..=200 => (3_000_000 + u64::from(index - 1) * 5_000, 0),
        201..=400 => (6_000_000 + u64::from(index - 201) * 5_000, index - 200),
        _ => return None,
    };
    let [hi, lo] = suffix.to_be_bytes();
    Some(Request {
        offset_us,
        sequence: index,
        source: [2, 0x4f, 0x45, 0x52, hi, lo],
    })
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Report {
    pub bssid: [u8; 6],
    pub submitted: u16,
    pub maximum_lateness_us: u64,
    pub error: Option<String>,
}

impl Report {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.bssid == [0; 6]
            || self.bssid[0] & 1 != 0
            || self.error.is_some()
            || self.submitted != REQUESTS
            || self.maximum_lateness_us > MAX_LATENESS_US
        {
            return Err("probe injection did not complete the timed workload");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
