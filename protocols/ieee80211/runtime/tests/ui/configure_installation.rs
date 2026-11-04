#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    ConfigurePermission, PortTypes, PortCommand, Station, StationReceive,
};
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelCaps, ModelTypes};
use oer_protocols_radio_port::StaticId;

pub enum Own {}
pub enum Foreign {}
type Types<I> = ModelTypes<ModelBackend<StaticId<I>>>;
type Path<'id, I> = <Types<I> as PortTypes<'id>>::StationCommandPath;

// A configure permission configures only the installation that minted it.
pub fn configure<'a, 'b>(
    own: ConfigurePermission<'a, Station, Path<'a, Own>>,
    foreign: ConfigurePermission<'b, Station, Path<'b, Foreign>>,
) -> PortCommand<'a, Types<Own>, ModelCaps> {
    #[cfg(ui_reject)]
    return PortCommand::Configure { permission: foreign, receive: StationReceive::Off }; // ui-error: E0308
    #[cfg(not(ui_reject))]
    {
        let _ = foreign;
        PortCommand::Configure { permission: own, receive: StationReceive::Off }
    }
}
