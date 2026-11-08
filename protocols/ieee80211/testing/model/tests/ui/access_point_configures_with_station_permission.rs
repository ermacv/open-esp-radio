#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPointCommand, AccessPointReceive};
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCaps, ModelResources, ModelTypes};

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = ModelBackend<Id>;

pub fn configure<'id>(resources: ModelResources<'id, Composition>) -> AccessPointCommand<'id, ModelTypes<Composition>, ModelCaps> {
    #[cfg(not(ui_reject))]
    let permission = resources.access_point.commands.configure;
    #[cfg(ui_reject)]
    let permission = resources.station.commands.configure;
    AccessPointCommand::Configure { permission, receive: AccessPointReceive::Off, tsf: None } // ui-error: E0308
}
