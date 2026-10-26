//! Local LP delivery, sleep commands and PMU observations; no RF or DMA.
use super::{Bits, Session, ble, ble_wave2, emit, pac};

const LOW_MAX: u32 = pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
const CAUSE_LIMIT: u32 = 200_000;
const WRAP_LIMIT: u32 = 40_000_000;

fn counter(s: &Session<'_>) -> u32 {
    s.ble.timer.read_word_0048_opaque().counter_low().get()
}
fn window() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}
fn state(s: &Session<'_>, tag: u32) {
    let t = &s.ble.timer;
    emit(
        tag,
        counter(s),
        t.read_word_0038_opaque().bits_0_31_opaque(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        u32::from(t.read_word_001c_opaque().bits_0_7_opaque()),
        u32::from(t.read_word_0020_opaque().bits_0_7_opaque()),
        u32::from(t.read_word_0024_opaque().low_byte()),
    );
    let m = t.read_word_0058_opaque();
    emit(
        tag + 2,
        u32::from(t.read_word_002c_opaque().low_byte()),
        u32::from(m.control_0())
            + 2 * u32::from(m.control_1())
            + 4 * u32::from(m.control_2())
            + 16 * u32::from(m.control_4()),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    // Sampling makes no acknowledgement. PMU enables and wake causes are
    // observed, never changed to manufacture a sleep/wake transition.
    emit(
        tag + 3,
        hp_raw(&s.ble.pmu),
        lp_raw(&s.ble.pmu),
        s.ble.pmu.read_slp_wakeup_status0().wakeup_cause(),
    );
}
fn hp_raw(p: &pac::pmu::PmuRadio) -> u32 {
    use pac::pmu::int_raw as f;
    let r = p.read_int_raw();
    u32::from(r.lp_cpu_exc_int_raw()) * f::LP_CPU_EXC_INT_RAW_MASK
        | u32::from(r.sdio_idle_int_raw()) * f::SDIO_IDLE_INT_RAW_MASK
        | u32::from(r.sw_int_raw()) * f::SW_INT_RAW_MASK
        | u32::from(r.soc_sleep_reject_int_raw()) * f::SOC_SLEEP_REJECT_INT_RAW_MASK
        | u32::from(r.soc_wakeup_int_raw()) * f::SOC_WAKEUP_INT_RAW_MASK
}
fn lp_raw(p: &pac::pmu::PmuRadio) -> u32 {
    use pac::pmu::lp_int_raw as f;
    let r = p.read_lp_int_raw();
    u32::from(r.lp_cpu_wakeup_int_raw()) * f::LP_CPU_WAKEUP_INT_RAW_MASK
        | u32::from(r.modem_switch_active_end_int_raw()) * f::MODEM_SWITCH_ACTIVE_END_INT_RAW_MASK
        | u32::from(r.sleep_switch_active_end_int_raw()) * f::SLEEP_SWITCH_ACTIVE_END_INT_RAW_MASK
        | u32::from(r.sleep_switch_modem_end_int_raw()) * f::SLEEP_SWITCH_MODEM_END_INT_RAW_MASK
        | u32::from(r.modem_switch_sleep_end_int_raw()) * f::MODEM_SWITCH_SLEEP_END_INT_RAW_MASK
        | u32::from(r.active_switch_sleep_end_int_raw()) * f::ACTIVE_SWITCH_SLEEP_END_INT_RAW_MASK
        | u32::from(r.modem_switch_active_start_int_raw())
            * f::MODEM_SWITCH_ACTIVE_START_INT_RAW_MASK
        | u32::from(r.sleep_switch_active_start_int_raw())
            * f::SLEEP_SWITCH_ACTIVE_START_INT_RAW_MASK
        | u32::from(r.sleep_switch_modem_start_int_raw()) * f::SLEEP_SWITCH_MODEM_START_INT_RAW_MASK
        | u32::from(r.modem_switch_sleep_start_int_raw()) * f::MODEM_SWITCH_SLEEP_START_INT_RAW_MASK
        | u32::from(r.active_switch_sleep_start_int_raw())
            * f::ACTIVE_SWITCH_SLEEP_START_INT_RAW_MASK
        | u32::from(r.hp_sw_trigger_int_raw()) * f::HP_SW_TRIGGER_INT_RAW_MASK
}
fn pmu_context(s: &Session<'_>, tag: u32) {
    let p = &s.ble.pmu;
    emit(
        tag,
        p.read_slp_wakeup_status1().reject_cause(),
        p.read_lpcpu_wakeup_cause().lpcpu_wakeup_cause(),
        u32::from(p.read_hp_int_st().soc_wakeup_int_st())
            + 2 * u32::from(p.read_hp_int_st().soc_sleep_reject_int_st())
            + 4 * u32::from(p.read_lp_int_st().lp_cpu_wakeup_int_st()),
    );
    emit(
        tag + 1,
        u32::from(p.read_hp_int_ena().soc_wakeup_int_ena()),
        u32::from(p.read_hp_int_ena().soc_sleep_reject_int_ena()),
        u32::from(p.read_lp_int_ena().lp_cpu_wakeup_int_ena()),
    );
}
fn mode(s: &mut Session<'_>, image: u32) {
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(image == 1 || image == 3)
            .set_control_4(image == 2 || image == 3);
    });
    oer_riscv::fence();
}
fn enable(s: &mut Session<'_>, rollover: bool, compare: bool) {
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_bit_1_opaque(rollover).set_bit_18_opaque(compare);
    });
    oer_riscv::fence();
}
fn quiet(s: &mut Session<'_>) {
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    s.ble.timer.disable_bluetooth_lp_sleep_16();
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
    oer_riscv::fence();
}
fn init(s: &mut Session<'_>) {
    ble::init_lp(s);
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    s.ble.timer.disable_bluetooth_lp_sleep_16();
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0020();
    s.ble.timer.modify_word_006c_opaque(|w| {
        w.set_bits_0_31_opaque(LOW_MAX);
    });
    enable(s, true, true);
}
fn crossed(s: &Session<'_>, origin: u32, distance: u32, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < CAUSE_LIMIT {
        if counter(s).wrapping_sub(origin) & LOW_MAX >= distance {
            emit(tag, 1, counter(s), oer_riscv::cycle32().wrapping_sub(start));
            return true;
        }
    }
    emit(tag, 0, counter(s), oer_riscv::cycle32().wrapping_sub(start));
    false
}

