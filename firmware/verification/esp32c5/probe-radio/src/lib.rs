#![no_std]

//! Link-time probes for compiled vendor/Rust analog I2C comparison.
//!
//! Every probe takes the vendor function's integer arguments in its order and
//! drives the shipping ESP32-C5 analog I2C owners. The vendor leaves ignore
//! their `host_id` argument and derive the host from the block; so does the
//! production owner. These wrappers are test-harness artifacts, never driver
//! entry points.

use oer_esp32c5_hal::analog::AnalogI2c;
use oer_esp32c5_pac::{PhyI2cAddress, PhyI2cBlock, PhyI2cHost, RadioPartitions};
use oer_radio_analog::{AnalogField, AnalogRegisterBus, FieldRead, FieldWrite, Step};

/// Bus actions (one start or completion observation each) a probe performs
/// before it reports an exhausted schedule. This bounds the harness only; it
/// is not hardware time.
const MAXIMUM_ACTIONS: u32 = 64;
/// Returned when the block is outside the libphy tables.
const INVALID_ARGUMENT: u32 = 0x1_0000;
/// Returned when the schedule is exhausted before completion.
const EXHAUSTED: u32 = 0x1_0001;

/// Borrow the isolated analog register bus.
fn with_bus<R>(call: impl FnOnce(&mut AnalogI2c) -> R) -> R {
    let RadioPartitions { phy_i2c, .. } = RadioPartitions::for_validation();
    call(&mut AnalogI2c::new(phy_i2c))
}

fn address(block: u32, register: u32) -> Option<PhyI2cAddress> {
    let block = PhyI2cBlock::from_vendor_abi(u8::try_from(block).ok()?)?;
    Some(PhyI2cAddress::new(block, register as u8))
}

