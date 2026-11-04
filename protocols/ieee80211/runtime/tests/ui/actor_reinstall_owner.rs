#![no_std]
use oer_protocols_ieee80211_contracts_port::PortTypes;
use oer_protocols_ieee80211_core_upper_mac::rate_selection::RateSelection;
use oer_protocols_ieee80211_runtime::datapath::{Actor, ActorEvents, ActorRxMeta, DatapathPort};

pub fn reinstall_once<'id, D, S, O>(actor: Actor<'id, D, S, O>)
where
    D: DatapathPort<'id>,
    S: RateSelection<
            ActorRxMeta<'id, D>,
            <D::Types as PortTypes<'id>>::AckEvidence,
            <D::Types as PortTypes<'id>>::BlockAckEvidence,
        >,
    O: ActorEvents,
{
    let next = actor.reinstall(Ok::<D::Port, (D::Port, ())>);
    #[cfg(ui_reject)]
    drop(actor); // ui-error: E0382
    drop(next);
}
