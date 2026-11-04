#![no_std]
use oer_protocols_ieee80211_contracts_port::{DataPort, GatePool, PhaseTypes, PortCommand};
use oer_protocols_radio_port::{Child, Mint, Root};

pub fn close<'id, P>(mint: Mint<'id, (GatePool, GatePool)>) -> PortCommand<'id, P::Types, P::Caps>
where
    P: DataPort<'id>,
    P::Types: PhaseTypes<'id, GatePath = Child<Root<oer_protocols_radio_port::LocalId<'id>>, 0>>,
{
    let ((first, _), (second, _)) = mint.into_pools();
    #[cfg(ui_reject)]
    return PortCommand::CloseGate(second); // ui-error: E0271
    #[cfg(not(ui_reject))]
    PortCommand::CloseGate(first)
}
