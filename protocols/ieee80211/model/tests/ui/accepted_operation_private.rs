#![no_std]
use oer_ieee80211_model::{ModelBackend, ModelCaps, ModelPool, ModelPrepared, ModelTickets};
use oer_ieee80211_port::{AdmittedAttempt, Station};
use oer_radio_port::StaticId;

pub enum Declaration {}
type Id = StaticId<Declaration>;
type Backend = ModelBackend<Id>;
type Prepared<'k, 'phase, 'p> = ModelPrepared<'k, 'phase, 'p, Station, ModelCaps, Backend>;
type Submitted<'k, 'phase, 'p> = AdmittedAttempt<'p, ModelTickets, Prepared<'k, 'phase, 'p>, ModelCaps, (), ModelPool<Id, 1>>;
pub fn inspect<'k, 'phase, 'p>(submitted: Submitted<'k, 'phase, 'p>) -> u8 {
    #[cfg(ui_reject)]
    return submitted.parts.operation.ticket().index(); // ui-error: E0616
    #[cfg(not(ui_reject))]
    submitted.operation().ticket().index()
}
