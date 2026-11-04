#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    Attempt, Protection, Station, SupportedPower, TxOperation, TxPower,
};
use oer_protocols_ieee80211_testing_model::{
    ModelBackend, ModelCaps, ModelPool, ModelPrepared, ModelTickets,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_radio_port::{Slot, StaticId};

pub enum Source {}
pub enum Target {}
type Prepared<'k, 'p, 'a> = ModelPrepared<'k, 'p, 'a, Station, ModelCaps, ModelBackend<StaticId<Source>>>;

// An MPDU prepared in one installation seals only with that installation's
// ticket.
pub fn seal<'k, 'p, 'a, 'b>(
    prepared: Prepared<'k, 'p, 'a>,
    own: Slot<'a, ModelTickets, ModelPool<StaticId<Source>, 1>>,
    foreign: Slot<'b, ModelTickets, ModelPool<StaticId<Target>, 1>>,
) {
    #[cfg(ui_reject)]
    let ticket = foreign;
    #[cfg(not(ui_reject))]
    let (ticket, _) = (own, foreign);
    let attempt = Attempt {
        context: (),
        ticket,
        source: prepared,
        access_category: WmmAccessCategory::BestEffort,
        protection: Protection::None,
        power: SupportedPower::<ModelCaps>::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
    };
    drop(attempt.admit()); // ui-error: E0599
}
