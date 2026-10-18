//! The production projection of the calibration this image published as
//! its startup artifact: the reviewed `phy_param` relation's calibration
//! words, parent words and committed state words of the retained
//! calibration, which the host's vendor calibration cross-check reads in
//! windows (`phy/calibration-projection/read`) instead of replaying the
//! calibration through a host build of the PHY.

use core::cell::RefCell;

use critical_section::Mutex;
use oer_esp32s31_phy_relation::projection::{
    CALIBRATION_WORDS, PARENT_WORDS, snapshot_calibration, snapshot_committed, snapshot_parent,
};
use oer_hil_protocol::{
    base::RejectReason,
    phy::{
        PHY_CALIBRATION_PROJECTION_WORDS, PhyCalibrationProjectionRequest,
        PhyCalibrationProjectionWords,
    },
};

/// Committed state words after the calibration projection.
const COMMITTED_WORDS: usize = 8;
/// Every word of the projection.
const WORDS: usize = CALIBRATION_WORDS + PARENT_WORDS + COMMITTED_WORDS;
/// Tracking progress of a fresh registration; the relation excludes the
/// progress field from the calibration comparison.
const NO_PROGRESS: u16 = 0;

static PROJECTION: Mutex<RefCell<Option<[u16; WORDS]>>> = Mutex::new(RefCell::new(None));

/// Project the retained calibration `cache` the image publishes; a cache
/// that does not replay leaves no projection, which a read reports.
pub(crate) fn record(cache: &oer_esp32s31_phy::PhyCalibrationCache) {
    let projection = oer_esp32s31_phy::validation::retained_calibration_state(*cache.snapshot())
        .ok()
        .map(|state| {
            let mut calibration = [0; CALIBRATION_WORDS];
            let mut parent = [0; PARENT_WORDS];
            let mut committed = [0; COMMITTED_WORDS];
            snapshot_calibration(&state, &mut calibration);
            snapshot_parent(&state, &mut parent);
            snapshot_committed(&state, NO_PROGRESS, &mut committed);
            let mut words = [0; WORDS];
            for (word, value) in words
                .iter_mut()
                .zip(calibration.iter().chain(&parent).chain(&committed))
            {
                *word = *value;
            }
            words
        });
    critical_section::with(|cs| *PROJECTION.borrow_ref_mut(cs) = projection);
}

/// The requested window, or a rejection: no projection yet, or a window
/// outside it or wider than one reply.
pub(crate) fn read(
    request: PhyCalibrationProjectionRequest,
) -> Result<oer_hil_protocol::phy::CalibrationProjectionWords, RejectReason> {
    let Some(words) = critical_section::with(|cs| *PROJECTION.borrow_ref(cs)) else {
        return Err(RejectReason::InvalidState);
    };
    let first = usize::from(request.first);
    let count = usize::from(request.count);
    if count > PHY_CALIBRATION_PROJECTION_WORDS || first + count > WORDS {
        return Err(RejectReason::InvalidConfiguration);
    }
    Ok(oer_hil_protocol::phy::CalibrationProjectionWords(
        PhyCalibrationProjectionWords {
            first: request.first,
            length: WORDS as u16,
            values: words[first..first + count].iter().copied().collect(),
        },
    ))
}
