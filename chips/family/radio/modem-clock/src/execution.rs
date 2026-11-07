//! Ordered execution of prepared modem edges over a chip's register port.
use super::{
    ModemClockDependency, ModemClockLease, ModemClockPlanner, PoisonedModemClockAcquire,
    PoisonedModemClockRelease, PreparedModemClockAcquire, PreparedModemClockRelease,
};

/// The chip profile binds a dependency to one physical action.
pub enum Action<Device, Clock> {
    Device(Device),
    Platform(Clock),
    /// Acquire upstream before opening the device; close it before release.
    Source {
        clock: Clock,
        device: Device,
    },
}

/// Register atoms and held platform references for modem release.
pub trait RegisterPort<D: ModemClockDependency> {
    type Device;
    type Clock;
    type Error;
    fn action(dependency: D) -> Action<Self::Device, Self::Clock>;
    fn configure_device(&mut self, device: Self::Device, enabled: bool);
    fn release_clock(&mut self, clock: Self::Clock) -> Result<(), Self::Error>;
}

/// Acquisition additionally borrows the platform's provider.
pub trait AcquirePort<D: ModemClockDependency>: RegisterPort<D> {
    fn prepare_power_state(&mut self);
    fn acquire_clock(&mut self, clock: Self::Clock) -> Result<(), Self::Error>;
}

/// Publish the common maps, execute low-bit-first edges and commit only success.
#[allow(
    clippy::result_large_err,
    reason = "poison retains the allocation-free planner"
)]
pub fn acquire<D: ModemClockDependency, P: AcquirePort<D>>(
    prepared: PreparedModemClockAcquire<D>,
    port: &mut P,
) -> Result<(ModemClockPlanner<D>, ModemClockLease<D>), PoisonedModemClockAcquire<D>> {
    port.prepare_power_state();
    super::execute_acquire(prepared, |dependency| {
        match <P as RegisterPort<D>>::action(dependency) {
            Action::Device(device) => port.configure_device(device, true),
            Action::Platform(clock) => port.acquire_clock(clock)?,
            Action::Source { clock, device } => {
                port.acquire_clock(clock)?;
                port.configure_device(device, true);
            }
        }
        Ok::<(), <P as RegisterPort<D>>::Error>(())
    })
}

/// Close a device before its source; a refused edge retains the poisoned owner.
#[allow(
    clippy::result_large_err,
    reason = "poison retains planner and lease without allocation"
)]
pub fn release<D: ModemClockDependency, P: RegisterPort<D>>(
    prepared: PreparedModemClockRelease<D>,
    port: &mut P,
) -> Result<ModemClockPlanner<D>, PoisonedModemClockRelease<D>> {
    super::execute_release(prepared, |dependency| {
        match <P as RegisterPort<D>>::action(dependency) {
            Action::Device(device) => port.configure_device(device, false),
            Action::Platform(clock) => port.release_clock(clock)?,
            Action::Source { clock, device } => {
                port.configure_device(device, false);
                port.release_clock(clock)?;
            }
        }
        Ok::<(), <P as RegisterPort<D>>::Error>(())
    })
}

#[cfg(test)]
mod tests;
