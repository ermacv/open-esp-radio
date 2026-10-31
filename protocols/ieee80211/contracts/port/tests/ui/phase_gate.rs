#![no_std]
use oer_ieee80211_contracts_port::{GatePool, LowerMacPhase, PhaseTypes};
use oer_radio_port::{Child, Mint, Root};

pub fn close<'p, 'id: 'p, P>(phase: &mut P, mint: Mint<'id, (GatePool, GatePool)>)
where
    P: LowerMacPhase<'p, 'id>,
    P::Types: PhaseTypes<'id, GatePath = Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
{
    let ((first, _), (second, _)) = mint.into_pools();
    #[cfg(ui_reject)]
    phase.close_gate(second); // ui-error: E0308
    #[cfg(not(ui_reject))]
    phase.close_gate(first);
}
