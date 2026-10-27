//! Golden memory images of the PHY register operations.
//!
//! Each case runs one operation on host memory filled with one value and
//! compares the whole block against the fill with the listed words, given
//! as byte offsets from the block's base and their stored values, replaced.
//! The images were recorded from the PAC transactions these operations
//! replace and match them for every fill tried; they are written out here
//! so a field accessor that changes cannot pass by reading back its own
//! error. The access order inside one operation is not visible in host
//! memory: each operation keeps the vendor order by construction.

use crate::modem::values::{CfrValue, ForcedRxGain};
use crate::phy::registers::{AgcRegisters, BasebandRegisters, ClockRegisters, FrequencyRegisters};
use oer_esp32s31_pac::{
    fe::{PhyIqEstimatorOracle, PhyMemory, PhyPbus},
    i2c_ana_mst::I2cAnaMst,
    i2c_ana_mst_mem::PhyI2cCommandRam,
    instances,
    lp_aonclkrst::LpAonClkrst,
    lp_peri_clkrst::LpPericlkrst,
    tsens::LpTsens,
};
use std::{vec, vec::Vec};

use crate::phy::{
    agc::ops as agc, baseband::ops as bb, clock::ops as clk, frequency::ops as fr, i2c::master,
    iq_estimator::ops as iq, memory::ops as mm, pbus::ops as pb, power_detector::platform as pd,
    temperature::ops as ts,
};

#[test]
fn estimator_indications_keep_the_pair_even_when_ready() {
    use oer_esp32s31_pac::fe;
    for ready in [false, true] {
        for pair in 0..4 {
            let mut memory = vec![0; instances::fe::SIZE.div_ceil(4)];
            memory[fe::estimator_ready_status::OFFSET / 4] = if ready {
                fe::estimator_ready_status::READY_MASK
            } else {
                0
            };
            memory[fe::word_08d0_opaque::OFFSET / 4] =
                pair << fe::word_08d0_opaque::BITS_20_21_OPAQUE_MASK.trailing_zeros();
            let estimator = PhyIqEstimatorOracle::host(&mut memory).expect("host block");
            let result = iq::sample_indications(&estimator);
            assert_eq!(result.ready, ready);
            assert_eq!(u32::from(result.opaque_pair), pair);
        }
    }
}

#[test]
fn estimator_restoration_returns_saved_fields_and_enable_state() {
    let mut memory = vec![0; instances::fe::SIZE.div_ceil(4)];
    let mut estimator = PhyIqEstimatorOracle::host(&mut memory).expect("host block");
    iq::configure(&mut estimator, 123);
    let saved = iq::capture_configuration(&estimator);
    assert!(!saved.enabled());
    iq::configure(&mut estimator, 1000);
    iq::set_start_enabled(&mut estimator, true);
    iq::set_measurement_enabled(&mut estimator, true);
    iq::restore_configuration(&mut estimator, saved);
    assert_eq!(
        estimator.read_estimator_control().bits_2_16_opaque().get(),
        123
    );
    assert!(!estimator.read_estimator_control().start_enable());
    assert!(!estimator.read_estimator_control().measurement_enable());
}

#[test]
fn frequency_read_restore_preserves_other_owner_changes_and_never_pulses_writers() {
    let mut memory = vec![0; 7994];
    let mut frequency = FrequencyRegisters::host(&mut memory).expect("host block");
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_switch_pulse(true);
    });
    assert!(fr::capture_read_configuration(&frequency).is_none());
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_switch_pulse(false);
    });
    let saved = fr::capture_read_configuration(&frequency).expect("quiet engine");
    fr::set_hardware_control(&mut frequency, false);
    fr::read_memory(&mut frequency, 0x400, 2);
    frequency.fe.modify_frequency_control(|w| {
        w.set_channel_index(17);
    });
    fr::restore_read_configuration(&mut frequency, saved);
    let control = frequency.fe.read_frequency_control();
    assert_eq!(control.channel_index(), 17);
    assert_eq!(control.bits_8_17_opaque().get(), 0);
    assert!(!control.bit_18_opaque());
    assert!(!control.hardware_frequency_disable());
    assert!(!control.memory_write_pulse());
    assert!(!control.channel_switch_pulse());
    assert_eq!(
        frequency
            .fe
            .read_i2c_number_control()
            .memory_read_mode()
            .get(),
        0
    );
    assert!(fr::observe(&frequency).is_some());
}

