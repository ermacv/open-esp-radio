#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPoint, KeyTypes, PortCommand, Station};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelKeys, ModelTypes};

pub fn permission_belongs_to_interface<'id>(
    station: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    ap: <ModelKeys<'id, Composition, AccessPoint> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) -> PortCommand<'id, ModelTypes<Composition>, ModelCaps> {
    #[cfg(ui_reject)]
    return PortCommand::SelectGroup { permission: ap, installed: selected }; // ui-error: E0308
    #[cfg(not(ui_reject))]
    {
        let _ = ap;
        PortCommand::SelectGroup { permission: station, installed: selected }
    }
}

pub enum UiSlot {}
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<oer_protocols_radio_port::StaticId<UiSlot>>;
