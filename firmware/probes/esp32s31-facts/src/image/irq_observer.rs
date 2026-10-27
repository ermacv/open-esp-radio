//! Masked CPU admission; independent matrix observations and validated edge IP.
#![forbid(unsafe_code)]
use super::{Bits, CONTROLS, Index, LINE, SOFTWARE_SOURCE, Session, emit, pac};

pub(super) const INPUTS: [Index<{ pac::clic::ctrl::COUNT }>; 2] =
    [Index::constant::<LINE>(), Index::constant::<{ LINE + 1 }>()];

pub(super) fn route(s: &mut Session<'_>, source: pac::Interrupt, map: u32) {
    if let Ok(i) = Index::new(source as usize) {
        s.interrupt_matrix.interrupt_core0.modify_route(i, |w| {
            w.set_map(Bits::truncate(map));
        });
    }
    oer_riscv::fence();
}
pub(super) fn inherited(s: &Session<'_>) {
    for source in [
        pac::Interrupt::MODEM_WIFI_BB,
        pac::Interrupt::MODEM_I2C_MST,
        pac::Interrupt::MODEM_ZB_MAC,
    ] {
        if let Ok(i) = Index::new(source as usize) {
            let r = s.interrupt_matrix.interrupt_core0.read_route(i);
            emit(0x3000, source as u32, r.map().get(), r.pass_level().get());
        }
    }
}
pub(super) fn clear(s: &mut Session<'_>) {
    for i in INPUTS {
        s.interrupt_matrix.clic.modify_ctrl(i, |w| {
            w.set_ip(false);
        });
    }
    oer_riscv::fence();
}
pub(super) fn pending(s: &Session<'_>) -> u32 {
    INPUTS.into_iter().enumerate().fold(0, |v, (n, i)| {
        v | (u32::from(s.interrupt_matrix.clic.read_ctrl(i).ip()) << n)
    })
}
pub(super) fn control(s: &mut Session<'_>) -> bool {
    // setup contains hart1, parks every route and masks every CPU line.
    super::setup(s);
    // Retain and feed the ROM reset backstop once per bounded leaf.
    s.ble.watchdog.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Unlock.bits());
    });
    s.ble.watchdog.trigger_feed_feed();
    s.ble.watchdog.modify_wprotect(|w| {
        w.set_wdt_wkey(pac::rtc_wdt::wprotect::WdtWkey::Lock.bits());
    });
    let mut valid = true;
    for (n, i) in INPUTS.into_iter().enumerate() {
        s.source.store_hp_cpu_int_from_cpu_2(false);
        route(s, pac::Interrupt::CPU_INTR_FROM_CPU_2, (LINE + n) as u32);
        s.interrupt_matrix.clic.modify_ctrl(i, |w| {
            w.set_ie(false)
                .set_attr_shv(true)
                .set_attr_trig(pac::clic::ctrl::AttrTrig::PositiveEdge)
                .set_ctl(CONTROLS[1])
                .set_ip(false);
        });
        super::wait_window();
        let before = s.interrupt_matrix.clic.read_ctrl(i).ip();
        s.source.store_hp_cpu_int_from_cpu_2(true);
        super::wait_window();
        let after = s.interrupt_matrix.clic.read_ctrl(i);
        emit(
            0x3001,
            n as u32,
            u32::from(before) | (u32::from(after.ip()) << 1) | (u32::from(after.ie()) << 2),
            s.interrupt_matrix
                .interrupt_core0_status
                .read_int_status_reg_2()
                .int_status_2(),
        );
        valid &= !before && after.ip() && !after.ie();
        s.source.store_hp_cpu_int_from_cpu_2(false);
        super::wait_window();
        clear(s);
    }
    s.interrupt_matrix
        .interrupt_core0
        .modify_route(SOFTWARE_SOURCE, |w| {
            w.set_map(Bits::constant::<0>());
        });
    emit(0x3002, u32::from(valid), pending(s), 0);
    valid && pending(s) == 0
}
