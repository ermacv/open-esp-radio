//! Isolated control4 and awake PMU-route observations. No sleep request.
use super::{Bits, LOW_MAX, Session, ble_wave2, crossed, emit, init, mode, pac, quiet, window};

fn int_raw(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::int_raw as f;
    let r = p.read_int_raw();
    (u32::from(r.lp_cpu_exc_int_raw()) * f::LP_CPU_EXC_INT_RAW_MASK)
        | (u32::from(r.sdio_idle_int_raw()) * f::SDIO_IDLE_INT_RAW_MASK)
        | (u32::from(r.sw_int_raw()) * f::SW_INT_RAW_MASK)
        | (u32::from(r.soc_sleep_reject_int_raw()) * f::SOC_SLEEP_REJECT_INT_RAW_MASK)
        | (u32::from(r.soc_wakeup_int_raw()) * f::SOC_WAKEUP_INT_RAW_MASK)
}
fn hp_int_st(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::hp_int_st as f;
    let r = p.read_hp_int_st();
    (u32::from(r.lp_cpu_exc_int_st()) * f::LP_CPU_EXC_INT_ST_MASK)
        | (u32::from(r.sdio_idle_int_st()) * f::SDIO_IDLE_INT_ST_MASK)
        | (u32::from(r.sw_int_st()) * f::SW_INT_ST_MASK)
        | (u32::from(r.soc_sleep_reject_int_st()) * f::SOC_SLEEP_REJECT_INT_ST_MASK)
        | (u32::from(r.soc_wakeup_int_st()) * f::SOC_WAKEUP_INT_ST_MASK)
}
fn hp_int_ena(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::hp_int_ena as f;
    let r = p.read_hp_int_ena();
    (u32::from(r.lp_cpu_exc_int_ena()) * f::LP_CPU_EXC_INT_ENA_MASK)
        | (u32::from(r.sdio_idle_int_ena()) * f::SDIO_IDLE_INT_ENA_MASK)
        | (u32::from(r.sw_int_ena()) * f::SW_INT_ENA_MASK)
        | (u32::from(r.soc_sleep_reject_int_ena()) * f::SOC_SLEEP_REJECT_INT_ENA_MASK)
        | (u32::from(r.soc_wakeup_int_ena()) * f::SOC_WAKEUP_INT_ENA_MASK)
}
fn lp_int_raw(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::lp_int_raw as f;
    let r = p.read_lp_int_raw();
    (u32::from(r.lp_cpu_wakeup_int_raw()) * f::LP_CPU_WAKEUP_INT_RAW_MASK)
        | (u32::from(r.modem_switch_active_end_int_raw()) * f::MODEM_SWITCH_ACTIVE_END_INT_RAW_MASK)
        | (u32::from(r.sleep_switch_active_end_int_raw()) * f::SLEEP_SWITCH_ACTIVE_END_INT_RAW_MASK)
        | (u32::from(r.sleep_switch_modem_end_int_raw()) * f::SLEEP_SWITCH_MODEM_END_INT_RAW_MASK)
        | (u32::from(r.modem_switch_sleep_end_int_raw()) * f::MODEM_SWITCH_SLEEP_END_INT_RAW_MASK)
        | (u32::from(r.active_switch_sleep_end_int_raw()) * f::ACTIVE_SWITCH_SLEEP_END_INT_RAW_MASK)
        | (u32::from(r.modem_switch_active_start_int_raw())
            * f::MODEM_SWITCH_ACTIVE_START_INT_RAW_MASK)
        | (u32::from(r.sleep_switch_active_start_int_raw())
            * f::SLEEP_SWITCH_ACTIVE_START_INT_RAW_MASK)
        | (u32::from(r.sleep_switch_modem_start_int_raw())
            * f::SLEEP_SWITCH_MODEM_START_INT_RAW_MASK)
        | (u32::from(r.modem_switch_sleep_start_int_raw())
            * f::MODEM_SWITCH_SLEEP_START_INT_RAW_MASK)
        | (u32::from(r.active_switch_sleep_start_int_raw())
            * f::ACTIVE_SWITCH_SLEEP_START_INT_RAW_MASK)
        | (u32::from(r.hp_sw_trigger_int_raw()) * f::HP_SW_TRIGGER_INT_RAW_MASK)
}
fn lp_int_st(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::lp_int_st as f;
    let r = p.read_lp_int_st();
    (u32::from(r.lp_cpu_wakeup_int_st()) * f::LP_CPU_WAKEUP_INT_ST_MASK)
        | (u32::from(r.modem_switch_active_end_int_st()) * f::MODEM_SWITCH_ACTIVE_END_INT_ST_MASK)
        | (u32::from(r.sleep_switch_active_end_int_st()) * f::SLEEP_SWITCH_ACTIVE_END_INT_ST_MASK)
        | (u32::from(r.sleep_switch_modem_end_int_st()) * f::SLEEP_SWITCH_MODEM_END_INT_ST_MASK)
        | (u32::from(r.modem_switch_sleep_end_int_st()) * f::MODEM_SWITCH_SLEEP_END_INT_ST_MASK)
        | (u32::from(r.active_switch_sleep_end_int_st()) * f::ACTIVE_SWITCH_SLEEP_END_INT_ST_MASK)
        | (u32::from(r.modem_switch_active_start_int_st())
            * f::MODEM_SWITCH_ACTIVE_START_INT_ST_MASK)
        | (u32::from(r.sleep_switch_active_start_int_st())
            * f::SLEEP_SWITCH_ACTIVE_START_INT_ST_MASK)
        | (u32::from(r.sleep_switch_modem_start_int_st()) * f::SLEEP_SWITCH_MODEM_START_INT_ST_MASK)
        | (u32::from(r.modem_switch_sleep_start_int_st()) * f::MODEM_SWITCH_SLEEP_START_INT_ST_MASK)
        | (u32::from(r.active_switch_sleep_start_int_st())
            * f::ACTIVE_SWITCH_SLEEP_START_INT_ST_MASK)
        | (u32::from(r.hp_sw_trigger_int_st()) * f::HP_SW_TRIGGER_INT_ST_MASK)
}
fn lp_int_ena(p: &pac::pmu::PmuWakeStatus) -> u32 {
    use pac::pmu::lp_int_ena as f;
    let r = p.read_lp_int_ena();
    (u32::from(r.lp_cpu_wakeup_int_ena()) * f::LP_CPU_WAKEUP_INT_ENA_MASK)
        | (u32::from(r.modem_switch_active_end_int_ena()) * f::MODEM_SWITCH_ACTIVE_END_INT_ENA_MASK)
        | (u32::from(r.sleep_switch_active_end_int_ena()) * f::SLEEP_SWITCH_ACTIVE_END_INT_ENA_MASK)
        | (u32::from(r.sleep_switch_modem_end_int_ena()) * f::SLEEP_SWITCH_MODEM_END_INT_ENA_MASK)
        | (u32::from(r.modem_switch_sleep_end_int_ena()) * f::MODEM_SWITCH_SLEEP_END_INT_ENA_MASK)
        | (u32::from(r.active_switch_sleep_end_int_ena()) * f::ACTIVE_SWITCH_SLEEP_END_INT_ENA_MASK)
        | (u32::from(r.modem_switch_active_start_int_ena())
            * f::MODEM_SWITCH_ACTIVE_START_INT_ENA_MASK)
        | (u32::from(r.sleep_switch_active_start_int_ena())
            * f::SLEEP_SWITCH_ACTIVE_START_INT_ENA_MASK)
        | (u32::from(r.sleep_switch_modem_start_int_ena())
            * f::SLEEP_SWITCH_MODEM_START_INT_ENA_MASK)
        | (u32::from(r.modem_switch_sleep_start_int_ena())
            * f::MODEM_SWITCH_SLEEP_START_INT_ENA_MASK)
        | (u32::from(r.active_switch_sleep_start_int_ena())
            * f::ACTIVE_SWITCH_SLEEP_START_INT_ENA_MASK)
        | (u32::from(r.hp_sw_trigger_int_ena()) * f::HP_SW_TRIGGER_INT_ENA_MASK)
}

