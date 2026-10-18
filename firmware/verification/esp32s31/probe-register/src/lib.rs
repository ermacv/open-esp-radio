#![no_std]

//! Small, low-level probes kept independent from AP/STA runtime adapters.
//!
//! Register conformance must remain buildable while higher-level driver work
//! is in progress. Fat LTO still inlines the production HAL/PAC leaves into
//! the retained ABI symbol inspected by Blobray.

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
        use oer_esp32s31_hal::{
            ieee80211::mac::validation_configure_role_receive_policy,
            types::{MacRoleReceivePolicy, MacStaPolicyMode},
        };

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
        validation_configure_role_receive_policy(policy);
        1
    }
}
