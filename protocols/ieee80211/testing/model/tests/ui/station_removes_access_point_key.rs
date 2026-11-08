#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPoint, PortCommand, Station};
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCaps, ModelGroupKey, ModelPool, ModelTypes};
use oer_protocols_radio_port::Slot;

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = ModelBackend<Id>;
#[cfg(not(ui_reject))]
type Role = Station;
#[cfg(ui_reject)]
type Role = AccessPoint;
type Permission<'id> = Slot<'id, oer_protocols_ieee80211_testing_model::ModelGroupPermission<Station>, ModelPool<Id, 8>>;

pub fn remove<'id>(permission: Permission<'id>, key: ModelGroupKey<'id, Composition, Role>) -> PortCommand<'id, ModelTypes<Composition>, ModelCaps> {
    PortCommand::RemoveGroup { permission, key } // ui-error: E0308
}