fn drive<T>(mut poll: impl FnMut() -> Step<T>) -> Option<T> {
    for _ in 0..MAXIMUM_ACTIONS {
        if let Step::Ready(value) = poll() {
            return Some(value);
        }
    }
    None
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

oer_probe_macros::probe! {
    /// `phy_get_i2c_hostid_(block)`: rewrite the host map, return the host.
    pub fn open_phy_i2c_trace_phy_get_i2c_hostid_(block: u32) -> u32 {
        let Some(block) = u8::try_from(block).ok().and_then(PhyI2cBlock::from_vendor_abi) else {
            return INVALID_ARGUMENT;
        };
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        match phy_i2c.configure_and_select_host(block) {
            PhyI2cHost::Host0 => 0,
            PhyI2cHost::Host1 => 1,
        }
    }
}

oer_probe_macros::probe! {
    /// `phy_get_i2c_read_mask_(block)`: the one-hot read mask; zero outside
    /// the table, as the vendor leaf returns.
    pub fn open_phy_i2c_trace_phy_get_i2c_read_mask_(block: u32) -> u32 {
        match u8::try_from(block).ok().and_then(PhyI2cBlock::from_vendor_abi) {
            Some(block) => !block.read_mask_complement_low() & 0x00ff_ffff,
            None => 0,
        }
    }
}

oer_probe_macros::probe! {
    /// `phy_chip_i2c_readReg(block, host_id, reg_add)`.
    pub fn open_phy_i2c_trace_phy_chip_i2c_readReg(block: u32, host_id: u32, reg_add: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        with_bus(|bus| {
            let mut started = false;
            let result = drive(|| {
                if !started {
                    started = bus.try_start_read(address).is_ok();
                    return Step::Pending;
                }
                match bus.try_finish_read(address) {
                    Ok(value) => Step::Ready(value),
                    Err(_) => Step::Pending,
                }
            });
            result.map_or(EXHAUSTED, u32::from)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_chip_i2c_writeReg(block, host_id, reg_add, data)`.
    pub fn open_phy_i2c_trace_phy_chip_i2c_writeReg(block: u32, host_id: u32, reg_add: u32, data: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Ok(data) = u8::try_from(data) else { return INVALID_ARGUMENT };
        with_bus(|bus| {
            let mut started = false;
            let result = drive(|| {
                if !started {
                    started = bus.try_start_write(address, data).is_ok();
                    return Step::Pending;
                }
                match bus.try_finish_write(address) {
                    Ok(()) => Step::Ready(()),
                    Err(_) => Step::Pending,
                }
            });
            result.map_or(EXHAUSTED, |()| 0)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_readReg_Mask(block, host_id, reg_add, msb, lsb)` as a polled
    /// field read over the portable analog bus.
    pub fn open_phy_i2c_trace_phy_i2c_readReg_Mask(block: u32, host_id: u32, reg_add: u32, msb: u32, lsb: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Some(field) = AnalogField::new(address, msb as u8, lsb as u8) else {
            return INVALID_ARGUMENT;
        };
        with_bus(|bus| {
            let mut read = FieldRead::new(field);
            let result = drive(|| read.poll(bus));
            result.map_or(EXHAUSTED, u32::from)
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_writeReg_Mask(block, host_id, reg_add, msb, lsb, data)` as a
    /// polled field write. A value wider than the field is rejected before any
    /// bus action, unlike the vendor leaf's unmasked OR.
    pub fn open_phy_i2c_trace_phy_i2c_writeReg_Mask(block: u32, host_id: u32, reg_add: u32, msb: u32, lsb: u32, data: u32) -> u32 {
        let _ = host_id;
        let Some(address) = address(block, reg_add) else { return INVALID_ARGUMENT };
        let Some(field) = AnalogField::new(address, msb as u8, lsb as u8) else {
            return INVALID_ARGUMENT;
        };
        let Some(mut write) = u8::try_from(data).ok().and_then(|data| FieldWrite::new(field, data).ok()) else {
            return INVALID_ARGUMENT;
        };
        with_bus(|bus| {
            let result = drive(|| write.poll(bus));
            result.map_or(EXHAUSTED, |()| 0)
        })
    }
}

/// Parallel-sequence polls before a probe reports an exhausted schedule:
/// select, 44 pairs of one start and two idle observations, restore.
const MAXIMUM_PARALLEL_ACTIONS: u32 = 4 * 64;

oer_probe_macros::probe! {
    /// `phy_i2c_paral_write(block0, reg0, data0, block1, reg1, data1, flag)`:
    /// both commands published, host 0 then host 1 polled idle. Production
    /// publishes without the start bit, so only `flag == 0` is accepted.
    pub fn open_phy_i2c_trace_phy_i2c_paral_write(block0: u32, reg0: u32, data0: u32, block1: u32, reg1: u32, data1: u32, flag: u32) -> u32 {
        let byte = |value: u32| u8::try_from(value).ok();
        let (Some(b0), Some(r0), Some(d0), Some(b1), Some(r1), Some(d1)) =
            (byte(block0), byte(reg0), byte(data0), byte(block1), byte(reg1), byte(data1))
        else {
            return INVALID_ARGUMENT;
        };
        if flag != 0 {
            return INVALID_ARGUMENT;
        }
        with_bus(|bus| {
            use oer_radio_analog::{ParallelAnalogBus, ParallelHost};
            bus.start_pair(oer_esp32c5_pac::PhyI2cParallelWrite::new((b0, r0, d0), (b1, r1, d1)));
            for host in [ParallelHost::First, ParallelHost::Second] {
                let mut polls = 0;
                while bus.is_busy(host) {
                    polls += 1;
                    if polls == MAXIMUM_ACTIONS {
                        return EXHAUSTED;
                    }
                }
            }
            0
        })
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_init1()` over the `phy_param` image at `parameters`: the
    /// production parallel initialization with the fields the vendor reads.
    ///
    /// The caller supplies a readable 1080-byte parameter image.
    pub fn open_phy_i2c_trace_phy_i2c_init1(parameters: u32) -> u32 {
        let byte = |offset: usize| {
            // SAFETY: the harness supplies a readable parameter image of the
            // vendor `phy_param` size at `parameters`.
            unsafe { core::ptr::read_volatile((parameters as usize + offset) as *const u8) }
        };
        let halfword = |offset: usize| u16::from(byte(offset)) | (u16::from(byte(offset + 1)) << 8);
        let inputs = oer_esp32c5_pac::PhyI2cInitializationInputs {
            parameter_f5: byte(0xf5),
            parameter_f6: byte(0xf6),
            parameter_f7: byte(0xf7),
            parameter_f8: byte(0xf8),
            parameter_f9: byte(0xf9),
            parameter_fa: byte(0xfa),
            parameter_fb: byte(0xfb),
            parameter_fc: byte(0xfc),
            parameter_410: byte(0x410),
            parameter_412: byte(0x412),
            parameter_416: halfword(0x416),
        };
        with_bus(|bus| {
            let mut sequence = oer_esp32c5_hal::analog::initialization(inputs);
            for _ in 0..MAXIMUM_PARALLEL_ACTIONS {
                if sequence.poll(bus) == Step::Ready(()) {
                    return 0;
                }
            }
            EXHAUSTED
        })
    }
}

/// Drive one analog configuration leaf to completion on the isolated bus.
fn configure(leaf: oer_esp32c5_pac::PhyI2cConfiguration) -> u32 {
    with_bus(|bus| {
        let mut configuration = oer_esp32c5_hal::analog::configuration(leaf);
        for _ in 0..MAXIMUM_PARALLEL_ACTIONS {
            match configuration.poll(bus) {
                Ok(Step::Ready(())) => return 0,
                Ok(Step::Pending) => {}
                Err(_) => return INVALID_ARGUMENT,
            }
        }
        EXHAUSTED
    })
}

/// Borrow the isolated PHY radio partition.
fn with_radio<R>(call: impl FnOnce(&mut oer_esp32c5_pac::PhyRadioRegisters) -> R) -> R {
    let RadioPartitions { mut phy_radio, .. } = RadioPartitions::for_validation();
    call(&mut phy_radio)
}

oer_probe_macros::probe! {
    /// `phy_open_i2c_xpd()`.
    pub fn open_phy_i2c_trace_phy_open_i2c_xpd() -> u32 {
        with_radio(|radio| radio.power_analog_i2c());
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_clk_sel(n)` for `n` within the SDA side-guard field.
    pub fn open_phy_i2c_trace_phy_i2c_clk_sel(selection: u32) -> u32 {
        let Some(selection) = u8::try_from(selection)
            .ok()
            .and_then(oer_esp32c5_pac::PhyI2cClockSelection::new)
        else {
            return INVALID_ARGUMENT;
        };
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        phy_i2c.select_clock(selection);
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_band_i2c_set(band)`; the vendor ignores the band.
    pub fn open_phy_i2c_trace_phy_band_i2c_set(band: u32) -> u32 {
        let _ = band;
        configure(oer_esp32c5_pac::PhyI2cConfiguration::Band)
    }
}

oer_probe_macros::probe! {
    /// `phy_xtal_reg_set()`.
    pub fn open_phy_i2c_trace_phy_xtal_reg_set() -> u32 =>
        configure(oer_esp32c5_pac::PhyI2cConfiguration::CrystalRegisters);
}

oer_probe_macros::probe! {
    /// `phy_bias_reg_set()`.
    pub fn open_phy_i2c_trace_phy_bias_reg_set() -> u32 =>
        configure(oer_esp32c5_pac::PhyI2cConfiguration::BiasRegisters);
}

oer_probe_macros::probe! {
    /// `phy_dac_rate_set(rate)`; the vendor ignores the rate.
    pub fn open_phy_i2c_trace_phy_dac_rate_set(rate: u32) -> u32 {
        let _ = rate;
        let result = configure(oer_esp32c5_pac::PhyI2cConfiguration::DacRate);
        if result == 0 {
            with_radio(|radio| radio.clear_dac_rate_bits());
        }
        result
    }
}

oer_probe_macros::probe! {
    /// `phy_adc_rate_set(rate)` for a rate of 0 or 1.
    pub fn open_phy_i2c_trace_phy_adc_rate_set(rate: u32) -> u32 {
        let rate = match rate {
            0 => false,
            1 => true,
            _ => return INVALID_ARGUMENT,
        };
        let result = configure(oer_esp32c5_pac::PhyI2cConfiguration::AdcRate(rate));
        if result == 0 {
            with_radio(|radio| radio.set_adc_rate_bits(rate));
        }
        result
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_bbpll_set()`: the DAC rate and then the ADC rate at zero.
    pub fn open_phy_i2c_trace_phy_i2c_bbpll_set() -> u32 {
        let result = open_phy_i2c_trace_phy_dac_rate_set(0);
        if result != 0 {
            return result;
        }
        open_phy_i2c_trace_phy_adc_rate_set(0)
    }
}

/// Byte `offset` of the `phy_param` image at `parameters`.
fn parameter(parameters: u32, offset: usize) -> u8 {
    // SAFETY: the harness supplies a readable parameter image of the vendor
    // `phy_param` size at `parameters`.
    unsafe { core::ptr::read_volatile((parameters as usize + offset) as *const u8) }
}

/// Whether phy_param byte 0x2A, set for a channel of at least 3001 MHz, is set.
fn high_band(parameters: u32) -> bool {
    parameter(parameters, 0x2a) != 0
}

oer_probe_macros::probe! {
    /// `phy_open_fe_bb_clk()`.
    pub fn open_phy_i2c_trace_phy_open_fe_bb_clk() -> u32 {
        with_radio(|radio| radio.open_front_end_baseband_clocks());
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_i2cmst_reg_init()`.
    pub fn open_phy_i2c_trace_phy_i2cmst_reg_init() -> u32 {
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        phy_i2c.initialize_master_registers();
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_iq_swap_set()` over the `phy_param` image at `parameters`.
    pub fn open_phy_i2c_trace_phy_iq_swap_set(parameters: u32) -> u32 {
        with_radio(|radio| radio.set_iq_swap(high_band(parameters)));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_fe_reg_init()` over the `phy_param` image at `parameters`.
    pub fn open_phy_i2c_trace_phy_fe_reg_init(parameters: u32) -> u32 {
        let (swap, scale) = (high_band(parameters), parameter(parameters, 0x28a));
        with_radio(|radio| radio.initialize_front_end(swap, scale));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_pwdet_reg_init()`.
    pub fn open_phy_i2c_trace_phy_pwdet_reg_init() -> u32 {
        with_radio(|radio| radio.initialize_power_detector());
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_dac_scale_set(scale)`.
    pub fn open_phy_i2c_trace_phy_dac_scale_set(scale: u32) -> u32 {
        with_radio(|radio| radio.set_dac_scale(scale != 0));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_rxiq_scale_set()` over the `phy_param` image at `parameters`.
    pub fn open_phy_i2c_trace_phy_rxiq_scale_set(parameters: u32) -> u32 {
        let selection = parameter(parameters, 0x28a);
        with_radio(|radio| radio.set_rx_iq_scale(selection));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_pwdet_sar2_init()` over the `phy_param` image at `parameters`.
    pub fn open_phy_i2c_trace_phy_pwdet_sar2_init(parameters: u32) -> u32 {
        let swap = high_band(parameters);
        with_radio(|radio| radio.initialize_power_detector_sar2(swap));
        0
    }
}

/// The `phy_param` fields `phy_i2c_init1` and `phy_filter_dcap_set` read.
fn initialization_inputs(parameters: u32) -> oer_esp32c5_pac::PhyI2cInitializationInputs {
    let byte = |offset| parameter(parameters, offset);
    oer_esp32c5_pac::PhyI2cInitializationInputs {
        parameter_f5: byte(0xf5),
        parameter_f6: byte(0xf6),
        parameter_f7: byte(0xf7),
        parameter_f8: byte(0xf8),
        parameter_f9: byte(0xf9),
        parameter_fa: byte(0xfa),
        parameter_fb: byte(0xfb),
        parameter_fc: byte(0xfc),
        parameter_410: byte(0x410),
        parameter_412: byte(0x412),
        parameter_416: u16::from(byte(0x416)) | (u16::from(byte(0x417)) << 8),
    }
}

oer_probe_macros::probe! {
    /// `phy_i2c_rc_cal_set(first, second, third)` within its fields.
    pub fn open_phy_i2c_trace_phy_i2c_rc_cal_set(first: u32, second: u32, third: u32) -> u32 {
        let byte = |value: u32| u8::try_from(value).ok();
        let Some(arguments) = byte(first)
            .zip(byte(second))
            .zip(byte(third))
            .and_then(|((a, b), c)| oer_esp32c5_pac::PhyI2cRcCalibration::new(a, b, c))
        else {
            return INVALID_ARGUMENT;
        };
        configure(oer_esp32c5_pac::PhyI2cConfiguration::RcCalibration(arguments))
    }
}

oer_probe_macros::probe! {
    /// `phy_filter_dcap_set()` over the `phy_param` image at `parameters`.
    pub fn open_phy_i2c_trace_phy_filter_dcap_set(parameters: u32) -> u32 =>
        configure(oer_esp32c5_pac::PhyI2cConfiguration::FilterCapacitors(
            initialization_inputs(parameters),
        ));
}

oer_probe_macros::probe! {
    /// `phy_i2c_pkdet_set()`.
    pub fn open_phy_i2c_trace_phy_i2c_pkdet_set() -> u32 =>
        configure(oer_esp32c5_pac::PhyI2cConfiguration::PeakDetector);
}

oer_probe_macros::probe! {
    /// `phy_i2c_sar2_init_code(code)` for a twelve-bit code.
    pub fn open_phy_i2c_trace_phy_i2c_sar2_init_code(code: u32) -> u32 {
        let Some(code) = u16::try_from(code).ok().and_then(oer_esp32c5_pac::PhyI2cSar2Code::new) else {
            return INVALID_ARGUMENT;
        };
        configure(oer_esp32c5_pac::PhyI2cConfiguration::Sar2InitializationCode(code))
    }
}

oer_probe_macros::probe! {
    /// `phy_set_tsens_power(on)` for `on` of 0 or 1.
    pub fn open_phy_i2c_trace_phy_set_tsens_power(on: u32) -> u32 {
        let on = match on {
            0 => false,
            1 => true,
            _ => return INVALID_ARGUMENT,
        };
        with_radio(|radio| radio.set_temperature_sensor_power(on));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_set_tsens_pwr()`.
    pub fn open_phy_i2c_trace_phy_set_tsens_pwr() -> u32 {
        with_radio(|radio| radio.power_temperature_sensor());
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_tsens_read_init(mode, code)`; the vendor ignores both arguments.
    pub fn open_phy_i2c_trace_phy_tsens_read_init(mode: u32, code: u32) -> u32 {
        let _ = (mode, code);
        with_radio(|radio| radio.initialize_temperature_sensor());
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_bbpll_cal(start)`.
    pub fn open_phy_i2c_trace_phy_bbpll_cal(start: u32) -> u32 {
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        phy_i2c.calibrate_bbpll(start != 0);
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_bbpll_recal()`.
    pub fn open_phy_i2c_trace_phy_bbpll_recal() -> u32 {
        let RadioPartitions { mut phy_i2c, .. } = RadioPartitions::for_validation();
        phy_i2c.recalibrate_bbpll();
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_rxevm_init_cfg(enable, second, third)` for seven-bit parameters.
    pub fn open_phy_i2c_trace_phy_rxevm_init_cfg(enable: u32, second: u32, third: u32) -> u32 {
        let (Some(second), Some(third)) = (
            oer_esp32c5_pac::PhyRxEvmParameter::new(second),
            oer_esp32c5_pac::PhyRxEvmParameter::new(third),
        ) else {
            return INVALID_ARGUMENT;
        };
        with_radio(|radio| radio.initialize_rx_evm(enable != 0, second, third));
        0
    }
}

oer_probe_macros::probe! {
    /// `phy_rxevm_reset_mem()`.
    pub fn open_phy_i2c_trace_phy_rxevm_reset_mem() -> u32 {
        with_radio(|radio| radio.reset_rx_evm_memory());
        0
    }
}