/// Run `operation` over a block of `words` words filled with `fill` and
/// compare the whole memory with the fill and `changed` words replaced.
fn check(words: usize, fill: u32, changed: &[(usize, u32)], operation: impl FnOnce(&mut [u32])) {
    let mut memory: Vec<u32> = vec![fill; words];
    operation(&mut memory);
    let mut expected: Vec<u32> = vec![fill; words];
    for &(offset, value) in changed {
        expected[offset / 4] = value;
    }
    for (index, (got, want)) in memory.iter().zip(&expected).enumerate() {
        assert_eq!(
            got,
            want,
            "word {:#06x} from fill {fill:#010x}: {got:#010x}, expected {want:#010x}",
            index * 4
        );
    }
}

macro_rules! golden {
    ($name:ident, $block:ident, $words:expr, |$b:ident| $operation:block, [$(($fill:expr, $changed:expr)),* $(,)?] $(,)?) => {
        #[test]
        fn $name() {
            $(
                check($words, $fill, $changed, |memory| {
                    let mut $b = $block::host(memory).expect("memory holds the block");
                    $operation
                });
            )*
        }
    };
}

golden!(
    agc_update_baseband_registers,
    AgcRegisters,
    8223,
    |b| {
        agc::update_baseband_registers(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x7044, 0xfe3fe1fe),
                (0x7048, 0xff7da4f3),
                (0x7104, 0x06acc7c8),
                (0x7124, 0xb2208553),
                (0x78a4, 0x0001721f),
                (0x8010, 0x000852a1),
                (0x8018, 0x00600030),
                (0x801c, 0x010000a0),
                (0x8020, 0x00000180),
                (0x8028, 0xc0403020),
                (0x802c, 0x01000080),
                (0x8070, 0x000008c7),
                (0x8078, 0x00700000)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x7044, 0xfe3fe1fe),
                (0x7048, 0xff7da4f3),
                (0x7104, 0x06acc7c8),
                (0x7124, 0xb2208553),
                (0x78a4, 0x0001721f),
                (0x8004, 0xfbffffff),
                (0x8010, 0x000852a1),
                (0x8018, 0x00600030),
                (0x801c, 0x010000a0),
                (0x8020, 0x00000180),
                (0x8028, 0xc0403020),
                (0x802c, 0x01000080),
                (0x8070, 0x000008c7)
            ]
        ),
    ]
);

golden!(
    agc_enable,
    AgcRegisters,
    8223,
    |b| {
        agc::set_enabled(&mut b, true);
    },
    [
        (0x00000000, &[]),
        (0xffffffff, &[(0x702c, 0xff7fffff), (0x7030, 0xdfffffff)]),
    ]
);

golden!(
    agc_initialize_registers,
    AgcRegisters,
    8223,
    |b| {
        agc::initialize_registers(&mut b, 0x40, 0x12);
    },
    [
        (
            0x00000000,
            &[
                (0x08bc, 0x00062400),
                (0x702c, 0x32004000),
                (0x705c, 0x00000bb8),
                (0x7094, 0x000000fc),
                (0x7128, 0xd2000000),
                (0x713c, 0x00fc0000)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x08bc, 0xfff6240f),
                (0x702c, 0x327fc0ff),
                (0x705c, 0xfff80bb8),
                (0x7094, 0xfffffeff),
                (0x7128, 0xd2ffffff),
                (0x713c, 0xfeffffff)
            ]
        ),
    ]
);

golden!(
    agc_rx_gain_limits_clamped,
    AgcRegisters,
    8223,
    |b| {
        agc::configure_rx_gain_limits(&mut b, 0x60);
    },
    [
        (0x00000000, &[(0x702c, 0x00006000), (0x713c, 0x01300000)]),
        (0xffffffff, &[(0x702c, 0xffffe0ff), (0x713c, 0xff33ffff)]),
    ]
);

