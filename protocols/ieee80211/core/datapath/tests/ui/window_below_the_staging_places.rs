#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{Arenas, Engine, EngineAgreements};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckStorage;
use oer_protocols_ieee80211_testing_model::{MODEL_CHAIN_SLOTS, MODEL_AGREEMENTS, MODEL_GROUPS, ModelCaps, ModelPort, StationModel};

pub type Station<const WINDOW: usize> = Engine<
    'static,
    ModelPort<'static, StationModel>,
    Arenas,
    (),
    1,
    8,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
    MODEL_CHAIN_SLOTS,
    WINDOW,
>;
pub type Agreements<const FRAMES: usize, const WINDOW: usize> =
    RxBlockAckStorage<EngineAgreements<'static, oer_protocols_ieee80211_contracts_port::Station, ModelCaps>, u8, 1, FRAMES, WINDOW>;

// The model port stages into eight places: a window of seven leaves one free.
#[cfg(not(ui_reject))]
pub const FITS: oer_protocols_ieee80211_core_datapath::engine::EngineFits = Station::<7>::GEOMETRY;
#[cfg(ui_reject)]
pub const FITS: oer_protocols_ieee80211_core_datapath::engine::EngineFits = Station::<8>::GEOMETRY;
// ui-error@outside: E0080: evaluation panicked: the receive window is below the port's staging places: WINDOW < STAGING
