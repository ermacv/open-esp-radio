use oer_bluetooth_le_model::Resources;
use oer_radio_port::SlotIdentity;
#[cfg(ui_reject)]
pub fn keep<Id: SlotIdentity + 'static>(resources: Resources<'_, Id>) -> Box<dyn core::any::Any> {
    Box::new(resources.session) // ui-error: lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn keep<'p, Id: SlotIdentity>(resources: Resources<'p, Id>) -> oer_bluetooth_le_port::Session<'p, oer_radio_port::Root<Id>, oer_bluetooth_le_port::Idle> {
    resources.session
}