golden!(
    agc_rf_rx_saturation,
    AgcRegisters,
    8223,
    |b| {
        agc::configure_rf_rx_saturation(&mut b, true);
    },
    [
        (0x00000000, &[(0x705c, 0xd1080800), (0x7068, 0x00000404)]),
        (0xffffffff, &[(0x705c, 0xfff80800), (0x7068, 0x00000404)]),
    ]
);

golden!(
    agc_post_initialization,
    AgcRegisters,
    8223,
    |b| {
        agc::update_post_initialization(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x705c, 0x04000000),
                (0x7064, 0x0818212d),
                (0x7104, 0x000001c0),
                (0x7114, 0x0818212d),
                (0x78c8, 0x00000b97)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x7064, 0x0818212d),
                (0x7104, 0xffffffc0),
                (0x7114, 0x0818212d),
                (0x78c8, 0xffffcb97)
            ]
        ),
    ]
);

golden!(
    agc_rx_11b_optimization,
    AgcRegisters,
    8223,
    |b| {
        agc::configure_rx_11b_optimization(&mut b, true);
    },
    [
        (
            0x00000000,
            &[
                (0x7044, 0x003f2100),
                (0x7104, 0x000001c8),
                (0x7124, 0x00008403),
                (0x8004, 0x00009000)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x7044, 0xffffe1ff),
                (0x7104, 0xffffffc8),
                (0x7124, 0xffff87f3),
                (0x8004, 0xffff9fff)
            ]
        ),
    ]
);

golden!(
    agc_forced_rx_gain,
    AgcRegisters,
    8223,
    |b| {
        agc::configure_forced_rx_gain(&mut b, true, ForcedRxGain::new(0x5a).unwrap());
    },
    [
        (0x00000000, &[(0x702c, 0x5a800000)]),
        (0xffffffff, &[(0x702c, 0x5affffff)]),
    ]
);

golden!(
    bb_tx_power_tracking,
    BasebandRegisters,
    7989,
    |b| {
        bb::configure_tx_power_tracking(&mut b, true);
    },
    [
        (
            0x00000000,
            &[
                (0x7454, 0x000003e1),
                (0x7458, 0x5f697300),
                (0x745c, 0x8d96a0b1),
                (0x7460, 0x00007983)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x7454, 0xffffffe1),
                (0x7458, 0xdf69737c),
                (0x745c, 0x8d96a0b1),
                (0x7460, 0xffff7983)
            ]
        ),
    ]
);

golden!(
    bb_initialize_baseband,
    BasebandRegisters,
    7989,
    |b| {
        bb::initialize_baseband_prefix(&mut b);
        bb::initialize_baseband_middle(&mut b);
        bb::initialize_baseband_tail(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x7400, 0x00006000),
                (0x7428, 0x00005500),
                (0x7808, 0x00003000),
                (0x7890, 0x01000000),
                (0x78dc, 0x00000100),
                (0x7c00, 0x00000200),
                (0x7ca8, 0x00100000),
                (0x7cd0, 0x000f000f)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x7428, 0xffffd5ff),
                (0x743c, 0xfffffe3f),
                (0x7808, 0xfffff07f),
                (0x7890, 0xfdffffff),
                (0x78dc, 0xffffc17f),
                (0x78e4, 0xffbfffff),
                (0x790c, 0xfffff7ff),
                (0x7980, 0xfdffffff),
                (0x7a28, 0xffbfffff),
                (0x7c30, 0xfff00fff)
            ]
        ),
    ]
);

golden!(
    bb_tx_pa_on,
    BasebandRegisters,
    7989,
    |b| {
        bb::configure_tx_pa_on(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x086c, 0x00007800),
                (0x0870, 0x0a0ec800),
                (0x7c00, 0x0000a000),
                (0x7c30, 0x0000001e),
                (0x7c6c, 0x0661a45f)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x086c, 0xffff78ff),
                (0x0870, 0x0a0ec8ff),
                (0x7c00, 0xffe0a7ff),
                (0x7c30, 0xfffffc1e),
                (0x7c6c, 0x0661a45f)
            ]
        ),
    ]
);

