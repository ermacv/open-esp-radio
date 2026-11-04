#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    ConfigurePermission, DataPort, PhaseTypes, PortCommand, Station, StationReceive,
};
pub fn configure<'id, P: DataPort<'id>>(
    permission: ConfigurePermission<
        'id,
        Station,
        <P::Types as PhaseTypes<'id>>::StationCommandPath,
        <P::Types as PhaseTypes<'id>>::StationConfigureContext,
    >,
) -> PortCommand<'id, P::Types, P::Caps> {
    let first = PortCommand::Configure { permission, receive: StationReceive::Off };
    #[cfg(ui_reject)]
    let _second = PortCommand::<'id, P::Types, P::Caps>::Configure { permission, receive: StationReceive::Off }; // ui-error: E0382
    first
}
