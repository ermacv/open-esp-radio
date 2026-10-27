//! Frequency-memory and channel-switch operations over the generated PHY
//! frequency/channel block.

#![forbid(unsafe_code)]

use crate::phy::registers::{FrequencyRegisters, PhyPlacement};
use oer_esp32s31_pac::i2c_ana_mst_mem::PhyI2cCommandRam;
use oer_mmio::{Bits, Index, Placement};

/// Capture admission and the exact fields changed by ownership and read selection.
pub fn capture_read_configuration<P: PhyPlacement>(
    frequency: &FrequencyRegisters<P>,
) -> Option<super::ReadConfiguration> {
    let control = frequency.fe.read_frequency_control();
    let read = frequency.fe.read_frequency_memory_read_control();
    if control.channel_switch_pulse() || control.memory_write_pulse() || read.read_pulse() {
        return None;
    }
    Some(super::ReadConfiguration {
        control,
        number: frequency.fe.read_i2c_number_control(),
    })
}

/// Restore selection and ownership with fresh preserving RMWs; never pulse a writer.
pub fn restore_read_configuration<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    saved: super::ReadConfiguration,
) {
    frequency.fe.modify_i2c_number_control(|w| {
        w.set_memory_read_mode(saved.number.memory_read_mode());
    });
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_address_low(saved.control.memory_address_low())
            .set_memory_address_high(saved.control.memory_address_high())
            .set_hardware_frequency_disable(saved.control.hardware_frequency_disable_variant());
    });
}

// This lookup is evaluated only in the inline const below; both offsets and
// image geometry are generated from the register model, never a local address map.
const fn image_index(offsets: &[u32], offset: usize) -> usize {
    let mut index = 0;
    while index < offsets.len() {
        if offsets[index] as usize == offset {
            return index;
        }
        index += 1;
    }
    panic!("frequency word must belong to its owner's diagnostic image")
}

/// Ordered, side-effect-free whole-word observations from the generated image.
pub fn observe<P: PhyPlacement>(frequency: &FrequencyRegisters<P>) -> Option<super::Observation> {
    use oer_esp32s31_pac::fe::{self, PhyFrequencyChannelOracleFe};
    let control_image = frequency.fe.radio_phy_register_image(
        const {
            image_index(
                &PhyFrequencyChannelOracleFe::<P::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS,
                fe::frequency_control::OFFSET,
            )
        },
    )?;
    let parameter_image = frequency.fe.radio_phy_register_image(
        const {
            image_index(
                &PhyFrequencyChannelOracleFe::<P::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS,
                fe::frequency_parameter_1_status::OFFSET,
            )
        },
    )?;
    let result = frequency.fe.read_frequency_memory_read_result().value();
    let control = frequency.fe.read_frequency_control();
    let ready = frequency
        .fe
        .read_frequency_parameter_1_status()
        .frequency_ready();
    Some(super::Observation {
        control_image,
        parameter_image,
        result,
        high_address_or_reset: control.memory_address_high(),
        ready,
    })
}

/// Corrected capacitor word, retaining the ROM's signed spill into the mode.
///
/// SOURCE: ROM_REV0_PHY_FREQUENCY_PACKING. The word keeps bits 15 and 13:8
/// of `raw`, takes the adjusted code's low byte, and places the adjusted
/// code sign-extended and shifted right by eight at bit 14 and up.
pub const fn frequency_cap_correction_word(raw: u32, adjusted: u16) -> u32 {
    let signed_code = adjusted as i16 as i32 as u32;
    (raw & 0x0000_bf00) | (adjusted as u8 as u32) | ((signed_code >> 8) << 14)
}

/// Semantic PHY-I2C number-address sequence consumed by
/// `phy_freq_i2c_num_addr`.
///
/// Callers provide only the eleven addresses recovered from the vendor
/// descriptor graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyFrequencyI2cNumberAddresses {
    values: [Bits<5>; 11],
}

