#![no_std]

//! Retained Bluetooth-only entry points for compiled production comparison.

use oer_esp32s31_bluetooth::validation::{
    BluetoothControllerSramAddress, BluetoothMemoryListPointerImage, BluetoothMemoryListSelector,
    BluetoothMemoryListSlot,
};

#[panic_handler]
#[allow(
    clippy::disallowed_methods,
    reason = "isolated verification image terminates on panic without a runtime executor"
)]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

oer_probe_macros::probe! {
    /// Production-path probe for the reviewed BLE NRT interrupt MMIO prefix.
    ///
    /// Verification stops the vendor side before its following log/callback
    /// suffix; this function executes the exact restricted PAC transaction.
    pub fn open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da() =>
        oer_esp32s31_bluetooth::validation::capture_and_acknowledge_interrupts();
}

oer_probe_macros::probe! {
    /// Production-path probe for the finite scheduler-table MMIO transaction.
    ///
    /// The vendor event/list suffix is deliberately outside this entry's claim.
    pub fn open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr() =>
        oer_esp32s31_bluetooth::validation::clear_scheduler_hardware_list_heads();
}

oer_probe_macros::probe! {
    /// Production execution modify (`r_btdm_sched_execution_modify`) of one
    /// hardware list, stepped until it leaves `Pending`. The return value is
    /// the disposition (zero ready, one rejected, `u32::MAX` invalid index).
    pub fn open_ble_scheduler_trace_execution_modify(index: u32, list_deletion: u32) -> u32 {
        // SAFETY: the comparison image models the list ownership of the
        // admission and performs no later radio operation.
        unsafe {
            oer_esp32s31_bluetooth::validation::run_scheduler_execution_modify(
                index as u8,
                list_deletion != 0,
            )
        }
        .unwrap_or(u32::MAX)
    }
}

oer_probe_macros::probe! {
    /// Production execution lock (`r_btdm_sched_execution_lock`) of one item
    /// on one hardware list, observed until it leaves `Pending`. The return
    /// value is the disposition (zero retained, one reconcile, two
    /// unsupported, `u32::MAX` invalid input).
    pub fn open_ble_scheduler_trace_execution_lock(address: u32, index: u32) -> u32 {
        // SAFETY: the comparison image models the merge-selected item and
        // list serialization and performs no later radio operation.
        unsafe {
            oer_esp32s31_bluetooth::validation::run_scheduler_execution_lock(address, index as u8)
        }
        .unwrap_or(u32::MAX)
    }
}

