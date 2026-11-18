#![no_std]
use oer_protocols_ieee80211_core_datapath::engine::{Arenas, Engine, EngineAgreements};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckStorage;
use oer_protocols_ieee80211_testing_model::{MODEL_AGREEMENTS, MODEL_GROUPS, ModelCaps, ModelPort};

pub type Station<const WINDOW: usize> = Engine<
    'static,
    ModelPort<'static, 8, 0>,
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
    RxBlockAckStorage<EngineAgreements<'static, ModelCaps>, u8, 1, FRAMES, WINDOW>;

// The model's capabilities name a window of at most 64.
#[cfg(not(ui_reject))]
pub const FITS: () = Agreements::<64, 64>::GEOMETRY;
#[cfg(ui_reject)]
pub const FITS: () = Agreements::<65, 65>::GEOMETRY;
// ui-error@outside: E0080: evaluation panicked: the receive window is 1 to the capabilities' rx_block_ack_max_window
