//! PHY-owned action lowering onto narrow ESP32-S31 HAL capabilities.
//!
//! Hardware-independent PHY state machines decide when an action occurs.
//! This module owns PHY-specific value transforms and composes the finite HAL
//! operations that realize each action. It does not own PAC capabilities,
//! register identities, polling policy, or runtime/protocol state.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::{ieee80211::channel::RadioChannelHal, owner::SharedPhyAccess};

/// Gate the calibration region around `phy_rf_init` and `phy_bb_init`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_phy_register_calibration_clock(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
) {
    registers.set_phy_calibration_clock(enabled);
}

/// Complete rev0 ROM `phy_bb_agc_reg_update`, size `0xa6`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_bb_agc_register_update(registers: &mut impl SharedPhyAccess) {
    oer_esp32s31_hal::phy::agc::update_baseband_registers(registers);
}

/// Complete rev0 ROM `phy_enable_agc`, size `0x28`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn enable_phy_agc(registers: &mut impl SharedPhyAccess) {
    oer_esp32s31_hal::phy::agc::set_enabled(registers, true);
}

/// Select the exact AGC state used by `phy_chip_set_chan`.
///
/// Complete ROM `phy_disable_agc` only sets the PAC's recovered AGC-disable
/// field in the shared AGC/antenna control word.
/// Re-enabling uses the already recovered three-write `phy_enable_agc`
/// sequence. Both branches are finite and touch no software state.
#[cfg(target_arch = "riscv32")]
pub(crate) fn set_phy_channel_agc(registers: &mut impl SharedPhyAccess, enabled: bool) {
    oer_esp32s31_hal::phy::agc::set_enabled(registers, enabled);
}

/// Complete both branches of rev0 ROM `phy_rx_11b_opt`, size `0xc4`.
#[cfg(target_arch = "riscv32")]
fn configure_phy_rx_11b_optimization(registers: &mut impl SharedPhyAccess, enabled: bool) {
    oer_esp32s31_hal::phy::agc::configure_rx_11b_optimization(registers, enabled);
}

/// Complete rev0 ROM `phy_reg_init` at `0x2f82_3ef8`, size `0x52`, with
/// every direct and tail child reproduced by source-owned safe HAL leaves.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_registers(
    registers: &mut impl oer_esp32s31_hal::owner::PhyInitializationAccess,
    parameters: crate::calibration::baseband::PhyRegisterInitParameters,
) {
    oer_esp32s31_hal::phy::baseband::enable_iq_correction(registers);
    oer_esp32s31_hal::phy::agc::initialize_registers(
        registers,
        parameters.parameter_121,
        parameters.parameter_120,
    );
    oer_esp32s31_hal::phy::agc::set_saturation_gain(registers, 0x0008_1825);
    oer_esp32s31_hal::phy::baseband::initialize_baseband(registers);
    oer_esp32s31_hal::phy::baseband::configure_watchdog(registers);
    oer_esp32s31_hal::phy::baseband::configure_tx_pa_on(registers);
    configure_phy_rx_11b_optimization(registers, true);
    oer_esp32s31_hal::phy::power_detector::configure_background(registers);
    oer_esp32s31_hal::phy::baseband::configure_noise_floor_auto(registers);
    oer_esp32s31_hal::phy::agc::configure_antenna(registers);
    oer_esp32s31_hal::phy::frequency::configure_bt_filter(registers);
    oer_esp32s31_hal::phy::frequency::enable_mac_baseband(registers);
}

