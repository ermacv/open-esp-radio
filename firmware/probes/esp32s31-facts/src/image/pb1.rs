//! PB-1a/1b: finite register facts, CPU admission masked, no RF or DMA.
use super::{Bits, Session, ble, ble_wave2, contain, emit, pac};

const MAX: u32 = pac::modem_pwr::word_0048_opaque::COUNTER_LOW_MAX;
const SHORT: u32 = 4_000_000; // 100 ms at the checked 40 MHz CPU.
const CPU_HZ: u64 = 40_000_000; // SOURCE: chip XTAL clock, ROM-loader CPU root0/div0.

fn counter(s: &Session<'_>) -> u32 {
    s.ble.timer.read_word_0048_opaque().counter_low().get()
}
fn window() {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < 4096 {}
}
fn feed(s: &mut Session<'_>) {
    let w = &mut s.ble.watchdog;
    w.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    w.trigger_feed_feed();
    w.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
}
fn state(s: &Session<'_>, tag: u32) {
    let t = &s.ble.timer;
    emit(
        tag,
        counter(s),
        s.ble.timer_status.read_bluetooth_modem_lp_status(),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
    );
    emit(
        tag + 1,
        t.read_bluetooth_modem_lp_pending_image(),
        t.read_bluetooth_modem_lp_rollover_image(),
        u32::from(t.read_word_0058_opaque().control_2()),
    );
    emit(
        tag + 2,
        t.read_word_001c_opaque().sleep16_flag_byte().into(),
        t.read_word_0020_opaque().sleep17_flag_byte().into(),
        s.modem_wake_status
            .read_modem_intr_status()
            .modem_intr_vector()
            .get(),
    );
}
fn mode(s: &mut Session<'_>, enabled: bool) {
    // SOURCE: BX4_S31_LP_MODE58_USERS_20261027: sourced clear/set pair.
    s.ble.timer.modify_word_0058_opaque(|w| {
        w.set_control_0(enabled).set_control_4(enabled);
    });
    oer_riscv::fence();
}
fn park(s: &mut Session<'_>) {
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    s.ble.timer.trigger_word_0058_opaque_control_1();
    oer_riscv::fence();
}
fn stop(s: &mut Session<'_>) {
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    s.ble.timer.disable_bluetooth_lp_sleep_16();
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    s.ble.etm.trigger_channel_enable_clear_ch6();
    s.ble.etm.trigger_channel_enable_clear_ch7();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0034();
    oer_riscv::fence();
}
fn init(s: &mut Session<'_>) -> bool {
    oer_riscv::disable_interrupts();
    // Existing sourced advancing-BTLC/digital-ETM prefix; no scheduler RUN.
    ble_wave2::prerequisite(s);
    ble::init_lp(s);
    s.sar
        .radio
        .modem_lpcon_shared_clock
        .modify_lp_timer_conf(|w| {
            w.set_clk_lp_timer_div_num(Bits::constant::<399>());
        });
    s.ble.timer.disable_bluetooth_lp_sleep_0();
    s.ble.timer.disable_bluetooth_lp_sleep_16();
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0020();
    park(s);
    mode(s, false);
    // SOURCE: BLOB_LIBBTDM_RUNTIME_CONTROL / BX4_S31_LP_SLEEP0_OBSERVERS:
    // event46 -> tasks11/0f; these positional selectors do not name endpoints.
    s.ble.etm.modify_ch6_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
    });
    s.ble.etm.modify_ch7_event(|w| {
        w.set_id(pac::modem_etm::ch_event::Id::BtdmLowPowerEvent46.bits());
    });
    s.ble.etm.modify_ch6_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask11.bits());
    });
    s.ble.etm.modify_ch7_task(|w| {
        w.set_id(pac::modem_etm::ch_task::Id::BtdmLowPowerTask0f.bits());
    });
    let cpu = s.sar.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
    let root = s
        .sar
        .boot
        .hp_sys_clkrst_boot_clock
        .read_soc_clk_sel()
        .reg_soc_clk_sel()
        .get();
    emit(
        0x240f0,
        root,
        cpu.reg_cpu_clk_div_num().into(),
        cpu.reg_cpu_clk_div_numerator().get(),
    );
    let conf = s.sar.radio.modem_lpcon_shared_clock.read_lp_timer_conf();
    emit(
        0x240f1,
        conf.clk_lp_timer_div_num().get(),
        u32::from(conf.clk_lp_timer_sel_xtal()),
        counter(s),
    );
    let okay =
        root == 0 && cpu.reg_cpu_clk_div_num() == 0 && cpu.reg_cpu_clk_div_numerator().get() == 0;
    if !okay {
        emit(0x24ee, 1, root, 0);
        stop(s);
    }
    okay
}
fn crossed(s: &Session<'_>, origin: u32, distance: u32, tag: u32) -> bool {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < SHORT {
        if counter(s).wrapping_sub(origin) & MAX >= distance {
            emit(
                tag,
                origin,
                counter(s),
                oer_riscv::cycle32().wrapping_sub(start),
            );
            return true;
        }
    }
    emit(0x24ee, 2, tag, counter(s));
    false
}
fn bank_state(s: &Session<'_>, tag: u32) {
    let r = s.ble.bank.read_irq_enable_1();
    emit(
        tag,
        u32::from(r.bit_0_opaque()),
        u32::from(r.bit_2_opaque()),
        u32::from(r.controller_time_overflow())
            + 2 * u32::from(r.source_3())
            + 4 * u32::from(r.source_8())
            + 8 * u32::from(r.source_9())
            + 16 * u32::from(r.source_12()),
    );
}
fn baseline(s: &mut Session<'_>, enabled: bool) {
    // SOURCE: BLOB_LIBBTDM_BASELINE_IRQ_CURRENT / reviewed 0x8000,0x1302.
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_15(enabled);
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_controller_time_overflow(enabled)
            .set_source_8(enabled)
            .set_source_9(enabled)
            .set_source_12(enabled);
    });
}
fn dynamic(s: &mut Session<'_>, enabled: bool) {
    // SOURCE: BLOB_LIBBTDM_CONTROLLER_IRQ / reviewed dynamic0x18200000,8.
    s.ble.bank.modify_irq_enable_0(|w| {
        w.set_source_21(enabled).set_sources_27_28(if enabled {
            Bits::constant::<3>()
        } else {
            Bits::constant::<0>()
        });
    });
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_source_3(enabled);
    });
}
pub(super) fn bank1(s: &mut Session<'_>) {
    oer_riscv::disable_interrupts();
    contain(s);
    ble::clocks(s);
    let old = s.ble.bank.read_irq_enable_1();
    for lane in [0, 2] {
        s.ble.bank.modify_irq_enable_1(|w| {
            w.set_bit_0_opaque(lane == 0).set_bit_2_opaque(lane == 2);
        });
        let tag = 0x24100 + lane * 0x100;
        bank_state(s, tag); // positive readback of separately seeded lanes
        s.ble.bank.clear_bluetooth_primary_baseline_bank_0();
        s.ble.bank.clear_bluetooth_primary_baseline_bank_1();
        bank_state(s, tag + 7);
        baseline(s, true);
        bank_state(s, tag + 1);
        s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_0();
        s.ble.bank.clear_bluetooth_scheduler_run_interrupts_bank_1();
        bank_state(s, tag + 8);
        dynamic(s, true);
        bank_state(s, tag + 2);
        dynamic(s, false);
        bank_state(s, tag + 3);
        baseline(s, false);
        bank_state(s, tag + 4);
        // SOURCE: bx-ble-bank1-clear.md: classic olc_intc init/deinit each
        // read then publish the unchanged ENABLE_1 word. PAC RMW does exactly that.
        s.ble.bank.modify_irq_enable_1(|_| {});
        bank_state(s, tag + 5);
        s.ble.bank.modify_irq_enable_1(|_| {});
        bank_state(s, tag + 6);
    }
    s.ble.bank.modify_irq_enable_1(|w| {
        w.set_bit_0_opaque(old.bit_0_opaque())
            .set_bit_2_opaque(old.bit_2_opaque());
    });
}
pub(super) fn sleep16(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    for enabled in [false, true] {
        let tag = if enabled { 0x25100 } else { 0x25000 };
        s.ble.timer.disable_bluetooth_lp_sleep_16();
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_001c();
        let origin = counter(s);
        let target = origin.wrapping_add(2048) & MAX;
        s.ble.timer.modify_word_0064_opaque(|w| {
            w.set_sleep16_target(Bits::truncate(target));
        });
        if enabled {
            s.ble.timer.enable_bluetooth_lp_sleep_16();
        }
        emit(tag + 8, origin, target, u32::from(enabled));
        state(s, tag);
        if !crossed(s, origin, 4096, tag + 9) {
            stop(s);
            return;
        }
        state(s, tag + 0x10);
    }
    stop(s);
}
pub(super) fn sleep17(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    s.ble.etm.trigger_channel_enable_set_ch6();
    s.ble.etm.trigger_channel_enable_set_ch7();
    s.ble.etm.trigger_channel_enable_set_bit_11_opaque();
    state(s, 0x26000);
    let origin = counter(s);
    let target = origin.wrapping_add(2048) & MAX;
    // No target68 read: existing WO word remains WO.
    s.ble.timer.write_word_0068_opaque(|w| {
        w.set_sleep17_target(Bits::truncate(target));
    });
    s.ble.timer.enable_bluetooth_lp_sleep_17();
    emit(0x26008, origin, target, 0);
    if !crossed(s, origin, 4096, 0x26009) {
        stop(s);
        return;
    }
    state(s, 0x26010);
    s.ble.timer.disable_bluetooth_lp_sleep_17();
    window();
    state(s, 0x26020);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0020();
    window();
    state(s, 0x26030);
    stop(s);
}
fn sleep0_command(s: &mut Session<'_>, enabled: bool) {
    // SOURCE: BX4_S31_LP_SLEEP0_OBSERVERS_20261027, complete helper order.
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
    oer_riscv::fence();
}
fn reference(s: &Session<'_>, tag: u32) {
    let before = counter(s);
    let r = s.ble.timer.read_word_0078_opaque();
    let etm = s.ble.etm.read_word_010c_opaque().bit_26_opaque();
    let after = counter(s);
    emit(tag, before, r.bits_0_23_opaque().get(), after);
    emit(
        tag + 1,
        u32::from(r.control_25()),
        u32::from(r.bit_26_opaque()),
        u32::from(etm),
    );
}
fn command_observation(s: &mut Session<'_>, tag: u32, enabled: bool) {
    reference(s, tag);
    sleep0_command(s, enabled);
    reference(s, tag + 2);
    window();
    reference(s, tag + 4);
}
pub(super) fn sleep0(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    command_observation(s, 0x27000, true);
    let origin = counter(s);
    if !crossed(s, origin, 128, 0x27009) {
        stop(s);
        return;
    }
    command_observation(s, 0x27010, false);
    // Isolate CONTROL25 publication while the other command state is retained.
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(false);
    });
    reference(s, 0x27020);
    let origin = counter(s);
    if !crossed(s, origin, 128, 0x27029) {
        stop(s);
        return;
    }
    reference(s, 0x27022);
    s.ble.timer.modify_word_0078_opaque(|w| {
        w.set_control_25(true);
    });
    reference(s, 0x27024);
    stop(s);
}
pub(super) fn width(s: &mut Session<'_>) {
    width_modes(s, &[false, true]);
}
pub(super) fn width_set(s: &mut Session<'_>) {
    width_modes(s, &[true]);
}
fn width_modes(s: &mut Session<'_>, modes: &[bool]) {
    if !init(s) {
        return;
    }
    const LOW_WIDTH: u32 = pac::modem_pwr::word_006c_opaque::COMPARE_TARGET_WIDTH;
    const UPPER_WIDTH: u32 = pac::modem_pwr::word_006c_opaque::BITS_24_31_OPAQUE_WIDTH;
    for &enabled in modes {
        for trial in 0..UPPER_WIDTH + 2 {
            // CONTROL4=1 retained latch2 in the first capture. Clear the
            // sourced mode pair before its CONTROL1 trigger, then restore
            // the tested pair before reading the independent precondition.
            mode(s, false);
            park(s);
            mode(s, enabled);
            let initial = s.ble.timer.read_word_0024_opaque().low_byte();
            let latch = s.ble.timer.read_word_0058_opaque().control_2();
            let upper = if trial == 0 {
                0
            } else if trial == UPPER_WIDTH + 1 {
                u8::MAX
            } else {
                1_u8 << (trial - 1)
            };
            let origin = counter(s);
            let target = origin.wrapping_add(2048) & MAX;
            s.ble.timer.modify_word_006c_opaque(|w| {
                w.set_compare_target(Bits::truncate(target))
                    .set_bits_24_31_opaque(upper);
            });
            let r = s.ble.timer.read_word_006c_opaque();
            s.ble.timer.write_word_0010_opaque(|w| {
                w.set_compare_software_delivery_set(true);
            });
            let tag = 0x28000 + u32::from(enabled) * 0x1000 + trial * 0x10;
            emit(
                tag,
                target | (u32::from(upper) << LOW_WIDTH),
                r.compare_target().get() | (u32::from(r.bits_24_31_opaque()) << LOW_WIDTH),
                origin,
            );
            if !crossed(s, origin, 4096, tag + 1) {
                stop(s);
                return;
            }
            emit(
                tag + 2,
                u32::from(initial),
                u32::from(latch),
                u32::from(s.ble.timer.read_word_0058_opaque().control_2()),
            );
            emit(
                tag + 3,
                s.ble.timer.read_bluetooth_modem_lp_pending_image(),
                s.ble.timer_status.read_bluetooth_modem_lp_status(),
                s.interrupt_matrix
                    .interrupt_core0_status
                    .read_int_status_reg_3()
                    .int_status_3(),
            );
            if initial != 0 || latch || s.ble.timer.read_word_0024_opaque().low_byte() == 0 {
                emit(0x24ee, 3, tag, 0);
                stop(s);
                return;
            }
        }
    }
    stop(s);
}
fn natural_wrap(s: &mut Session<'_>, tag: u32, observe_reference: bool) -> bool {
    let mut last_cycle = oer_riscv::cycle32();
    let mut elapsed = 0_u64;
    let mut last_feed = 0_u64;
    let mut prior = counter(s);
    let mut observed = false;
    while elapsed < CPU_HZ * 190 {
        let cycle = oer_riscv::cycle32();
        elapsed += u64::from(cycle.wrapping_sub(last_cycle));
        last_cycle = cycle;
        if elapsed - last_feed >= CPU_HZ {
            feed(s);
            last_feed = elapsed;
        }
        let now = counter(s);
        if now < prior {
            emit(tag, prior, now, elapsed as u32);
            emit(tag + 1, (elapsed >> 32) as u32, 190, u32::from(observed));
            return true;
        }
        if observe_reference && !observed && now >= MAX - 64 {
            command_observation(s, tag + 0x10, true);
            command_observation(s, tag + 0x20, false);
            observed = true;
        }
        prior = now;
    }
    emit(0x24ee, 4, tag, counter(s));
    false
}
pub(super) fn whole(s: &mut Session<'_>) {
    if !init(s) {
        return;
    }
    // WHOLE1/4: positive software publication, mask retention and unmask.
    for masked in [false, true] {
        s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
        if !masked {
            s.ble.timer.write_word_0010_opaque(|w| {
                w.set_compare_software_delivery_set(true);
            });
        }
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
        let tag = if masked { 0x2a100 } else { 0x2a000 };
        state(s, tag);
        s.ble
            .timer
            .publish_bluetooth_modem_lp_timer_software_pending();
        window();
        state(s, tag + 0x10);
        if masked {
            s.ble.timer.write_word_0010_opaque(|w| {
                w.set_compare_software_delivery_set(true);
            });
            window();
            state(s, tag + 0x20);
        }
        s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
        window();
        state(s, tag + 0x30);
    }
    park(s);
    s.ble.timer.write_word_0010_opaque(|w| {
        w.set_rollover_delivery_set(true);
    });
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    state(s, 0x2a200);
    // Exactly two permitted natural divider399 wraps; no counter seeding.
    if !natural_wrap(s, 0x2a2f0, true) {
        stop(s);
        return;
    }
    state(s, 0x2a210);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    window();
    state(s, 0x2a220);
    if !natural_wrap(s, 0x2a3f0, false) {
        stop(s);
        return;
    }
    state(s, 0x2a230);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    window();
    state(s, 0x2a240);
    stop(s);
}
