//! T2/T4 own their state in the shared P1 driver; gain publication has its own poll.
use super::{Failure, Io, Registers, Step};
use crate::{
    analog::pbus::PhyForceTxRxAction,
    state::PhyState,
    tracking::{
        observation::{EngineTiming, GainTiming},
        parameters::{PhyParamTrackingCompletion, PhyParamTrackingTxPowerTransition},
        power::{PhyTxPowerTrackingAction as Action, PhyTxPowerTrackingTransition},
    },
};
use oer_time::Timer;

pin_project_lite::pin_project! {
    struct Power<F> { #[pin] inner: F }
}
impl<F: super::Engine> core::future::Future for Power<F> {
    type Output = (
        Result<PhyParamTrackingCompletion, Failure>,
        EngineTiming,
        GainTiming,
    );
    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        let mut this = self.project();
        match this.inner.as_mut().poll(cx) {
            core::task::Poll::Pending => core::task::Poll::Pending,
            core::task::Poll::Ready(result) => {
                core::task::Poll::Ready(this.inner.finish_power(result))
            }
        }
    }
}

pub(crate) fn run_param_tx_power<
    'a,
    D: crate::executor::wait::PhyShortDelay + 'a,
    R: Registers,
    T: Timer,
>(
    timer: &'a T,
    child: PhyParamTrackingTxPowerTransition<'a>,
    registers: &'a mut R,
    measured: bool,
) -> impl core::future::Future<
    Output = (
        Result<PhyParamTrackingCompletion, Failure>,
        EngineTiming,
        GainTiming,
    ),
> + 'a {
    Power {
        inner: super::driver::<D, _, _>(
            super::Input::Power { child, measured },
            registers,
            timer,
            None,
            None,
        ),
    }
}

/// The state owner keeps generator inputs immutable until terminal commit.
/// No image or pointer into the driver survives a yield.
#[derive(Debug)]
pub(crate) struct Cursor {
    generator: crate::tx::gain::Generator,
    entry: u8,
    memory_base: u8,
    pub timing: GainTiming,
}
impl Default for Cursor {
    fn default() -> Self {
        Self {
            generator: Default::default(),
            entry: u8::MAX,
            memory_base: 0,
            timing: Default::default(),
        }
    }
}

/// One complete gain record, BBPLL edge or full force/settle pair per poll.
#[inline(never)]
pub(super) fn advance<D: crate::executor::wait::PhyShortDelay, C: oer_time::Clock>(
    io: &mut Io<'_, '_>,
    child: &mut PhyParamTrackingTxPowerTransition<'_>,
    gain: &mut Cursor,
    clock: &C,
    measured: bool,
) -> Result<Step, Failure> {
    let (transition, state) = child.parts();
    let action = transition.action();
    if matches!(action, Action::Complete(_)) {
        return Ok(Step::Complete);
    }
    if matches!(
        action,
        Action::RegenerateWifiGain { .. } | Action::RegenerateBluetoothIeee802154Gain { .. }
    ) {
        let request = transition.request();
        let gain_base = match action {
            Action::RegenerateWifiGain { gain_base, .. }
            | Action::RegenerateBluetoothIeee802154Gain { gain_base } => gain_base,
            _ => return Err(Failure::Binding),
        };
        let started = measured.then(|| clock.now());
        let done = if gain.entry == u8::MAX {
            let done = if let Some(generator) =
                state.tracking_gain_generator(request.class, request.wifi_channel, gain_base)
            {
                gain.generator = generator;
                gain.memory_base = io.gain_base(action)?.wrapping_add(
                    if request.class == crate::tracking::parameters::PhyCalibrationTrackClass::Wifi
                    {
                        0
                    } else {
                        32
                    },
                );
                gain.entry = 0;
                false
            } else {
                true
            };
            elapsed(clock, started, &mut gain.timing.lower_micros);
            done
        } else {
            let entry = gain_record(state, request.class, gain)?;
            elapsed(clock, started, &mut gain.timing.arithmetic_micros);
            let published = measured.then(|| clock.now());
            io.gain_entry(entry)?;
            elapsed(clock, published, &mut gain.timing.publication_micros);
            gain.entry += 1;
            gain.entry
                == if request.class == crate::tracking::parameters::PhyCalibrationTrackClass::Wifi {
                    32
                } else {
                    16
                }
        };
        if done {
            use crate::tracking::power::PhyTxPowerTrackingCompletion as Completion;
            transition
                .advance(match action {
                    Action::RegenerateWifiGain { channel, gain_base } => {
                        Completion::WifiGainRegenerated { channel, gain_base }
                    }
                    Action::RegenerateBluetoothIeee802154Gain { gain_base } => {
                        Completion::BluetoothIeee802154GainRegenerated { gain_base }
                    }
                    _ => return Err(Failure::Binding),
                })
                .map_err(|_| Failure::Binding)?;
        }
        return Ok(Step::Runnable);
    }
    execute::<D>(io, transition)?;
    // Configure and its mandatory 1-us minimum remain one indivisible atom.
    // The next configure phase and the gain publisher each get a fresh poll.
    if matches!(
        transition.action(),
        Action::ForceTxRx(PhyForceTxRxAction::DelayMicros { .. })
    ) {
        execute::<D>(io, transition)?;
    }
    Ok(Step::Runnable)
}

