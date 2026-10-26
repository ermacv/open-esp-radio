//! Phased arrivals, not a timing sweep or a same-cycle set/clear priority test.
use super::{COUNTER_MAX, WAIT_LIMIT, command18, counter, init_lp, lp_state, target, window};
use crate::image::{Session, emit};

fn wait_compare(s: &Session<'_>) -> bool {
    let start = oer_riscv::cycle32();
    while oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {
        if s.ble.timer.read_word_0024_opaque().low_byte() != 0 {
            return true;
        }
    }
    false
}

pub(in crate::image) fn compare_ack(s: &mut Session<'_>) {
    init_lp(s);
    // Read zero, then let a verified compare arrive before the stale zero store.
    let old = s.ble.timer.read_word_0024_opaque().low_byte();
    target(&mut s.ble, 4096);
    command18(&mut s.ble);
    let arrived = wait_compare(s);
    emit(0x8000, old.into(), u32::from(arrived), counter(&s.ble));
    lp_state(s, 0x8010);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    oer_riscv::fence();
    lp_state(s, 0x8020);

    // Positive ordering control: zero before expiry must not prevent arrival.
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    target(&mut s.ble, 4096);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    command18(&mut s.ble);
    let arrived = wait_compare(s);
    emit(0x8030, u32::from(arrived), counter(&s.ble), COUNTER_MAX);
    lp_state(s, 0x8040);

    // Production-like nonzero sample: another armed deadline crosses while
    // that sample is retained. The byte alone cannot count the second event.
    let old = s.ble.timer.read_word_0024_opaque().low_byte();
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    target(&mut s.ble, 4096);
    let deadline = s.ble.timer.read_word_006c_opaque().bits_0_31_opaque();
    command18(&mut s.ble);
    let start = oer_riscv::cycle32();
    while counter(&s.ble) < deadline && oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {}
    window(4096);
    emit(0x8050, old.into(), deadline, counter(&s.ble));
    lp_state(s, 0x8060);
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_0024();
    oer_riscv::fence();
    lp_state(s, 0x8070);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
}

pub(in crate::image) fn two_wraps(s: &mut Session<'_>) {
    init_lp(s);
    s.ble.timer.disable_bluetooth_modem_lp_timer_compare();
    // Exactly two requested wraps, each bounded by 40,000,000 CPU cycles.
    // No acknowledgement or counter seed occurs between them.
    for phase in 0..2 {
        let old = s.ble.timer.read_word_002c_opaque().low_byte();
        let start = oer_riscv::cycle32();
        let mut prior = counter(&s.ble);
        let mut wrapped = false;
        while oer_riscv::cycle32().wrapping_sub(start) < WAIT_LIMIT {
            let now = counter(&s.ble);
            if now < prior {
                wrapped = true;
                break;
            }
            prior = now;
        }
        emit(
            0x8100 + phase,
            old.into(),
            u32::from(wrapped),
            oer_riscv::cycle32().wrapping_sub(start),
        );
        lp_state(s, 0x8120 + phase * 0x10);
        if !wrapped {
            return;
        }
    }
    // The second rollover arrived after the retained nonzero first sample.
    s.ble.timer.clear_bluetooth_modem_lp_timer_state_002c();
    oer_riscv::fence();
    lp_state(s, 0x8140);
    window(4096);
    lp_state(s, 0x8150);
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0014();
    s.ble.timer.prepare_bluetooth_modem_lp_timer_command_0004();
}
