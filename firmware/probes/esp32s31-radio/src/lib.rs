#![no_std]

//! Link-time probes for compiled vendor/Rust MMIO comparison.
//!
//! These wrappers are test-harness artifacts, never driver entry points. Fat
//! LTO inlines the safe HAL leaf into each retained symbol so Blobray
//! verifier can compare the resulting instruction-level MMIO transaction
//! sequence.

// Leaf-level PHY comparison probes receive only a borrowed protocol-neutral
// PHY partition. They cannot acquire, release, or recover the complete Wi-Fi
// owner. Release-relevant production probes below acquire opaque HAL owners.

use oer_esp32s31_pac::{RadioPhyRegisters, WifiRadioRegisters};

use oer_esp32s31_ieee80211_mac::ap_tsf::AccessPointTsf;

mod ampdu_resort;
mod calibration_leaves;
mod calibration_projection;
mod coex;
mod i2c;
mod production_trace;
mod rx_append;

oer_probe_macros::probe! {
    /// Retain real MAC and power publication paths for SRAM placement checks.
    pub fn open_mac_irq_publication(events: u32) {
        let mut mac = oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage::new();
        let mut power = oer_esp32s31_ieee80211_mac::irq::consumer::PowerIrqStorage::new();
        let (mac, _consumer) = mac.split();
        let (power, _power_consumer) = power.split();
        // Retain the actual publication and registered-waker paths after LTO.
        core::hint::black_box(&mac).publish(events);
        core::hint::black_box(&power).publish(
            oer_esp32s31_hal::types::MacPowerInterruptObservation::from_semantic_events(
                true, false, false, false, false, false,
            ),
        );
    }
}

/// Expose externally supplied PHY registers through the HAL shared-PHY port
/// with a fresh per-call restore slot.
fn shared_phy(
    registers: &mut RadioPhyRegisters,
) -> oer_esp32s31_hal::owner::ValidationSharedPhy<'_> {
    oer_esp32s31_hal::owner::ValidationSharedPhy::new(registers)
}

/// Borrow an isolated shared radio-PHY partition.
/// The closure cannot return a borrow of this temporary validation capability.
fn with_phy<R>(call: impl FnOnce(&mut RadioPhyRegisters) -> R) -> R {
    let mut registers = oer_esp32s31_pac::validation::shared_radio_registers();
    call(registers.radio_phy_mut())
}

// Stable test-only projection protocol. Production PHY types keep their Rust
// layout private; these small C-layout records are the explicit binary
// boundary consumed by the host parity verifier.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CanonicalDot11pState {
    pub enabled: u8,
    pub configuration: u8,
}

macro_rules! canonical_byte_state {
    ($name:ident) => {
        #[repr(transparent)]
        #[derive(Clone, Copy)]
        pub struct $name {
            pub value: u8,
        }
    };
}

canonical_byte_state!(CanonicalCurrentLevelState);
canonical_byte_state!(CanonicalBtPowerTrackingState);
canonical_byte_state!(CanonicalBleChannelBaseState);
canonical_byte_state!(CanonicalInitializationParameterState);
canonical_byte_state!(CanonicalSlowTxPowerTrackingState);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CanonicalTemperatureTrackingState {
    pub first: u8,
    pub second: u8,
}

#[panic_handler]
#[allow(
    clippy::disallowed_methods,
    reason = "isolated verification image has no executor; panic is a terminal probe failure"
)]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Harness-only delay edge intercepted by the Blobray verifier before this body runs.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn ets_delay_us(micros: u32) {
    core::hint::black_box(micros);
}

oer_probe_macros::probe! {
    /// Harness delay adapter preserving integer inputs around the explicit event edge.
    /// The guest body executes these saves/restores; the inner model supplies only
    /// requested-time observation, never production completion or register values.
    #[unsafe(naked)]
    pub fn open_phy_trace_preserving_delay(_micros: u32) {
        core::arch::naked_asm!(
            "addi sp, sp, -64",
            "sw ra, 0(sp)",
            "sw t0, 4(sp)",
            "sw t1, 8(sp)",
            "sw t2, 12(sp)",
            "sw t3, 16(sp)",
            "sw t4, 20(sp)",
            "sw t5, 24(sp)",
            "sw t6, 28(sp)",
            "sw a0, 32(sp)",
            "sw a1, 36(sp)",
            "sw a2, 40(sp)",
            "sw a3, 44(sp)",
            "sw a4, 48(sp)",
            "sw a5, 52(sp)",
            "sw a6, 56(sp)",
            "sw a7, 60(sp)",
            "call open_phy_trace_delay_event",
            "lw ra, 0(sp)",
            "lw t0, 4(sp)",
            "lw t1, 8(sp)",
            "lw t2, 12(sp)",
            "lw t3, 16(sp)",
            "lw t4, 20(sp)",
            "lw t5, 24(sp)",
            "lw t6, 28(sp)",
            "lw a0, 32(sp)",
            "lw a1, 36(sp)",
            "lw a2, 40(sp)",
            "lw a3, 44(sp)",
            "lw a4, 48(sp)",
            "lw a5, 52(sp)",
            "lw a6, 56(sp)",
            "lw a7, 60(sp)",
            "addi sp, sp, 64",
            "ret",
        );
    }
}

