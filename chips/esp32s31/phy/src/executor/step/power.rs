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

/// Clock-free lowering, including progress and the final-record decision.
/// Its value is consumed in this poll, before any Runnable checkpoint.
enum Work {
    Complete,
    Control(crate::tracking::power::ControlBinding),
    Lower(u8),
    Skipped,
    Record(oer_esp32s31_hal::types::PhyGainMemoryEntry, bool),
}
#[inline(never)]
fn prepare(action: Action, state: &PhyState, gain: &mut Cursor) -> Result<Work, Failure> {
    use crate::tracking::parameters::PhyCalibrationTrackClass as Class;
    let (class, channel, gain_base, count, bank) = match action {
        Action::Complete(_) => return Ok(Work::Complete),
        Action::RegenerateWifiGain { channel, gain_base } => {
            (Class::Wifi, channel, gain_base, 32, 0)
        }
        Action::RegenerateBluetoothIeee802154Gain { gain_base } => {
            (Class::BluetoothIeee802154, 0, gain_base, 16, 32)
        }
        action => {
            return crate::tracking::power::ControlBinding::lower(action)
                .map(Work::Control)
                .map_err(|_| Failure::Binding);
        }
    };
    if gain.entry == u8::MAX {
        if let Some(generator) = state.tracking_gain_generator(class, channel, gain_base) {
            gain.generator = generator;
            Ok(Work::Lower(bank))
        } else {
            Ok(Work::Skipped)
        }
    } else {
        let record = gain_record(state, class, gain)?;
        Ok(Work::Record(record, gain.entry + 1 == count))
    }
}

/// One complete gain record, BBPLL edge or full force/settle pair per poll.
/// Dispatch and acknowledgement are shared, independent of the clock type.
#[inline(never)]
pub(super) fn advance<D: crate::executor::wait::PhyShortDelay, C: oer_time::Clock>(
    io: &mut Io<'_, '_>,
    child: &mut PhyParamTrackingTxPowerTransition<'_>,
    gain: &mut Cursor,
    action: Action,
    clock: &C,
    observed_gain: bool,
) -> Result<Step, Failure> {
    let (transition, state) = child.parts();
    let started = observed_gain.then(|| clock.now());
    match prepare(action, state, gain)? {
        Work::Complete => return Ok(Step::Complete),
        Work::Control(binding) => {
            execute::<D>(io, transition, binding)?;
            let next = transition.action();
            if matches!(
                next,
                Action::ForceTxRx(PhyForceTxRxAction::DelayMicros { .. })
            ) {
                let binding = crate::tracking::power::ControlBinding::lower(next)
                    .map_err(|_| Failure::Binding)?;
                execute::<D>(io, transition, binding)?;
            }
        }
        Work::Lower(bank) => {
            gain.memory_base = io.gain_base(action)?.wrapping_add(bank);
            gain.entry = 0;
            elapsed(clock, started, &mut gain.timing.lower_micros);
        }
        Work::Skipped => {
            elapsed(clock, started, &mut gain.timing.lower_micros);
            acknowledge(transition)?;
        }
        Work::Record(record, last) => {
            elapsed(clock, started, &mut gain.timing.arithmetic_micros);
            let published = observed_gain.then(|| clock.now());
            io.gain_entry(record)?;
            elapsed(clock, published, &mut gain.timing.publication_micros);
            gain.entry += 1;
            if last {
                acknowledge(transition)?;
            }
        }
    }
    Ok(Step::Runnable)
}

#[inline(never)]
fn acknowledge(transition: &mut PhyTxPowerTrackingTransition) -> Result<(), Failure> {
    transition.gain_published().map_err(|_| Failure::Binding)
}

#[inline(never)]
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
    // Exactly the quartet from the general seed/output32 view. Both pinned
    // profiles select its seed-only rows; unsupported extra rows fail closed.
    let (seed, config) = state
        .tracking_gain_seed(class, value.gain_64)
        .ok_or(Failure::Binding)?;
    Ok(oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
        gain.memory_base.wrapping_add(gain.entry),
        value.gain_72,
        value.gain_64,
        value.gain_32,
        seed,
        config,
    ))
}

#[inline(never)]
fn execute<D: crate::executor::wait::PhyShortDelay>(
    io: &mut Io<'_, '_>,
    transition: &mut PhyTxPowerTrackingTransition,
    binding: crate::tracking::power::ControlBinding,
) -> Result<(), Failure> {
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
