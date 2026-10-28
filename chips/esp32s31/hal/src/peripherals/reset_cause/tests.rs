//! The reset-cause words over host memory.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::instances;

use super::{ResetCause, ResetCauseRegisters, ResetCauses};
use crate::platform::{BOOT_HART, Hart};

/// Host memory of `bytes` bytes, each word `fill`.
fn memory(bytes: usize, fill: u32) -> Vec<u32> {
    vec![fill; bytes.div_ceil(4)]
}

#[test]
fn reset_causes_decode_each_core_with_the_chip_table() {
    let mut words = memory(instances::lp_aon_clkrst::SIZE, 0b111);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(
        causes.of(BOOT_HART),
        ResetCause::CoreSoftware
    );
    assert_eq!(
        causes.of(Hart::SECONDARIES[0].hart()),
        ResetCause::CoreSoftware
    );
    let mut words = memory(instances::lp_aon_clkrst::SIZE, u32::MAX);
    let causes = ResetCauses::read(
        ResetCauseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    assert_eq!(
        causes.of(BOOT_HART),
        ResetCause::Other(0x3f)
    );
}