fn state(s: &Session<'_>, tag: u32) {
    let t = &s.ble.timer;
    let p = &s.wake_status;
    emit(
        tag,
        t.read_word_0048_opaque().counter_low().get(),
        s.ble.timer_status.read_bluetooth_modem_lp_status(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        u32::from(t.read_word_001c_opaque().sleep16_flag_byte()),
        u32::from(t.read_word_0020_opaque().sleep17_flag_byte()),
        u32::from(t.read_word_0024_opaque().low_byte()),
    );
    let m = t.read_word_0058_opaque();
    use pac::modem_pwr::word_0058_opaque as f;
    let readable_mode = u32::from(m.control_0()) * f::CONTROL_0_MASK
        | u32::from(m.control_2()) * f::CONTROL_2_MASK
        | u32::from(m.bit_3_opaque()) * f::BIT_3_OPAQUE_MASK
        | u32::from(m.control_4()) * f::CONTROL_4_MASK
        | (m.bits_5_31_opaque().get() << f::BITS_5_31_OPAQUE_MASK.trailing_zeros());
    emit(
        tag + 2,
        u32::from(t.read_word_002c_opaque().low_byte()),
        readable_mode,
        p.read_slp_wakeup_status0().wakeup_cause(),
    );
    emit(tag + 3, int_raw(p), hp_int_st(p), hp_int_ena(p));
    emit(tag + 4, lp_int_raw(p), lp_int_st(p), lp_int_ena(p));
    emit(
        tag + 5,
        p.read_slp_wakeup_status1().reject_cause(),
        p.read_lpcpu_wakeup_cause().lpcpu_wakeup_cause(),
        p.read_slp_wakeup_cntl2().wakeup_ena(),
    );
    let main = p.read_main_state();
    emit(
        tag + 6,
        main.pmu_main_last_st_state().get(),
        main.pmu_main_tar_st_state().get(),
        main.pmu_main_cur_st_state().get(),
    );
    let r = &s.modem_wake_status;
    let e = r.read_modem_intr_wakeup_conf();
    use pac::modem_lpcon::modem_intr_wakeup_conf as w;
    let enables = (u32::from(e.wifi_mac_intr_wakeup_en()) * w::WIFI_MAC_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.wifi_mac_nmi_wakeup_en()) * w::WIFI_MAC_NMI_WAKEUP_EN_MASK)
        | (u32::from(e.wifi_pwr_intr_wakeup_en()) * w::WIFI_PWR_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.wifi_bb_intr_wakeup_en()) * w::WIFI_BB_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.bt_mac_intr_wakeup_en()) * w::BT_MAC_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.bt_bb_intr_wakeup_en()) * w::BT_BB_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.bt_bb_nmi_wakeup_en()) * w::BT_BB_NMI_WAKEUP_EN_MASK)
        | (u32::from(e.lp_timer_intr_wakeup_en()) * w::LP_TIMER_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.coex_intr_wakeup_en()) * w::COEX_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.ble_timer_intr_wakeup_en()) * w::BLE_TIMER_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.ble_sec_intr_wakeup_en()) * w::BLE_SEC_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.i2c_mst_intr_wakeup_en()) * w::I2C_MST_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.zb_mac_intr_wakeup_en()) * w::ZB_MAC_INTR_WAKEUP_EN_MASK)
        | (u32::from(e.bt_mac_int1_wakeup_en()) * w::BT_MAC_INT1_WAKEUP_EN_MASK);
    emit(
        tag + 7,
        r.read_modem_intr_status().modem_intr_vector().get(),
        enables,
        u32::from(
            s.core_control
                .lp_sys_core_control
                .read_sys_ctrl()
                .lp_core_etm_wakeup_flag(),
        ),
    );
}

