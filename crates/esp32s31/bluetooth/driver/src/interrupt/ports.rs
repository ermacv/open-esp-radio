//! Platform storage ports for the published Controller interrupt owners.
//!
//! Implementations (the platform adapter) keep the register owners in stable
//! interrupt-handler storage; the driver only publishes, dispatches through
//! and restores them.

use oer_esp32s31_hal::bluetooth::{InterruptRegistersOwner, ModemLpTimerInterruptReadyOwner};

use super::{NrtDefaultInterruptEpoch, PrimaryInterruptStep};

/// Platform boundary that publishes both disjoint owners in stable ISR slots.
///
/// Implementations must either publish both owners atomically and return one
/// affine lease, or return the storage value and both unchanged owners. This
/// transition must not enable a CPU route; routing is a later lifecycle edge.
pub trait InterruptOwnerStorage: Sized {
    /// Affine proof that both owners remain in the implementation's storage.
    type Published;
    /// Exact pre-publication rejection reason.
    type Error;

    /// Publish both owners without enabling any interrupt source.
    fn publish(
        self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    >;
}

/// Stable platform dispatch over the published shared interrupt owner.
///
/// Implementations must retain the unique primary/NRT register owner in
/// stable storage across every call. Both methods execute exactly one finite
/// Controller disposition and enable no CPU route themselves.
pub trait SharedInterruptDispatchStorage {
    /// Exact reason the shared owner could not service an entry.
    type Error;

    /// Capture, acknowledge and classify one primary source-124 epoch.
    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error>;

    /// Capture and acknowledge one default-profile NRT source-133 epoch.
    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error>;
}

/// Restore initialized register owners into the same lifetime-long ISR reservation.
/// Implementations serialize both slots with route binding, reject live routes or
/// occupied slots without mutation, and never issue another publication lease.
pub trait InterruptOwnerRestartStorage {
    type RestartError;
    fn restore_initialized_interrupt_owners(
        &self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        (),
        (
            Self::RestartError,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    >;
}