golden!(
    bb_power_detector_initialization,
    BasebandRegisters,
    7989,
    |b| {
        bb::initialize_power_detector_registers(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x0808, 0x00200500),
                (0x0810, 0x0f0f0fff),
                (0x0814, 0x00ff0f64),
                (0x0818, 0x0000aaaa)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0808, 0xffaff50f),
                (0x0810, 0x0f0f0fff),
                (0x0814, 0x00ff0f64),
                (0x0818, 0x0000aaaa)
            ]
        ),
    ]
);

golden!(
    bb_calibration_tone_stop,
    BasebandRegisters,
    7989,
    |b| {
        bb::configure_calibration_tone(&mut b, false, 0x155, 3);
    },
    [
        (
            0x00000000,
            &[
                (0x040c, 0x00000003),
                (0x0410, 0x0003fb00),
                (0x041c, 0x0003f455),
                (0x0428, 0x00000001)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0410, 0x0003fb00),
                (0x0414, 0x00000000),
                (0x041c, 0xf003f455),
                (0x0420, 0xf0000000),
                (0x0428, 0xfffffff1)
            ]
        ),
    ]
);

golden!(
    bb_txiq_mismatch_first,
    BasebandRegisters,
    7989,
    |b| {
        bb::configure_txiq_mismatch_power(&mut b, true, true, 7, 0x2a6);
    },
    [
        (0x00000000, &[(0x041c, 0x042fe4a9), (0x0428, 0x00000002)]),
        (0xffffffff, &[(0x041c, 0xf42fe4a9), (0x0428, 0xfffffffe)]),
    ]
);

golden!(
    bb_tx_iq_coefficients,
    BasebandRegisters,
    7989,
    |b| {
        bb::set_tx_iq_gain_coefficient(&mut b, -40);
        bb::set_tx_iq_phase_coefficient(&mut b, 100);
    },
    [
        (0x00000000, &[(0x0c0c, 0x00000fe1)]),
        (0xffffffff, &[(0x0c0c, 0xffffefe1)]),
    ]
);

golden!(
    bb_front_end_suffix,
    BasebandRegisters,
    7989,
    |b| {
        bb::initialize_front_end_suffix(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x040c, 0x00000004),
                (0x0438, 0xe0000000),
                (0x0448, 0x00000003),
                (0x086c, 0x00000004),
                (0x0c0c, 0x0000e000),
                (0x0c20, 0x00000057)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0444, 0xfffff7ff),
                (0x086c, 0xffffff04),
                (0x0c20, 0xffffff57)
            ]
        ),
    ]
);

golden!(
    bb_forced_iccfr,
    BasebandRegisters,
    7989,
    |b| {
        bb::configure_forced_iccfr(&mut b, true, true, CfrValue::new(0xabc).unwrap());
    },
    [
        (0x00000000, &[(0x7478, 0x0000f579)]),
        (0xffffffff, &[(0x7478, 0xfffff579), (0x747c, 0xf9ffffff)]),
    ]
);

golden!(
    clock_open_frontend_baseband,
    ClockRegisters,
    7969,
    |b| {
        clk::open_frontend_baseband_internal_clocks(&mut b);
    },
    [
        (
            0x00000000,
            &[
                (0x0400, 0x000001e7),
                (0x0800, 0x00000003),
                (0x7c80, 0xffffffff)
            ]
        ),
        (0xffffffff, &[(0x0400, 0x000001e7)]),
    ]
);

golden!(
    clock_forced_digital_gain,
    ClockRegisters,
    7969,
    |b| {
        bb::configure_forced_digital_gain(&mut b, true, -2, 5);
    },
    [
        (0x00000000, &[(0x0408, 0x000105fe)]),
        (0xffffffff, &[(0x0408, 0xffff05fe)]),
    ]
);

golden!(
    frequency_initialize_registers,
    FrequencyRegisters,
    7994,
    |b| {
        fr::initialize_registers(&mut b, true);
    },
    [
        (
            0x00000000,
            &[
                (0x001c, 0x48000000),
                (0x0024, 0x19800249),
                (0x0028, 0x25824e58)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x001c, 0x4837ffff),
                (0x0024, 0x19800249),
                (0x0028, 0x25824e58)
            ]
        ),
    ]
);