#[derive(Clone, Copy)]
enum Stimulus {
    Compare,
    Sleep16,
    Sleep17,
}

pub(crate) fn compare(s: &mut Session<'_>) {
    run(s, Stimulus::Compare);
}
pub(crate) fn sleep16(s: &mut Session<'_>) {
    run(s, Stimulus::Sleep16);
}
pub(crate) fn sleep17(s: &mut Session<'_>) {
    run(s, Stimulus::Sleep17);
}

fn run(s: &mut Session<'_>, stimulus: Stimulus) {
    if matches!(stimulus, Stimulus::Sleep17) {
        ble_wave2::prerequisite(s);
    }
    init(s);
    if matches!(stimulus, Stimulus::Sleep17) {
        s.ble.etm.trigger_channel_enable_set_ch7();
        s.ble.etm.trigger_channel_enable_set_ch6();
        s.ble.etm.trigger_channel_enable_set_bit_11_opaque();
    }
    // One crossing in each control4 state, CONTROL0 held set. This varies a
    // register bit under local stimuli; there is no image A/B or series.
    for image in [1, 3] {
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        s.ble.timer.disable_bluetooth_lp_sleep_17();
        s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
        s.ble.timer.trigger_word_0058_opaque_control_1();
        mode(s, image);
        let tag = 0x17000 + image * 0x100;
        state(s, tag);
        let origin = super::counter(s);
        // No output or host round-trip between target publication and enable.
        let target = origin.wrapping_add(2048) & LOW_MAX;
        match stimulus {
            Stimulus::Compare => s.ble.timer.modify_word_006c_opaque(|w| {
                w.set_compare_target(Bits::truncate(target));
            }),
            Stimulus::Sleep16 => {
                s.ble.timer.modify_word_0064_opaque(|w| {
                    w.set_sleep16_target(Bits::truncate(target));
                });
                s.ble.timer.enable_bluetooth_lp_sleep_16();
            }
            Stimulus::Sleep17 => {
                s.ble.timer.write_word_0068_opaque(|w| {
                    w.set_sleep17_target(Bits::truncate(target));
                });
                s.ble.timer.enable_bluetooth_lp_sleep_17();
            }
        }
        emit(tag + 0xf0, origin, target, image);
        if !crossed(s, origin, 4096, tag + 0xf1) {
            cleanup(s, stimulus);
            return;
        }
        state(s, tag + 0x10);
        // Observe a pending stimulus while changing only CONTROL4.
        mode(s, if image == 1 { 3 } else { 1 });
        window();
        state(s, tag + 0x20);
        s.ble.timer.trigger_word_0058_opaque_control_1();
        state(s, tag + 0x30);
        s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
        window();
        state(s, tag + 0x40);
        // Park compare before the next independent sleep target.
        s.ble.timer.modify_word_006c_opaque(|w| {
            w.set_compare_target(Bits::truncate(LOW_MAX));
        });
    }
    cleanup(s, stimulus);
}
fn cleanup(s: &mut Session<'_>, stimulus: Stimulus) {
    if matches!(stimulus, Stimulus::Sleep17) {
        s.ble.etm.trigger_channel_enable_clear_ch6();
        s.ble.etm.trigger_channel_enable_clear_ch7();
    }
    quiet(s);
}
