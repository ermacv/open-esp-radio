use oer_ieee802154_model::{ModelVacant, OwnedModel};
use oer_radio_port::SlotIdentity;

pub fn resume<Id: SlotIdentity>(hardware: ModelVacant) {
    let backend = OwnedModel::<Id>::resume(hardware);
    #[cfg(ui_reject)]
    let _ = OwnedModel::<Id>::resume(hardware); // ui-error: E0382
    drop(backend);
}
