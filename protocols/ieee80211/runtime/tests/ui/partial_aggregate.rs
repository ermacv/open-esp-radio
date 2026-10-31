#![no_std]
use oer_ieee80211_contracts_port::{InFlight, PhaseTypes, TxCompleted};
use oer_ieee80211_testing_model::{
    ModelAggregateOutcome, ModelSingleOutcome, ModelSubframes, ModelTypes,
};
use oer_radio_port::Slot;

type Ticket<'id> = Slot<
    'id,
    <ModelTypes<Composition> as PhaseTypes<'id>>::TicketKind,
    <ModelTypes<Composition> as PhaseTypes<'id>>::TicketPath,
>;
pub fn finish<'id>(
    operation: InFlight<(), Ticket<'id>, ModelSubframes<'id, Composition>>,
    outcome: ModelAggregateOutcome,
    _single: ModelSingleOutcome,
    _access: &mut oer_ieee80211_contracts_port::TerminalAccess<'id, <ModelTypes<Composition> as PhaseTypes<'id>>::TicketKind, <ModelTypes<Composition> as PhaseTypes<'id>>::TicketPath>,
) -> TxCompleted<'id, ModelTypes<Composition>> {
    #[cfg(ui_reject)]
    {
        let parts = operation.complete(outcome).into_parts(_access);
        let one = parts.payload.into_iter().next().unwrap();
        TxCompleted::Single(InFlight::admit(parts.context, parts.ticket, one).complete(_single)) // ui-error: E0624
    }
    #[cfg(not(ui_reject))]
    TxCompleted::Aggregate(operation.complete(outcome))
}

pub enum UiSlot {}
type Composition = oer_ieee80211_testing_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
