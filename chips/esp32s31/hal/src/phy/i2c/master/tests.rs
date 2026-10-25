//! The bus driver over host memory: each start refuses a busy host without
//! changing any word of the master, and each completion is one sample.
//!
//! Words are named by their byte offsets from the master's base: the host
//! command words at 0x0 and 0x4, the read-mask word at 0x1c and the host map
//! at 0x20. A command word holds the block, the register, the data byte, the
//! read/write bit (24), the busy bit (25) and the start bit (26).

use super::{AnalogBus, BluetoothTxPowerControlRegister, PhyI2cAccessError};
use oer_esp32s31_pac::analog_i2c;
use oer_esp32s31_pac::{
    analog_i2c::{Host, blocks},
    i2c_ana_mst::I2cAnaMst,
};
use std::{vec, vec::Vec};

/// Word indices of the byte offsets above.
const HOST_0: usize = 0;
const HOST_1: usize = 1;
const READ_MASK: usize = 0x1c / 4;
const HOST_MAP: usize = 0x20 / 4;
const BUSY: u32 = 1 << 25;
const START: u32 = 1 << 26;
const WRITE: u32 = 1 << 24;

fn memory() -> Vec<u32> {
    vec![0; oer_esp32s31_pac::instances::i2c_ana_mst::SIZE / 4]
}

#[test]
fn an_idle_read_publishes_the_read_mask_then_the_command() {
    let mut words = memory();
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    // BBTOP register 2: block 0x67 on host one, read select 0x4.
    AnalogBus::new(&mut master)
        .try_start_read(analog_i2c::bbtop::rf_close_control::ADDRESS)
        .expect("an idle host starts");
    drop(master);
    assert_eq!(words[READ_MASK], 0xff00_0000 | (0x00ff_ffff & !0x4));
    assert_eq!(words[HOST_1], 0x67 | (0x2 << 8) | START);
    assert_eq!(words[HOST_0], 0);
    assert_eq!(words[HOST_MAP], 0x3fa0 << 4, "the radio host map");
}

fn assert_busy_refusal(
    host: usize,
    start: impl FnOnce(&mut AnalogBus<'_, oer_mmio::Host<'_>>) -> Result<(), PhyI2cAccessError>,
) {
    let mut words = memory();
    words.fill(0xa5a5_5a5a);
    words[HOST_0] &= !BUSY;
    words[HOST_1] &= !BUSY;
    words[host] |= BUSY;
    words[HOST_MAP] = 0x0a << 4;
    let before = words.clone();
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    assert_eq!(
        start(&mut AnalogBus::new(&mut master)),
        Err(PhyI2cAccessError::Busy)
    );
    drop(master);
    assert_eq!(
        bytemuck::cast_slice::<u32, u8>(&words),
        bytemuck::cast_slice::<u32, u8>(&before),
        "Busy leaves the entire master block unchanged"
    );
}

#[test]
fn busy_refusal_read_leaves_the_master_unchanged() {
    assert_busy_refusal(HOST_1, |bus| {
        bus.try_start_read(analog_i2c::bbtop::rf_close_control::ADDRESS)
    });
    assert_busy_refusal(HOST_0, |bus| {
        bus.try_start_read(analog_i2c::bbpll::oc_ref_div::ADDRESS)
    });
}

#[test]
fn busy_refusal_write_leaves_the_master_unchanged() {
    assert_busy_refusal(HOST_1, |bus| {
        bus.try_start_write(analog_i2c::bbtop::rf_close_control::ADDRESS, 0x07)
    });
    assert_busy_refusal(HOST_0, |bus| {
        bus.try_start_write(analog_i2c::bbpll::oc_ref_div::ADDRESS, 0x07)
    });
}

#[test]
fn busy_refusal_bluetooth_read_leaves_the_master_unchanged() {
    assert_busy_refusal(HOST_1, |bus| {
        bus.start_bluetooth_tx_power_control_read(BluetoothTxPowerControlRegister::Low0)
    });
}

#[test]
fn busy_refusal_bluetooth_write_leaves_the_master_unchanged() {
    assert_busy_refusal(HOST_1, |bus| {
        bus.start_bluetooth_tx_power_control_write(BluetoothTxPowerControlRegister::Low0, 0x07)
    });
}

#[test]
fn an_idle_write_publishes_one_write_command() {
    let mut words = memory();
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    AnalogBus::new(&mut master)
        .try_start_write(analog_i2c::bbtop::rf_close_control::ADDRESS, 0x07)
        .expect("an idle host starts");
    drop(master);
    assert_eq!(
        words[HOST_1],
        0x67 | (0x2 << 8) | (0x07 << 16) | WRITE | START
    );
    assert_eq!(words[READ_MASK], 0, "a write publishes no read mask");
}

#[test]
fn a_completion_is_one_sample_of_the_busy_bit() {
    let mut words = memory();
    words[HOST_1] = BUSY | (0x5a << 16);
    let master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    assert_eq!(
        super::try_finish_read(&master, analog_i2c::bbtop::rf_close_control::ADDRESS),
        Err(PhyI2cAccessError::Busy)
    );
    drop(master);
    words[HOST_1] = 0x5a << 16;
    let master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    assert_eq!(
        super::try_finish_read(&master, analog_i2c::bbtop::rf_close_control::ADDRESS),
        Ok(0x5a)
    );
    assert_eq!(
        super::try_finish_write(&master, analog_i2c::bbtop::rf_close_control::ADDRESS),
        Ok(())
    );
}

#[test]
fn an_unrouted_block_publishes_nothing() {
    let mut words = memory();
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    assert_eq!(
        AnalogBus::new(&mut master).try_start_read(blocks::CPLL.register(0)),
        Err(PhyI2cAccessError::UnroutedAddress)
    );
    drop(master);
    assert!(words.iter().all(|word| *word == 0));
}

#[test]
fn a_parallel_pair_starts_host_zero_then_host_one_and_the_map_is_restored() {
    let mut words = memory();
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    let mut bus = AnalogBus::new(&mut master);
    bus.select_parallel_host_map();
    let pair = super::super::PhyI2cCommandMemoryInputs::new(0x11, 0x22, 0x33, 0x3e, 0x55, 0x66)
        .initialization_stage_two_pair(0)
        .expect("the first pair exists");
    bus.start_parallel_pair(pair);
    assert!(!bus.is_busy(Host::Host0) && !bus.is_busy(Host::Host1));
    drop(master);
    assert_eq!(words[HOST_MAP], 0x0a << 4, "the parallel map");
    // Pair zero: TXRF capacitor-low 0x73 on host zero, BBTOP register 2 0x27
    // on host one.
    assert_eq!(words[HOST_0] & 0xff, 0x6b);
    assert_eq!(words[HOST_0] >> 16, (0x73 | ((WRITE | START) >> 16)));
    assert_eq!(
        words[HOST_1],
        0x67 | (0x2 << 8) | (0x27 << 16) | WRITE | START
    );
    let mut master = I2cAnaMst::host(&mut words).expect("memory holds the block");
    AnalogBus::new(&mut master).restore_radio_host_map();
    drop(master);
    assert_eq!(words[HOST_MAP], 0x3fa0 << 4);
}