impl PhyFrequencyI2cNumberAddresses {
    /// Bind the complete recovered address sequence to its five-bit hardware
    /// domain.
    pub const fn new(values: [u8; 11]) -> Option<Self> {
        let mut bits = [Bits::<5>::constant::<0>(); 11];
        let mut index = 0;
        while index != values.len() {
            match Bits::new(values[index] as u32) {
                Ok(value) => bits[index] = value,
                Err(_) => return None,
            }
            index += 1;
        }
        Some(Self { values: bits })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChannelCbwFields {
    pub(crate) tx_offset: u8,
    pub(crate) control_0: u8,
    pub(crate) control_1_high: u8,
    pub(crate) control_1_low: u8,
}

pub(crate) const fn bss_tx_offset(cbw: u8) -> u8 {
    if cbw == 2 {
        2
    } else if cbw == 3 {
        1
    } else {
        0
    }
}

pub(crate) const fn channel_cbw_fields(cbw: u32) -> ChannelCbwFields {
    let high = cbw / 16;
    if high != 0 {
        let normalized = high.wrapping_sub(1) as u8;
        let low = (cbw % 16) as u8;
        ChannelCbwFields {
            tx_offset: low,
            control_0: low % 4,
            control_1_high: normalized % 8,
            control_1_low: normalized % 4,
        }
    } else {
        let low = (cbw % 16) as u8;
        let normalized = low.saturating_sub(2);
        ChannelCbwFields {
            tx_offset: normalized,
            control_0: normalized % 4,
            control_1_high: if low != 0 { 1 } else { 0 },
            control_1_low: if low >= 2 { 1 } else { 0 },
        }
    }
}

pub(crate) const fn rv32_signed_div(dividend: i32, divisor: i32) -> i32 {
    if divisor == 0 {
        -1
    } else if dividend == i32::MIN && divisor == -1 {
        i32::MIN
    } else {
        dividend / divisor
    }
}

/// Complete rev0 ROM `phy_nrx_freq_set`'s quotient, below 2^24.
///
/// SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL.
pub(crate) fn nrx_frequency_quotient(shift: u32, frequency: u32) -> Bits<24> {
    let numerator = 0x50_u32.wrapping_mul(2_u32.wrapping_pow(shift % 32)) as i32;
    let quotient = rv32_signed_div(numerator, frequency as i32) as u32;
    Bits::truncate(quotient)
}

/// The frequency-memory address domain: eleven bits.
const fn frequency_memory_address(address: u16) -> (Bits<10>, bool) {
    assert!(
        address < 0x800,
        "frequency-memory address must fit its eleven-bit domain"
    );
    (Bits::truncate(address as u32), address & 0x400 != 0)
}

/// Select the recovered two-bit baseband mode.
pub fn set_baseband_mode<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, mode: u8) {
    let mode = Bits::<2>::new(u32::from(mode)).expect("baseband mode must fit its two-bit field");
    frequency.fe.modify_frequency_parameter_1_status(|w| {
        w.set_bits_0_1_opaque(mode);
    });
}

/// Apply `phy_freq_module_resetn`'s bit18 low/high writes with two fresh RMWs.
/// The electrical reset target and effective edge remain unqualified.
pub fn reset_module<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>) {
    // SOURCE: BX10_S31_FE_FREQ_RESET_CONTEXT_20261027.
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_address_high(false);
    });
    // SOURCE: BX10_S31_FE_FREQ_RESET_CONTEXT_20261027.
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_address_high(true);
    });
}

/// Retain the observation of the I2C-number word after software frequency
/// ownership settles; the caller samples the SDM deadline counter first.
pub fn observe_software_frequency_boundary<P: PhyPlacement>(frequency: &FrequencyRegisters<P>) {
    let _ = frequency.fe.read_i2c_number_control();
}

/// Select whether hardware owns frequency updates.
pub fn set_hardware_control<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, enabled: bool) {
    frequency.fe.modify_frequency_control(|w| {
        w.set_hardware_frequency_disable(if !enabled {
            oer_esp32s31_pac::fe::frequency_control::HardwareFrequencyDisable::Software
        } else {
            oer_esp32s31_pac::fe::frequency_control::HardwareFrequencyDisable::Hardware
        });
    });
}

/// Complete rev0 ROM `phy_freq_reg_init`'s register mode with the
/// parameter override.
///
/// SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL.
const REGISTER_MODE_PARAMETER_OVERRIDE: u8 = 0x20;

/// Complete rev0 ROM `phy_freq_reg_init`'s default register mode.
///
/// SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL.
const REGISTER_MODE_DEFAULT: u8 = 0x42;

/// Apply complete rev0 ROM `phy_freq_reg_init(2, 4)`.
pub fn initialize_registers<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    parameter_override: bool,
) {
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_switch_pulse(false)
            .set_hardware_frequency_disable(if false {
                oer_esp32s31_pac::fe::frequency_control::HardwareFrequencyDisable::Software
            } else {
                oer_esp32s31_pac::fe::frequency_control::HardwareFrequencyDisable::Hardware
            });
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_bit_30_opaque(true);
    });
    let mode = if parameter_override {
        REGISTER_MODE_PARAMETER_OVERRIDE
    } else {
        REGISTER_MODE_DEFAULT
    };
    frequency.fe.modify_frequency_control(|w| {
        w.set_bits_22_29_opaque(mode);
    });
    // The two vendor initialization images write undeclared bits; they stay
    // whole-word PAC operations.
    frequency.fe.frequency_parameter_0();
    frequency.fe.frequency_parameter_1_initialization();
}

