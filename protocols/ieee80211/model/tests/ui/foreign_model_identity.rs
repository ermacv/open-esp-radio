#![no_std]
use oer_ieee80211_model::{ModelBackend, ModelBuffer, ModelSmall};
use oer_radio_port::StaticId;

pub enum First {}
pub enum Second {}
type Source = ModelBackend<StaticId<First>>;
#[cfg(ui_reject)]
type Destination = ModelBackend<StaticId<Second>>;
#[cfg(not(ui_reject))]
type Destination = Source;

pub fn transfer<'p>(buffer: ModelBuffer<'p, Source, ModelSmall>) -> ModelBuffer<'p, Destination, ModelSmall> {
    buffer // ui-error: E0308
}
