//! Compile-time SRAM budgets for the largest PHY transitions.

use core::mem::size_of;

use crate::{
    PhyCalibrationCache, PhyState, RegisteredPhyState,
    calibration::{baseband::PhyBbInitTransition, registration::PhyRegisterTransition},
    rx::{
        gain::{PhyRxGainInitExternalBinding, PhyRxGainInitTransition, PhyRxGainPublishTransition},
        gain_calibration::{PhyRxDcCalibrationExternalBinding, PhyRxGainDcExternalBinding},
    },
};

// These are reviewed RV32 budgets, rounded above the 1.97.1 layouts rather
// than snapshots of compiler-selected padding. A transition that crosses a
// boundary must be split or receive an explicit SRAM-budget review.
//
// Registration briefly owns both the live semantic state and its typed
// calibration snapshot. These ceilings are rounded above the reviewed RV32
// layouts; they prevent a return to an opaque 508-byte state plus a 524-byte
// duplicate without making compiler padding part of the API.
const PHY_STATE_LIMIT: usize = 384;
// The shared domain retains the registration plus the bounded client
// scheduler.
const CONCURRENT_PHY_LIMIT: usize = 448;
const PHY_CALIBRATION_CACHE_LIMIT: usize = 320;
const PHY_REGISTER_TRANSITION_LIMIT: usize = 2_560;
const PHY_BB_INIT_TRANSITION_LIMIT: usize = 1_600;
const PHY_RX_GAIN_INIT_TRANSITION_LIMIT: usize = 512;
const PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT: usize = 128;

// Hardware commands cross hot synchronous/async call boundaries. Terminal
// coefficient arrays must not enlarge their ABI or future storage.
const RX_EXTERNAL_BINDING_LIMIT: usize = 32;

const _: () = {
    assert!(size_of::<PhyRxGainInitExternalBinding>() <= RX_EXTERNAL_BINDING_LIMIT);
    assert!(size_of::<PhyRxGainDcExternalBinding>() <= RX_EXTERNAL_BINDING_LIMIT);
    assert!(size_of::<PhyRxDcCalibrationExternalBinding>() <= RX_EXTERNAL_BINDING_LIMIT);
    assert!(size_of::<PhyState>() <= PHY_STATE_LIMIT);
    assert!(size_of::<RegisteredPhyState>() <= PHY_STATE_LIMIT);
    assert!(size_of::<crate::concurrent::ConcurrentPhy>() <= CONCURRENT_PHY_LIMIT);
    assert!(size_of::<PhyCalibrationCache>() <= PHY_CALIBRATION_CACHE_LIMIT);
    assert!(size_of::<PhyRegisterTransition>() <= PHY_REGISTER_TRANSITION_LIMIT);
    assert!(size_of::<PhyBbInitTransition>() <= PHY_BB_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainInitTransition>() <= PHY_RX_GAIN_INIT_TRANSITION_LIMIT);
    assert!(size_of::<PhyRxGainPublishTransition>() <= PHY_RX_GAIN_PUBLISH_TRANSITION_LIMIT);
};