pub(super) fn sleep16(s: &mut Session<'_>) {
    init(s);
    pmu_context(s, 0xcff0);
    for image in 0..4 {
        let tag = 0xc000 + image * 0x100;
        mode(s, image);
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
        let origin = counter(s);
        let target = origin.wrapping_add(512) & LOW_MAX;
        s.ble.timer.modify_word_0064_opaque(|w| {
            w.set_bits_0_23_opaque(Bits::truncate(target));
        });
        emit(tag + 0xf0, origin, target, image);
        state(s, tag);
        s.ble.timer.enable_bluetooth_lp_sleep_16();
        if !crossed(s, origin, 1024, tag + 0xf1) {
            quiet(s);
            return;
        }
        state(s, tag + 0x10);
        window();
        state(s, tag + 0x20);
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        state(s, tag + 0x30);
        s.ble.timer.enable_bluetooth_lp_sleep_16();
        state(s, tag + 0x40);
        s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
        window();
        state(s, tag + 0x50);
        // Known delivery bank remains enabled throughout the sleep16 test.
        // A fresh software cause checks it after the destructive clear.
        s.ble
            .timer
            .publish_bluetooth_modem_lp_timer_software_pending();
        window();
        state(s, tag + 0x60);
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
        s.ble.timer.disable_bluetooth_lp_sleep_16();
    }
    quiet(s);
}

