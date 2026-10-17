#![no_std]
use oer_ieee80211_lower_mac::{
    InFlight, PhaseAggregateOutcome, PhaseSingleOutcome, PhaseTypes, TxCompleted,
};
use oer_radio_port::Slot;

type Ticket<'id, P> =
    Slot<'id, <P as PhaseTypes<'id>>::TicketKind, <P as PhaseTypes<'id>>::TicketPath>;

pub fn finish<'id, P: PhaseTypes<'id>>(
    operation: InFlight<P::Context, Ticket<'id, P>, P::TxMpdu>,
    single: PhaseSingleOutcome<'id, P>,
    aggregate: PhaseAggregateOutcome<'id, P>,
) -> TxCompleted<'id, P> {
    let _ = (&single, &aggregate);
    #[cfg(ui_reject)]
    return TxCompleted::Single(operation.complete(aggregate)); // ui-error: E0308
    #[cfg(not(ui_reject))]
    TxCompleted::Single(operation.complete(single))
}