golden!(
    frequency_write_memory,
    FrequencyRegisters,
    7994,
    |b| {
        fr::write_memory(&mut b, 0x5a5, 0xab_cdef, 0x12);
    },
    [
        (0x00000000, &[(0x001c, 0x0005a500), (0x002c, 0x12abcdef)]),
        (0xffffffff, &[(0x001c, 0xffeda5ff), (0x002c, 0x12abcdef)]),
    ]
);

golden!(
    frequency_i2c_number_addresses,
    FrequencyRegisters,
    7994,
    |b| {
        fr::configure_i2c_number_addresses(
            &mut b,
            fr::PhyFrequencyI2cNumberAddresses::new([0x15, 0x0a, 1, 2, 3, 4, 5, 6, 7, 8, 0x1f])
                .unwrap(),
        );
    },
    [
        (
            0x00000000,
            &[
                (0x0030, 0x00015500),
                (0x0034, 0x0c520c41),
                (0x0038, 0x00007d07)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0030, 0xfffd55ff),
                (0x0034, 0x0c520c41),
                (0x0038, 0x00007d07),
                (0x003c, 0x00000000)
            ]
        ),
    ]
);

golden!(
    frequency_channel_cbw,
    FrequencyRegisters,
    7994,
    |b| {
        fr::configure_channel_cbw(&mut b, 0x53);
    },
    [
        (
            0x00000000,
            &[
                (0x4400, 0x00000003),
                (0x7ce0, 0x00000003),
                (0x7ce4, 0x00000010)
            ]
        ),
        (0xffffffff, &[(0x4400, 0xfffffff3), (0x7ce4, 0xfffffff0)]),
    ]
);

golden!(
    frequency_nrx,
    FrequencyRegisters,
    7994,
    |b| {
        fr::configure_nrx_frequency(&mut b, 2412);
    },
    [(0x00000000, &[]), (0xffffffff, &[(0x7848, 0xff000000)]),]
);

golden!(
    memory_pbus_entry,
    PhyMemory,
    instances::fe::SIZE / 4,
    |b| {
        mm::program_pbus_memory_entry(
            &mut b,
            Some(mm::PbusMemoryGroupBoundary {
                group: 7,
                first_entry: 0x21,
                last_entry: 0x3c,
            }),
            0x1234_5678,
            0x2a5,
        )
        .unwrap();
    },
    [
        (
            0x00000000,
            &[
                (0x0844, 0x00152800),
                (0x0848, 0x12345678),
                (0x0860, 0x3c210000)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0844, 0xfff52fff),
                (0x0848, 0x12345678),
                (0x0860, 0x3c21ffff)
            ]
        ),
    ]
);

golden!(
    memory_gain_entry_transmit,
    PhyMemory,
    instances::fe::SIZE / 4,
    |b| {
        mm::program_gain_memory_entry(
            &mut b,
            mm::PhyGainMemoryEntry::transmit(
                0x33,
                0x1ab,
                0x2cd,
                0x7e,
                [0x101, 0x203, 0x305, 0x407],
                0x1abc,
            ),
        );
    },
    [
        (
            0x00000000,
            &[
                (0x0844, 0x00099800),
                (0x0848, 0xc1c0fabc),
                (0x084c, 0x90d70101),
                (0x0850, 0x003f7f95)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0844, 0xfff99800),
                (0x0848, 0xc1c0fabc),
                (0x084c, 0x90d70101),
                (0x0850, 0x003f7f95)
            ]
        ),
    ]
);

golden!(
    memory_gain_entry_receive,
    PhyMemory,
    instances::fe::SIZE / 4,
    |b| {
        mm::program_gain_memory_entry(
            &mut b,
            mm::PhyGainMemoryEntry::generated_receive(
                0x44,
                [0x1f3, 0x0e1],
                [0x55, 0x66],
                0x1234,
                0x0001_f8a7,
                0x5,
                0xc4,
            ),
        );
    },
    [
        (
            0x00000000,
            &[
                (0x0844, 0x000a2000),
                (0x0848, 0x999c3234),
                (0x084c, 0xa0ae55f9),
                (0x0850, 0x0000007f)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0844, 0xfffa2000),
                (0x0848, 0x999c3234),
                (0x084c, 0xa0ae55f9),
                (0x0850, 0x0000007f)
            ]
        ),
    ]
);

