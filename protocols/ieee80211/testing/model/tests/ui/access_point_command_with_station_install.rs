#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPointCommand, PortCommand};
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCaps, ModelTypes};

pub enum UiSlot {}
type Id = oer_protocols_radio_port::StaticId<UiSlot>;
type Composition = ModelBackend<Id>;
type Types = ModelTypes<Composition>;

pub fn reuse<'id>(station: PortCommand<'id, Types, ModelCaps>, access_point: AccessPointCommand<'id, Types, ModelCaps>) -> Option<AccessPointCommand<'id, Types, ModelCaps>> {
    #[cfg(not(ui_reject))]
    let _ = station;
    #[cfg(not(ui_reject))]
    let command = access_point;
    #[cfg(ui_reject)]
    let _ = access_point;
    #[cfg(ui_reject)]
    let PortCommand::InstallPairwise(install) = station else { return None };
    #[cfg(ui_reject)]
    let command = AccessPointCommand::InstallPairwise(install); // ui-error: E0271
    Some(command)
}