/// Complete pinned `libphy.a[phy_rx_gain.o]::phy_rx_table_init`, size `0x7c`.
///
/// The unique [`crate::state::PhyState`] owner must call
/// `prepare_rx_table_init` before executing this action. That explicit local
/// step performs the reference's `*(u16 *)(phy_param + 0x120) = 0x4f4f`
/// mutation. This leaf
/// then publishes exactly 79 gain entries and runs the already complete
/// register-init, AGC-update and AGC-enable suffix.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_rx_table(
    registers: &mut impl oer_esp32s31_hal::owner::PhyInitializationAccess,
    parameters: crate::calibration::baseband::PhyRxTableInitParameters,
) {
    let mut index = 0_u8;
    while index != crate::calibration::baseband::PHY_RX_TABLE_ENTRY_COUNT {
        let entry = crate::calibration::baseband::phy_rx_table_gain_entry(parameters, index);
        oer_esp32s31_hal::phy::memory::program_gain_memory_entry(registers, entry);
        index += 1;
    }
    configure_phy_registers(
        registers,
        crate::calibration::baseband::PhyRegisterInitParameters {
            parameter_121: parameters.parameter_121,
            parameter_120: crate::calibration::baseband::PHY_RX_TABLE_ENTRY_COUNT,
        },
    );
    configure_phy_bb_agc_register_update(registers);
    enable_phy_agc(registers);
}

#[cfg(any(target_arch = "riscv32", test))]
const fn tx_baseband_gain_index(gain: u16) -> usize {
    match gain {
        0x0080 => 1,
        0x0100 => 2,
        0x0020 => 3,
        0x00a0 => 4,
        _ => 0,
    }
}

#[cfg(any(target_arch = "riscv32", test))]
const fn packed_halfword(words: &[u32], index: usize) -> u16 {
    (words[index >> 1] >> ((index & 1) * 16)) as u16
}

#[cfg(any(target_arch = "riscv32", test))]
const fn packed_byte(words: &[u32], index: usize) -> u8 {
    (words[index >> 2] >> ((index & 3) * 8)) as u8
}

#[cfg(any(target_arch = "riscv32", test))]
fn tx_gain_seed_halfword(image: &crate::channel::PhyWifiTxGainImage, index: usize) -> u16 {
    if index < image.seed.len() * 2 {
        packed_halfword(&image.seed, index)
    } else {
        packed_halfword(&image.output_32, index - image.seed.len() * 2)
    }
}

#[cfg(target_arch = "riscv32")]
fn bluetooth_tx_gain_seed_halfword(
    image: &crate::calibration::bluetooth::PhyBluetoothTxGainImage,
    index: usize,
) -> u16 {
    packed_halfword(&image.seed, index)
}

/// Apply the complete direct-register prefix/suffix of ROM
/// `phy_set_rx_gain_cal_dc`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_rx_gain_dc_registers(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
) {
    if enabled {
        registers.set_phy_calibration_clock(true);
    }
    registers.set_rx_gain_dc_calibration(enabled);
}

/// Program the complete crystal-duty calibration tone without `g_phyFuns`.
///
/// Primary reference: pinned
/// `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new`, size `0xf4`, together
/// with its `g_phyFuns + 0x30` target
/// `phy_txgain_comp_pacfg_new`, size `0x5c`.
///
/// The calibration caller supplies only the three nonzero-capable arguments;
/// the second path is zero in both evidenced calls. `enabled=true` reproduces
/// `(1, 0x80, 0, 0, 0, 0)`, while `enabled=false` reproduces
/// `(0, 0x80, 0x28, 0, 0, 0)`. Every fresh volatile read and intermediate
/// write is retained because the registers are hardware state. There is no
/// callback, loop, wait, allocation, or software-global access.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_calibration_tone(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    selector: u8,
    step: u8,
) {
    configure_phy_calibration_tone_wide(registers, enabled, selector as u16, step);
}

/// Program the enabled path of rev0 ROM `phy_start_tx_tone_step`.
///
/// Unlike the archive's `_new` replacement below, the ROM leaf first disables
/// the DAC scale and TX-gain compensation, and leaves both disabled while the
/// power-control loop measures the tone.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_power_control_tone(
    registers: &mut impl SharedPhyAccess,
    selector: u16,
    step: u8,
) {
    registers.configure_power_control_tone(selector, step);
}

/// Wide-selector form used by TX-DC calibration, whose evidenced selector is
/// 600 and therefore cannot be represented by the older `u8` child actions.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(crate) fn configure_phy_calibration_tone_wide(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    selector: u16,
    step: u8,
) {
    registers.configure_calibration_tone(enabled, selector, step);
}

