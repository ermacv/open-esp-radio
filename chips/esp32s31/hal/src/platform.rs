//! The platform's owners, split from the reset root's partitions: the
//! factory identity from eFuse, entropy from the true random number
//! generator, why each HP core last left reset, core control
//! ([`core_control`]), the software interrupt lines
//! ([`software_interrupt`]), the system timer ([`system_timer`]) and the
//! hart's interrupt controller ([`interrupt_controller`]), which the
//! interrupt table's router drives.

pub use oer_esp32s31_knowledge::reset_cause::ResetCause;
pub use oer_espressif_efuse::{FactoryMac, Interface, MacAddress};
use oer_mmio::{Fixed, Placement};
use oer_time::{Clock, Duration, Instant};

use oer_esp32s31_pac::{
    efuse::Efuse,
    instances,
    lp_aonclkrst::LpAonResetCause,
    lp_peri_clkrst::LpPericlkrstRngInit,
    ownership::{EfusePeripherals, EntropyPeripherals, ResetCausePeripherals},
    trng::Trng,
};
use oer_time::settle::{Settle, SettleLimit, settle};

pub use core_control::{CoreControlRegisters, Uart0ClockRegisters};
pub use interrupt_controller::{Hart1Interrupts, InterruptBlocks, InterruptController};
pub use port::Port;
pub use software_interrupt::{
    FromCpu0, FromCpu0Irq, FromCpu1, FromCpu1Irq, FromCpu2, FromCpu2Irq, FromCpu3, FromCpu3Irq,
    SoftwareInterrupts,
};
pub use system_timer::{
    SystemTimer, SystimerAlarm0, SystimerAlarm1, SystimerAlarm2, SystimerComparators,
    SystimerCounter,
};

pub mod core_control;
pub mod interrupt_controller;
pub(crate) mod port;
pub mod software_interrupt;
pub mod system_timer;
#[cfg(test)]
mod tests;
pub mod timg0_alarm;

pub use oer_esp32s31_pac::{BOOT_HART, HARTS, Hart, SecondaryHart};

/// The eFuse controller's read words of the factory MAC.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EfuseRegisters<P: Placement = Fixed<{ instances::efuse::BASE }>>(Efuse<P>);

impl EfuseRegisters {
    /// Own the root's eFuse partition.
    pub fn new(peripherals: EfusePeripherals) -> Self {
        Self(peripherals.efuse)
    }
}

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

/// The reset-cause words of both HP cores.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ResetCauseRegisters<P: Placement = Fixed<{ instances::lp_aon_clkrst::BASE }>>(
    LpAonResetCause<P>,
);

impl ResetCauseRegisters {
    /// Own the root's reset-cause partition.
    pub fn new(peripherals: ResetCausePeripherals) -> Self {
        Self(peripherals.lp_aon_reset_cause)
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

/// The chip's factory identity, read once from eFuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FactoryIdentity {
    mac: FactoryMac,
}

impl FactoryIdentity {
    /// Read the factory MAC fields of block 1, which the controller mirrors
    /// after reset; reading needs no command, and the eFuse owner is not
    /// needed again.
    pub fn read<P: Placement>(efuse: EfuseRegisters<P>) -> Self {
        let low = efuse.0.read_rd_mac_sys0();
        let high = efuse.0.read_rd_mac_sys1();
        Self {
            mac: FactoryMac {
                mac_0: low.mac_0(),
                mac_1: high.mac_1(),
                mac_ext: high.mac_ext(),
            },
        }
    }

    /// The factory (universally administered) MAC address.
    pub const fn base_mac(self) -> MacAddress {
        self.mac.base_mac()
    }

    /// The MAC extension bits, as the PHY calibration identity records them.
    pub const fn mac_extension(self) -> u16 {
        self.mac.mac_extension()
    }

    /// The address `interface` uses.
    pub const fn address(self, interface: Interface) -> MacAddress {
        interface.address(self.base_mac())
    }
}

/// The reset cause of each HP core, read once after boot: the words hold
/// until the next reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCauses {
    causes: [ResetCause; HARTS],
}

impl ResetCauses {
    /// Read both cores' causes; the owner is not needed again.
    pub fn read<P: Placement>(registers: ResetCauseRegisters<P>) -> Self {
        Self {
            causes: Hart::ALL.map(|hart| {
                let cause = registers
                    .0
                    .read_hpcore_reset_cause(hart.index())
                    .hpcore_reset_cause();
                ResetCause::from_code(cause.get().to_le_bytes()[0])
            }),
        }
    }

    /// Why `hart` last left reset.
    pub const fn of(self, hart: Hart) -> ResetCause {
        *hart.at(&self.causes)
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
impl<'a> EfuseRegisters<oer_mmio::Host<'a>> {
    /// The eFuse words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Efuse::host(words).map(Self)
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

#[cfg(any(test, feature = "test-support"))]
impl<'a> ResetCauseRegisters<oer_mmio::Host<'a>> {
    /// The reset-cause words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpAonResetCause::host(words).map(Self)
    }
}