oer_probe_macros::probe! {
    /// The diagnostic scheduler-BUSY sample that opens the production
    /// scheduler stop sequence; the vendor side stops at its following log.
    pub fn open_ble_scheduler_stop_busy_trace_r_btdm_sched_stop() {
        let _busy = oer_esp32s31_bluetooth::validation::sample_scheduler_stop_busy();
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the complete 49-operation BTDM controller
    /// HAL-init body under its exact standalone caller-derived profile.
    pub fn open_btdm_hal_init_trace_r_sym_bt_a_gdrujd2_mu_az_wyh75ba_r() {
        // SAFETY: the comparison image models the recovered powered and quiescent
        // prerequisites, retains the inactive IRQ owner, and performs no later
        // radio operation.
        unsafe {
            oer_esp32s31_bluetooth::validation::initialize_controller_hal_reviewed_standalone();
        }
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the source-127 controller-register prefix.
    pub fn open_btdm_modem_lp_timer_register_prefix() {
        // SAFETY: this terminal comparison image models all earlier controller
        // software stages and never installs a CPU route or resumes radio work.
        unsafe {
            oer_esp32s31_bluetooth::validation::prepare_modem_lp_timer_registers();
        }
    }
}

oer_probe_macros::probe! {
    /// Production-path probe for the exact finite MMIO behavior of
    /// `bt_bb_v2_init_cmplx(1)`.
    ///
    /// The vendor's version log is deliberately not part of the claim. The second
    /// ABI argument supplies the reviewed byte at `phy_param + 0x120` to the Rust
    /// side; the comparison profile seeds the same byte in the vendor image.
    pub fn open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1(
        _print_version: u32,
        gain_parameter: u32,
    ) {
        // SAFETY: this dedicated comparison image seeds the vendor and production
        // executions after their modeled common-PHY prerequisite. It performs no
        // subsequent radio operation and terminates without reconstructing cold
        // ownership.
        unsafe {
            oer_esp32s31_bluetooth::validation::initialize_baseband_v2(gain_parameter as u8);
        }
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the accredited domain of
    /// `phy_get_i2c_hostid_new`.
    ///
    /// This wrapper delegates through the standalone Bluetooth route's shared-PHY
    /// owner and converts its typed result at the C ABI boundary. Host selection
    /// and the complete `ANA_CONF2` transaction remain in production PHY/PAC
    /// code.
    pub fn open_phy_i2c_host_trace_phy_get_i2c_hostid_new(block: u32) -> u32 =>
        oer_esp32s31_bluetooth::validation::configure_and_select_phy_i2c_host(block as u8);
}

#[inline(always)]
fn memory_list_selector(raw: u32) -> Option<BluetoothMemoryListSelector> {
    match raw {
        1 => Some(BluetoothMemoryListSelector::One),
        2 => Some(BluetoothMemoryListSelector::Two),
        3 => Some(BluetoothMemoryListSelector::Three),
        _ => None,
    }
}

#[inline(always)]
fn memory_list_image(raw: u32) -> Option<BluetoothMemoryListPointerImage> {
    if raw == 0 {
        Some(BluetoothMemoryListPointerImage::Zero)
    } else {
        BluetoothControllerSramAddress::new(raw)
            .ok()
            .map(BluetoothMemoryListPointerImage::Address)
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the current-RX memory-list setter.
    pub fn open_ble_memory_list_a_trace_r_sym_ble_lbo_ru27_ea_u8_mv8_q7_uuf_z(
        selector: u32,
        pointer: u32,
    ) {
        let (Some(selector), Some(image)) =
            (memory_list_selector(selector), memory_list_image(pointer))
        else {
            return;
        };
        // SAFETY: this isolated image supplies only reviewed selectors and
        // pointer encodings, performs no concurrent or subsequent radio work,
        // and terminates without reconstructing cold ownership.
        unsafe {
            oer_esp32s31_bluetooth::validation::program_memory_list_pointer(
                selector,
                BluetoothMemoryListSlot::CurrentRx,
                image,
            );
        }
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the next-RX memory-list setter.
    pub fn open_ble_memory_list_b_trace_r_sym_ble_zzr_ex_mrn8_edi_tfi7_penk(
        selector: u32,
        pointer: u32,
    ) {
        let (Some(selector), Some(image)) =
            (memory_list_selector(selector), memory_list_image(pointer))
        else {
            return;
        };
        // SAFETY: same isolated-image conditions as the current-RX probe apply.
        unsafe {
            oer_esp32s31_bluetooth::validation::program_memory_list_pointer(
                selector,
                BluetoothMemoryListSlot::NextRx,
                image,
            );
        }
    }
}

oer_probe_macros::probe! {
    /// Compiled production entry for the complete BLE PHY register-init body,
    /// `r_ble_phy_init_registers` (`r_sym_ble_nENHlP4KBuQYlFVffaR5` in the
    /// pinned `libble_app.a`).
    ///
    /// The vendor function obtains these five values from linked globals and
    /// providers: the timing byte at controller configuration `+0x10`, the
    /// environment and resolving-list pointers, the branch byte `+0x55` of
    /// the SDK options and the controller configuration word `+0x40`. The
    /// comparison profile seeds those exact vendor locations and passes the
    /// same values through this explicit Rust ABI projection. Production
    /// routes modem ETM channel two where the vendor routes channel zero.
    pub fn open_ble_phy_register_init_trace_r_ble_phy_init_registers(
        private_timing_source_byte: u32,
        environment_address: u32,
        resolving_list_address: u32,
        set_branch_control_0470_bit_18: u32,
        configuration_word_40: u32,
    ) {
        // SAFETY: every comparison case models the recovered prerequisite state,
        // supplies live controller storage for the execution lifetime, performs
        // no later radio operation, and terminates without reconstructing cold
        // ownership.
        unsafe {
            let _accepted = oer_esp32s31_bluetooth::validation::initialize_phy_registers(
                private_timing_source_byte as u8,
                environment_address,
                resolving_list_address,
                set_branch_control_0470_bit_18 != 0,
                configuration_word_40,
            );
        }
    }
}

oer_probe_macros::probe! {
    /// The production lease selection of the Bluetooth low-power clock.
    pub fn open_bluetooth_trace_select_low_power_clock() -> u32 =>
        u32::from(oer_esp32s31_bluetooth::validation::select_low_power_clock());
}

oer_probe_macros::probe! {
    /// The production deselection transaction of the Bluetooth low-power clock.
    pub fn open_bluetooth_trace_deselect_low_power_clock() =>
        oer_esp32s31_bluetooth::validation::deselect_low_power_clock();
}
