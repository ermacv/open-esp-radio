//! One SAR/PWDET conversion, in the task of its caller, lent the registers.
use crate::executor::wait::{Kind, PhyShortDelay, WaitTally, delay};
use oer_portable_clock::Timer;
use oer_portable_settle::SettleCounter;
use oer_portable_time::Duration;

// MEASUREMENT: s31-a run 1792521908966-002e2465, 21,512 conversions,
// 100 forced tracking ticks, post-2-us-settle ready observations <= 20 us.
// Twice that maximum bounds only not-ready samples, never late ready samples.
pub(crate) const READY_LIMIT: Duration = Duration::from_micros(40);
const SAMPLE_INTERVAL_MICROS: u64 = 1;
const ARM_SETTLE_MICROS: u64 = 1;
const TRIGGER_SETTLE_MICROS: u64 = 2;
/// The two settles and the readiness bound.
const BOUND: Duration =
    Duration::from_micros(ARM_SETTLE_MICROS + TRIGGER_SETTLE_MICROS + READY_LIMIT.as_micros());

pub(crate) trait Io {
    fn arm_tone(&mut self);
    fn trigger(&mut self);
    fn ready(&mut self) -> bool;
    fn clear_tone(&mut self);
    fn sample(&mut self) -> u16;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Readiness {
    pub elapsed_micros: u64,
    pub observations: u16,
    pub ready: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    pub value: u16,
    pub readiness: Readiness,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotReady(Readiness),
    DeadlineOverflow,
}
/// One conversion, lent the registers it reads and the timer of its task:
/// the tone armed, a named settle of at least 1 µs, the SAR triggered, a
/// named settle of at least 2 µs, then readiness read and, while it is not
/// ready and the bound of [`READY_LIMIT`] from the second settle's end is
/// not past, a wait of 1 µs on the timer before the next read. Each read is
/// made before the bound is compared, so a late wake that finds the
/// conversion finished succeeds; an unready end reads no result.
///
/// SOURCE: rev0 ROM `phy_pwdet_tone_start`: tone arm, minimum 1 µs, SAR
/// trigger, minimum 2 µs, ready observation, optional clear, result read.
/// HW17 (`research/hw-sar-hold.md`) qualifies retained ready/result
/// through 100 µs, so a read later than readiness loses nothing.
///
/// # Errors
///
/// [`Error::NotReady`] at the bound, with what was observed;
/// [`Error::DeadlineOverflow`] before any edge when the bound is past the
/// timer's range.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) async fn convert<D: PhyShortDelay, T: Timer + SettleCounter + WaitTally + ?Sized>(
    io: &mut impl Io,
    timer: &T,
    clear_tone: bool,
) -> Result<Sample, Error> {
    // The whole bound is checked before the first edge.
    if timer.now().checked_add(BOUND).is_none() {
        return Err(Error::DeadlineOverflow);
    }
    io.arm_tone();
    settle::<D, T>(timer, ARM_SETTLE_MICROS).await?;
    io.trigger();
    settle::<D, T>(timer, TRIGGER_SETTLE_MICROS).await?;
    let started = timer.now();
    let deadline = started
        .checked_add(READY_LIMIT)
        .ok_or(Error::DeadlineOverflow)?;
    let mut observations = 0_u16;
    loop {
        // `now` precedes this read: a wake after it must not expire a
        // conversion the read had not yet seen finish.
        let now = timer.now();
        let ready = io.ready();
        observations = observations.saturating_add(1);
        let readiness = Readiness {
            elapsed_micros: timer.now().saturating_duration_since(started).as_micros(),
            observations,
            ready,
        };
        if ready {
            if clear_tone {
                io.clear_tone();
            }
            let value = io.sample();
            return Ok(Sample { value, readiness });
        }
        if now >= deadline {
            return Err(Error::NotReady(readiness));
        }
        delay::<D, T>(timer, Kind::Completion, SAMPLE_INTERVAL_MICROS)
            .await
            .map_err(|_| Error::DeadlineOverflow)?;
    }
}

/// A named settle of `micros`, which `D` completes synchronously.
#[cfg(any(target_arch = "riscv32", test))]
async fn settle<D: PhyShortDelay, T: Timer + SettleCounter + WaitTally + ?Sized>(
    timer: &T,
    micros: u64,
) -> Result<(), Error> {
    delay::<D, T>(timer, Kind::Settle, micros)
        .await
        .map_err(|_| Error::DeadlineOverflow)
}

#[cfg(target_arch = "riscv32")]
impl Io for oer_chips_esp32s31_hal::radio::domain::PhyHardware<'_> {
    fn arm_tone(&mut self) {
        crate::hardware::arm_phy_power_detector_tone(self);
    }
    fn trigger(&mut self) {
        oer_chips_esp32s31_hal::radio::phy::power_detector::trigger_sar(self);
    }
    fn ready(&mut self) -> bool {
        oer_chips_esp32s31_hal::radio::phy::power_detector::sample_ready(self)
    }
    fn clear_tone(&mut self) {
        crate::hardware::clear_phy_power_detector_tone_arm(self);
    }
    fn sample(&mut self) -> u16 {
        oer_chips_esp32s31_hal::radio::phy::power_detector::sample_sar(self)
    }
}
#[cfg(test)]
mod tests;