/// Apply complete rev0 ROM `phy_freq_i2c_mem_write`.
pub fn write_memory<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    address: u16,
    value: u32,
    mode: u8,
) {
    let data = Bits::<24>::new(value).expect("frequency-memory data must fit 24 bits");
    let (address_low, address_high) = frequency_memory_address(address);
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_address_low(address_low)
            .set_memory_address_high(address_high);
    });
    frequency.fe.write_frequency_memory_data(|w| {
        w.set_data(data).set_mode(mode);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_write_pulse(true);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_write_pulse(false);
    });
}

/// Apply complete rev0 ROM `phy_read_rf_freq_mem`.
pub fn read_memory<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    address: u16,
    mode: u8,
) -> u32 {
    let (address_low, address_high) = frequency_memory_address(address);
    let mode = Bits::<2>::new(u32::from(mode))
        .expect("frequency-memory read mode must fit its two-bit field");
    frequency.fe.modify_frequency_control(|w| {
        w.set_memory_address_low(address_low)
            .set_memory_address_high(address_high);
    });
    frequency.fe.modify_i2c_number_control(|w| {
        w.set_memory_read_mode(mode);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_memory_read_control(|w| {
        w.set_read_pulse(true);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_memory_read_control(|w| {
        w.set_read_pulse(false);
    });
    frequency.fe.read_frequency_memory_read_result().value()
}

/// Restore the low frequency-control channel index without pulsing a switch.
pub fn set_channel_index<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, index: u8) {
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_index(index);
    });
}

/// `phy_freq_i2c_num_addr`'s writes: the two prefix addresses in one
/// fresh-read RMW of `I2C_NUMBER_CONTROL`, then the three address words in
/// ascending order, each written whole.
pub fn configure_i2c_number_addresses<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    addresses: PhyFrequencyI2cNumberAddresses,
) {
    let [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10] = addresses.values;
    let zero = Bits::<5>::constant::<0>();
    frequency.fe.modify_i2c_number_control(|w| {
        w.set_bits_8_12_opaque(a0).set_bits_13_17_opaque(a1);
    });
    write_i2c_number_word(frequency, Index::constant::<0>(), [a2, a3, a4, a5, a6, a7]);
    write_i2c_number_word(
        frequency,
        Index::constant::<1>(),
        [a8, a9, a10, zero, zero, zero],
    );
    write_i2c_number_word(frequency, Index::constant::<2>(), [zero; 6]);
}

/// Store one address word whole: six five-bit addresses, zero above them.
fn write_i2c_number_word<P: PhyPlacement>(
    frequency: &mut FrequencyRegisters<P>,
    index: Index<3>,
    [f0, f1, f2, f3, f4, f5]: [Bits<5>; 6],
) {
    frequency.fe.write_i2c_number_word(index, |w| {
        w.set_bits_0_4_opaque(f0)
            .set_bits_5_9_opaque(f1)
            .set_bits_10_14_opaque(f2)
            .set_bits_15_19_opaque(f3)
            .set_bits_20_24_opaque(f4)
            .set_bits_25_29_opaque(f5)
            .set_bits_30_31_opaque(Bits::constant::<0>());
    });
}

/// Publish the two pre-delay edges of `phy_freq_chan_en_sw`.
pub fn start_channel_switch<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, index: u8) {
    set_channel_index(frequency, index);
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_switch_pulse(true);
    });
}

/// Complete the caller-delayed clear of `phy_freq_chan_en_sw`.
pub fn clear_channel_switch<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>) {
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_switch_pulse(false);
    });
}

/// Sample the frequency-ready field exactly once.
pub fn frequency_ready<P: PhyPlacement>(frequency: &FrequencyRegisters<P>) -> bool {
    frequency
        .fe
        .read_frequency_parameter_1_status()
        .frequency_ready()
}

/// Apply complete rev0 ROM `phy_nrx_freq_set`.
pub fn configure_nrx_frequency<P: PhyPlacement>(
    frequency_registers: &mut FrequencyRegisters<P>,
    frequency: u32,
) {
    configure_nrx_frequency_between_samples(frequency_registers, frequency, |_| {});
}

