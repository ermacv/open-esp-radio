//! The chip's FROM_CPU software interrupt lines, each with one owner.
//!
//! Owners reach their line only through this module. A line's owner is a
//! [`Line`] variant whose discriminant is the line number, so two owners of
//! one line fail the build (a discriminant is assigned once); a new owner
//! takes a free line here. The image owns only line 0, core 0's executor wake.

use esp_hal::interrupt::software::SoftwareInterrupt;

/// The owner of each line; the discriminant is its FROM_CPU line number.
#[repr(u8)]
pub(crate) enum Line {
    /// Core 0's executor wake.
    Executor0 = 0,
}

/// Core 0's executor wake, from the line's peripheral.
pub(crate) fn executor0(
    line: esp_hal::peripherals::FROM_CPU_INTR0<'static>,
) -> SoftwareInterrupt<'static, { Line::Executor0 as u8 }> {
    SoftwareInterrupt::new(line)
}
