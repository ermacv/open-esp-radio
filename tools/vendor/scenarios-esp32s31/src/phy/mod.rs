//! ESP32-S31 PHY scenarios over the engine's PHY session
//! (`oer_vendor_scenario::phy`) and [`crate::layout::PHY_LAYOUT`].

pub mod calibration_leaves;
pub mod calibration_prefix;
pub mod channel;
pub use oer_esp32s31_knowledge::phy_relation::committed;
pub mod gain;
pub mod gain_state;
pub mod i2c;
pub mod i2c_transport;
pub mod rfpll;
pub mod rx_gain;
pub mod tracking;
pub mod tracking_graph;
pub mod tx_dc;
pub use oer_vendor_scenario::phy::*;

/// Prepare the route's physical temperature owner outside the retained leaf.
/// The vendor leaf assumes RF initialization already acquired these resources.
/// This setup runs the real HAL acquisition; no restore state is fabricated.
pub(crate) fn prepare_calibration(
    image: &PhyImage,
    fill: u8,
) -> crate::harness::Result<oer_analysis_blobray_domain::Invocation> {
    use crate::layout::{fill_word, register_bank};
    // SOURCE: registers/types/{pmu,lp-peri-clkrst,tsens}.toml;
    // HAL phy/temperature.rs::acquire touches these three resource words.
    let model = register_bank(
        "temperature-owner",
        "retained TSENS/SAR-I2C resource words before calibration admission",
        [0x2070_4208, 0x2071_0030, 0x2081_8000]
            .map(|address| (address, fill_word(fill)))
            .to_vec(),
    );
    let probe = image.probes.invoke(
        "open_phy_validation_arbiter_init",
        vec![],
        vec![model],
        vec![],
    )?;
    let mut phase = image.enter_probe(probe);
    phase.calls = delay_calls("temperature-owner-reset", image.sym(2, "ets_delay_us"));
    Ok(phase)
}
