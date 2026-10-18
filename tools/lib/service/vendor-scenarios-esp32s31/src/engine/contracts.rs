//! Reviewed effect contracts and output projections of PHY comparisons.
//! The engine's rules make the analog I2C transport's polling ignored
//! plumbing and compare every other MMIO, fence and delay effect exactly;
//! each reviewed `phy_param` field compares with its production output
//! location.
use crate::layout::{OUTPUT, PHY_PARAM_BYTES};
use blobray_domain::{
    CallEndpoint, FieldLocation, LayoutDomain, LayoutEndpoint, LayoutField, LayoutProjection,
};

pub use crate::phy::committed::OutputField;
pub use oer_vendor_scenario_engine::phy::contracts::*;

/// Final-state projection from the vendor `phy_param` at `parameter` to the
/// production output at `OUTPUT`. Output bytes outside `fields` are not claimed.
pub fn output_projection(
    vendor: CallEndpoint,
    parameter: u32,
    replacement: CallEndpoint,
    output_bytes: u32,
    fields: &[OutputField],
    applicability: &str,
) -> LayoutProjection {
    let endpoint = |entry, address, length| LayoutEndpoint {
        entry,
        domains: vec![LayoutDomain { address, length }],
    };
    let location = |offset| FieldLocation { domain: 0, offset };
    LayoutProjection {
        vendor: endpoint(vendor, parameter, PHY_PARAM_BYTES),
        replacement: endpoint(replacement, OUTPUT, output_bytes),
        fields: fields
            .iter()
            .map(|f| LayoutField {
                name: f.name.into(),
                vendor: location(f.parameter),
                replacement: location(f.output),
                width: f.width,
                count: f.count,
                final_state: true,
                timeline: false,
            })
            .collect(),
        branches: vec![],
        applicability: applicability.into(),
        reason: "the production output publishes the vendor's committed phy_param state".into(),
    }
}