/// Enter or leave the TX-IQ coefficient calibration register mode.
///
/// Reference: complete ROM `phy_rfcal_txiq` prefix and suffix. Each branch is
/// one finite read/modify/write and owns no software state.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_txiq_correction(registers: &mut impl SharedPhyAccess, begin: bool) {
    registers.configure_tx_iq_correction(begin);
}

/// Capture tone control into the route TX-IQ restore slot.
#[cfg(target_arch = "riscv32")]
pub(crate) fn prepare_phy_txiq_tone_control_restore(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), oer_esp32s31_hal::types::TxIqToneControlPrepareError> {
    registers.prepare_txiq_tone_control_restore()
}

/// Restore and consume PAC-private tone control after TX-IQ cleanup.
#[cfg(target_arch = "riscv32")]
pub(crate) fn restore_phy_txiq_tone_control(
    registers: &mut impl SharedPhyAccess,
) -> Result<(), oer_esp32s31_hal::types::TxIqToneControlRestoreError> {
    registers.restore_txiq_tone_control()
}

/// Configure one of the two mismatch-power polarities.
///
/// The first branch reproduces both writes at the head of
/// `phy_txiq_get_mis_pwr`; the second branch changes only bits 27:24 after
/// the first linear-power sample. The two-microsecond intervals remain
/// separate async actions in `phy_txiq`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_txiq_mis_power(
    registers: &mut impl SharedPhyAccess,
    first: bool,
    polarity: bool,
    attenuation: u8,
    selector: u16,
) {
    registers.configure_txiq_mismatch_power(first, polarity, attenuation, selector);
}

/// Publish one bounded TX-IQ gain or phase coefficient.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_txiq_coefficient(
    registers: &mut impl SharedPhyAccess,
    kind: crate::tx::iq::PhyTxIqCoefficientKind,
    value: i8,
) {
    match kind {
        crate::tx::iq::PhyTxIqCoefficientKind::Gain => registers.set_tx_iq_gain_coefficient(value),
        crate::tx::iq::PhyTxIqCoefficientKind::Phase => {
            registers.set_tx_iq_phase_coefficient(value)
        }
    }
}

/// Publish one bounded RX-IQ gain or phase coefficient.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_rxiq_coefficient(
    registers: &mut impl SharedPhyAccess,
    kind: crate::rx::iq::PhyRxIqCoefficientKind,
    value: i8,
) {
    match kind {
        crate::rx::iq::PhyRxIqCoefficientKind::Gain => registers.set_rx_iq_gain_coefficient(value),
        crate::rx::iq::PhyRxIqCoefficientKind::Phase => {
            registers.set_rx_iq_phase_coefficient(value)
        }
    }
}

/// Select the finite correction path at entry to ROM `phy_rfcal_rxiq`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_rxiq_calibration_mode(registers: &mut impl SharedPhyAccess) {
    registers.configure_rx_iq_calibration_mode();
}

/// Apply the finite MMIO suffix of rev0 ROM `phy_adc_rate_set`.
///
/// The complete parent action performs its masked PHY-I2C transaction first.
/// This leaf preserves the following two fresh-read writes to the generated
/// PAC `ADC_RATE_AND_FRONT_END_CONTROL` identity.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_adc_rate(
    registers: &mut impl SharedPhyAccess,
    rate: oer_esp32s31_hal::phy::i2c::PhyAdcRate,
) {
    registers.configure_adc_rate(rate);
}

/// Apply complete rev0 ROM `phy_fe_reg_init`.
///
/// The pinned body at `0x2f82_7740`, size `0xf6`, is a finite sequence of
/// seventeen MMIO writes. Calls below are deliberately unrolled and retain
/// repeated fresh-read writes to the same register. There is no wait, delay,
/// loop, callback, or mutable software-state access.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_front_end_registers(registers: &mut impl SharedPhyAccess) {
    oer_esp32s31_hal::phy::baseband::initialize_front_end(registers);
}

/// Apply complete pinned `libphy.a[phy_reg.o]::phy_fe_reg_update`.
///
/// The 0x32-byte archive body used by `phy_rf_init` is smaller than the
/// similarly named ROM function: it performs exactly three fresh-read MMIO
/// updates and returns. In particular, this call site does not include the ROM
/// tail-call to `phy_dac_scale_set`. There is no loop, delay, callback, or
/// mutable software-state access.
#[cfg(target_arch = "riscv32")]
pub(crate) fn configure_phy_front_end_update(registers: &mut impl SharedPhyAccess) {
    oer_esp32s31_hal::phy::baseband::update_front_end(registers);
}

