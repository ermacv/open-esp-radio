//! The build profile's shape; `build.rs` writes its one value.

/// How the station authenticates; the profile names it, nothing defaults.
#[derive(Clone, Copy)]
#[allow(dead_code, reason = "each build profile constructs one variant")]
pub enum Security {
    Open,
    Wpa2Personal { passphrase: &'static [u8] },
}

/// The network, regulatory domain, channel and transmit power limit.
#[derive(Clone, Copy)]
pub struct StationProfile {
    pub ssid: &'static [u8],
    pub security: Security,
    pub country: [u8; 2],
    pub channel: u8,
    pub cap_dbm: i8,
}

include!(concat!(env!("OUT_DIR"), "/profile.rs"));