golden!(
    memory_tx_cfr_entry,
    PhyMemory,
    instances::fe::SIZE / 4,
    |b| {
        mm::program_tx_cfr_entry(&mut b, 0xdead_beef, 0x9c);
    },
    [
        (0x00000000, &[(0x0844, 0x0004e000), (0x0848, 0xdeadbeef)]),
        (0xffffffff, &[(0x0844, 0xffdce7ff), (0x0848, 0xdeadbeef)]),
    ]
);

golden!(
    pbus_force_test,
    PhyPbus,
    instances::fe::SIZE / 4,
    |b| {
        pb::publish_force_test(&mut b, 0x13, 1, 0x3a5);
    },
    [
        (0x00000000, &[(0x0884, 0x0000e94e)]),
        (0xffffffff, &[(0x0884, 0xfffee94f)]),
    ]
);

golden!(
    pbus_clock_pairs,
    PhyPbus,
    instances::fe::SIZE / 4,
    |b| {
        pb::set_rx_clock_pair(&mut b, true);
        pb::set_tx_clock_pair(&mut b, true);
        pb::set_force_txrx_mode(&mut b, true, false);
    },
    [
        (0x00000000, &[(0x0890, 0x0003ca00)]),
        (0xffffffff, &[(0x0890, 0xfffffaff)]),
    ]
);

golden!(
    iq_estimator_configure,
    PhyIqEstimatorOracle,
    instances::fe::SIZE / 4,
    |b| {
        iq::configure(&mut b, 0xa5a5);
    },
    [
        (0x00000000, &[(0x044c, 0x04000000), (0x0450, 0x00109694)]),
        (0xffffffff, &[(0x044c, 0xf7ffffff), (0x0450, 0xfff69697)]),
    ]
);

golden!(
    tsens_cold_readout,
    LpTsens,
    instances::lp_tsens::SIZE / 4,
    |b| {
        ts::enable_register_bank(&mut b);
        ts::enable_phy_readout(&mut b);
        ts::enable_phy_conversion(&mut b);
        ts::set_power(&mut b, true);
    },
    [
        (0x00000000, &[(0x0000, 0x00400000), (0x0018, 0x00800201)]),
        (0xffffffff, &[]),
    ]
);

golden!(
    tsens_clock_reset,
    LpPericlkrst,
    instances::lp_periclkrst::SIZE / 4,
    |b| {
        ts::set_clock(&mut b, true);
        ts::set_reset(&mut b, false);
    },
    [
        (0x00000000, &[(0x0030, 0x40000000)]),
        (0xffffffff, &[(0x0030, 0x7fffffff)]),
    ]
);

golden!(
    power_detector_circuit_calibration,
    LpAonClkrst,
    instances::lp_aon_clkrst::SIZE / 4,
    |b| {
        pd::select_circuit_calibration_mode(&mut b);
    },
    [
        (0x00000000, &[(0x0068, 0x00000002)]),
        (0xffffffff, &[(0x0068, 0xfffffffa)]),
    ]
);

golden!(
    i2c_clock_selection_and_registers,
    I2cAnaMst,
    instances::i2c_ana_mst::SIZE / 4,
    |b| {
        master::configure_clock_selection(&mut b);
        master::configure_master_registers(&mut b);
        master::set_bbpll_calibration(&mut b, true);
    },
    [
        (
            0x00000000,
            &[
                (0x0018, 0x00000448),
                (0x0024, 0x00000084),
                (0x0028, 0x00000084),
                (0x002c, 0x00000084)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0018, 0xfffffdfb),
                (0x0024, 0xfffff884),
                (0x0028, 0xfffff884),
                (0x002c, 0xfffff884)
            ]
        ),
    ]
);

