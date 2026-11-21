//! The IEEE 802.15.4 experiments of `radio_local::wave2` (entries 223 and
//! 224), out of the build with their radio: no module declares this file.
use super::*;

fn zb_enable(s: &mut Session<'_>, enabled: bool) {
    zb_mask(s, false);
    s.radio.zb.ieee802154_mac_task.modify_event_enable(|w| {
        w.set_rx_abort(enabled).set_ed_done(enabled);
    });
    oer_portable_riscv::fence();
}
fn zb_observe(s: &Session<'_>, tag: u32) -> u32 {
    let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
    let e = s.radio.zb.ieee802154_mac_task.read_event_enable();
    let bits = u32::from(r.ed_done()) + 2 * u32::from(r.rx_abort());
    emit(
        tag,
        bits,
        u32::from(e.ed_done()) + 2 * u32::from(e.rx_abort()),
        s.interrupt_matrix
            .interrupt_core0_status
            .read_int_status_reg_4()
            .int_status_4(),
    );
    emit(
        tag + 1,
        s.radio
            .zb
            .ieee802154_mac_task
            .read_rx_status()
            .abort_reason()
            .get(),
        s.radio.idle.read_status().txrx_state().get(),
        0,
    );
    bits
}
/// Whether the energy detection runs long enough to be stopped, or completes.
#[derive(Clone, Copy, PartialEq)]
enum EdRun {
    Completed,
    Stopped,
}

impl EdRun {
    /// The record's code: 1 for [`Self::Stopped`].
    const fn code(self) -> u32 {
        match self {
            Self::Completed => 0,
            Self::Stopped => 1,
        }
    }
}

fn ed_leaf(s: &mut Session<'_>, stop: EdRun) {
    use pac::ieee802154::command::Opcode;
    ieee_prepare(s);
    zb_enable(s, true);
    zb_clear(s);
    // SOURCE: public vendor LL disable-coexistence helper (HAL lifecycle).
    // RF stays closed. This removes the observed ED_COEX_REJECT prerequisite.
    s.radio.zb.ieee802154_mac_task.modify_coex_pti(|w| {
        w.set_txrx_pti(Bits::constant::<3>());
    });
    s.radio.zb.ieee802154_mac_task.modify_coex_pti(|w| {
        w.set_ack_pti(Bits::constant::<3>());
    });
    let pti = s.radio.zb.ieee802154_mac_task.read_coex_pti();
    emit(0x22302, pti.txrx_pti().get(), pti.ack_pti().get(), 0);
    s.radio.zb.ieee802154_mac_task.modify_rx_abort_enable(|w| {
        w.set_ed_stop(true)
            .set_ed_abort(true)
            .set_ed_coexistence_reject(true);
    });
    s.radio.zb.ieee802154_mac_task.modify_ed_duration(|w| {
        w.set_duration(Bits::truncate(if stop == EdRun::Stopped {
            100_000
        } else {
            32
        }))
        .set_delay(Bits::constant::<0>());
    });
    s.radio.zb.ieee802154_mac_task.modify_ed_config(|w| {
        w.set_ed_sample_rate(pac::ieee802154::ed_config::EdSampleRate::OnePerUs)
            .set_ed_sample_mode(pac::ieee802154::ed_config::EdSampleMode::Maximum);
    });
    zb_observe(s, 0x22300);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::EdStart, |_| {});
    if stop == EdRun::Stopped {
        window();
        zb_observe(s, 0x22310);
        s.radio
            .zb
            .ieee802154_mac_task
            .write_command(Opcode::Stop, |_| {});
    }
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if r.ed_done() || r.rx_abort() {
            break;
        }
    }
    emit(
        0x22318,
        stop.code(),
        oer_portable_riscv::cycle32().wrapping_sub(start),
        0,
    );
    let positive = zb_observe(s, 0x22320);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::Stop, |_| {});
    if positive != 0 {
        let sample = s.radio.zb.ieee802154_mac_irq.sample_event_status();
        zb_enable(s, false);
        zb_observe(s, 0x22330);
        zb_clear(s); // A masked zero sample, independently from retained sample.
        zb_enable(s, true);
        zb_observe(s, 0x22340);
        zb_enable(s, false);
        s.radio
            .zb
            .ieee802154_mac_irq
            .acknowledge_event_status(sample);
        oer_portable_riscv::fence();
        zb_observe(s, 0x22350);
        zb_enable(s, true);
        zb_observe(s, 0x22360);
    }
    // Independent enabled-clear case with a freshly started producer.
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::EdStart, |_| {});
    if stop == EdRun::Stopped {
        window();
        s.radio
            .zb
            .ieee802154_mac_task
            .write_command(Opcode::Stop, |_| {});
    }
    let start = oer_portable_riscv::cycle32();
    while oer_portable_riscv::cycle32().wrapping_sub(start) < LOCAL_LIMIT {
        let r = s.radio.zb.ieee802154_mac_irq.read_event_status();
        if r.ed_done() || r.rx_abort() {
            break;
        }
    }
    zb_observe(s, 0x22370);
    s.radio
        .zb
        .ieee802154_mac_task
        .write_command(Opcode::Stop, |_| {});
    zb_clear(s);
    oer_portable_riscv::fence();
    zb_observe(s, 0x22380);
    zb_clear(s);
    zb_enable(s, false);
    zb_observe(s, 0x223f0);
}
pub(in crate::image) fn ed(s: &mut Session<'_>) {
    ed_leaf(s, EdRun::Completed);
}
pub(in crate::image) fn ed_stop(s: &mut Session<'_>) {
    ed_leaf(s, EdRun::Stopped);
}
