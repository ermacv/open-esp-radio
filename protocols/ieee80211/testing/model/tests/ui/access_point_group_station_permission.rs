#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPoint, AccessPointCommands, GroupKeyId, KeyTypes};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCommands, ModelKeys, ModelResources};
use oer_protocols_ieee80211_wire::security::Spp;

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = ModelBackend<Id>;

pub fn install<'id>(commands: &mut ModelCommands<'id, Id>, resources: ModelResources<'id, Composition>, key_id: GroupKeyId) {
    let association = ModelKeys::<'_, Composition, AccessPoint>::begin_association(resources.access_point.association, Spp::NotNegotiated);
    let [slot, _] = resources.access_point.group;
    #[cfg(not(ui_reject))]
    let permission = resources.access_point.group_permission;
    #[cfg(ui_reject)]
    let permission = resources.station.group_permission;
    let _install = commands.install_access_point_group(&association, permission, slot, key_id, Ccmp128Key::new(&[1; 16])); // ui-error: E0308
}
