#![no_std]
use oer_ieee80211_lower_mac::{InFlight, PhaseTypes, TxCompleted};
use oer_ieee80211_runtime::lower_mac_owner::model::{
    ModelAggregateOutcome, ModelSingleOutcome, ModelSubframes, ModelTypes,
};
use oer_radio_port::Slot;

type Ticket<'id> = Slot<
    'id,
    <ModelTypes as PhaseTypes<'id>>::TicketKind,
    <ModelTypes as PhaseTypes<'id>>::TicketPath,
>;
pub fn finish<'id>(
    operation: InFlight<(), Ticket<'id>, ModelSubframes<'id>>,
    outcome: ModelAggregateOutcome,
    _single: ModelSingleOutcome,
    _access: &mut oer_ieee80211_lower_mac::TerminalAccess<'id, <ModelTypes as PhaseTypes<'id>>::TicketKind, <ModelTypes as PhaseTypes<'id>>::TicketPath>,
) -> TxCompleted<'id, ModelTypes> {
    #[cfg(ui_reject)]
    {
        let parts = operation.complete(outcome).into_parts(_access);
        let one = parts.payload.into_iter().next().unwrap();
        TxCompleted::Single(InFlight::admit(parts.context, parts.ticket, one).complete(_single)) // ui-error: E0624
    }
    #[cfg(not(ui_reject))]
    TxCompleted::Aggregate(operation.complete(outcome))
}
