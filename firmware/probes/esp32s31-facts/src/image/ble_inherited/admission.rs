//! Digital admission subsequence; no RF, Link Layer or scheduler RUN.
use super::*;
pub(super) fn run(s: &mut Session<'_>) {
    // SOURCE: HAL modem/syscon.rs::reset_bluetooth_controller_domains.
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(pac::modem_syscon::modem_rst_conf::RstBtmac::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(pac::modem_syscon::modem_rst_conf::RstBtmacApb::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(pac::modem_syscon::modem_rst_conf::RstBleTimer::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(pac::modem_syscon::modem_rst_conf::RstModemEcb::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(pac::modem_syscon::modem_rst_conf::RstModemCcm::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(pac::modem_syscon::modem_rst_conf::RstModemBah::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(pac::modem_syscon::modem_rst_conf::RstModemSec::Asserted);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(pac::modem_syscon::modem_rst_conf::RstModemEcb::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(pac::modem_syscon::modem_rst_conf::RstModemCcm::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(pac::modem_syscon::modem_rst_conf::RstModemBah::Released);
    });
    s.sar.radio.modem_syscon_radio.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(pac::modem_syscon::modem_rst_conf::RstModemSec::Released);
    });
    oer_riscv::fence();
    state(s, 0x2130);
    // SOURCE: BLOB_LIBBTDM_HAL_CONTROLLER_INIT, HAL controller/init.rs
    // execute_hal_init(reviewed_standalone): 49 ordered writes, profile
    // (16,11,33,2000), shift3, transformed bytes22/66; upper timer byte kept.
    let registers = &mut s.ble.core;

    registers.modify_scheduler_sram_pointer_prefix(|w| {
        w.set_bits_0_21_opaque(Bits::constant::<0x0>());
        w.set_address_prefix(Bits::constant::<0xbc>());
    });

    // The configuration's input domains give only shifts 2..=5.
    registers.modify_btlc_timer_control(|w| {
        w.set_clock_shift(Bits::constant::<3>());
    });

    registers.modify_hal_init_bytes(|w| {
        w.set_value_0(22);
    });

    registers.modify_hal_init_bytes(|w| {
        w.set_value_1(66);
    });

    // The vendor stores the latch from zero with only CONFIG_6
    // set; the word's bits outside its fields keep their value.
    registers.modify_hal_init_latch(|w| {
        w.set_config_6(true);
        w.set_enable_31(false);
    });

    registers.write_hal_init_low_20(|w| {
        w.set_config_low_20(Bits::constant::<0x000f_ffff>());
    });

    registers.modify_hal_init_latch(|w| {
        w.set_enable_31(true);
    });

    registers.modify_wdt_config_1(|w| {
        w.set_config_15(true);
        w.set_config_18_19(Bits::constant::<0x3>());
    });

    registers.modify_wdt_config_1(|w| {
        w.set_config_3(true);
        w.set_config_6_7(Bits::constant::<0x3>());
    });

    registers.modify_wdt_config_0(|w| {
        w.set_enable(true);
    });

    registers.modify_hal_init_scheduler_control(|w| {
        w.set_config_16_20(Bits::constant::<0x0>());
    });

    registers.modify_hal_init_scheduler_control(|w| {
        w.set_config_16_20(Bits::constant::<0x10>());
    });

    registers.modify_hal_init_scheduler_control(|w| {
        w.set_enable_31(true);
    });

    registers.modify_hal_init_low_half(|w| {
        w.set_config_low_16(0x0);
    });

    registers.modify_hal_init_low_half(|w| {
        w.set_config_low_16(0xffff);
    });

    registers.modify_hal_init_scheduler_control(|w| {
        w.set_config_byte_1(0x0);
    });

    registers.modify_hal_init_scheduler_control(|w| {
        w.set_config_byte_1(0x20);
    });

    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_0_index_high(false).set_bit_3_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_0_enable(true);
        w.set_lane_0_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_1_index_high(false).set_bit_7_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_1_enable(true);
        w.set_lane_1_retained_index_low(true);
        w.set_lane_1_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_2_index_high(false).set_bit_11_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_2_enable(true);
        w.set_lane_2_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_3_index_high(false).set_bit_15_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_3_enable(true);
        w.set_lane_3_retained_index_low(true);
        w.set_lane_3_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_4_index_high(false).set_bit_19_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_4_enable(true);
        w.set_lane_4_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_5_index_high(false).set_bit_23_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_5_enable(true);
        w.set_lane_5_retained_index_low(true);
        w.set_lane_5_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_6_index_high(false).set_bit_27_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_6_enable(true);
        w.set_lane_6_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_7_index_high(false).set_bit_31_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<0>(), |w| {
        w.set_lane_7_enable(true);
        w.set_lane_7_retained_index_low(true);
        w.set_lane_7_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_0_index_high(false).set_bit_3_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_0_enable(true);
        w.set_lane_0_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_1_index_high(false).set_bit_7_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_1_enable(true);
        w.set_lane_1_retained_index_low(true);
        w.set_lane_1_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_2_index_high(false).set_bit_11_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_2_enable(true);
        w.set_lane_2_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_3_index_high(false).set_bit_15_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_3_enable(true);
        w.set_lane_3_retained_index_low(true);
        w.set_lane_3_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_4_index_high(false).set_bit_19_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_4_enable(true);
        w.set_lane_4_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_5_index_high(false).set_bit_23_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_5_enable(true);
        w.set_lane_5_retained_index_low(true);
        w.set_lane_5_index_high(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_6_index_high(false).set_bit_27_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_6_enable(true);
        w.set_lane_6_index_high(true);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_7_index_high(false).set_bit_31_opaque(false);
    });
    registers.modify_hal_init_slot_map(Index::constant::<1>(), |w| {
        w.set_lane_7_enable(true);
        w.set_lane_7_retained_index_low(true);
        w.set_lane_7_index_high(true);
    });
    oer_riscv::fence();
    // SOURCE: HAL scheduler.rs::clear_scheduler_hardware_list_heads.
    for i in (0..pac::btdm::entry::COUNT).filter_map(|i| Index::new(i).ok()) {
        s.ble.scheduler.modify_entry(i, |w| {
            w.clear_head_pointer();
        });
    }
    oer_riscv::fence();
    // SOURCE: HAL modem_timer.rs::initialize_low_power_hardware.
    let etm = &mut s.ble.etm;
    etm.trigger_channel_enable_clear_ch5();
    etm.modify_ch5_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent43.bits());
    });
    etm.modify_ch5_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask12.bits());
    });
    etm.trigger_channel_enable_clear_ch4();
    etm.modify_ch4_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent45.bits());
    });
    etm.modify_ch4_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask11.bits());
    });
    etm.trigger_channel_enable_clear_ch6();
    etm.modify_ch6_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
    });
    etm.modify_ch6_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask11.bits());
    });
    etm.trigger_channel_enable_clear_ch7();
    etm.modify_ch7_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
    });
    etm.modify_ch7_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask0f.bits());
    });
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(false).set_control_4(false);
    });
    if s.ble.timer.read_word_0058_opaque().control_2() {
        s.ble.timer.trigger_word_0058_opaque_control_1();
    }
    oer_riscv::fence();
    state(s, 0x2140);
    // SOURCE: HAL interrupt.rs::execute_primary_prepare. This is the exact
    // register subsequence, not a reproduction of the actor/RF prerequisites.
    s.ble.bank.clear_bluetooth_primary_baseline_bank_0();
    s.ble.bank.clear_bluetooth_primary_baseline_bank_1();
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_15(true);
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_controller_time_overflow(true)
            .set_source_8(true)
            .set_source_9(true)
            .set_source_12(true);
    });
    mac::command(s, 0);
}
