#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    Configured, DataPort, LowerMacTerminal, PortCommand, StationReceive,
};
pub fn reuse<'id, P: DataPort<'id>>(
    terminal: LowerMacTerminal<'id, P::Types>,
) -> Option<PortCommand<'id, P::Types, P::Caps>> {
    let LowerMacTerminal::Configured(Configured::Station(permission)) = terminal else {
        return None;
    };
    let next = PortCommand::Configure { permission, receive: StationReceive::Off };
    #[cfg(ui_reject)]
    let _again = PortCommand::<'id, P::Types, P::Caps>::Configure { permission, receive: StationReceive::Off }; // ui-error: E0382
    Some(next)
}