fn elapsed<C: oer_time::Clock>(clock: &C, started: Option<oer_time::Instant>, maximum: &mut u16) {
    if let Some(started) = started {
        let micros = clock.now().saturating_duration_since(started).as_micros();
        *maximum = (*maximum).max(u16::try_from(micros).unwrap_or(u16::MAX));
    }
}

#[inline(never)]
fn gain_record(
    state: &PhyState,
    class: crate::tracking::parameters::PhyCalibrationTrackClass,
    gain: &mut Cursor,
) -> Result<oer_esp32s31_hal::types::PhyGainMemoryEntry, Failure> {
    let value = match class {
        crate::tracking::parameters::PhyCalibrationTrackClass::Wifi => {
            crate::channel::next_wifi_tx_gain(&mut gain.generator)
        }
        crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154 => {
            crate::calibration::bluetooth::next_bluetooth_tx_gain(&mut gain.generator, gain.entry)
        }
    };
    let (seed, config) = state.tracking_gain_seed(class);
    // Both recovered profiles select only 0/0x80/0x100 baseband gains. Their
    // quartet is inside seed[0..12], so the unchanged seed/output32 view does
    // not read output32. Whole-image publishers retain the general view;
    // tests verify the profile invariant and compare every encoded record.
    let index = crate::hardware::tx_baseband_gain_index(value.gain_64) * 4;
    if index + 4 > seed.len() * 2 {
        return Err(Failure::Binding);
    }
    Ok(oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
        gain.memory_base.wrapping_add(gain.entry),
        value.gain_72,
        value.gain_64,
        value.gain_32,
        [
            crate::hardware::packed_halfword(&seed, index),
            crate::hardware::packed_halfword(&seed, index + 1),
            crate::hardware::packed_halfword(&seed, index + 2),
            crate::hardware::packed_halfword(&seed, index + 3),
        ],
        config,
    ))
}

fn execute<D: crate::executor::wait::PhyShortDelay>(
    io: &mut Io<'_, '_>,
    transition: &mut PhyTxPowerTrackingTransition,
) -> Result<(), Failure> {
    let binding = crate::tracking::power::ControlBinding::lower(transition.action())
        .map_err(|_| Failure::Binding)?;
    if let Some(micros) = binding.delay_micros()
        && !D::settle_micros(micros)
    {
        #[cfg(target_arch = "riscv32")]
        return Err(Failure::Target(
            crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable,
        ));
        #[cfg(not(target_arch = "riscv32"))]
        return Err(Failure::Binding);
    }
    let completion = io.power(binding)?;
    transition.advance(completion).map_err(|_| Failure::Binding)
}
