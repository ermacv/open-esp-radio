#![no_std]
use oer_ieee80211_model::{ModelBackend, ModelCaps, ModelMpdu, ModelPool, ModelPrepared, ModelTickets, ModelTxExecution};
use oer_ieee80211_port::{Admission, AdmissionOwners, PreparedTx, Station};
use oer_radio_port::{Slot, StaticId};

pub enum Declaration {}
type Id = StaticId<Declaration>;
type Backend = ModelBackend<Id>;
type Source<'k, 'phase, 'p> = ModelPrepared<'k, 'phase, 'p, Station, ModelCaps, Backend>;
#[cfg(ui_reject)]
type Other<'k, 'phase, 'p> = ModelPrepared<'k, 'phase, 'p, oer_ieee80211_port::AccessPoint, ModelCaps, Backend>;
#[cfg(not(ui_reject))]
type Other<'k, 'phase, 'p> = Source<'k, 'phase, 'p>;

pub fn forward<'k, 'phase, 'p>(source: Source<'k, 'phase, 'p>, ticket: Slot<'p, ModelTickets, ModelPool<Id, 1>>, authority: Admission<Other<'k, 'phase, 'p>>) -> AdmissionOwners<(), Slot<'static, ModelTickets, ModelPool<Id, 1>>, ModelMpdu<'static, Backend>, ModelTxExecution> {
    source.into_admission((), ticket, authority) // ui-error: E0308
}
