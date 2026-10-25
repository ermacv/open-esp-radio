//! Platform values the image reads from blocks taken from esp-hal: the
//! factory identity from eFuse, entropy from the true random number
//! generator and why each HP core last left reset.
//!
//! Each owner is built from the PAC owner of its domain, which an esp-hal
//! adapter makes from its handoff. Nothing here writes a register but the
//! hart's interrupt controller ([`interrupt_controller`]), which the
//! interrupt table's router drives.

pub use oer_esp32s31_knowledge::reset_cause::ResetCause;
/// The step-4 platform blocks (system timer, software interrupt lines,
/// interrupt matrix and CLIC) and the handoffs that claim them, for the
/// executor and the interrupt table.
pub use oer_esp32s31_pac::platform::{
    Clic, FromCpu0, FromCpu1, FromCpu2, FromCpu3, InterruptCore0, InterruptCore0Status,
    InterruptCore1, InterruptCore1Status, InterruptMatrixHandoff, InterruptMatrixPeripherals,
    SoftwareInterrupt0Handoff, SoftwareInterrupt0Peripherals, SoftwareInterrupt1Handoff,
    SoftwareInterrupt1Peripherals, SoftwareInterrupt2Handoff, SoftwareInterrupt2Peripherals,
    SoftwareInterrupt3Handoff, SoftwareInterrupt3Peripherals, SystemTimerHandoff,
    SystemTimerPeripherals, Systimer, SystimerAcknowledge, SystimerCounter, clic, from_cpu_0,
    from_cpu_1, from_cpu_2, from_cpu_3, interrupt_core0, interrupt_core0_status, interrupt_core1,
    interrupt_core1_status, systimer, systimer_acknowledge, systimer_counter,
};
pub use oer_esp32s31_pac::platform::{
    EfuseHandoff, EfuseRegisters, EntropyHandoff, EntropyRegisters, ResetCauseHandoff,
    ResetCauseRegisters, RngClockPlacement, TrngPlacement,
};
pub use oer_espressif_efuse::{FactoryMac, Interface, MacAddress};
use oer_mmio::Placement;
use oer_time::{Clock, Duration, Instant};

use oer_time::settle::{Settle, SettleLimit, settle};

pub use interrupt_controller::{Hart1Interrupts, InterruptBlocks, InterruptController};

pub(crate) mod core_control;
pub mod interrupt_controller;
#[cfg(test)]
mod tests;

/// An HP core of the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HpCore {
    Core0,
    Core1,
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
    /// Read the factory MAC fields; the eFuse owner is not needed again.
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
    core0: ResetCause,
    core1: ResetCause,
}

impl ResetCauses {
    /// Read both cores' causes; the owner is not needed again.
    pub fn read<P: Placement>(registers: ResetCauseRegisters<P>) -> Self {
        let core0 = registers.0.read_hpcore0_reset_cause().hpcore0_reset_cause();
        let core1 = registers.0.read_hpcore1_reset_cause().hpcore1_reset_cause();
        Self {
            core0: ResetCause::from_code(core0.get().to_le_bytes()[0]),
            core1: ResetCause::from_code(core1.get().to_le_bytes()[0]),
        }
    }

    /// Why `core` last left reset.
    pub const fn of(self, core: HpCore) -> ResetCause {
        match core {
            HpCore::Core0 => self.core0,
            HpCore::Core1 => self.core1,
        }
    }
}

/// The shortest time between two reads of the generator's output, as `C`
/// measures it.
///
/// SOURCE: ESP-IDF 4d59230d `components/esp_hw_support/hw_random.c:43,89`: consecutive
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

/// Owns the true random number generator for the image's lifetime.
///
/// esp-hal enables and configures the generator in `esp_hal::init`; this
/// owner only reads it, spacing reads by [`READ_GAP`] with a named settle.
/// The time of the last read lives here, not in a global.
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
    /// SOURCE: ESP-IDF 4d59230d
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

#[cfg(test)]
mod register_tests;
