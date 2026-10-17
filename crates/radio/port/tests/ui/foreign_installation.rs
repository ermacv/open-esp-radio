#![no_std]
use oer_radio_port::Slot;

pub enum Kind {}

pub fn retain(place: &mut Option<Slot<'static, Kind>>) {
    let mut fresh_installation = |slot: Slot<'_, Kind>| {
        #[cfg(ui_reject)]
        {
            *place = Some(slot); // ui-error: E0521
        }
        #[cfg(not(ui_reject))]
        drop(slot);
    };
    // The same universal bound as the installation factory: no caller can
    // choose the fresh brand to be the captured outer installation's brand.
    let _: &mut dyn for<'fresh> FnMut(Slot<'fresh, Kind>) = &mut fresh_installation;
}
