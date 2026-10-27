//! One explicitly authorized natural period, not part of request-zero batches.
//! No VALUE/UPDATE writes, timer seeding, RF, scheduler RUN or interrupt service.
use super::{Session, all_mask, command, emit, enable, finish, sample, window};

// SOURCE: ESP-IDF 4d59230d soc/esp32s31/include/soc/clk_tree_defs.h:125-127,
// also used by the production boot clock owner. Read-back must establish
// XTAL / 1 before using this nominal reference.
const CPU_HZ: u64 = 40_000_000;
// SOURCE: owner authorization 2026-10-27, hw-btlc-natural-wrap task.
const JOURNAL_SECONDS: u64 = 90;
const ACK_SECONDS: u64 = 2250;
const END_SECONDS: u64 = 2280;

struct Observation {
    before: u64,
    after: u64,
    raw: u32,
    primary: u32,
    matrix3: u32,
    matrix4: u32,
    nrt1: u32,
}

fn observe(s: &mut Session<'_>) -> Option<Observation> {
    let before = oer_riscv::cycle64();
    let raw = sample(s)?;
    let after = oer_riscv::cycle64();
    Some(Observation {
        before,
        after,
        raw,
        primary: s
            .ble
            .bank
            .sample_bluetooth_primary_interrupt_bank_1()
            .bits(),
        matrix3: s
            .interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_3()
            .int_status_3(),
        matrix4: s
            .interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
        nrt1: s.ble.bank.read_irq_status_snapshot_1().pending_bits(),
    })
}

fn journal(tag: u32, o: &Observation) {
    emit(tag, o.raw, o.before as u32, (o.before >> 32) as u32);
    emit(tag + 1, o.after as u32, (o.after >> 32) as u32, o.primary);
    emit(tag + 2, o.matrix3, o.matrix4, o.nrt1);
}

fn feed(s: &mut Session<'_>) {
    let w = &mut s.ble.watchdog;
    w.modify_wprotect(|w| {
        w.set_wdt_wkey(super::pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    w.trigger_feed_feed();
    w.modify_wprotect(|w| {
        w.set_wdt_wkey(super::pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
}

fn reference(s: &Session<'_>) -> bool {
    let cpu = s.sar.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
    let root = s
        .sar
        .boot
        .hp_sys_clkrst_boot_clock
        .read_soc_clk_sel()
        .reg_soc_clk_sel()
        .get();
    emit(
        0xe600,
        root,
        u32::from(cpu.reg_cpu_clk_div_num()),
        cpu.reg_cpu_clk_div_numerator().get(),
    );
    emit(
        0xe601,
        cpu.reg_cpu_clk_div_denominator().get(),
        CPU_HZ as u32,
        JOURNAL_SECONDS as u32,
    );
    root == 0 && cpu.reg_cpu_clk_div_num() == 0 && cpu.reg_cpu_clk_div_numerator().get() == 0
}

pub(in crate::image) fn natural_wrap(s: &mut Session<'_>) {
    run(s, true);
}

/// Bounded readiness fact: prove reset-origin captures advance before leasing
/// the full observation interval. This never starts the long observation.
pub(in crate::image) fn natural_ready(s: &mut Session<'_>) {
    run(s, false);
}

fn run(s: &mut Session<'_>, full_period: bool) {
    // Unlike setup(), this stops before the positional seed / TIMER_ARM writes.
    // The hardware reset supplies the initial state; CONTROL0 starts it directly.
    super::super::digital_setup(s);
    all_mask(s);
    s.ble.core.modify_btlc_timer_control(|w| {
        w.set_clock_shift(super::Bits::constant::<3>())
            .set_config_24(false)
            .set_compare_0_enable(false)
            .set_compare_1_enable(false)
            .set_compare_2_enable(false)
            .set_compare_3_enable(false);
    });
    // SOURCE: BLOB_LIBBTDM_BASELINE_IRQ_CURRENT: enable only the cause under test.
    // Baseline group bits are unnecessary for the primary bank in prior HW9/HW12.
    enable(s, 2, true);
    command(s, 0);
    let control = s.ble.core.read_btlc_timer_control();
    emit(
        0xe602,
        control.clock_shift().get(),
        u32::from(control.config_24()),
        u32::from(control.timer_arm()),
    );
    if !reference(s) {
        emit(0xe6ff, 1, 0, 0);
        finish(s);
        return;
    }
    let Some(mut prior) = observe(s) else {
        emit(0xe6ff, 2, 0, 0);
        finish(s);
        return;
    };
    journal(0xe610, &prior);
    let origin = prior.after;
    window(4_800_000);
    let Some(rate) = observe(s) else {
        emit(0xe6ff, 3, 0, 0);
        finish(s);
        return;
    };
    journal(0xe620, &rate);
    if rate.raw <= prior.raw || prior.primary != 0 || rate.primary != 0 {
        emit(0xe6ff, 4, prior.raw, rate.raw);
        finish(s);
        return;
    }
    if !full_period {
        emit(0xe6fe, 1, 0, 0);
        finish(s);
        return;
    }
    prior = rate;
    let mut next = origin + JOURNAL_SECONDS * CPU_HZ;
    let mut index = 1;
    let mut wraps = 0;
    let mut rises = 0;
    let mut acked = false;
    loop {
        // Keep the ROM watchdog armed. Each digital poll and latch is bounded;
        // only this owner-authorized outer observation spans a full period.
        feed(s);
        window(400_000);
        let Some(now) = observe(s) else {
            emit(0xe6ff, 5, index, 0);
            break;
        };
        if now.raw < prior.raw {
            wraps += 1;
            journal(0xe700, &prior);
            journal(0xe710, &now);
        }
        if now.primary != 0 && prior.primary == 0 {
            rises += 1;
            journal(0xe720, &prior);
            journal(0xe730, &now);
        }
        if now.after >= next {
            journal(0xe800 + index * 4, &now);
            index += 1;
            next += JOURNAL_SECONDS * CPU_HZ;
        }
        if !acked && now.after - origin >= ACK_SECONDS * CPU_HZ {
            journal(0xe740, &now);
            let pending = s.ble.bank.sample_bluetooth_primary_interrupt_bank_1();
            emit(0xe74f, pending.bits(), wraps, rises);
            // Affine positive image is the only acknowledgement during the run.
            s.ble
                .bank
                .acknowledge_bluetooth_primary_interrupt_bank_1(pending);
            oer_riscv::fence();
            acked = true;
            if let Some(after) = observe(s) {
                journal(0xe750, &after);
            }
        }
        if now.after - origin >= END_SECONDS * CPU_HZ {
            journal(0xe760, &now);
            emit(0xe770, wraps, rises, u32::from(acked));
            break;
        }
        prior = now;
    }
    finish(s);
}
