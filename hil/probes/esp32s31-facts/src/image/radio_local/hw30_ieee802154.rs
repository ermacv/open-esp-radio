//! The IEEE 802.15.4 experiment of `radio_local::hw30` (entry 269), out of
//! the build with its radio: no module declares this file.
use super::*;

pub(in crate::image) fn route132(s: &mut Session<'_>) {
    irq_observer::inherited(s);
    if !irq_observer::control(s) {
        return;
    }
    ieee_prepare(s);
    zb_mask(s, false);
    zb_clear(s);
    zb_state(s, 0x3300);
    zb_start(s, ZbTimer::First);
    zb_state(s, 0x3310);
    zb_mask(s, true);
    zb_state(s, 0x3320); // route132 remains parked, CPU MIE stays zero.
    let retained = s.radio.zb.ieee802154_mac_irq.sample_event_status();
    zb_mask(s, false);
    s.radio
        .zb
        .ieee802154_mac_irq
        .acknowledge_event_status(retained);
    zb_mask(s, true);
    window();
    zb_state(s, 0x3330);
    zb_mask(s, false);
}
