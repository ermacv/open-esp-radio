#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{Arenas, Engine, EngineAgreements};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckStorage;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, ModelCaps, ModelPort, StationModel};

pub type Station<const WINDOW: usize> = Engine<
    'static,
    ModelPort<'static, StationModel<8>>,
    Arenas,
    (),
    1,
    8,
    8,
    MODEL_GROUPS,
    MODEL_AGREEMENTS,
    WINDOW,
>;
pub type Agreements<const FRAMES: usize, const WINDOW: usize> =
    RxBlockAckStorage<EngineAgreements<'static, oer_protocols_ieee80211_contracts_port::Station, ModelCaps>, u8, 1, FRAMES, WINDOW>;

// The retained frames hold a whole window.
#[cfg(not(ui_reject))]
pub const FITS: oer_protocols_ieee80211_core_upper_mac::block_ack::WindowFits = Agreements::<16, 16>::GEOMETRY;
#[cfg(ui_reject)]
pub const FITS: oer_protocols_ieee80211_core_upper_mac::block_ack::WindowFits = Agreements::<15, 16>::GEOMETRY;
// ui-error@outside: E0080: evaluation panicked: the retained frames hold a whole window: WINDOW <= FRAMES