oer_probe_macros::probe! {
    /// Modeled requested-delay observation, reached through the captured ABI adapter.
    pub fn open_phy_trace_delay_event(micros: u32) {
        core::hint::black_box(micros);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bt_index_to_bb(index: u32) -> u32 =>
        oer_esp32s31_phy::calibration::bluetooth::bluetooth_gain_index_to_baseband(index);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bt_bb_to_index(baseband: u32) -> u32 =>
        oer_esp32s31_phy::calibration::bluetooth::bluetooth_baseband_to_gain_index(baseband);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_enabled(&mut shared_phy(registers), false);
}

oer_probe_macros::probe! {
    pub fn open_libpp_trace_hal_mac_interrupt_ret_get_event() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_interrupt_get_event()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_trace_hal_mac_interrupt_ret_clr_event(events: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_interrupt_clr_event(events)
    }
}

oer_probe_macros::probe! {
    pub fn open_wifi_sta_trace_hal_disable_sta_beacon_filter() {
        // SAFETY: these validation-only PAC capabilities are used only by this
        // isolated probe image. The called helper is the production transaction.
        let mut owner = oer_esp32s31_hal::validation::runtime_owner();
        let _ = oer_esp32s31_hal::validation::mac_interrupt_setup()
            .prepare_connected_sta_without_power_save(&mut owner);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_power_irq_trace_hal_pwr_interrupt_get_event() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_pwr_interrupt_get_event()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event(events: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_pwr_interrupt_clr_event(events)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_disable() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_disable(0);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_enable() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_enable(0);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_set_base(address: u32) {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_set_base(address);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_is_dscr_reload() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_rx_is_dscr_reload()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_rx_trace_hal_mac_rx_set_dscr_reload() {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        let _ = oer_esp32s31_hal::validation::hal_mac_rx_set_dscr_reload(0);
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

// External coexistence leaves of `libcoexist.a[hal_external_coexist.o]`.
// The vendor keeps the follower mode in its local `s_external_coex_is_slv_mode`
// and asks `esp_coex_external_get_wire_type` for the wires; the probes take
// both as words. A mode or wire outside the production domain returns
// `REJECTED` without a write.

oer_probe_macros::probe! {
    /// `hal_set_extern_pti_mode(mode)` with the case's wires.
    pub fn open_coex_external_trace_pti_mode(mode: u32, wire: u32) -> u32 {
        oer_esp32s31_hal::validation::coex_external::hal_set_extern_pti_mode(mode, wire)
    }
}

oer_probe_macros::probe! {
    /// `hal_set_extern_pti(first, second, third)` in the case's mode.
    pub fn open_coex_external_trace_set_pti(
        mode: u32,
        wire: u32,
        first: u32,
        second: u32,
        third: u32,
    ) -> u32 {
        oer_esp32s31_hal::validation::coex_external::hal_set_extern_pti(
            mode, wire, first, second, third,
        )
    }
}

oer_probe_macros::probe! {
    /// `hal_clr_extern_pti` in the case's mode.
    pub fn open_coex_external_trace_clear_pti(mode: u32) -> u32 {
        oer_esp32s31_hal::validation::coex_external::hal_clr_extern_pti(mode)
    }
}

oer_probe_macros::probe! {
    /// `hal_enable_extern_coex`.
    pub fn open_coex_external_trace_enable() -> u32 {
        oer_esp32s31_hal::validation::coex_external::hal_set_extern_coex_enabled(true)
    }
}

oer_probe_macros::probe! {
    /// `hal_disable_extern_coex`.
    pub fn open_coex_external_trace_disable() -> u32 {
        oer_esp32s31_hal::validation::coex_external::hal_set_extern_coex_enabled(false)
    }
}

oer_probe_macros::probe! {
    /// `ic_set_extern_coex_params(mode, {delay, validate_high})`: the work
    /// mode, then the grant delay and validity.
    pub fn open_coex_external_trace_params(
        mode: u32,
        wire: u32,
        delay_us: u32,
        validate_high: u32,
    ) -> u32 {
        oer_esp32s31_hal::validation::coex_external::ic_set_extern_coex_params(
            mode,
            wire,
            delay_us,
            validate_high != 0,
        )
    }
}

oer_probe_macros::probe! {
    /// `ieee802154_txon_delay_set` of libbtbb: the IEEE 802.15.4 MAC
    /// timing delays of the MAC foundation. Always zero.
    pub fn open_ieee802154_btbb_trace_txon_delay() -> u32 {
        oer_esp32s31_hal::ieee802154::coex_trace::set_txon_delay()
    }
}

/// The priority the shared coexistence table assigns an IEEE 802.15.4
/// level (1 high, 2 middle, 3 low, 4 idle), as libcoexist's local
/// `coex_ieee802154_pti_get` reads it.
fn ieee802154_level_pti(level: u32) -> Option<u32> {
    use oer_esp32s31_hal::coex::{CoexPtiTable, Ieee802154CoexLevel};
    let level = match level {
        1 => Ieee802154CoexLevel::High,
        2 => Ieee802154CoexLevel::Middle,
        3 => Ieee802154CoexLevel::Low,
        4 => Ieee802154CoexLevel::Idle,
        _ => return None,
    };
    Some(u32::from(
        CoexPtiTable::VENDOR.ieee802154_pti(level).value(),
    ))
}

oer_probe_macros::probe! {
    /// `esp_coex_ieee802154_txrx_pti_set` of libcoexist: the level's
    /// priority from the shared table, written as the TX/RX PTI. Levels
    /// outside the four the vendor admits return `u32::MAX` without a write.
    pub fn open_ieee802154_coex_trace_txrx_level(level: u32) -> u32 {
        ieee802154_level_pti(level)
            .map_or(u32::MAX, oer_esp32s31_hal::ieee802154::coex_trace::set_txrx_pti)
    }
}

oer_probe_macros::probe! {
    /// `esp_coex_ieee802154_ack_pti_set` of libcoexist: the level's priority
    /// from the shared table, written as the ACK PTI.
    pub fn open_ieee802154_coex_trace_ack_level(level: u32) -> u32 {
        ieee802154_level_pti(level)
            .map_or(u32::MAX, oer_esp32s31_hal::ieee802154::coex_trace::set_ack_pti)
    }
}

oer_probe_macros::probe! {
    /// `hal_set_IEEE802154_TXRX_pti` of libcoexist: the IEEE 802.15.4 MAC
    /// owner's TX/RX PTI write. Zero, or `u32::MAX` without a write for a
    /// priority outside the four-bit domain.
    pub fn open_ieee802154_coex_trace_txrx_pti(pti: u32) -> u32 {
        oer_esp32s31_hal::ieee802154::coex_trace::set_txrx_pti(pti)
    }
}

oer_probe_macros::probe! {
    /// `hal_set_IEEE802154_ACK_pti` of libcoexist: the IEEE 802.15.4 MAC
    /// owner's ACK PTI write. Zero, or `u32::MAX` without a write for a
    /// priority outside the four-bit domain.
    pub fn open_ieee802154_coex_trace_ack_pti(pti: u32) -> u32 {
        oer_esp32s31_hal::ieee802154::coex_trace::set_ack_pti(pti)
    }
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
    /// Compiled production-path probe for the complete `coex_hw_timer_set`
    /// transaction. The public vendor ABI is `(index, client, pti, latency,
    /// duration)`; notably, duration is written to the primary word before
    /// latency is written to the secondary word.
    pub fn open_coex_set_trace_coex_hw_timer_set(
        index: u32,
        client: u32,
        pti: u32,
        latency: u32,
        duration: u32,
        is_real_chip: u32,
    ) {
        use oer_esp32s31_radio_coex::{CoexClient, CoexPti, CoexTimerIndex};

        let Ok(index) = CoexTimerIndex::new(index as u8) else {
            return;
        };
        let Some(pti) = CoexPti::new(pti as u8) else {
            return;
        };
        let client = if client == 0 {
            CoexClient::Bluetooth
        } else {
            CoexClient::Wifi
        };
        let _ = coex::program_timer(
            is_real_chip != 0,
            index,
            client,
            pti,
            latency,
            duration,
        );
    }
}

/// The radio domain of a fresh probe root, kept for the whole entry and
/// never dropped: the vendor leaves keep no domain, so its teardown is not
/// compared.
fn grant_protect_domain() -> core::mem::ManuallyDrop<oer_esp32s31_hal::domain::RadioDomain<()>> {
    core::mem::ManuallyDrop::new(
        oer_esp32s31_hal::root::RadioHardware::for_probe()
            .split(())
            .0,
    )
}

/// The vendor status of a grant-protect edge: zero, as `coex_core_request`
/// and `coex_core_release` return for event 48.
fn grant_protect_status(result: Result<(), oer_esp32s31_hal::coex::PhyGrantProtectError>) -> u32 {
    match result {
        Ok(()) => 0,
        Err(_) => u32::MAX,
    }
}

oer_probe_macros::probe! {
    /// `phy_acquire_grant_protect`: the arbiter's
    /// `coex_core_request(2, 48, 0, 0)` on timer 5, client field 0. Zero
    /// timing arguments take no clock sample.
    pub fn open_coex_trace_phy_acquire_grant_protect() -> u32 {
        let mut radio = grant_protect_domain();
        grant_protect_status(radio.acquire_phy_grant_protect())
    }
}

oer_probe_macros::probe! {
    /// `phy_release_grant_protect`: the arbiter's `coex_core_release(2, 48)`,
    /// which disables timer 5, from a programmed request.
    pub fn open_coex_trace_phy_release_grant_protect() -> u32 {
        let mut radio = grant_protect_domain();
        radio.hold_phy_grant_protect_for_host_test();
        grant_protect_status(radio.release_phy_grant_protect())
    }
}

oer_probe_macros::probe! {
    /// Complete register projection of `coex_core_request` under the reviewed
    /// Rust lifecycle precondition that the COEX owner is enabled.
    pub fn open_coex_core_trace_request(
        client: u32,
        event: u32,
        latency: u32,
        duration: u32,
        is_real_chip: u32,
    ) -> u32 {
        use oer_esp32s31_radio_coex::{CoexClientRequest, CoexError, CoexEventId};

        let Some(event) = CoexEventId::new(event as u8) else {
            return 0x102;
        };
        let request = CoexClientRequest {
            event,
            latency,
            duration,
        };
        let result =
            coex::core_request(is_real_chip != 0, client != 0, request);
        match result {
            Ok(_) => 0,
            Err(CoexError::InvalidEvent) => 0x102,
            Err(_) => u32::MAX,
        }
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
            Ok(_) => 0,
            Err(CoexError::InvalidEvent) => 0x102,
            Err(_) => u32::MAX,
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_set_cca(value: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_tx_set_cca(value)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state() -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_get_txq_in_trig_flow_state()
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_is_txq_enabled(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_is_txq_enabled(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_is_txq_valid(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_is_txq_valid(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_set_txq_invalid(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_set_txq_invalid(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_txq_disable(queue: u32) -> u32 {
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_txq_disable(queue)
    }
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_initialize_cts() =>
        oer_esp32s31_hal::validation::initialize_mac_software_cts();
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_control_configure_rts(
        queue: u32,
        enabled: u32,
        _unused: u32,
        duration_threshold: u32,
        threshold_bytes: u32,
    ) {
        assert!(enabled <= 1);
        assert!(duration_threshold <= 1);
        oer_esp32s31_hal::validation::configure_mac_tx_rts(
            queue,
            enabled != 0,
            (duration_threshold != 0).then_some(threshold_bytes as u16),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_tx_protection_control_disable_he_threshold() =>
        oer_esp32s31_hal::validation::disable_mac_he_rts_threshold();
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_publish(queue: u8) =>
        oer_esp32s31_hal::validation::ordinary_tx_publish(queue);
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_acknowledge(selector: u32, queue: u8) {
        assert_eq!(selector, 2);
        oer_esp32s31_hal::validation::ordinary_tx_acknowledge_completion(queue);
    }
}

oer_probe_macros::probe! {
    pub fn open_ordinary_tx_ownership_disable(queue: u32) -> u32 =>
        oer_esp32s31_hal::validation::hal_mac_txq_disable(queue);
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_config_edca(
        _vendor_config_address: u32,
        queue: u32,
        aifsn: u32,
        contention_window: u32,
        interface: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::MacInterface;
        let interface = match interface {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            2 => MacInterface::Context2,
            3 => MacInterface::Context3,
            _ => panic!("verification MAC interface is out of range"),
        };
        // The vendor side decodes these semantic arguments from its pointer-rich
        // ABI object. The Rust probe receives the reviewed projection directly;
        // every case keeps both representations and exact MMIO comparison proves
        // that the projection agrees with the vendor object.
        // SAFETY: this validation-only function is the sole user of the stolen
        // peripheral in its isolated probe image.
        oer_esp32s31_hal::validation::hal_mac_tx_config_edca(
            queue,
            aifsn as u8,
            contention_window as u16,
            interface,
        )
    }
}

#[repr(C)]
struct CanonicalHtTxParameters {
    queue: u32,
    descriptor_head: u32,
    mcs: u32,
    guard_interval: u32,
    channel_width: u32,
    format: u32,
    length: u32,
    descriptor_count: u32,
    protection_spacing: u32,
    timeout: u32,
    scheduler_priority: u32,
    packet_priority: u32,
    priority_count: u32,
    aifsn: u32,
    contention_window: u32,
    interface: u32,
    hardware_key_selector: u32,
    txop: u32,
}

#[repr(C)]
struct CanonicalLegacyTxParameters {
    queue: u32,
    descriptor_head: u32,
    rate: u32,
    signal: u32,
    timeout: u32,
    scheduler_priority: u32,
    packet_priority: u32,
    priority_count: u32,
    aifsn: u32,
    contention_window: u32,
    interface: u32,
    group_receiver: u32,
    hardware_key_selector: u32,
}

#[repr(C)]
struct CanonicalHeTxParameters {
    queue: u32,
    descriptor_head: u32,
    mcs: u32,
    guard_interval_and_ltf: u32,
    ldpc: u32,
    dcm: u32,
    format: u32,
    apep_length: u32,
    descriptor_count: u32,
    bss_color: u32,
    spatial_reuse: u32,
    protection_spacing: u32,
    timeout: u32,
    scheduler_priority: u32,
    packet_priority: u32,
    priority_count: u32,
    aifsn: u32,
    contention_window: u32,
    interface: u32,
    hardware_key_selector: u32,
}

struct ValidationPreparedTxDma(u32);

// SAFETY: this verification-only authority is instantiated solely from the
// profile-owned stable descriptor memory used by the isolated instruction
// trace. It is never exposed to production code or real hardware.
unsafe impl oer_base_memory::PreparedTxDma for ValidationPreparedTxDma {
    fn descriptor_head(&self) -> u32 {
        self.0
    }
}

oer_probe_macros::probe! {
    /// Reviewed ABI projection around the exact compiled production HT queue
    /// transaction. The vendor side receives its pointer-rich PP context at `a0`;
    /// the Rust side receives semantic HT parameters at the same address. The
    /// PAC constructs every register word from those reviewed values.
    pub fn open_libpp_tx_trace_hal_mac_tx_set_ppdu(
        program_address: u32,
        _vendor_auxiliary: u32,
        power_table: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{
            MacHtChannelWidth, MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate,
            MacHtTxFormat, MacHtTxParameters, MacHtTxProgram, MacInterface, MacTxControlFrame,
            MacTxProtection,
        };

        let parameters = program_address as *const CanonicalHtTxParameters;
        // SAFETY: the verification profile supplies one initialized, aligned and
        // immutable CanonicalHtTxParameters for the duration of this call.
        let parameters = unsafe { &*parameters };
        let interface = match parameters.interface {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            2 => MacInterface::Context2,
            3 => MacInterface::Context3,
            _ => panic!("verification MAC interface is out of range"),
        };
        let mcs = match parameters.mcs {
            0 => MacHtMcs::Mcs0,
            1 => MacHtMcs::Mcs1,
            2 => MacHtMcs::Mcs2,
            3 => MacHtMcs::Mcs3,
            4 => MacHtMcs::Mcs4,
            5 => MacHtMcs::Mcs5,
            6 => MacHtMcs::Mcs6,
            7 => MacHtMcs::Mcs7,
            _ => panic!("verification HT MCS is out of range"),
        };
        let guard_interval = match parameters.guard_interval {
            0 => MacHtGuardInterval::Long800Ns,
            1 => MacHtGuardInterval::Short400Ns,
            _ => panic!("verification HT guard interval is out of range"),
        };
        let channel_width = match parameters.channel_width {
            0 => MacHtChannelWidth::Mhz20,
            1 => MacHtChannelWidth::Mhz40,
            _ => panic!("verification HT channel width is out of range"),
        };
        let format = match parameters.format {
            0 => MacHtTxFormat::SingleMpdu,
            1 => MacHtTxFormat::Ampdu,
            _ => panic!("verification HT format is out of range"),
        };
        let protection_spacing = match parameters.protection_spacing {
            0 => MacHtProtectionSpacing::Density0To4,
            1 => MacHtProtectionSpacing::Density5,
            2 => MacHtProtectionSpacing::Density6,
            3 => MacHtProtectionSpacing::Density7,
            _ => panic!("verification HT protection spacing is out of range"),
        };
        // Production publishes the vendor `mac_tx_get_rts_rate` image for an
        // unprotected PPDU; take it from the compiled driver mapping.
        let ht = oer_esp32s31_ieee80211_mac::tx::HtRate::new(
            oer_esp32s31_ieee80211_mac::tx::HtMcs::from_index(parameters.mcs as u8)
                .expect("verification HT MCS is in range"),
            match guard_interval {
                MacHtGuardInterval::Long800Ns => {
                    oer_esp32s31_ieee80211_mac::tx::HtGuardInterval::Long800Ns
                }
                MacHtGuardInterval::Short400Ns => {
                    oer_esp32s31_ieee80211_mac::tx::HtGuardInterval::Short400Ns
                }
            },
            match channel_width {
                MacHtChannelWidth::Mhz20 => oer_esp32s31_ieee80211_mac::tx::HtChannelWidth::Mhz20,
                MacHtChannelWidth::Mhz40 => oer_esp32s31_ieee80211_mac::tx::HtChannelWidth::Mhz40,
            },
        );
        let control = ht.vendor_control_rate();
        let control_rate = control.pac_rate();
        // The power table holds one (primary, alternate) byte pair per rate
        // code; production selects the codes.
        let power = |code: u8| {
            let pair = (power_table + 2 * u32::from(code)) as *const [u8; 2];
            // SAFETY: the verification profile supplies the whole table.
            unsafe { pair.read() }
        };
        let [data_power_primary, data_power_alternate] = power(ht.power_lookup_code());
        let [rts_power_primary, rts_power_alternate] = power(control.code());
        let dma = ValidationPreparedTxDma(parameters.descriptor_head);
        let program = MacHtTxProgram::new(
            &dma,
            MacHtTxParameters {
                control: MacTxControlFrame {
                    protection: MacTxProtection::None,
                    rate: control_rate,
                    power_primary: rts_power_primary,
                    power_alternate: rts_power_alternate,
                },
                rate: MacHtRate {
                    mcs,
                    guard_interval,
                    channel_width,
                },
                format,
                length: parameters.length as u16,
                descriptor_count: parameters.descriptor_count as u8,
                data_power_primary,
                data_power_alternate,
                protection_spacing,
                timeout: parameters.timeout as u16,
                scheduler_priority: parameters.scheduler_priority as u8,
                packet_priority: parameters.packet_priority as u8,
                priority_count: parameters.priority_count as u16,
                aifsn: parameters.aifsn as u8,
                contention_window: parameters.contention_window as u16,
                interface,
                hardware_key_selector: parameters.hardware_key_selector as u8,
                txop: parameters.txop != 0,
            },
        )
        .expect("verification HT parameters are in the reviewed PAC domain");
        oer_esp32s31_hal::validation::hal_mac_tx_set_ppdu(parameters.queue as u8, program)
    }
}

oer_probe_macros::probe! {
    /// The legacy responsibility of `hal_mac_tx_set_ppdu` over canonical
    /// parameters; powers come from the power table at the codes production
    /// selects.
    pub fn open_libpp_tx_trace_hal_mac_tx_set_legacy_ppdu(
        program_address: u32,
        _vendor_auxiliary: u32,
        power_table: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{
            MacInterface, MacLegacyTxParameters, MacLegacyTxProgram, MacTxControlFrame,
            MacTxProtection,
        };
        use oer_esp32s31_ieee80211_mac::tx::LegacyRate;

        let parameters = program_address as *const CanonicalLegacyTxParameters;
        // SAFETY: the verification profile supplies one initialized, aligned
        // and immutable parameter object for the duration of this call.
        let parameters = unsafe { &*parameters };
        let interface = match parameters.interface {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            2 => MacInterface::Context2,
            3 => MacInterface::Context3,
            _ => panic!("verification MAC interface is out of range"),
        };
        let rate = LegacyRate::from_code(parameters.rate as u8)
            .expect("verification legacy rate is in range");
        let control = rate.vendor_control_rate();
        let power = |code: u8| {
            let pair = (power_table + 2 * u32::from(code)) as *const [u8; 2];
            // SAFETY: the verification profile supplies the whole table.
            unsafe { pair.read() }
        };
        let [data_power, _] = power(rate.code());
        let [rts_power_primary, rts_power_alternate] = power(control.code());
        let dma = ValidationPreparedTxDma(parameters.descriptor_head);
        let program = MacLegacyTxProgram::new(
            &dma,
            MacLegacyTxParameters {
                control: MacTxControlFrame {
                    protection: MacTxProtection::None,
                    rate: control.pac_rate(),
                    power_primary: rts_power_primary,
                    power_alternate: rts_power_alternate,
                },
                rate: rate.pac_rate(),
                signal: parameters.signal as u16,
                data_power,
                timeout: parameters.timeout as u16,
                scheduler_priority: parameters.scheduler_priority as u8,
                packet_priority: parameters.packet_priority as u8,
                priority_count: parameters.priority_count as u16,
                aifsn: parameters.aifsn as u8,
                contention_window: parameters.contention_window as u16,
                interface,
                response: if parameters.group_receiver != 0 {
                    oer_esp32s31_hal::types::MacLegacyTxResponse::None
                } else {
                    oer_esp32s31_hal::types::MacLegacyTxResponse::Ack
                },
                hardware_key_selector: parameters.hardware_key_selector as u8,
            },
        )
        .expect("verification legacy parameters are in the reviewed PAC domain");
        oer_esp32s31_hal::validation::hal_mac_tx_set_legacy_ppdu(parameters.queue as u8, program)
    }
}

oer_probe_macros::probe! {
    /// The HE SU responsibility of `hal_mac_tx_set_ppdu` over canonical
    /// parameters; powers come from the power table at the codes production
    /// selects.
    pub fn open_libpp_tx_trace_hal_mac_tx_set_he_ppdu(
        program_address: u32,
        _vendor_auxiliary: u32,
        power_table: u32,
    ) -> u32 {
        use oer_esp32s31_hal::types::{
            MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
            MacHeTxParameters, MacHeTxProgram, MacInterface, MacTxControlFrame, MacTxProtection,
        };
        use oer_esp32s31_ieee80211_mac::{
            rx::HeGuardIntervalAndLtf,
            tx::{HeMcs, HeRate},
        };

        let parameters = program_address as *const CanonicalHeTxParameters;
        // SAFETY: the verification profile supplies one initialized, aligned
        // and immutable parameter object for the duration of this call.
        let parameters = unsafe { &*parameters };
        let interface = match parameters.interface {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            2 => MacInterface::Context2,
            3 => MacInterface::Context3,
            _ => panic!("verification MAC interface is out of range"),
        };
        let mcs = match parameters.mcs {
            0 => MacHeMcs::Mcs0,
            1 => MacHeMcs::Mcs1,
            2 => MacHeMcs::Mcs2,
            3 => MacHeMcs::Mcs3,
            4 => MacHeMcs::Mcs4,
            5 => MacHeMcs::Mcs5,
            6 => MacHeMcs::Mcs6,
            7 => MacHeMcs::Mcs7,
            8 => MacHeMcs::Mcs8,
            9 => MacHeMcs::Mcs9,
            _ => panic!("verification HE MCS is out of range"),
        };
        let (gi_ltf, driver_gi_ltf) = match parameters.guard_interval_and_ltf {
            0 => (MacHeGuardIntervalAndLtf::OneLtf800Ns, HeGuardIntervalAndLtf::OneLtf800Ns),
            1 => (MacHeGuardIntervalAndLtf::TwoLtf800Ns, HeGuardIntervalAndLtf::TwoLtf800Ns),
            2 => (MacHeGuardIntervalAndLtf::TwoLtf1600Ns, HeGuardIntervalAndLtf::TwoLtf1600Ns),
            3 => (MacHeGuardIntervalAndLtf::FourLtf3200Ns, HeGuardIntervalAndLtf::FourLtf3200Ns),
            _ => panic!("verification HE guard interval is out of range"),
        };
        let format = match parameters.format {
            0 => MacHeTxFormat::Smpdu,
            1 => MacHeTxFormat::Ampdu,
            _ => panic!("verification HE format is out of range"),
        };
        let rate = HeRate::new(
            HeMcs::from_index(parameters.mcs as u8).expect("verification HE MCS is in range"),
            driver_gi_ltf,
        );
        let control = rate.vendor_control_rate();
        let power = |code: u8| {
            let pair = (power_table + 2 * u32::from(code)) as *const [u8; 2];
            // SAFETY: the verification profile supplies the whole table.
            unsafe { pair.read() }
        };
        let [data_power_primary, data_power_alternate] = power(rate.power_lookup_code());
        let [rts_power_primary, rts_power_alternate] = power(control.code());
        let dma = ValidationPreparedTxDma(parameters.descriptor_head);
        let program = MacHeTxProgram::new(
            &dma,
            MacHeTxParameters {
                control: MacTxControlFrame {
                    protection: MacTxProtection::None,
                    rate: control.pac_rate(),
                    power_primary: rts_power_primary,
                    power_alternate: rts_power_alternate,
                },
                rate: MacHeRate {
                    mcs,
                    guard_interval_and_ltf: gi_ltf,
                    fec_coding: if parameters.ldpc != 0 {
                        MacHeFecCoding::Ldpc
                    } else {
                        MacHeFecCoding::Bcc
                    },
                    dcm: parameters.dcm != 0,
                },
                format,
                apep_length: parameters.apep_length as u16,
                descriptor_count: parameters.descriptor_count as u8,
                bss_color: parameters.bss_color as u8,
                spatial_reuse: parameters.spatial_reuse as u8,
                software_he_control: None,
                data_power_primary,
                data_power_alternate,
                protection_spacing: parameters.protection_spacing as u16,
                timeout: parameters.timeout as u16,
                scheduler_priority: parameters.scheduler_priority as u8,
                packet_priority: parameters.packet_priority as u8,
                priority_count: parameters.priority_count as u16,
                aifsn: parameters.aifsn as u8,
                contention_window: parameters.contention_window as u16,
                interface,
                hardware_key_selector: parameters.hardware_key_selector as u8,
            },
        )
        .expect("verification HE parameters are in the reviewed PAC domain");
        oer_esp32s31_hal::validation::hal_mac_tx_set_he_ppdu(parameters.queue as u8, program)
    }
}

#[repr(C)]
pub struct CanonicalTxBlockAck {
    pub control: u8,
    pub reserved: u8,
    pub starting_sequence: u16,
    pub bitmap_low: u32,
    pub bitmap_high: u32,
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_trace_hal_mac_tx_get_blockack(queue: u32, output_address: u32) -> u32 {
        let payload = oer_esp32s31_hal::validation::hal_mac_tx_get_blockack(queue as u8)
            .expect("profile constrains ordinary TX queue 0..=3");
        let output = output_address as *mut CanonicalTxBlockAck;
        // SAFETY: the verification profile supplies one initialized, writable
        // twelve-byte output object for the duration of this call.
        unsafe {
            (*output).control = payload.control();
            (*output).starting_sequence = payload.starting_sequence();
            (*output).bitmap_low = payload.bitmap_low();
            (*output).bitmap_high = payload.bitmap_high();
        }
        0
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
        oer_esp32s31_hal::validation::runtime_owner()
            .wifi_mac_hal()
            .configure_role_receive_policy(policy);
        1
    }
}

oer_probe_macros::probe! {
    /// The role-neutral suffix of vendor `wifi_set_rx_policy(0)`: the exact
    /// production operation that disables both receive contexts after the
    /// cold transaction published the interface addresses.
    pub fn open_wifi_sta_ap_trace_disable_all_role_receive(_policy: u32) -> u32 {
        oer_esp32s31_ieee80211_mac::sta_ap_registers::disable_all_role_receive_registers(
            &mut oer_esp32s31_hal::validation::runtime_owner().wifi_mac_hal(),
        );
        1
    }
}

oer_probe_macros::probe! {
    /// ABI projection around the exact production normal-rate selector.
    ///
    /// The vendor entry receives a pointer-rich descriptor and stores the chosen
    /// rate at byte `0x0c`. The wrapper performs only that ABI projection; rate
    /// selection is owned by the compiled production function.
    pub fn open_libpp_tx_retry_trace_rc_get_rate(
        _rate_context: u32,
        descriptor_address: u32,
        initial_rate: u32,
    ) {
        use oer_esp32s31_ieee80211_mac::tx::{
            HtChannelWidth, TxPhyRate,
            runtime::{OrdinaryRetryCounters, select_ordinary_retry_rate},
        };

        let descriptor = descriptor_address as *mut u8;
        // SAFETY: every comparison case supplies a writable descriptor object
        // covering the vendor counter and selected-rate bytes.
        let counters = unsafe {
            OrdinaryRetryCounters {
                mpdu: descriptor.add(5).read(),
                short: descriptor.add(6).read(),
                long: descriptor.add(7).read(),
            }
        };
        // The byte carries no channel width; the selected code does not
        // depend on it.
        let initial = TxPhyRate::from_code(initial_rate as u8, HtChannelWidth::Mhz20)
            .expect("reviewed rcGetRate cases start from a legacy or HT rate");
        let selected = select_ordinary_retry_rate(initial, counters)
            .expect("reviewed rcGetRate cases remain inside their schedule");
        // SAFETY: the same case-owned descriptor covers byte 0x0c.
        unsafe { descriptor.add(0x0c).write(selected.code()) };
    }
}

oer_probe_macros::probe! {
    /// ABI projection around the production retry-rate selector for an HE
    /// single-MPDU: the initial and selected rates are 802.11ax
    /// rate-control bytes.
    pub fn open_libpp_tx_retry_trace_rc_get_he_rate(
        _rate_context: u32,
        descriptor_address: u32,
        initial_rate: u32,
    ) {
        use oer_esp32s31_ieee80211_mac::{
            rate::schedule::RateScheduleKind,
            rx::HeGuardIntervalAndLtf,
            tx::{
                HtChannelWidth, TxPhyRate,
                runtime::{OrdinaryRetryCounters, select_ordinary_retry_rate},
            },
        };

        let descriptor = descriptor_address as *mut u8;
        // SAFETY: every comparison case supplies a writable descriptor object
        // covering the vendor counter and selected-rate bytes.
        let counters = unsafe {
            OrdinaryRetryCounters {
                mpdu: descriptor.add(5).read(),
                short: descriptor.add(6).read(),
                long: descriptor.add(7).read(),
            }
        };
        // The byte carries no 800-ns LTF count; the selected byte does not
        // depend on it.
        let initial = TxPhyRate::from_rate_control_code(
            RateScheduleKind::Dot11Ax,
            initial_rate as u8,
            HtChannelWidth::Mhz20,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
        )
        .expect("reviewed HE rcGetRate cases start from an 802.11ax rate");
        let selected = select_ordinary_retry_rate(initial, counters)
            .expect("reviewed rcGetRate cases remain inside their schedule");
        let code = selected
            .rate_control_code()
            .expect("an 802.11ax schedule selects a rate-control byte");
        // SAFETY: the same case-owned descriptor covers byte 0x0c.
        unsafe { descriptor.add(0x0c).write(code) };
    }
}

/// The ordinary retry owner and runtime policy one retry sequence evolves.
struct RetryProbeState(
    core::cell::UnsafeCell<
        Option<(
            oer_esp32s31_ieee80211_mac::tx::runtime::OrdinaryMpduRetryState,
            oer_esp32s31_ieee80211_mac::tx::runtime::WifiTxRuntimePolicy,
            oer_esp32s31_ieee80211_mac::tx::LegacyTxQueue,
        )>,
    >,
);

// SAFETY: Blobray executes this probe image on one thread and invokes its
// stateful retry entries serially.
unsafe impl Sync for RetryProbeState {}

static RETRY_STATE: RetryProbeState = RetryProbeState(core::cell::UnsafeCell::new(None));

/// Decisions the retry step returns: complete, retry, and retry with the
/// 802.11 Retry bit.
const RETRY_COMPLETE: u32 = 0;
const RETRY_AGAIN: u32 = 1;
const RETRY_AGAIN_WITH_BIT: u32 = 2;
/// Frame-control flags byte and its Retry bit.
const FRAME_FLAGS: usize = 1;
const FRAME_RETRY: u8 = 0x08;

oer_probe_macros::probe! {
    /// Start one ordinary MPDU of queue `queue` with the vendor default
    /// runtime policy.
    pub fn open_libpp_tx_retry_trace_reset(queue: u32, mpdu_retry_limit: u32, long_frame: u32) -> u32 {
        use oer_esp32s31_ieee80211_mac::tx::{
            LegacyRate, LegacyTxQueue, TxPhyRate,
            runtime::{OrdinaryFrameClass, OrdinaryMpduRetryState, WifiTxRuntimePolicy},
        };
        let queue = match queue {
            0 => LegacyTxQueue::Voice,
            1 => LegacyTxQueue::Video,
            2 => LegacyTxQueue::BestEffort,
            3 => LegacyTxQueue::Background,
            _ => panic!("verification ordinary queue is out of range"),
        };
        let state = OrdinaryMpduRetryState::new(
            queue,
            TxPhyRate::Legacy(LegacyRate::Ofdm6M),
            mpdu_retry_limit as u8,
            if long_frame != 0 {
                OrdinaryFrameClass::Long
            } else {
                OrdinaryFrameClass::Short
            },
        )
        .expect("verification retry limit is nonzero");
        // SAFETY: see `RetryProbeState`.
        unsafe {
            *RETRY_STATE.0.get() = Some((state, WifiTxRuntimePolicy::vendor_defaults(), queue))
        };
        0
    }
}

oer_probe_macros::probe! {
    /// Apply one completion to the retry owner and project its state in the
    /// vendor layout: descriptor counters at 5..8, the queue's contention
    /// exponent at context byte 8 and the frame's Retry bit.
    pub fn open_libpp_tx_retry_trace_step(
        disposition: u32,
        context: u32,
        descriptor: u32,
        buffer: u32,
    ) -> u32 {
        use oer_esp32s31_ieee80211_mac::tx::runtime::OrdinaryRetryDecision;
        use oer_ieee80211_lower_mac::TxStatus;
        let disposition = match disposition {
            0 => TxStatus::AckTimeout,
            1 => TxStatus::CtsTimeout,
            2 => TxStatus::Collision,
            3 => TxStatus::Success,
            _ => panic!("verification disposition is out of range"),
        };
        // SAFETY: see `RetryProbeState`; the reset entry ran first.
        let (state, policy, queue) = unsafe { (*RETRY_STATE.0.get()).as_mut() }
            .expect("the retry sequence starts with its reset entry");
        let decision = state.observe_completion(policy, disposition);
        let counters = state.counters();
        let exponent = policy.contention_exponent(*queue);
        // SAFETY: every retry case supplies writable descriptor, context and
        // frame objects covering these bytes.
        unsafe {
            let descriptor = descriptor as *mut u8;
            descriptor.add(5).write(counters.mpdu);
            descriptor.add(6).write(counters.short);
            descriptor.add(7).write(counters.long);
            (context as *mut u8).add(8).write(exponent);
        }
        match decision {
            OrdinaryRetryDecision::Complete => RETRY_COMPLETE,
            OrdinaryRetryDecision::Retry { set_retry_bit: false } => RETRY_AGAIN,
            OrdinaryRetryDecision::Retry { set_retry_bit: true } => {
                // SAFETY: as above.
                unsafe {
                    let flags = (buffer as *mut u8).add(FRAME_FLAGS);
                    flags.write(flags.read() | FRAME_RETRY);
                }
                RETRY_AGAIN_WITH_BIT
            }
        }
    }
}

// These validation-only leaves make the result of the compiled production
// completion classifier observable without introducing a shadow numeric
// encoding. The non-pure inline assembly prevents LLVM from deleting the
// calls; comparison stops at the selected leaf before executing its body.
oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_ack_timeout(queue: u32) {
        // SAFETY: an empty volatile assembly block has no machine-visible inputs
        // or outputs and exists only as an optimizer barrier in the probe image.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 1",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_cts_timeout(queue: u32) {
        // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 2",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_collision(queue: u32) {
        // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 3",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_tx_retry_security_key_error(queue: u32) {
        // SAFETY: see `open_libpp_tx_retry_ack_timeout`.
        unsafe {
            core::arch::asm!(
                "addi zero, zero, 5",
                in("a0") queue,
                options(nomem, nostack)
            )
        };
    }
}

oer_probe_macros::probe! {
    /// ABI projection: compare the zero-argument hardware path with the
    /// product decoder; the probe alone represents other vendor ABI calls.
    ///
    /// SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxError +0xb2 handles
    /// collision details only for a2 == 0; nonzero a2 routes them to ACK
    /// timeout at +0xca. Detail zero always reaches CTS timeout (+0xe2),
    /// and 0xc0 with nonzero a2 returns without ending FES (+0xea).
    /// SOURCE(esp32s31): libpp[lmac.o]::lmacProcessTxRtsError +0xd2 likewise
    /// suppresses FES end for 0xc0 with nonzero a3. +0x1a..+0x2c resolves
    /// virtual queue 10 through our_tx_eb, descriptor +52, word +4 bits 7:4;
    /// +0x42 requires an instance below five. Neither is a hardware decoder
    /// input: the product retains LegacyTxQueue directly and
    /// libpp[lmac.o]::lmacProcessTxComplete supplies zero at +0x19a and +0x1cc.
    pub fn open_libpp_tx_retry_trace_lmac_process_tx_error(queue: u32, detail: u32, selector: u32) {
        use oer_esp32s31_ieee80211_mac::tx::{CompletionFault, CompletionObservation, CompletionRaw, decode};
        use oer_ieee80211_lower_mac::TxStatus;

        if selector != 0 {
            match detail as u8 {
                0 => open_libpp_tx_retry_cts_timeout(queue),
                0xc0 => {},
                _ => open_libpp_tx_retry_ack_timeout(queue),
            }
            return;
        }
        let raw = oer_esp32s31_hal::types::MacTxCompletionObservation::new_validation(4, detail as u8);
        match decode(CompletionRaw::ordinary(raw)) {
            Ok(CompletionObservation::Observed(TxStatus::AckTimeout)) => open_libpp_tx_retry_ack_timeout(queue),
            Ok(CompletionObservation::Observed(TxStatus::CtsTimeout)) => open_libpp_tx_retry_cts_timeout(queue),
            Ok(CompletionObservation::Observed(TxStatus::Collision)) => open_libpp_tx_retry_collision(queue),
            Err(CompletionFault::SecurityKeyError) => open_libpp_tx_retry_security_key_error(queue),
            Ok(CompletionObservation::NoEffect) => {},
            Ok(CompletionObservation::Observed(TxStatus::Success | TxStatus::TimedOut))
            | Err(CompletionFault::InvalidStatus { .. }) => {},
        }
    }
}

#[repr(C)]
pub struct CanonicalOrdinaryRetryState {
    pub mpdu: u8,
    pub short: u8,
    pub long: u8,
    pub publications: u8,
    pub selected_rate: u8,
    pub decision: u8,
    pub retry_bit_mask: u8,
    pub contention_exponent: u8,
}

const ORDINARY_TX_PROBE_BUFFER_SIZE: usize = 256;

struct OrdinaryTxProbePower;

impl oer_esp32s31_ieee80211_driver::tx::WifiTxPowerProfile for OrdinaryTxProbePower {
    fn power_pair(&self, _rate_code: u8) -> oer_esp32s31_ieee80211_driver::tx::WifiTxPowerPair {
        oer_esp32s31_ieee80211_driver::tx::WifiTxPowerPair {
            primary: 1,
            alternate: 1,
        }
    }
}

struct OrdinaryTxProbeEntropy;

impl oer_esp32s31_ieee80211_driver::tx::WifiTxEntropy for OrdinaryTxProbeEntropy {
    fn next_u32(&mut self) -> u32 {
        0
    }
}

struct OrdinaryTxProbeTimer;

/// Time that stands still; waits end at once.
impl oer_base_time::Clock for OrdinaryTxProbeTimer {
    fn now(&self) -> oer_base_time::Instant {
        oer_base_time::Instant::from_micros(1)
    }
}

impl oer_base_time::Timer for OrdinaryTxProbeTimer {
    fn wait_until(
        &self,
        _deadline: oer_base_time::Instant,
    ) -> impl core::future::Future<Output = ()> {
        core::future::ready(())
    }
}

oer_probe_macros::probe! {
    /// Reviewed probe edge emitted by every real production publication.
    pub fn open_libpp_tx_retry_publication(queue: u32) {
        // SAFETY: this instruction is a machine-visible optimizer barrier only.
        unsafe { core::arch::asm!("addi zero, zero, 4", in("a0") queue, options(nomem, nostack)) };
    }
}

struct OrdinaryTxProbeHardware {
    publications: u8,
}

impl oer_esp32s31_ieee80211_mac::tx::TxHardware for OrdinaryTxProbeHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn oer_esp32s31_ieee80211_mac::tx::PreparedTxDma,
        _queue: u8,
        _program: oer_esp32s31_hal::types::MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(
        &mut self,
        _dma: &dyn oer_esp32s31_ieee80211_mac::tx::HardwareOwnedTxDma,
        queue: u8,
    ) {
        self.publications = self.publications.saturating_add(1);
        if self.publications > 1 {
            open_libpp_tx_retry_publication(u32::from(queue));
        }
    }

    fn take_tx_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacTxCompletionObservation> {
        Some(oer_esp32s31_hal::types::MacTxCompletionObservation::new_validation(5, 0))
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: oer_esp32s31_hal::types::MacTxDetachReason,
        detached: impl for<'detached> FnOnce(
            oer_esp32s31_hal::types::MacTxQueueDetached<'detached>,
        ) -> R,
    ) -> oer_esp32s31_hal::types::MacTxDetachOutcome<R> {
        use oer_esp32s31_hal::types::{MacTxDetachOutcome, MacTxDetachReason, MacTxQueueDetached};

        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_validation(expected_descriptor_head),
            )),
            MacTxDetachReason::Timeout | MacTxDetachReason::Collision => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }
}

type OrdinaryTxProbeOwner = oer_esp32s31_ieee80211_driver::ordinary_tx::OrdinaryTxOwner<
    'static,
    OrdinaryTxProbePower,
    OrdinaryTxProbeEntropy,
    OrdinaryTxProbeTimer,
    ORDINARY_TX_PROBE_BUFFER_SIZE,
>;

struct OrdinaryTxProbeState {
    owner: OrdinaryTxProbeOwner,
    hardware: OrdinaryTxProbeHardware,
}

#[derive(bytemuck::Zeroable)]
struct OrdinaryTxProbeCell<T>(core::cell::UnsafeCell<T>);

// SAFETY: Blobray executes this probe image on one thread and invokes
// its exported stateful entry serially.
unsafe impl<T> Sync for OrdinaryTxProbeCell<T> {}

oer_esp32s31_hal::dma_static! {
    static ORDINARY_TX_DMA:
        oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage<ORDINARY_TX_PROBE_BUFFER_SIZE> =
        zeroed in ".dma.bss.ordinary_tx";
}
static ORDINARY_TX_SLOT: OrdinaryTxProbeCell<
    core::mem::MaybeUninit<oer_esp32s31_ieee80211_mac::tx::TxSlot<ORDINARY_TX_PROBE_BUFFER_SIZE>>,
> = OrdinaryTxProbeCell(core::cell::UnsafeCell::new(core::mem::MaybeUninit::uninit()));
static ORDINARY_TX_PROBE: OrdinaryTxProbeCell<(
    bool,
    core::mem::MaybeUninit<OrdinaryTxProbeState>,
)> = OrdinaryTxProbeCell(core::cell::UnsafeCell::new((
    false,
    core::mem::MaybeUninit::uninit(),
)));

fn initialize_ordinary_tx_probe() -> Result<OrdinaryTxProbeState, u32> {
    use oer_esp32s31_ieee80211_driver::{
        ordinary_tx::{OrdinaryTxInterface, OrdinaryTxOwner, OrdinaryTxPlan},
        tx::{WifiTxProgress, WifiTxResources},
    };

    use oer_esp32s31_ieee80211_mac::tx::{
        LegacyRate, LegacyTxQueue, TxPhyRate, TxSlot, runtime::WifiTxRuntimePolicy,
    };

    use oer_ieee80211_softmac::MacTxPlan;

    let dma = oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage::pin::<
        oer_esp32s31_hal::ieee80211::dma::space::ChipDma,
    >(ORDINARY_TX_DMA.take().ok_or(10_u32)?)
    .map_err(|_| 10_u32)?;
    // SAFETY: this storage is initialized once and then borrowed exclusively
    // by the retained `OrdinaryTxOwner` for the rest of the image lifetime.
    let slot = unsafe {
        let slot = &mut *ORDINARY_TX_SLOT.0.get();
        slot.write(TxSlot::from_dma(dma));
        core::pin::Pin::new_unchecked(slot.assume_init_mut())
    };
    let mut owner = OrdinaryTxOwner::new(WifiTxResources {
        slot,
        policy: WifiTxRuntimePolicy::vendor_defaults(),
        power: OrdinaryTxProbePower,
        entropy: OrdinaryTxProbeEntropy,
        timer: OrdinaryTxProbeTimer,
    });
    owner.buffer_mut().map_err(|_| 11_u32)?[..32].fill(0);
    let mut hardware = OrdinaryTxProbeHardware { publications: 0 };
    let progress = owner
        .start(
            &mut hardware,
            OrdinaryTxPlan {
                frame_length: 24,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: LegacyTxQueue::BestEffort.access_category(),
                    initial_rate: TxPhyRate::Legacy(LegacyRate::Ofdm54M),
                    publication_limit: 0x20,
                    publication_timeout: oer_base_time::Duration::from_micros(1_000),
                },
                hardware_mic_length: 0,
                hardware_key_selector: 0,
                interface: OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
                priority_count: 1,
            },
        )
        .map_err(|_| 12_u32)?;
    if progress != WifiTxProgress::Pending {
        return Err(13_u32);
    }
    Ok(OrdinaryTxProbeState { owner, hardware })
}

/// Drive one ACK-timeout edge through the exact compiled production TX owner.
#[inline(never)]
fn ordinary_tx_ack_timeout_state(output_address: u32) -> u32 {
    use oer_esp32s31_ieee80211_driver::tx::{WifiTxProgress, WifiTxWake};

    if output_address == 0 {
        return 4;
    }
    // SAFETY: this is the sole accessor in the single-threaded probe image.
    let storage = unsafe { &mut *ORDINARY_TX_PROBE.0.get() };
    if !storage.0 {
        let Ok(state) = initialize_ordinary_tx_probe() else {
            return 5;
        };
        storage.1.write(state);
        storage.0 = true;
    }
    // SAFETY: the branch above initializes the state before every read.
    let state = unsafe { storage.1.assume_init_mut() };
    // Route the hardware completion through the exact production atomic
    // interrupt handoff. This keeps the compiled comparison from bypassing
    // the adapter boundary by constructing `WifiTxWake` directly.
    let mut irq = oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage::new();
    let (publisher, mut consumer) = irq.split();
    publisher.publish(oer_esp32s31_ieee80211_mac::irq::EVENT_TX_COMPLETE);
    let events = consumer
        .take_batch()
        .fold(0, |events, work| events | work.event_bit());
    if events == 0 {
        return 8;
    }
    let progress = state
        .owner
        .service(&mut state.hardware, WifiTxWake::Interrupt { events });
    if progress != Ok(WifiTxProgress::Pending) {
        return 6;
    }
    let Ok(Some(snapshot)) = state.owner.active_snapshot() else {
        return 7;
    };
    let output = output_address as *mut CanonicalOrdinaryRetryState;
    // SAFETY: the comparison case supplies this writable eight-byte object.
    unsafe {
        output.write(CanonicalOrdinaryRetryState {
            mpdu: snapshot.counters.mpdu,
            short: snapshot.counters.short,
            long: snapshot.counters.long,
            publications: snapshot.publications,
            selected_rate: snapshot.current_rate.code(),
            decision: 1,
            retry_bit_mask: u8::from(snapshot.retry_bit_set) * 8,
            contention_exponent: state
                .owner
                .policy()
                .contention_exponent(oer_esp32s31_ieee80211_mac::tx::LegacyTxQueue::BestEffort),
        })
    };
    0
}

oer_probe_macros::probe! {
    /// Vendor-ABI-shaped entry for the stateful ACK-timeout short-frame profile.
    /// The fixed output address belongs to the comparison scenario; all retry
    /// decisions and counters come from the exact production state above.
    pub fn open_libpp_tx_retry_trace_ack_timeout_state(_queue: u32, _selector: u32) -> u32 =>
        ordinary_tx_ack_timeout_state(0x3fff_5000);
}

oer_probe_macros::probe! {
    /// The scenario domain is STA/AP, CCMP, management policy enabled, SPP 0/1.
    /// Execute the production association transaction before any key install.
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
        let mut owner = oer_esp32s31_hal::validation::runtime_owner();
        owner.wifi_mac_hal().configure_ccmp_association(interface, spp);
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
            oer_esp32s31_hal::validation::runtime_owner()
                .wifi_mac_hal()
                .program_interface_address(interface, *address);
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_interface_trace_hal_mac_set_bssid(interface: u32, address: &[u8; 6]) {
        if let Some(interface) = mac_interface(interface) {
            oer_esp32s31_hal::validation::runtime_owner()
                .wifi_mac_hal()
                .program_interface_bssid(interface, *address);
        }
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_trace_hal_disable_softap_tsf() {
        let mut owner = oer_esp32s31_hal::validation::runtime_owner();
        let mut hardware = owner.wifi_mac_hal();
        AccessPointTsf::new(0).stop(&mut hardware);
    }
}

oer_probe_macros::probe! {
    pub fn open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset(selector: u32) {
        if selector == 0 {
            let mut owner = oer_esp32s31_hal::validation::runtime_owner();
            let mut hardware = owner.wifi_mac_hal();
            AccessPointTsf::new(0).restart(&mut hardware);
        }
    }
}

oer_probe_macros::probe! {
    /// Read STA TSF into the caller's optional output words.
    ///
    /// # Safety
    /// Non-null outputs must be aligned, exclusively writable, non-overlapping words.
    pub unsafe fn open_rom_power_tsf_trace_hal_get_sta_tsf(low: *mut u32, high: *mut u32) {
        // SAFETY: the executable profile supplies either null or one aligned,
        // writable scratch word for each pointer, matching the ROM ABI.
        let low = unsafe { low.as_mut() };
        // SAFETY: same closed profile contract as `low`.
        let high = unsafe { high.as_mut() };
        oer_esp32s31_hal::validation::hal_get_sta_tsf(low, high);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_enabled(&mut shared_phy(registers), true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_vht_support(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_vht_support(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_csidump_force_lltf_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_csi_dump_force_lltf(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_hemu_ru26_good_res(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_he_ru26_good_response(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_band_reg_set(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_frequency_band(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_fe_reg_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::initialize_front_end(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_fe_reg_update(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::update_front_end(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bbtx_outfilter(
        input_0: u32,
        input_1: u32,
        input_2: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_tx_output_filter(
            &mut shared_phy(registers), input_0, input_1, input_2,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_rst_enable(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_watchdog_reset_enabled(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_int_enable(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::set_watchdog_interrupt_enabled(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdt_timeout_clear(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::clear_watchdog_timeout(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_bb_wdt_get_status(registers: &mut RadioPhyRegisters) -> u32 =>
        oer_esp32s31_hal::phy::baseband::watchdog_status(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_lltf_mask_en(input_0: u32, input_1: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_lltf_mask(&mut shared_phy(registers), input_0, input_1);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ant_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_antenna(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_wdg_cfg(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_watchdog(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bt_filter_reg(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_bt_filter(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_module_resetn(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::reset_module(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_en_hw_set_freq(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::set_hardware_control(&mut shared_phy(registers), true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_dis_hw_set_freq(registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::frequency::set_hardware_control(&mut shared_phy(registers), false);
        ets_delay_us(2);
        // Keep the delay as a non-tail edge so the executor sees the named
        // harness symbol independently of linker tail-call relaxation.
        core::hint::black_box(registers);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_reg_init(
        _vendor_parameter_0: u32,
        _vendor_parameter_1: u32,
        parameter_override: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::frequency::initialize_registers(&mut shared_phy(registers), parameter_override != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_iq_corr_enable(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::enable_iq_correction(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_noise_floor_auto_set(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_noise_floor_auto(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_read_hw_noisefloor(registers: &mut WifiRadioRegisters) -> u32 =>
        oer_esp32s31_hal::ieee80211::phy_rate::read_hardware_noise_floor(registers) as u32;
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_tx_paon_set(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_tx_pa_on(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_wifi_agc_sat_gain(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_saturation_gain(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_low_rate(registers: &mut WifiRadioRegisters) =>
        oer_esp32s31_hal::ieee80211::phy_rate::set_low_rate_enabled(registers, true);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_low_rate(registers: &mut WifiRadioRegisters) =>
        oer_esp32s31_hal::ieee80211::phy_rate::set_low_rate_enabled(registers, false);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_is_low_rate_enabled(registers: &mut WifiRadioRegisters) -> u32 =>
        u32::from(oer_esp32s31_hal::ieee80211::phy_rate::low_rate_enabled(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_dcmem_clr(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::clear_dc_memory(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rx_11b_opt(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rx_11b_optimization(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rfrx_sat_rst(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rf_rx_saturation(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_set_rxclk_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_rx_clock(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_set_txclk_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_pbus_debugmode(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_i2c_txrate_init(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_i2c_tx_rate(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_nrx_freq_set(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_nrx_frequency(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_bb_cbw_chan_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::frequency::configure_channel_cbw(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_agc_reg_init(
        parameter_121: u32,
        parameter_120: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::agc::initialize_registers(
            &mut shared_phy(registers),
            parameter_121 as u8,
            parameter_120 as u8,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_set_rx_comp_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_rx_compensation(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bb_txpwr_track(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_tx_power_tracking(&mut shared_phy(registers), input & 1 != 0);
}

oer_probe_macros::probe! {
    /// Current vendor TX-gain restore boundary, compiled from production HAL/PAC.
    pub fn open_phy_calibration_trace_restore_tx_gain_compensation(
        _enabled: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::baseband::restore_tx_gain_compensation(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_force_digital_gain(
        enabled: u32,
        gain_0: u32,
        gain_1: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_forced_digital_gain(
            &mut shared_phy(registers),
            enabled & 1 != 0,
            gain_0 as i8,
            gain_1 as i8,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_temperature_to_power(
        current: u32,
        reference: u32,
        bluetooth: u32,
    ) -> i32 {
        let class = if bluetooth == 0 {
            oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass::Wifi
        } else {
            oer_esp32s31_phy::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154
        };
        i32::from(
            oer_esp32s31_phy::tracking::parameters::temperature_to_tracking_power(
                current as i16,
                reference as i16,
                class,
            ),
        )
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_calibration_trace_post_init_agc(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::update_post_initialization(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_reg_update_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::update_post_initialization(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_dc_mem_clr(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::clear_dc_memory(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_set_ftm_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::set_ftm_enabled_from_vendor_argument(&mut shared_phy(registers), input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_stop_tx_tone_new(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::stop_tx_tone(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_close_fe_bb_clk(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::clock::close_frontend_baseband(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_config_hccfr(
        enabled: u32,
        value: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_hccfr_from_vendor_arguments(
            &mut shared_phy(registers), enabled, value,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_iccfr_en(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_iccfr_gate(&mut shared_phy(registers), input != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_force_iccfr(
        mode: u32,
        enabled: u32,
        value: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::baseband::configure_forced_iccfr_from_vendor_arguments(
            &mut shared_phy(registers), mode, enabled, value,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_pwdet_always_en() =>
        oer_esp32s31_phy::tx::power_detector::phy_pwdet_always_en();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_pwdet_onetime_en() =>
        oer_esp32s31_phy::tx::power_detector::phy_pwdet_onetime_en();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_11p_set(
        enabled: u32,
        configuration: u32,
        output: &mut CanonicalDot11pState,
    ) {
        let initial = *output;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_dot11p_configuration(initial.enabled, initial.configuration);
        state.set_dot11p_configuration(enabled as u8, configuration as u8);
        let projected = state.dot11p_configuration();
        *output = CanonicalDot11pState {
            enabled: projected.enabled,
            configuration: projected.configuration,
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_current_level_set(value: u32, output: &mut CanonicalCurrentLevelState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_current_level(initial);
        state.set_current_level(value as u8);
        output.value = state.current_level();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bt_power_track(value: u32, output: &mut CanonicalBtPowerTrackingState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_bt_power_tracking(initial);
        state.set_bt_power_tracking(value as u8);
        output.value = state.bt_power_tracking();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_ble_set_chan_base(value: u32, output: &mut CanonicalBleChannelBaseState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_ble_channel_base(initial);
        state.set_ble_channel_base(value as u8);
        output.value = state.ble_channel_base();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_init_param_set(
        value: u32,
        output: &mut CanonicalInitializationParameterState,
    ) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_initialization_parameter(u32::from(initial != 0));
        state.set_initialization_parameter(value);
        output.value = u8::from(state.initialization_parameter());
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_track_temp_debug(
        first: u32,
        second: u32,
        output: &mut CanonicalTemperatureTrackingState,
    ) {
        let initial = *output;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_temperature_tracking_debug(initial.first, initial.second);
        state.set_temperature_tracking_debug(first as u8, second as u8);
        let projected = state.temperature_tracking_debug();
        *output = CanonicalTemperatureTrackingState {
            first: projected.first,
            second: projected.second,
        };
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_noise_check_loop() =>
        oer_esp32s31_phy::rx::signal_power::noise_check_loop();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_bbpll_en_usb() =>
        oer_esp32s31_phy::analog::rfpll::phy_bbpll_en_usb();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_freq_mem_backup() =>
        oer_esp32s31_phy::analog::frequency::phy_freq_mem_backup();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_freq_offset_set() =>
        oer_esp32s31_phy::analog::frequency::phy_freq_offset_set();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_get_i2c_data() =>
        oer_esp32s31_phy::analog::i2c::phy_get_i2c_data();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_archive_set_bb_wdg() =>
        oer_esp32s31_phy::calibration::baseband::set_bb_wdg();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_get_rf_cal_version() -> u32 =>
        oer_esp32s31_phy::analog::rfpll::phy_get_rf_cal_version();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_get_rfdata_num() -> u32 =>
        oer_esp32s31_phy::calibration::cold::phy_get_rfdata_num();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_get_bias_ref_code() -> u32 =>
        oer_esp32s31_phy::tx::calibration::get_bias_ref_code();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_phy_internal_delay() -> u32 =>
        oer_esp32s31_phy::calibration::cold::phy_internal_delay();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_enter_critical() =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_enter_critical();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_exit_critical() =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_exit_critical();
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_get_dc_value(output: &mut [u16; 2], value: u32) =>
        oer_esp32s31_phy::calibration::estimator::get_dc_value(output, value);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_master_mem_cfg(configuration: &mut [u8; 6]) =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_master_mem_cfg(configuration);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_i2c_master_command_mem_cfg(configuration: &mut [u8; 8], mode: &mut u32) =>
        oer_esp32s31_phy::analog::i2c::phy_i2c_master_command_mem_cfg(configuration, mode);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_phy_tx_atten_comp(values: &mut [u8; 3]) =>
        oer_esp32s31_phy::tx::calibration::phy_tx_atten_comp(values);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ant_dft_cfg(input: u32, registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::agc::configure_antenna_diversity(&mut shared_phy(registers), input & 1 != 0);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_btbb_wifi_bb_cfg2(registers: &mut RadioPhyRegisters) =>
        oer_esp32s31_hal::phy::baseband::configure_bt_wifi_baseband(&mut shared_phy(registers));
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_chan_dump_cfg(
        value: u32,
        enabled: u32,
        mode: u32,
        registers: &mut RadioPhyRegisters,
    ) =>
        oer_esp32s31_hal::phy::baseband::configure_channel_dump(&mut shared_phy(registers), value, enabled, mode);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_dac_rate_set(rate: u32, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::baseband::configure_dac_rate(
            &mut shared_phy(registers),
            oer_esp32s31_hal::types::PhyAdcRate::from_vendor_rate(rate),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_force_pwr_index(enabled: bool, index: u8, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::memory::configure_forced_power_index(
            &mut shared_phy(registers),
            enabled,
            oer_esp32s31_hal::phy::memory::PhyForcedPowerIndex::new(u32::from(index))
                .expect("probe input must fit the reviewed forced-power index"),
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_force_rx_gain(enabled: u32, gain: u32, registers: &mut RadioPhyRegisters) {
        oer_esp32s31_hal::phy::agc::configure_forced_rx_gain_from_vendor_arguments(
            &mut shared_phy(registers), enabled, gain,
        );
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_rx11blr_cfg(input: u32, registers: &mut WifiRadioRegisters) =>
        oer_esp32s31_hal::ieee80211::phy_rate::configure_rx_11b_low_rate(registers, input);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_enable_cca(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::validation::set_cca_enabled(true);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_disable_cca(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::validation::set_cca_enabled(false);
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_sifs_reg_init(registers: &mut RadioPhyRegisters) {
        let _ = registers;
        oer_esp32s31_hal::validation::initialize_sifs();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_abs_temp(input: u32) -> u32 =>
        oer_esp32s31_phy::calibration::math::absolute_temperature(input as i32);
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_ret_get_freq_mem_addr(
        base: u32,
        stride: u32,
        index: u32,
        offset: u32,
    ) -> u32 {
        u32::from(oer_esp32s31_phy::analog::frequency::phy_get_freq_mem_addr(
            base, stride, index, offset,
        ))
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_txpwr_track_slow(value: u32, output: &mut CanonicalSlowTxPowerTrackingState) {
        let initial = output.value;
        let mut state = oer_esp32s31_phy::state::PhyState::default();
        state.set_tx_power_tracking_slow(initial);
        state.set_tx_power_tracking_slow(value as u8);
        output.value = state.tx_power_tracking_slow();
    }
}

oer_probe_macros::probe! {
    pub fn open_phy_trace_freq_i2c_mem_write(
        address: u32,
        value: u32,
        mode: u32,
        registers: &mut RadioPhyRegisters,
    ) {
        oer_esp32s31_hal::phy::frequency::write_memory(
            &mut shared_phy(registers),
            (address & 0x07ff) as u16,
            value & 0x00ff_ffff,
            mode as u8,
        );
    }
}

oer_probe_macros::probe! {
    /// Execute the shipping command-memory transaction with explicit parameter bytes.
    /// The isolated image constructs its own owner; no private owner layout is part
    /// of the guest ABI. Encoding and all MMIO remain in the production HAL/PAC.
    pub fn open_phy_trace_command_memory(parameters: &[u8; 6]) {
        with_phy(|registers| {
            let inputs = oer_esp32s31_hal::phy::i2c::PhyI2cCommandMemoryInputs::new(
                parameters[0],
                parameters[1],
                parameters[2],
                parameters[3],
                parameters[4],
                parameters[5],
            );
            oer_esp32s31_hal::phy::i2c::configure_command_memory(&mut shared_phy(registers), inputs);
        })
    }
}

oer_probe_macros::probe! {
    /// Initialize captured mutable parameter memory during an explicit setup phase.
    /// This harness copy supplies scenario inputs; it performs no PHY computation.
    pub fn open_phy_trace_initialize_parameters(destination: &mut [u8; 400], source: &[u8; 400]) =>
        destination.copy_from_slice(source);
}

oer_probe_macros::probe! {
    /// Exercise the production software-frequency transition with an owned PHY
    /// partition. This also exposes the preserved ordinary delay-call boundary.
    pub fn open_phy_trace_owned_software_frequency_control() =>
        with_phy(|registers| open_phy_trace_dis_hw_set_freq(registers));
}

/// Byte offsets of the vendor `coex_schm_env` fields the schedule owns.
mod coex_schm_env {
    pub const SCHEME: usize = 0;
    pub const PHASE_INDEX: usize = 4;
    pub const WIFI: usize = 6;
    pub const BLE: usize = 8;
    pub const BT: usize = 10;
    pub const INTERVAL: usize = 12;
    pub const EXTERNAL_COEX: usize = 56;
    pub const FLEXIBLE_PERIOD: usize = 58;
    pub const IEEE802154: usize = 60;
    pub const BYTES: usize = 64;
}

oer_probe_macros::probe! {
    /// Compiled production-path probe for the `coexist_scheme.o` schedule
    /// entries. `env` is a vendor-shaped `coex_schm_env` image and `schemes`
    /// the vendor address of every `coex_schm_<name>` in
    /// `CoexSchemeId::ALL` order, so the selected scheme is reported as the
    /// vendor pointer. `operation` selects `coex_schm_status_change` (0),
    /// `coex_schm_status_bit_set(kind, bits)` (1), `_clear` (2),
    /// `coex_schm_timeout_process` (3) or `coex_schm_process_restart` (4).
    /// The image is rewritten with the resulting state; `report` receives
    /// whether the phase timer was re-armed, its microseconds and whether
    /// Wi-Fi and Bluetooth were notified. Returns 1 when a phase step ran.
    ///
    /// # Safety
    /// `env` must point to 64 writable bytes, `schemes` to one readable word
    /// per scheme and `report` to four writable words.
    pub unsafe fn open_coex_schm_trace_step(
        env: *mut u8,
        schemes: *const u32,
        operation: u32,
        kind: u32,
        bits: u32,
        report: *mut u32,
    ) -> u32 {
        use coex_schm_env as field;
        use oer_esp32s31_radio_coex::{
            CoexSchedule, CoexSchemeId, CoexStatusType, CoexStatusWords,
        };
        // SAFETY: the caller supplies the image, the address table and the
        // report as documented; the probe is their only user.
        let (env, schemes, report) = unsafe {
            (
                core::slice::from_raw_parts_mut(env, field::BYTES),
                core::slice::from_raw_parts(schemes, CoexSchemeId::ALL.len()),
                core::slice::from_raw_parts_mut(report, 4),
            )
        };
        let half = |env: &[u8], at: usize| u16::from_le_bytes([env[at], env[at + 1]]);
        let word = |env: &[u8], at: usize| {
            u32::from_le_bytes([env[at], env[at + 1], env[at + 2], env[at + 3]])
        };
        let Some(scheme) = CoexSchemeId::ALL
            .into_iter()
            .zip(schemes)
            .find(|(_, address)| **address == word(env, field::SCHEME))
            .map(|(scheme, _)| scheme)
        else {
            return u32::MAX;
        };
        let status = CoexStatusWords {
            wifi: half(env, field::WIFI),
            ble: half(env, field::BLE),
            bt: half(env, field::BT),
            external_coex: half(env, field::EXTERNAL_COEX),
            ieee802154: half(env, field::IEEE802154),
        };
        let mut schedule = CoexSchedule::for_validation(
            status,
            scheme,
            env[field::PHASE_INDEX],
            word(env, field::INTERVAL),
            env[field::FLEXIBLE_PERIOD],
        );
        let kind = match kind {
            0 => Some(CoexStatusType::Wifi),
            1 => Some(CoexStatusType::Ble),
            2 => Some(CoexStatusType::Bt),
            3 => Some(CoexStatusType::ExternalCoex),
            4 => Some(CoexStatusType::Ieee802154),
            _ => None,
        };
        let step = match (operation, kind) {
            (0, _) => {
                schedule = CoexSchedule::for_validation(
                    status,
                    status.select(),
                    schedule.phase_index(),
                    schedule.interval(),
                    schedule.flexible_period(),
                );
                None
            }
            (1, Some(kind)) => schedule.set_status_bits(kind, bits as u16),
            (2, Some(kind)) => {
                schedule.clear_status_bits(kind, bits as u16);
                None
            }
            (1 | 2, None) => None,
            (3, _) => schedule.timeout().ok(),
            (4, _) => schedule.restart().ok(),
            _ => return u32::MAX,
        };
        let selected = schemes[CoexSchemeId::ALL
            .into_iter()
            .position(|scheme| scheme == schedule.scheme())
            .expect("every scheme is listed")];
        env[field::SCHEME..field::SCHEME + 4].copy_from_slice(&selected.to_le_bytes());
        env[field::PHASE_INDEX] = schedule.phase_index();
        let status = schedule.status();
        for (at, value) in [
            (field::WIFI, status.wifi),
            (field::BLE, status.ble),
            (field::BT, status.bt),
            (field::EXTERNAL_COEX, status.external_coex),
            (field::IEEE802154, status.ieee802154),
        ] {
            env[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        report.copy_from_slice(&match step {
            Some(step) => [
                u32::from(step.timer_micros.is_some()),
                step.timer_micros.unwrap_or(0),
                u32::from(step.notify_wifi),
                u32::from(step.notify_bluetooth),
            ],
            None => [0; 4],
        });
        u32::from(step.is_some())
    }
}

/// ESP-IDF `MODEM_CLOCK_LPCLK_SRC_RC_SLOW` and `MODEM_CLOCK_LPCLK_SRC_XTAL32K`
/// of `hal/modem_clock_types.h`: the two sources `esp_perip_clk_init` selects
/// for the Wi-Fi power domain.
const MODEM_CLOCK_LPCLK_SRC_RC_SLOW: u32 = 0;
const MODEM_CLOCK_LPCLK_SRC_XTAL32K: u32 = 4;

oer_probe_macros::probe! {
    /// The production Wi-Fi power-domain low-power clock selection for the
    /// ESP-IDF source enumerator `source`; any other source selects nothing.
    pub fn open_modem_clock_trace_select_wifi_low_power_clock(source: u32) {
        let source = match source {
            MODEM_CLOCK_LPCLK_SRC_RC_SLOW => oer_esp32s31_pac::WifiLowPowerClockSource::SlowOscillator,
            MODEM_CLOCK_LPCLK_SRC_XTAL32K => oer_esp32s31_pac::WifiLowPowerClockSource::Crystal32Khz,
            _ => return,
        };
        with_phy(|registers| registers.select_wifi_low_power_clock(source));
    }
}
