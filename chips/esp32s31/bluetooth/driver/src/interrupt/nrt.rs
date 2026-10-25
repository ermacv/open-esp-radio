//! Bounded default-profile disposition for one NRT source-133 interrupt.
//!
//! The pinned Controller archives define no subscriber of the NRT ISR's
//! linker-assembled broker table. Its complete hard-handler effect is therefore the
//! HAL sample/sample/acknowledge/acknowledge transaction. This
//! module retains only an acknowledged token without assigning Link-Layer
//! names or publishing synthetic work.

use oer_esp32s31_hal::bluetooth::{BluetoothNrtInterruptAcknowledged, InterruptRegistersOwner};

/// One acknowledged NRT epoch for the pinned standalone default profile.
///
/// A future feature-specific policy must consume the epoch before it can
/// publish any Link-Layer work. No raw status image crosses the HAL boundary.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged NRT epoch must be retained or explicitly consumed"]
pub struct NrtDefaultInterruptEpoch {
    _acknowledged: BluetoothNrtInterruptAcknowledged,
}

trait NrtInterruptBackend {
    fn capture_nrt_and_acknowledge(&mut self) -> Option<BluetoothNrtInterruptAcknowledged>;
}

impl NrtInterruptBackend for InterruptRegistersOwner {
    fn capture_nrt_and_acknowledge(&mut self) -> Option<BluetoothNrtInterruptAcknowledged> {
        self.capture_nrt_and_acknowledge()
    }
}

fn execute_nrt_default_interrupt_step(
    backend: &mut impl NrtInterruptBackend,
) -> Option<NrtDefaultInterruptEpoch> {
    backend
        .capture_nrt_and_acknowledge()
        .map(|acknowledged| NrtDefaultInterruptEpoch {
            _acknowledged: acknowledged,
        })
}

/// Capture and acknowledge one source-133 epoch for the default profile;
/// `None` when both snapshots were clear (the shared line raised by another
/// source), with nothing written.
///
/// The function is finite and publishes no scheduler, timer, HCI or
/// Link-Layer event. Supporting a later feature that registers an NRT consumer
/// requires a different typed policy rather than changing this disposition.
pub(crate) fn step_nrt_default_interrupt(
    interrupts: &mut InterruptRegistersOwner,
) -> Option<NrtDefaultInterruptEpoch> {
    execute_nrt_default_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests;
