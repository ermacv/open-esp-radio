#![no_std]
use oer_protocols_ieee80211_contracts_port::{Attempt, DataPort, PortTypes, Station};

type Prepared<'k, 'p, 'id, P> = <<P as DataPort<'id>>::Types as PortTypes<'id>>::Prepared<
    'k,
    'p,
    Station,
    <P as DataPort<'id>>::Caps,
>;

pub fn submit<'k, 'p, 'id: 'k + 'p, P: DataPort<'id>>(
    port: &P,
    attempt: Attempt<
        'id,
        <P::Types as PortTypes<'id>>::TicketKind,
        Prepared<'k, 'p, 'id, P>,
        P::Caps,
        <P::Types as PortTypes<'id>>::Context,
        <P::Types as PortTypes<'id>>::TicketPath,
    >,
) -> P::Admitted {
    #[cfg(ui_reject)]
    return port.admit(attempt); // ui-error: E0308
    #[cfg(not(ui_reject))]
    port.admit(attempt.admit())
}
