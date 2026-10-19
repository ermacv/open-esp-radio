#![no_std]
use oer_ieee80211_lower_mac::{Completed, InFlight, PhaseTypes, TerminalAccess};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelMpdu, ModelSingleOutcome, ModelTypes};
use oer_radio_port::Slot;
type Kind<'id> = <ModelTypes as PhaseTypes<'id>>::TicketKind;
type Path<'id> = <ModelTypes as PhaseTypes<'id>>::TicketPath;
type Ticket<'id> = Slot<'id, Kind<'id>, Path<'id>>;
type Done<'id, C> = Completed<C, Ticket<'id>, ModelMpdu<'id>, ModelSingleOutcome>;

pub fn swap<'id, C>(
    a: InFlight<C, Ticket<'id>, ModelMpdu<'id>>,
    b: InFlight<C, Ticket<'id>, ModelMpdu<'id>>,
    out: ModelSingleOutcome,
    _access: &mut TerminalAccess<'id, Kind<'id>, Path<'id>>,
) -> (Done<'id, C>, Done<'id, C>) {
    #[cfg(ui_reject)]
    {
        // Even the consumer permission cannot re-admit raw mixed components.
        let a = a.complete(out).into_parts(_access);
        let b = b.complete(out).into_parts(_access);
        (
            InFlight::admit(a.context, b.ticket, b.payload).complete(out), // ui-error: E0624
            InFlight::admit(b.context, a.ticket, a.payload).complete(out), // ui-error: E0624
        )
    }
    #[cfg(not(ui_reject))]
    (a.complete(out), b.complete(out))
}
