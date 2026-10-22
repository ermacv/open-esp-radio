//! RX-gain program inputs and terminal products, including gain publication.
//! The one retained program uses the pure generated bank encodings below;
//! measured coefficients stay in its owner until terminal handoff.

pub use super::program::RxProgram;

use crate::{
    analog::pbus::PhyPbusForceTest,
    calibration::baseband::{
        PhyGeneratedRxGainTable, PhyRxGainBank, PhyRxGainMemoryParameters,
        generate_phy_rx_gain_table,
    },
    rx::gain_calibration::{PhyRxGainDcFailure, PhyRxGainDcOutcome, PhyRxGainDcParameters},
};

/// Fixed gain encodings depend only on the hardware bank. They are generated
/// once at compile time; measured DC corrections remain in each PHY owner.
pub(super) const fn generated_table(bank: PhyRxGainBank) -> &'static PhyGeneratedRxGainTable {
    static WIFI: PhyGeneratedRxGainTable = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
    static SHARED: PhyGeneratedRxGainTable = generate_phy_rx_gain_table(PhyRxGainBank::Shared);
    match bank {
        PhyRxGainBank::Wifi => &WIFI,
        PhyRxGainBank::Shared => &SHARED,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishFailure {
    PbusTimedOut {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
}

/// Explicit vendor `phy_param` inputs of complete
/// `libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table`, size 650.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitParameters {
    pub dc_calibrated: bool,
    pub tables_initialized: bool,
    pub dc: PhyRxGainDcParameters,
    pub memory: PhyRxGainMemoryParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitOutcome {
    pub dc: Option<PhyRxGainDcOutcome>,
    pub generated_tables: bool,
    pub wifi_last_index: u8,
    pub shared_last_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitFailure {
    Dc(PhyRxGainDcFailure),
    Publish(PhyRxGainPublishFailure),
}
