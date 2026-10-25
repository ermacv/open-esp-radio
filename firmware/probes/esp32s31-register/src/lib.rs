#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]

//! Small, low-level probes kept independent from AP/STA runtime adapters.
//!
//! Register conformance must remain buildable while higher-level driver work
//! is in progress. Fat LTO still inlines the production HAL/PAC leaves into
//! the retained ABI symbol inspected by Blobray. Every entry reaches the
//! hardware through the HAL's public API alone:
//! [`open_probe_session_init`] takes the reset root once and the later
//! entries borrow the session's Wi-Fi owner.

use oer_esp32s31_hal::{
    domain::{CommonPower, RadioDomain, WifiClient},
    ieee80211::client::{WifiClocksOn, WifiCold},
    owner::RadioAccess,
    power::ChipClocks,
    root::Root,
};
use oer_memory::ClaimCell;

/// The owners the session's entries borrow.
struct Session {
    wifi: RadioAccess,
    _radio: RadioDomain<ChipClocks>,
    _power: CommonPower<WifiClient>,
    _clocks_on: WifiClocksOn,
    _clocks: ChipClocks,
}

static SESSION: ClaimCell<Session> = ClaimCell::new();

/// Start the session: take the reset root, split it and bring Wi-Fi up
/// through its public client path. Zero, or why it did not start: one when
/// the root was taken before, two or three when Wi-Fi's power or clocks did
/// not come up, four when a session was installed before.
fn start_session() -> u32 {
    let Some(root) = Root::take() else {
        return 1;
    };
    let runtime = root.into_runtime();
    let clocks = runtime.clocks;
    let (mut radio, partitions) = runtime.radio.split::<ChipClocks, _>(());
    let Ok((powered, power)) =
        WifiCold::from_partition(partitions.wifi).power_up(&mut radio, &clocks)
    else {
        return 2;
    };
    let Ok(clocked) = powered.enable_clocks(&mut radio, &clocks) else {
        return 3;
    };
    let (wifi, _interrupts, clocks_on) = clocked.into_running();
    let session = Session {
        wifi,
        _radio: radio,
        _power: power,
        _clocks_on: clocks_on,
        _clocks: clocks,
    };
    match SESSION.install(session) {
        Ok(()) => 0,
        Err(_) => 4,
    }
}

#[panic_handler]
#[allow(
    clippy::disallowed_methods,
    reason = "isolated verification image terminates on panic without a runtime executor"
)]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn mac_address_from_words(low: u32, high: u32) -> [u8; 6] {
    let low = low.to_le_bytes();
    let high = high.to_le_bytes();
    [low[0], low[1], low[2], low[3], high[0], high[1]]
}

oer_probe_macros::probe! {
    /// Start the verifier session; nothing of it is compared.
    pub fn open_probe_session_init() -> u32 {
        start_session()
    }
}

oer_probe_macros::probe! {
    /// Register-only production projection of vendor `wifi_set_rx_policy` cases
    /// two, six, eight and nine.
    ///
    /// Arguments one and two carry the reviewed global-context address into the
    /// Rust probe. Argument three selects the closed case-six register submode;
    /// case eight deliberately ignores it.
    pub fn open_wifi_sta_ap_trace_wifi_set_rx_policy(
        policy: u32,
        address_low: u32,
        address_high: u32,
        mode: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{MacRoleReceivePolicy, MacStaPolicyMode};

        let address = mac_address_from_words(address_low, address_high);
        let policy = match policy {
            2 => MacRoleReceivePolicy::StationDisabled,
            6 => {
                let mode = if mode == 2 {
                    MacStaPolicyMode::Mode2
                } else {
                    MacStaPolicyMode::Mode1
                };
                MacRoleReceivePolicy::Station {
                    bssid: address,
                    mode,
                }
            }
            8 => MacRoleReceivePolicy::AccessPoint { address },
            9 => MacRoleReceivePolicy::AccessPointDisabled,
            _ => return 0,
        };
        SESSION
            .lend(|session| {
                session.wifi.wifi_mac_hal().configure_role_receive_policy(policy);
                1
            })
            .unwrap_or(0)
    }
}
