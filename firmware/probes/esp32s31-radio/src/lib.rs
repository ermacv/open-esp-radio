#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg(target_arch = "riscv32")]
#![deny(clippy::undocumented_unsafe_blocks)]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so Blobray
//! verifier can compare the resulting instruction-level MMIO transaction
//! sequence.
//!
//! Every entry reaches the hardware through the production crates' public
//! API alone: [`open_probe_session_init`] takes the reset root once and the
//! later entries borrow the session's owners ([`session`]).

use oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf;
use oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile;
use oer_ieee80211_port::backend::RetryCounters;

mod calibration_leaves;
mod coex;
mod est_freq;
mod session;

/// A vendor argument outside the reviewed domain, or a session that did not
/// start: no write.
const REJECTED: u32 = u32::MAX;

/// Harness-only delay edge intercepted by the Blobray verifier before this body runs.
// SAFETY: the probe image links no ROM, so this is the one `ets_delay_us`,
// with the ROM function's C signature the vendor code calls.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn ets_delay_us(micros: u32) {
    core::hint::black_box(micros);
}

oer_probe_macros::probe! {
    /// Start the verifier session: take the reset root, split it and bring
    /// Wi-Fi up through its public client path. Zero, or why the session
    /// did not start; nothing of it is compared.
    pub fn open_probe_session_init() -> u32 {
        session::init()
    }
}

