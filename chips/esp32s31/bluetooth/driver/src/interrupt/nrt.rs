//! Bounded default-profile disposition for one NRT source-133 interrupt.
//!
//! The pinned Controller archives define no subscriber of the NRT ISR's
//! linker-assembled broker table. Its complete hard-handler effect is therefore the
//! HAL sample/sample/acknowledge/acknowledge transaction. A sampled primary
//! overflow clear lane instead returns a fault before either acknowledgement.
//! This module retains only an acknowledged token without assigning Link-Layer
//! names or publishing synthetic work.

use oer_esp32s31_hal::bluetooth::{
    BluetoothNrtInterruptAcknowledged, BluetoothNrtInterruptFault, InterruptRegistersOwner,
};

/// One acknowledged NRT epoch for the pinned standalone default profile.
///
/// A future feature-specific policy must consume the epoch before it can
/// publish any Link-Layer work. A successful acknowledgement exposes no raw
/// status image.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged NRT epoch must be retained or explicitly consumed"]
pub struct NrtDefaultInterruptEpoch {
    _acknowledged: BluetoothNrtInterruptAcknowledged,
}

trait NrtInterruptBackend {
    fn capture_nrt_and_acknowledge(
        &mut self,
    ) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault>;
}

impl NrtInterruptBackend for InterruptRegistersOwner {
    fn capture_nrt_and_acknowledge(
        &mut self,
    ) -> Result<Option<BluetoothNrtInterruptAcknowledged>, BluetoothNrtInterruptFault> {
        self.capture_nrt_and_acknowledge()
    }
}

fn execute_nrt_default_interrupt_step(
    backend: &mut impl NrtInterruptBackend,
) -> Result<Option<NrtDefaultInterruptEpoch>, BluetoothNrtInterruptFault> {
    Ok(backend
        .capture_nrt_and_acknowledge()?
        .map(|acknowledged| NrtDefaultInterruptEpoch {
            _acknowledged: acknowledged,
        }))
}

/// Capture and acknowledge one source-133 epoch for the default profile;
/// `Ok(None)` when both snapshots were clear (the shared line raised by another
/// source), with nothing written.
///
/// An unexpected primary overflow clear lane preserves the exact sampled
/// pair in a fault, leaving both banks unacknowledged for gate quarantine.
///
/// The function is finite and publishes no scheduler, timer, HCI or
/// Link-Layer event. Supporting a later feature that registers an NRT consumer
/// requires a different typed policy rather than changing this disposition.
pub(crate) fn step_nrt_default_interrupt(
    interrupts: &mut InterruptRegistersOwner,
) -> Result<Option<NrtDefaultInterruptEpoch>, BluetoothNrtInterruptFault> {
    execute_nrt_default_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests;
