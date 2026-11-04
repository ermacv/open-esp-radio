#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    AggregateOutcome, InFlight, PortSingleOutcome, PortTypes, TxCompleted,
};
use oer_protocols_radio_port::Slot;

type Ticket<'id, P> =
    Slot<'id, <P as PortTypes<'id>>::TicketKind, <P as PortTypes<'id>>::TicketPath>;

pub fn finish<'id, P: PortTypes<'id>>(
    operation: InFlight<P::Context, Ticket<'id, P>, P::TxMpdu>,
    single: PortSingleOutcome<'id, P>,
    aggregate: AggregateOutcome<P::BlockAckEvidence, P::AirDenied>,
) -> TxCompleted<'id, P> {
    let _ = (&single, &aggregate);
    #[cfg(ui_reject)]
    return TxCompleted::Single(operation.complete(aggregate)); // ui-error: E0308
    #[cfg(not(ui_reject))]
    TxCompleted::Single(operation.complete(single))
}