oer_probe_macros::probe! {
    pub fn open_wifi_sta_trace_hal_disable_sta_beacon_filter() {
        let _ = session::with(|session| {
            let _ = session.wifi.prepare_connected_sta_without_power_save();
        });
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_is_dscr_reload() -> u32 {
        session::with(|session| u32::from(session.wifi.rx_dma().snapshot().reload_pending))
            .unwrap_or(REJECTED)
    }
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_enable(index: u32) =>
        coex::enable_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_disable(index: u32) =>
        coex::disable_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_force(index: u32) =>
        coex::force_timer(index);
}

oer_probe_macros::probe! {
    pub fn open_coex_trace_coex_hw_timer_unforce(index: u32) =>
        coex::unforce_timer(index);
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for the complete `coex_core_pti_get`
    /// contract. The vendor ABI returns `0x102` for a null output pointer and
    /// otherwise copies one entry from its priority table, which the radio
    /// arbiter owns.
    ///
    /// # Safety
    /// A non-null `output` must point to one exclusively writable byte.
    pub unsafe fn open_coex_core_trace_pti_get(event: u32, output: *mut u8) -> u32 {
        if output.is_null() {
            return 0x102;
        }
        let Some(event) = oer_esp32s31_radio_coex::CoexEventId::new(event as u8) else {
            return 0x102;
        };
        let pti = oer_esp32s31_radio_coex::CoexPtiTable::VENDOR.pti(event);
        // SAFETY: verification profiles provide a writable caller-owned output
        // byte and compare its final state with the vendor execution.
        unsafe { output.write(pti.value()) };
        0
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for `coex_core_event_duration_get`.
    /// The vendor leaf accepts only the five events backed by `g_coex_param`,
    /// clears a non-null output for every other event, and uses `u32::MAX` as its
    /// invalid-argument status.
    ///
    /// # Safety
    /// A non-null `output` must point to an aligned, exclusively writable word.
    pub unsafe fn open_coex_core_trace_event_duration_get(event: u32, output: *mut u32) -> u32 {
        if output.is_null() {
            return u32::MAX;
        }
        let duration = oer_esp32s31_radio_coex::CoexEventId::new(event as u8)
            .and_then(|event| oer_esp32s31_radio_coex::CoexEventDurations::reviewed_vendor().duration(event));
        // SAFETY: verification profiles provide a writable caller-owned output
        // word and compare its final state with the vendor execution.
        unsafe { output.write(duration.unwrap_or(0)) };
        if duration.is_some() { 0 } else { u32::MAX }
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path projection of the complete vendor event-to-timer
    /// switch. `0xff` is the exact unmapped sentinel returned by the vendor leaf.
    pub fn open_coex_core_trace_timer_idx_get(event: u32) -> u32 {
        let Some(event) = oer_esp32s31_radio_coex::CoexEventId::new(event as u8) else {
            return 0xff;
        };
        oer_esp32s31_radio_coex::timer_index(event).map_or(0xff, |index| u32::from(index.value()))
    }
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for the complete
    /// `coex_hw_timer_freq_set(selector, divisor)` transaction, returning its
    /// acceptance as the vendor's one or zero.
    pub fn open_coex_trace_timer_freq_set(selector: u32, divisor: u32) -> u32 {
        u32::from(coex::set_timer_clock(selector, divisor))
    }
}

oer_probe_macros::probe! {
    /// Complete register projection of `coex_core_release`. The vendor ABI keeps
    /// the client in `a0` but selects the timer exclusively from event `a1`.
    pub fn open_coex_core_trace_release(_client: u32, event: u32) -> u32 {
        use oer_esp32s31_radio_coex::{CoexError, CoexEventId};

        let Some(event) = CoexEventId::new(event as u8) else {
            return 0x102;
        };
        match coex::core_release(event) {
            Some(Ok(())) => 0,
            Some(Err(CoexError::InvalidEvent)) => 0x102,
            Some(Err(_)) | None => u32::MAX,
        }
    }
}

oer_probe_macros::probe! {
    /// Register-only production projection of vendor `wifi_set_rx_policy`
    /// policies two, six, eight and nine through the role-receive HAL.
    ///
    /// The address words carry the policy's BSSID or access-point address;
    /// `mode` selects the policy-six station submode and is ignored
    /// otherwise.
    pub fn open_wifi_sta_ap_trace_wifi_set_rx_policy(
        policy: u32,
        address_low: u32,
        address_high: u32,
        mode: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{MacRoleReceivePolicy, MacStaPolicyMode};

        let low = address_low.to_le_bytes();
        let high = address_high.to_le_bytes();
        let address = [low[0], low[1], low[2], low[3], high[0], high[1]];
        let policy = match policy {
            2 => MacRoleReceivePolicy::StationDisabled,
            6 => MacRoleReceivePolicy::Station {
                bssid: address,
                mode: if mode == 2 {
                    MacStaPolicyMode::Mode2
                } else {
                    MacStaPolicyMode::Mode1
                },
            },
            8 => MacRoleReceivePolicy::AccessPoint { address },
            9 => MacRoleReceivePolicy::AccessPointDisabled,
            _ => return 0,
        };
        session::with(|session| {
            session.wifi.wifi_mac_hal().configure_role_receive_policy(policy);
            1
        })
        .unwrap_or(0)
    }
}

oer_probe_macros::probe! {
    /// The role-neutral suffix of vendor `wifi_set_rx_policy(0)`: the exact
    /// production operation that disables both receive contexts after the
    /// cold transaction published the interface addresses.
    pub fn open_wifi_sta_ap_trace_disable_all_role_receive(_policy: u32) -> u32 {
        session::with(|session| {
            oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers(
                &mut session.wifi.wifi_mac_hal(),
            );
            1
        })
        .unwrap_or(0)
    }
}

oer_probe_macros::probe! {
    /// ABI projection around the exact production normal-rate selector.
    ///
    /// The vendor entry receives a pointer-rich descriptor and stores the chosen
    /// rate at byte `0x0c`. The wrapper performs only that ABI projection; rate
    /// selection is owned by the compiled production function. A rate the
    /// selector does not take leaves the descriptor unwritten.
    pub fn open_libpp_tx_retry_trace_rc_get_rate(
        _rate_context: u32,
        descriptor_address: u32,
        initial_rate: u32,
    ) {
        use oer_esp32s31_ieee80211_mac::tx::{HtChannelWidth, TxPhyRate};

        let descriptor = descriptor_address as *mut u8;
        // SAFETY: every comparison case supplies a writable descriptor object
        // covering the vendor counter and selected-rate bytes.
        let counters = unsafe {
            RetryCounters {
                mpdu: descriptor.add(5).read(),
                short: descriptor.add(6).read(),
                long: descriptor.add(7).read(),
            }
        };
        let Some(initial) = TxPhyRate::from_code(initial_rate as u8, HtChannelWidth::Mhz20) else {
            return;
        };
        let Ok(selected) = oer_ieee80211_upper_mac::backend_retry::select_ordinary_retry_rate::<
            EspressifBackendTxProfile,
            _,
        >(initial, counters) else {
            return;
        };
        // SAFETY: the same descriptor object covers the selected-rate byte.
        unsafe { descriptor.add(0x0c).write(selected.code()) };
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_crypto_trace_hal_crypto_enable(interface: u32, _algorithm: u32, _management: u32, spp: u32) {
        let interface = match interface {
            0 => oer_esp32s31_hal::types::MacInterface::Station,
            1 => oer_esp32s31_hal::types::MacInterface::AccessPoint,
            _ => return,
        };
        let spp = match spp {
            0 => oer_esp32s31_hal::types::MacSppMode::Unauthenticated,
            1 => oer_esp32s31_hal::types::MacSppMode::Authenticated,
            _ => return,
        };
        let _ = session::with(|session| {
            session.wifi.wifi_mac_hal().configure_ccmp_association(interface, spp)
        });
    }
}

/// The MAC interface of the vendor's interface index, or `None` past the four.
fn mac_interface(interface: u32) -> Option<oer_esp32s31_hal::types::MacInterface> {
    use oer_esp32s31_hal::types::MacInterface;
    match interface {
        0 => Some(MacInterface::Station),
        1 => Some(MacInterface::AccessPoint),
        2 => Some(MacInterface::Context2),
        3 => Some(MacInterface::Context3),
        _ => None,
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_interface_trace_hal_mac_set_addr(interface: u32, address: &[u8; 6]) {
        if let Some(interface) = mac_interface(interface) {
            let _ = session::with(|session| {
                session
                    .wifi
                    .wifi_mac_hal()
                    .program_interface_address(interface, *address)
            });
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_interface_trace_hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
        if let Some(interface) = mac_interface(interface) {
            let _ = session::with(|session| {
                session
                    .wifi
                    .wifi_mac_hal()
                    .program_interface_bssid(interface, *address)
            });
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_trace_hal_disable_softap_tsf() {
        let _ = session::with(|session| {
            AccessPointTsf::new(0).stop(&mut session.wifi.wifi_mac_hal())
        });
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset(selector: u32) {
        if selector == 0 {
            let _ = session::with(|session| {
                AccessPointTsf::new(0).restart(&mut session.wifi.wifi_mac_hal())
            });
        }
    }
}

oer_probe_macros::probe! {
    /// Read the STA TSF into the caller's two output words: the production
    /// snapshot reads both words.
    ///
    /// # Safety
    /// Non-null outputs must be aligned, exclusively writable, non-overlapping words.
    pub unsafe fn open_rom_power_tsf_trace_hal_get_sta_tsf(low: *mut u32, high: *mut u32) {
        let Some(tsf) = session::with(|session| session.wifi.wifi_mac_hal().station_tsf()) else {
            return;
        };
        // SAFETY: the executable profile supplies either null or one aligned,
        // writable scratch word for each pointer, matching the ROM ABI.
        if let Some(low) = unsafe { low.as_mut() } {
            *low = tsf as u32;
        }
        // SAFETY: same closed profile contract as `low`.
        if let Some(high) = unsafe { high.as_mut() } {
            *high = (tsf >> 32) as u32;
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_dis_hw_set_freq() {
        let _ = session::with(|session| session.phy(|phy| {
            oer_esp32s31_hal::phy::frequency::set_hardware_control(phy, false)
        }));
        ets_delay_us(2);
        // Keep the delay as a non-tail edge so the executor sees the named
        // harness symbol independently of linker tail-call relaxation.
        core::hint::black_box(());
    }
}

oer_probe_macros::probe! {
    /// Execute the shipping command-memory transaction with explicit parameter
    /// bytes on the session's shared PHY. Encoding and all MMIO remain in the
    /// production HAL/PAC.
    pub fn open_phy_trace_command_memory(parameters: &[u8; 6]) {
        let inputs = oer_esp32s31_hal::phy::i2c::PhyI2cCommandMemoryInputs::new(
            parameters[0],
            parameters[1],
            parameters[2],
            parameters[3],
            parameters[4],
            parameters[5],
        );
        let _ = session::with(|session| session.phy(|phy| {
            oer_esp32s31_hal::phy::i2c::configure_command_memory(phy, inputs)
        }));
    }
}

oer_probe_macros::probe! {
    /// Initialize captured mutable parameter memory during an explicit setup phase.
    /// This harness copy supplies scenario inputs; it performs no PHY computation.
    pub fn open_phy_trace_initialize_parameters(destination: &mut [u8; 400], source: &[u8; 400]) =>
        destination.copy_from_slice(source);
}

oer_probe_macros::probe! {
    /// Exercise the production software-frequency transition on the session's
    /// shared PHY. This also exposes the preserved ordinary delay-call boundary.
    pub fn open_phy_trace_owned_software_frequency_control() =>
        open_phy_trace_dis_hw_set_freq();
}
