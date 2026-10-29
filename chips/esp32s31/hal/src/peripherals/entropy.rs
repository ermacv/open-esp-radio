//! Entropy from the true random number generator, which stage two only
//! reads.

use oer_mmio::{Fixed, Placement};
use oer_time::settle::{Settle, SettleLimit, settle};
use oer_time::{Clock, Duration, Instant};

use oer_esp32s31_pac::{
    instances, lp_peri_clkrst::LpPericlkrstRngInit, ownership::EntropyPeripherals, trng::Trng,
};

#[cfg(test)]
mod tests;

/// Where the device's TRNG block lies.
pub type TrngPlacement = Fixed<{ instances::trng::BASE }>;

/// Where the device's `RNG_CTRL` word lies.
pub type RngClockPlacement = Fixed<{ instances::lp_periclkrst::BASE }>;

/// The true random number generator's output and status, and its clock
/// and reset word, which stage two only reads: the bootstrap enabled the
/// generator ([`crate::boot::Soc`]).
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EntropyRegisters<P: Placement = TrngPlacement, Q: Placement = RngClockPlacement> {
    trng: Trng<P>,
    clock: LpPericlkrstRngInit<Q>,
}

impl EntropyRegisters {
    /// Own the root's entropy partition and the generator's clock word of
    /// its SoC partition.
    pub fn new(peripherals: EntropyPeripherals, clock: LpPericlkrstRngInit) -> Self {
        Self {
            trng: peripherals.trng,
            clock,
        }
    }
}

/// The generator's state as its owner reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntropyStatus {
    /// The generator's clock runs and its reset is released.
    pub clocked: bool,
    /// Sampling and the output conditioner are on.
    pub sampling: bool,
    /// Health tests run on a selected noise source.
    pub health_tested: bool,
    /// A startup or continuous health test failed.
    pub health_error: bool,
}

impl EntropyStatus {
    /// Every condition of a working source holds.
    pub const fn is_healthy(self) -> bool {
        self.clocked && self.sampling && self.health_tested && !self.health_error
    }
}

/// The shortest time between two reads of the generator's output, as `C`
/// measures it.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/esp_hw_support/hw_random.c:43,89`: consecutive
/// reads are at least 16 APB clock cycles apart (`apb_cycle_wait_num`).
/// That is under one microsecond at every APB frequency of at least
/// 16 MHz; two microseconds of a clock counting whole microseconds
/// guarantee at least one.
pub const READ_GAP: Duration = Duration::from_micros(2);

/// The settle that waits out [`READ_GAP`], checked when the crate compiles.
const READ_GAP_SETTLE: SettleLimit = match SettleLimit::new(READ_GAP) {
    Some(limit) => limit,
    None => panic!("the read gap is a valid settle limit"),
};

/// Owns the true random number generator's output for the image's lifetime.
///
/// This owner only reads the generator, spacing reads by [`READ_GAP`] with
/// a named settle; [`Entropy::status`] tells whether it runs. The time of
/// the last read lives here, not in a global.
pub struct Entropy<C: Clock, P: Placement = TrngPlacement, Q: Placement = RngClockPlacement> {
    registers: EntropyRegisters<P, Q>,
    clock: C,
    last: Option<Instant>,
}

impl<C: Clock, P: Placement, Q: Placement> Entropy<C, P, Q> {
    /// Own the generator, timing reads with `clock`.
    pub const fn new(registers: EntropyRegisters<P, Q>, clock: C) -> Self {
        Self {
            registers,
            clock,
            last: None,
        }
    }

    /// One random word, at least [`READ_GAP`] after the previous read.
    ///
    /// A named settle of at most [`READ_GAP`] waits out the gap; a
    /// monotonic clock always passes it, since the settle's last attempt
    /// runs after its limit, which is at least the gap since the last read.
    pub fn random(&mut self) -> u32 {
        if let Some(last) = self.last {
            let clock = &self.clock;
            settle(Settle::new(READ_GAP_SETTLE, clock), || {
                (clock.now().saturating_duration_since(last) >= READ_GAP).then_some(())
            })
            .expect("a monotonic clock passes the read gap by the settle's last attempt");
        }
        let word = self
            .registers
            .trng
            .read_crc_sync_data()
            .sw_crc_random_data_sync();
        self.last = Some(self.clock.now());
        word
    }

    /// Fill `bytes` with random bytes, four from each word, little-endian.
    pub fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(4) {
            let word = self.random().to_le_bytes();
            for (byte, random) in chunk.iter_mut().zip(word) {
                *byte = random;
            }
        }
    }

    /// `N` random bytes.
    pub fn random_bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0; N];
        self.fill(&mut bytes);
        bytes
    }

    /// The generator's clock, sampling and health-test state.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2
    /// `components/hal/esp32s31/include/hal/rng_ll.h:163-178`
    /// (`rng_ll_enable`: clocks, reset, sampling and health-test configuration).
    pub fn status(&self) -> EntropyStatus {
        let clock = self.registers.clock.read_rng_ctrl();
        let conf = self.registers.trng.read_conf();
        let debug = self.registers.trng.read_debug_conf();
        EntropyStatus {
            clocked: clock.lp_rng_clk_en() && !clock.lp_rng_rst_en(),
            sampling: conf.sample_enable() && conf.noise_crc_en() && conf.random_output_mode(),
            health_tested: !debug.health_test_bypass() && conf.noise_source_sel().get() != 0,
            health_error: self.registers.trng.read_int_raw().error_int_raw(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b> EntropyRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// The generator over host memory from each instance's base.
    pub fn host(trng: &'a mut [u32], clock: &'b mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            trng: oer_esp32s31_pac::trng::trng_host(trng)?.1,
            clock: LpPericlkrstRngInit::host(clock)?,
        })
    }
}
