//! Shared exact arithmetic leaves from the ESP32-S31 rev0 PHY ROM.
//!
//! These functions are kept separate from protocol state machines because
//! they neither access hardware nor retain vendor parameter-image state.

/// Exact non-I/O arithmetic of complete rev0 ROM `phy_abs_temp` at
/// `0x2f82_5fa2`.
///
/// The RV32 sequence computes a wrapping absolute value, so `i32::MIN`
/// remains `0x8000_0000` rather than trapping or saturating.
pub const fn absolute_temperature(value: i32) -> u32 {
    value.wrapping_abs() as u32
}

/// Exact signed bounds selection of complete rev0 ROM `phy_get_data_sat` at
/// `0x2f82_6024`.
///
/// Vendor callers pass `(value, upper, lower)`. The comparisons deliberately
/// retain ROM order, including its deterministic behavior for inverted
/// bounds: an input above `upper` selects `upper` before `lower` is examined.
pub const fn saturate_signed(value: i32, upper: i32, lower: i32) -> i32 {
    if upper < value {
        upper
    } else if value >= lower {
        value
    } else {
        lower
    }
}

#[cfg(test)]
mod tests;
