//! Platform storage ports for the published Controller interrupt owners.
//!
//! Implementations (the platform adapter) keep the register owners in stable
//! interrupt-handler storage; the driver only publishes, dispatches through
//! and restores them.

use super::ControllerInterruptHalf;
use oer_esp32s31_hal::bluetooth::ModemLpTimerInterruptReadyOwner;
use oer_interrupt_table::signal::SignalExecutor;

use super::{NrtDefaultInterruptEpoch, PrimaryInterruptCapture};

/// Platform boundary that publishes both disjoint owners in stable ISR slots.
///
/// Implementations must either publish both owners atomically and return one
/// affine lease, or return the storage value and both unchanged owners. This
/// transition must not enable a CPU route; routing is a later lifecycle edge.
pub trait InterruptOwnerStorage<X: SignalExecutor>: Sized {
    /// Affine proof that both owners remain in the implementation's storage.
    type Published;
    /// Exact pre-publication rejection reason.
    type Error;

    /// Publish both owners without enabling any interrupt source.
    fn publish(
        self,
        interrupts: ControllerInterruptHalf<X>,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            ControllerInterruptHalf<X>,
            ModemLpTimerInterruptReadyOwner,
        ),
    >;
}

/// Stable platform dispatch over the published shared interrupt owner.
///
/// Implementations must retain the unique primary/NRT register owner in
/// stable storage across every call. Every method executes one finite
/// Controller disposition and enable no CPU route themselves.
pub trait SharedInterruptDispatchStorage {
    /// Exact reason the shared owner could not service an entry.
    type Error;

    /// Capture and acknowledge one primary source-124 epoch for the P2 latch.
    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptCapture, Self::Error>;

    /// Capture and acknowledge one default-profile NRT source-133 epoch.
    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error>;

    /// Service one scheduler request after acknowledging the shared doorbell.
    fn service_scheduler_doorbell(&self) -> Result<bool, Self::Error>;
}

/// Restore initialized register owners into the same lifetime-long ISR reservation.
/// Implementations serialize both slots with route binding, reject live routes or
/// occupied slots without mutation, and never issue another publication lease.
pub trait InterruptOwnerRestartStorage<X: SignalExecutor> {
    type RestartError;
    fn restore_initialized_interrupt_owners(
        &self,
        interrupts: ControllerInterruptHalf<X>,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        (),
        (
            Self::RestartError,
            ControllerInterruptHalf<X>,
            ModemLpTimerInterruptReadyOwner,
        ),
    >;
}