/// Arm one PWDET tone sample before the async one-microsecond timer edge.
#[cfg(target_arch = "riscv32")]
pub(crate) fn arm_phy_power_detector_tone(registers: &mut impl SharedPhyAccess) {
    registers.set_power_detector_tone_armed(true);
}

/// Clear the temporary tone-arm bit selected by former `phy_param[0x1aa]`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn clear_phy_power_detector_tone_arm(registers: &mut impl SharedPhyAccess) {
    registers.set_power_detector_tone_armed(false);
}

/// Stop the calibration tone exactly as `phy_stop_tx_tone(1)`.
///
/// This includes the two fresh-read `phy_dac_scale_set(1)` field writes. It
/// is an unconditional cleanup leaf with no wait, branch, callback, or
/// software-global access.
#[cfg(target_arch = "riscv32")]
pub(crate) fn stop_phy_power_detector_tone(registers: &mut impl SharedPhyAccess) {
    registers.stop_power_detector_tone();
}

/// Trigger one TX-DC comparator measurement using the three fresh-read writes
/// at rev0 ROM `phy_txdc_cal+0x9c..=0xbe`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn trigger_phy_tx_dc_measurement(registers: &mut impl SharedPhyAccess) {
    registers.trigger_tx_dc_measurement();
}

/// Read one TX-DC readiness sample. Repetition remains an executor decision.
#[cfg(target_arch = "riscv32")]
pub(crate) fn read_phy_tx_dc_ready_status(registers: &mut impl SharedPhyAccess) -> bool {
    registers.tx_dc_measurement_is_ready()
}

/// Preserve the two independent post-ready comparator reads from the ROM.
#[cfg(target_arch = "riscv32")]
pub(crate) fn read_phy_tx_dc_comparator_status(registers: &mut impl SharedPhyAccess) -> [bool; 2] {
    registers.sample_tx_dc_comparators()
}

/// Clear the TX-DC measurement controls as two fresh-read writes.
#[cfg(target_arch = "riscv32")]
pub(crate) fn clear_phy_tx_dc_measurement(registers: &mut impl SharedPhyAccess) {
    registers.clear_tx_dc_measurement();
}

/// Encode and publish a finite PHY transmit-gain table.
///
/// Reference: pinned
/// `libphy.a[phy_tx_gain.o]::phy_set_tx_gain_mem_new`, size `0x130`, plus the
/// complete rev0 ROM leaves `phy_txbbgain_to_index` at `0x2f826ac8` and
/// `phy_write_gain_mem` at `0x2f8274f0`.
///
/// The vendor body accepts 16 BT or 32 Wi-Fi entries. The open channel path
/// owns and publishes the exact 32-entry Wi-Fi image. Its historical
/// `seed_and_output_32` pointer treated the six seed words and eight packed
/// output words as one contiguous halfword view; Rust models that
/// concatenation explicitly instead of relying on struct layout or pointer
/// arithmetic.
///
/// Every iteration performs three ordinary input reads and selects four
/// halfwords from that contiguous layout. It passes those semantic values to
/// the PAC-owned gain-memory encoder and publisher. There is no allocation,
/// wait, indirect call, hidden state, raw pointer, or hardware-dependent loop
/// exit.
#[cfg(target_arch = "riscv32")]
trait PhyGainMemory {
    fn table_memory_base_index(&self) -> u8;
    fn program_gain_memory_entry(&mut self, entry: oer_esp32s31_hal::types::PhyGainMemoryEntry);
}

#[cfg(target_arch = "riscv32")]
struct SharedPhyGainMemory<'registers, T: SharedPhyAccess> {
    registers: &'registers mut T,
}

