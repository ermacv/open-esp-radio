//! The common low-power clock lifecycle over chip-owned ports.

pub trait LowPowerPort {
    type Guard;
    type Error;
    fn deselect(&mut self);
    fn release(&mut self, guard: Self::Guard) -> Result<(), (Self::Guard, Self::Error)>;
}
pub trait LowPowerAcquirePort: LowPowerPort {
    fn select(&mut self);
    fn acquire(&mut self) -> Result<Self::Guard, Self::Error>;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowPowerError<E> {
    AlreadySelected,
    NotSelected,
    Platform(E),
}
pub fn select_low_power<P: LowPowerAcquirePort>(
    held: &mut Option<P::Guard>,
    port: &mut P,
) -> Result<(), LowPowerError<P::Error>> {
    if held.is_some() {
        return Err(LowPowerError::AlreadySelected);
    }
    port.select();
    *held = Some(port.acquire().map_err(LowPowerError::Platform)?);
    Ok(())
}
pub fn deselect_low_power<P: LowPowerPort>(
    held: &mut Option<P::Guard>,
    port: &mut P,
) -> Result<(), LowPowerError<P::Error>> {
    let Some(guard) = held.take() else {
        return Err(LowPowerError::NotSelected);
    };
    port.deselect();
    if let Err((guard, error)) = port.release(guard) {
        *held = Some(guard);
        return Err(LowPowerError::Platform(error));
    }
    Ok(())
}

/// The chip supplies an opaque config and validated delay; no PAC values cross.
pub trait ExternalPort<C> {
    type Error;
    fn enabled(&mut self, enabled: bool);
    fn clear_priorities(&mut self, config: C);
    fn disable_clock(&mut self) -> Result<(), Self::Error>;
}
pub trait ExternalStartPort<C>: ExternalPort<C> {
    type Delay;
    fn already_active() -> Self::Error;
    fn grant_delay(config: C) -> Result<Self::Delay, Self::Error>;
    fn enable_clock(&mut self) -> Result<(), Self::Error>;
    fn mode(&mut self, config: C);
    fn grant(&mut self, config: C, delay: Self::Delay);
    fn priorities(&mut self, config: C);
}
pub fn start_external<C: Copy, P: ExternalStartPort<C>>(
    held: &mut Option<C>,
    config: C,
    port: &mut P,
) -> Result<(), P::Error> {
    if held.is_some() {
        return Err(P::already_active());
    }
    let delay = P::grant_delay(config)?;
    port.enable_clock()?;
    port.mode(config);
    port.grant(config, delay);
    port.priorities(config);
    port.enabled(true);
    *held = Some(config);
    Ok(())
}
pub fn stop_external<C: Copy, P: ExternalPort<C>>(
    held: &mut Option<C>,
    port: &mut P,
) -> Result<(), P::Error> {
    if let Some(config) = held.take() {
        port.clear_priorities(config);
        port.enabled(false);
    }
    port.disable_clock()
}
