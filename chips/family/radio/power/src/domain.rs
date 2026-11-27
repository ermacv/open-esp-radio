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