/// [`configure_nrx_frequency`] with `between` run once between the ROM's two
/// samples of the word, where a test changes the word to tell them apart.
///
/// The ROM samples the word twice: the first sample supplies the eight-bit
/// shift selector, the second the high fields its single store keeps. The
/// second sample is the read of the field update.
pub(crate) fn configure_nrx_frequency_between_samples<P: PhyPlacement>(
    frequency_registers: &mut FrequencyRegisters<P>,
    frequency: u32,
    between: impl FnOnce(&mut FrequencyRegisters<P>),
) {
    let first = frequency_registers.wifi_bb.read_nrx_frequency_control();
    let shift = first.frequency_numerator_shift().get() + first.bits_29_31_opaque().get() * 0x20;
    between(frequency_registers);
    let quotient = nrx_frequency_quotient(shift, frequency);
    frequency_registers
        .wifi_bb
        .modify_nrx_frequency_control(|w| {
            w.set_bits_0_23_opaque(quotient);
        });
}

/// Apply the two NRX writes inside complete rev0 ROM `phy_bb_reg_init`.
pub fn initialize_nrx_baseband<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>) {
    // The complete ROM body replaces the twenty-four-bit multifunction field
    // with this bounded initialization image.
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.wifi_bb.modify_nrx_frequency_control(|w| {
        w.set_bits_0_23_opaque(Bits::constant::<0x433af>());
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.wifi_bb.modify_nrx_frequency_control(|w| {
        w.set_frequency_numerator_shift(Bits::constant::<0x17>());
    });
}

/// Publish the channel-offset prefix of complete `phy_bb_bss_cbw40`.
pub fn configure_bss_cbw_prefix<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, cbw: u8) {
    // Lossless: the offset is at most two.
    let offset = Bits::<4>::truncate(u32::from(bss_tx_offset(cbw)));
    frequency.wifi_mac.modify_channel_tx_offset_control(|w| {
        w.set_primary_20_channel_offset(offset);
    });
}

/// Publish the three FBW suffix edges of complete `phy_bb_bss_cbw40`.
pub fn configure_bss_cbw_suffix<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, cbw: u8) {
    let selection: Bits<2> = if cbw == 0 {
        Bits::constant::<0>()
    } else {
        Bits::constant::<1>()
    };
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bit_16_opaque(false).set_bit_19_opaque(false);
    });
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bits_17_18_opaque(selection);
    });
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bits_20_21_opaque(selection);
    });
}

/// Apply the four fresh-read replacements of complete `phy_bb_cbw_chan_cfg`.
pub fn configure_channel_cbw<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>, cbw: u32) {
    let fields = channel_cbw_fields(cbw);
    // Lossless: each value is reduced below its field's width.
    let tx_offset = Bits::<4>::truncate(u32::from(fields.tx_offset));
    let control_0 = Bits::<2>::truncate(u32::from(fields.control_0));
    let control_1_high = Bits::<3>::truncate(u32::from(fields.control_1_high));
    let control_1_low = Bits::<2>::truncate(u32::from(fields.control_1_low));
    // Channel offset owns only the low nibble. Preserve the adjacent
    // minimum-power setting, as both ROM channel setters do.
    frequency.wifi_mac.modify_channel_tx_offset_control(|w| {
        w.set_primary_20_channel_offset(tx_offset);
    });
    frequency.wifi_bb.modify_channel_cbw_control_0(|w| {
        w.set_bits_0_1_opaque(control_0);
    });
    frequency.wifi_bb.modify_channel_cbw_control_1(|w| {
        w.set_bits_2_4_opaque(control_1_high);
    });
    frequency.wifi_bb.modify_channel_cbw_control_1(|w| {
        w.set_bits_0_1_opaque(control_1_low);
    });
}

/// Apply the three fresh-read updates of complete `phy_bt_filter_reg`.
pub fn configure_bt_filter<P: PhyPlacement>(frequency: &mut FrequencyRegisters<P>) {
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bit_25_opaque(true);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bit_22_opaque(false);
    });
    // SOURCE: ROM_REV0_PHY_FREQUENCY_CHANNEL, S31_ESP_PACS_SVD.
    frequency.fe.modify_fbw_bt_filter_control(|w| {
        w.set_bits_23_24_opaque(Bits::constant::<0x0>());
    });
}

/// Publish the TX-cap readback into command-memory entry one.
///
/// SOURCE: ROM_REV0_PHY_I2C. ROM `phy_i2c_master_mem_txcap` stores TXRF
/// register 2 with the readback in command slot one.
pub fn publish_tx_cap<P: Placement>(command_ram: &mut PhyI2cCommandRam<P>, value: u8) {
    command_ram.write_command_memory(Index::constant::<1>(), |w| {
        w.set_block(oer_esp32s31_pac::analog_i2c::blocks::TXRF.code())
            .set_register(2)
            .set_data(value);
    });
}

#[cfg(test)]
mod tests;