#[cfg(target_arch = "riscv32")]
impl<T: SharedPhyAccess> PhyGainMemory for SharedPhyGainMemory<'_, T> {
    fn table_memory_base_index(&self) -> u8 {
        oer_esp32s31_hal::phy::memory::read_table_memory_base_index(self.registers)
    }

    fn program_gain_memory_entry(&mut self, entry: oer_esp32s31_hal::types::PhyGainMemoryEntry) {
        oer_esp32s31_hal::phy::memory::program_gain_memory_entry(self.registers, entry);
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> PhyGainMemory for RadioChannelHal<'_, P> {
    fn table_memory_base_index(&self) -> u8 {
        self.table_memory_base_index()
    }

    fn program_gain_memory_entry(&mut self, entry: oer_esp32s31_hal::types::PhyGainMemoryEntry) {
        self.program_gain_memory_entry(entry);
    }
}

#[cfg(target_arch = "riscv32")]
fn publish_phy_tx_gain_memory_to(
    registers: &mut impl PhyGainMemory,
    bank: bool,
    image: crate::channel::PhyWifiTxGainImage,
) {
    let hardware_base = registers.table_memory_base_index();
    let memory_base = hardware_base.wrapping_add(if bank { 32 } else { 0 });
    let mut entry = 0_u8;
    while entry != 32 {
        let entry_index = usize::from(entry);
        let gain_72 = packed_halfword(&image.output_72, entry_index);
        let gain_64 = packed_halfword(&image.output_64, entry_index);
        let gain_32 = packed_byte(&image.output_32, entry_index);
        let seed_index = tx_baseband_gain_index(gain_64) * 4;
        let memory_entry = oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
            memory_base.wrapping_add(entry),
            gain_72,
            gain_64,
            gain_32,
            [
                tx_gain_seed_halfword(&image, seed_index),
                tx_gain_seed_halfword(&image, seed_index + 1),
                tx_gain_seed_halfword(&image, seed_index + 2),
                tx_gain_seed_halfword(&image, seed_index + 3),
            ],
            image.config,
        );

        registers.program_gain_memory_entry(memory_entry);
        entry += 1;
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn publish_phy_tx_gain_memory(
    registers: &mut impl SharedPhyAccess,
    bank: bool,
    image: crate::channel::PhyWifiTxGainImage,
) {
    publish_phy_tx_gain_memory_to(&mut SharedPhyGainMemory { registers }, bank, image);
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn publish_phy_tx_gain_memory_channel<P>(
    channel: &mut RadioChannelHal<'_, P>,
    bank: bool,
    image: crate::channel::PhyWifiTxGainImage,
) {
    publish_phy_tx_gain_memory_to(channel, bank, image);
}

/// Publish the 16-entry Bluetooth bank produced by
/// `phy_bt_get_tx_tab_new`. The common hardware encoding is identical to the
/// Wi-Fi bank, while the table length, bank and typed input image remain
/// explicitly Bluetooth-owned.
#[cfg(target_arch = "riscv32")]
pub(crate) fn publish_bluetooth_tx_gain_memory(
    registers: &mut impl SharedPhyAccess,
    image: crate::calibration::bluetooth::PhyBluetoothTxGainImage,
) {
    let hardware_base = oer_esp32s31_hal::phy::memory::read_table_memory_base_index(registers);
    let memory_base = hardware_base.wrapping_add(32);
    let mut entry = 0_u8;
    while entry != 16 {
        let entry_index = usize::from(entry);
        let gain_72 = image.output_72[entry_index];
        let gain_64 = image.output_64[entry_index];
        let gain_32 = image.output_32[entry_index];
        let seed_index = tx_baseband_gain_index(gain_64) * 4;
        let memory_entry = oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
            memory_base.wrapping_add(entry),
            gain_72,
            gain_64,
            gain_32,
            [
                bluetooth_tx_gain_seed_halfword(&image, seed_index),
                bluetooth_tx_gain_seed_halfword(&image, seed_index + 1),
                bluetooth_tx_gain_seed_halfword(&image, seed_index + 2),
                bluetooth_tx_gain_seed_halfword(&image, seed_index + 3),
            ],
            image.config,
        );
        oer_esp32s31_hal::phy::memory::program_gain_memory_entry(registers, memory_entry);
        entry += 1;
    }
}

#[cfg(test)]
mod tests;