pub(super) fn delivery(s: &mut Session<'_>) {
    init(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    let start = oer_riscv::cycle32();
    let mut prior = counter(s);
    let mut wrapped = false;
    while oer_riscv::cycle32().wrapping_sub(start) < WRAP_LIMIT {
        let now = counter(s);
        if now < prior {
            wrapped = true;
            break;
        }
        prior = now;
    }
    emit(
        0xdff0,
        u32::from(wrapped),
        counter(s),
        oer_riscv::cycle32().wrapping_sub(start),
    );
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    enable(s, true, true);
    state(s, 0xd000);
    if !wrapped || s.ble.timer.read_word_002c_opaque().low_byte() == 0 {
        quiet(s);
        return;
    }
    // Each sourced mode image and each sleep command sees the same positive
    // frozen causes; this is a register-semantics experiment, not baselines.
    for image in 0..4 {
        mode(s, image);
        window();
        state(s, 0xd010 + image * 0x10);
    }
    s.ble.timer.enable_bluetooth_lp_sleep_0();
    state(s, 0xd050);
    s.ble.timer.enable_bluetooth_lp_sleep_16();
    state(s, 0xd060);
    s.ble.timer.enable_bluetooth_lp_sleep_17();
    state(s, 0xd070);
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    state(s, 0xd080);
    s.ble.timer.disable_bluetooth_lp_sleep_16();
    state(s, 0xd090);
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    state(s, 0xd0a0);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    state(s, 0xd0b0);
    enable(s, true, false);
    state(s, 0xd0c0);
    enable(s, false, true);
    state(s, 0xd0d0);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
    window();
    state(s, 0xd0e0);
    s.ble
        .timer
        .publish_bluetooth_modem_lp_timer_software_pending();
    window();
    state(s, 0xd0f0);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    window();
    state(s, 0xd100);
    pmu_context(s, 0xdff2);
    quiet(s);
}

fn sleep0(s: &mut Session<'_>, enabled: bool) {
    // SOURCE: BLOB_LIBBTDM_LP_SLEEP_ZERO_0030 and
    // BLOB_LIBBTDM_LP_SLEEP_ETM_COMMANDS; complete positional command pairs.
    if enabled {
        s.ble.etm.publish_bluetooth_lp_sleep_etm_0110();
        s.ble.timer.enable_bluetooth_lp_sleep_0();
        s.ble.timer.modify_word_0078_opaque(|w| {
            w.set_bit_26_opaque(true);
        });
    } else {
        s.ble.timer.modify_word_0078_opaque(|w| {
            w.set_control_25(false);
        });
        s.ble.timer.disable_bluetooth_lp_sleep_0();
        s.ble.timer.modify_word_0078_opaque(|w| {
            w.set_bit_26_opaque(false);
        });
        s.ble.etm.publish_bluetooth_lp_sleep_etm_0114();
    }
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0030();
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(true);
    });
}
pub(super) fn sleep_other(s: &mut Session<'_>) {
    // Establish positively advancing completed BTLC captures first. This
    // sourced digital prefix never starts scheduler RUN or publishes DMA.
    ble_wave2::prerequisite(s);
    init(s);
    mode(s, 0);
    if s.ble.timer.read_word_0058_opaque().control_2() {
        s.ble.timer.modify_word_0058_opaque(|w| {
            w.set_control_1(true);
        });
    }
    pmu_context(s, 0xeff0);
    for companion16 in [false, true] {
        let tag = if companion16 { 0xe100 } else { 0xe000 };
        let origin = counter(s);
        let target17 = origin.wrapping_add(1024) & LOW_MAX;
        s.ble.timer.write_word_0068_opaque(|w| {
            w.set_bits_0_23_opaque(Bits::truncate(target17));
        });
        s.ble.timer.modify_word_0064_opaque(|w| {
            w.set_bits_0_23_opaque(Bits::truncate(origin.wrapping_add(512) & LOW_MAX));
        });
        emit(tag + 0xf0, origin, target17, u32::from(companion16));
        state(s, tag);
        s.ble.etm.trigger_channel_enable_set_ch7();
        s.ble.etm.trigger_channel_enable_set_ch6();
        s.ble.etm.trigger_channel_enable_set_bit_11_opaque();
        s.ble.timer.enable_bluetooth_lp_sleep_17();
        sleep0(s, true);
        if companion16 {
            s.ble.timer.enable_bluetooth_lp_sleep_16();
        }
        if !crossed(s, origin, 2048, tag + 0xf1) {
            quiet(s);
            return;
        }
        state(s, tag + 0x10);
        window();
        state(s, tag + 0x20);
        sleep0(s, false);
        s.ble.timer.disable_bluetooth_lp_sleep_17();
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        state(s, tag + 0x30);
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_0020();
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
    }
    s.ble.etm.trigger_channel_enable_clear_ch6();
    s.ble.etm.trigger_channel_enable_clear_ch7();
    pmu_context(s, 0xeff2);
    quiet(s);
}