golden!(
    i2c_command_memory,
    PhyI2cCommandRam,
    instances::i2c_ana_mst_mem::SIZE / 4,
    |b| {
        master::configure_command_memory(
            &mut b,
            crate::phy::i2c::PhyI2cCommandMemoryInputs::new(0x11, 0x22, 0x33, 0x3e, 0x55, 0x66),
        );
    },
    [
        (
            0x00000000,
            &[
                (0x0000, 0x00070267),
                (0x0004, 0x0001016b),
                (0x0008, 0x0073026b),
                (0x000c, 0x00ba036b),
                (0x0010, 0x0088046b),
                (0x0014, 0x0001056b),
                (0x0018, 0x0011066b),
                (0x001c, 0x00fd076b),
                (0x0020, 0x00bf086b),
                (0x0024, 0x0002096b),
                (0x0028, 0x00080a6b),
                (0x002c, 0x00040b6b),
                (0x0030, 0x00a70c6b),
                (0x0034, 0x00770d6b),
                (0x0038, 0x00f40e6b),
                (0x003c, 0x00810f6b),
                (0x0040, 0x00680062),
                (0x0044, 0x00a80462),
                (0x0048, 0x00440b62),
                (0x004c, 0x000a0d62),
                (0x0050, 0x00110f62),
                (0x0054, 0x00081562),
                (0x0058, 0x00700266),
                (0x005c, 0x00270267),
                (0x0060, 0x00220467),
                (0x0064, 0x00220567),
                (0x0068, 0x00330667),
                (0x006c, 0x00330767),
                (0x0070, 0x00220c67),
                (0x0074, 0x00220d67),
                (0x0078, 0x00330e67),
                (0x007c, 0x00330f67),
                (0x0080, 0x003c1467),
                (0x0084, 0x003c1567),
                (0x0088, 0x003c1667),
                (0x008c, 0x003e1767),
                (0x0090, 0x00571867),
                (0x0094, 0x00571967),
                (0x0098, 0x00661c67),
                (0x009c, 0x00661d67),
                (0x00a0, 0x00661e67),
                (0x00a4, 0x00661f67),
                (0x00a8, 0x00000663),
                (0x00ac, 0x00af006a),
                (0x00b0, 0x007f016a)
            ]
        ),
        (
            0xffffffff,
            &[
                (0x0000, 0x00070267),
                (0x0004, 0x0001016b),
                (0x0008, 0x0073026b),
                (0x000c, 0x00ba036b),
                (0x0010, 0x0088046b),
                (0x0014, 0x0001056b),
                (0x0018, 0x0011066b),
                (0x001c, 0x00fd076b),
                (0x0020, 0x00bf086b),
                (0x0024, 0x0002096b),
                (0x0028, 0x00080a6b),
                (0x002c, 0x00040b6b),
                (0x0030, 0x00a70c6b),
                (0x0034, 0x00770d6b),
                (0x0038, 0x00f40e6b),
                (0x003c, 0x00810f6b),
                (0x0040, 0x00680062),
                (0x0044, 0x00a80462),
                (0x0048, 0x00440b62),
                (0x004c, 0x000a0d62),
                (0x0050, 0x00110f62),
                (0x0054, 0x00081562),
                (0x0058, 0x00700266),
                (0x005c, 0x00270267),
                (0x0060, 0x00220467),
                (0x0064, 0x00220567),
                (0x0068, 0x00330667),
                (0x006c, 0x00330767),
                (0x0070, 0x00220c67),
                (0x0074, 0x00220d67),
                (0x0078, 0x00330e67),
                (0x007c, 0x00330f67),
                (0x0080, 0x003c1467),
                (0x0084, 0x003c1567),
                (0x0088, 0x003c1667),
                (0x008c, 0x003e1767),
                (0x0090, 0x00571867),
                (0x0094, 0x00571967),
                (0x0098, 0x00661c67),
                (0x009c, 0x00661d67),
                (0x00a0, 0x00661e67),
                (0x00a4, 0x00661f67),
                (0x00a8, 0x00000663),
                (0x00ac, 0x00af006a),
                (0x00b0, 0x007f016a)
            ]
        ),
    ]
);
