//! Application brownout policy, including the threshold transaction.
//! SOURCE: ESP-IDF 4d59230d `esp_brownout_init`, `brownout_hal_config`.
use crate::{
    Access,
    wait::{BootDeadline, BootWait, WaitFailure},
};
use oer_espressif_analog::{
    AnalogField, AnalogRegisterBus, FieldWrite, Step, TransitionError, ValueTooWide,
};
use oer_time::Clock;

/// The chip's detector cycles, actions and analog threshold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settings<W> {
    pub interrupt_wait: W,
    pub reset_wait: W,
    pub close_flash: bool,
    pub power_down_rf: bool,
    pub reset_enabled: bool,
    pub system_reset: bool,
}

/// One sampled mode-0 word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode0<W> {
    pub settings: Settings<W>,
    pub enabled: bool,
}

pub struct Profile<W> {
    pub settings: Settings<W>,
    pub threshold: u8,
    pub threshold_wait: BootWait,
}

/// A sourced fixed write series, one event write, or one word sample.
pub trait Registers {
    type Wait: Copy + Eq;
    type Address: Copy + Eq + core::fmt::Debug;
    fn configure_mode0(&mut self, access: Access<'_>, settings: Settings<Self::Wait>);
    fn enable(&mut self, access: Access<'_>);
    fn mode0(&self, access: Access<'_>) -> Mode0<Self::Wait>;
    fn mode1_software(&self, access: Access<'_>) -> bool;
    fn mode1_reset(&self, access: Access<'_>) -> bool;
    fn threshold_field() -> AnalogField<Self::Address>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrownoutFailure {
    Threshold(WaitFailure),
    Readback,
    Transition(TransitionError),
    /// A supplied profile threshold does not fit its chip field.
    ThresholdValue(ValueTooWide),
}

/// Configure mode 0, finish the threshold, arm the event and verify policy.
/// The threshold is not read back, matching the original application policy.
pub fn apply<R, B, C>(
    registers: &mut R,
    bus: &mut B,
    profile: Profile<R::Wait>,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BrownoutFailure>
where
    R: Registers,
    B: AnalogRegisterBus<Address = R::Address>,
    C: Clock + ?Sized,
{
    registers.configure_mode0(Access::new(), profile.settings);
    let mut write = FieldWrite::new(bus, R::threshold_field(), profile.threshold)
        .map_err(BrownoutFailure::ThresholdValue)?;
    deadline
        .poll(profile.threshold_wait, || match write.poll() {
            Ok(Step::Ready(())) => Some(Ok(())),
            Ok(Step::Pending) => None,
            Err(error) => Some(Err(BrownoutFailure::Transition(error))),
        })
        .map_err(BrownoutFailure::Threshold)??;
    registers.enable(Access::new());
    // Preserve the original sample and short-circuit read order.
    let mode0 = registers.mode0(Access::new());
    if registers.mode1_software(Access::new())
        && !registers.mode1_reset(Access::new())
        && mode0.enabled
        && mode0.settings == profile.settings
    {
        Ok(())
    } else {
        Err(BrownoutFailure::Readback)
    }
}
