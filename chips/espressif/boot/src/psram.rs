//! An octal (OPI, DDR) PSRAM brought up cold by the boot: its supply and
//! clock, its controller, its mode registers through the command port, its
//! density, and its mapping.
//!
//! The order:
//!
//! 1. the controller's bus clock opened, the controller reset and released,
//!    its source the memory PLL;
//! 2. the PHY supply: current limit on, the level, the voltage detector,
//!    the supply powered, the limit off, then the fixed settle its source
//!    states;
//! 3. the memory PLL's reference dividers, its power (the PMU's immediate
//!    ties and its clock gate, then the PMU's active state), the
//!    controller's function clock;
//! 4. the bus divider of both controllers, the DLL, the pads and the chip
//!    select timing;
//! 5. the mode registers read, changed and written back in pairs, the
//!    neighbour of each changed register written as it read;
//! 6. the density read from MR2: a code the boot does not know is
//!    [`PsramFailure::UnsupportedDensity`], one that differs from the
//!    board's size [`PsramFailure::DensityMismatch`]; nothing is mapped then;
//! 7. the cache's access to the PSRAM configured, the window mapped from
//!    its first page and dropped from the data cache.
//!
//! Every command through the command port puts the port in octal DDR mode,
//! sets its phases, selects the PSRAM's chip select and starts once none
//! runs; a command that does not complete is a typed failure that ends the
//! boot before the PSRAM is used.
//!
//! SOURCE: esp-hal 1f646c5 `esp-hal/src/psram/esp32s31.rs:136-187` (steps
//! 1, 3-7), `esp-hal/src/soc/esp32s31/clocks.rs:283-351` (steps 2 and 3:
//! the supply before the PLL, 1000 µs after it),
//! `esp-hal/src/psram/oct_hex.rs:189-253` (step 5),
//! `:9-21` (step 6; its fallback to 32 MiB is not kept) and `:347-460`
//! (the command port; the ignored command timeout is not kept); ESP-IDF
//! 4d59230d `components/esp_psram/device/esp_psram_impl_ap_oct.c:70-131`
//! (the mode registers' fields), `:418-475` (the order and the densities)
//! and `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:200-208`
//! (the memory PLL's power).

use oer_time::Clock;

use crate::{
    Access, BootFailure, Busy,
    cache::{CacheGeometry, CacheSyncRegisters, invalidate_data},
    clock::ActivePllPower,
    mmu::{self, MappedRange, MmuRegisters, MmuTarget, MmuWindow},
    wait::{BootDeadline, BootWait},
};

/// The memory PLL, the PHY supply and the controller's clocks and reset.
pub trait MemoryClockRegisters {
    /// A level of the PHY supply.
    type SupplyLevel: Copy;
    /// Open the controller's bus clock.
    fn open_controller_clock(&mut self, access: Access<'_>);
    /// Assert or release the controller's reset.
    fn reset_controller(&mut self, access: Access<'_>, asserted: bool);
    /// Clock the controller from the memory PLL.
    fn select_memory_pll(&mut self, access: Access<'_>);
    /// Limit, or stop limiting, the supply's inrush current.
    fn limit_supply_current(&mut self, access: Access<'_>, limited: bool);
    /// Set the supply's level, regulated rather than tied high.
    fn set_supply_level(&mut self, access: Access<'_>, level: Self::SupplyLevel);
    /// Enable the supply's voltage detector.
    fn enable_supply_detector(&mut self, access: Access<'_>);
    /// Power the supply.
    fn power_supply(&mut self, access: Access<'_>);
    /// Set the dividers of the references the memory PLL feeds.
    fn set_memory_pll_references(&mut self, access: Access<'_>);
    /// Power the memory PLL and open its gates: the PMU's immediate ties
    /// and its clock gate.
    fn power_memory_pll(&mut self, access: Access<'_>);
    /// Open the controller's function clock.
    fn open_function_clock(&mut self, access: Access<'_>);
}

/// The chip select's timing, in cycles of the PSRAM's clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChipSelectTiming {
    pub setup: u8,
    pub hold: u8,
    pub hold_delay: u8,
}

/// The cache's octal DDR access to the PSRAM: its dummy cycle lengths as
/// the controller holds them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheAccess {
    pub read_dummy_cyclelen: u8,
    pub write_dummy_cyclelen: u8,
}

/// The PSRAM controllers' clock, DLL, pads, chip select and cache access.
pub trait PsramControllerRegisters {
    /// Divide both controllers' source clock by `divider` (at least one).
    fn set_bus_divider(&mut self, access: Access<'_>, divider: u8);
    /// Enable the DLL of the data strobe's timing.
    fn enable_dll(&mut self, access: Access<'_>);
    /// Set the pads' drive and power the data strobe's pad.
    fn configure_pads(&mut self, access: Access<'_>);
    /// Set the chip select's timing.
    fn set_chip_select_timing(&mut self, access: Access<'_>, timing: ChipSelectTiming);
    /// Configure the cache's octal DDR access and allow AXI accesses.
    fn configure_cache_access(&mut self, access: Access<'_>, cache: CacheAccess);
}

/// One command through the PSRAM command port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PsramCommand {
    pub command: u16,
    pub command_bits: u8,
    pub address: u32,
    pub address_bits: u8,
    pub output_bits: u16,
    pub input_bits: u16,
    pub dummy_bits: u8,
}

/// The PSRAM command port.
pub trait PsramCommandRegisters {
    /// Put the port in octal DDR mode.
    fn set_octal_ddr_mode(&mut self, access: Access<'_>);
    /// Set the command's phases and address.
    fn configure(&mut self, access: Access<'_>, command: PsramCommand);
    /// Set the first output data word.
    fn set_output(&mut self, access: Access<'_>, word: u32);
    /// Select the PSRAM's chip select, the flash's deselected.
    fn select_psram(&mut self, access: Access<'_>);
    /// Start the command, or [`Busy`] while one runs.
    fn start(&mut self, access: Access<'_>) -> Result<(), Busy>;
    /// Whether the command still runs.
    fn busy(&self, access: Access<'_>) -> bool;
    /// The first input data word of the last command.
    fn input(&self, access: Access<'_>) -> u32;
}

/// A board's octal PSRAM at one speed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OctalPsramProfile<Level> {
    /// The PHY supply's level.
    pub supply: Level,
    /// The divider from the memory PLL to the PSRAM's clock.
    pub divider: u8,
    /// MR0's read latency code.
    pub read_latency: u8,
    /// MR4's write latency code.
    pub write_latency: u8,
    /// The dummy bits of a mode register read.
    pub register_dummy_bits: u8,
    pub chip_select: ChipSelectTiming,
    pub cache: CacheAccess,
    /// The board's PSRAM, in bytes: the density must name it.
    pub size: u32,
}

/// The named waits of the bring-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PsramWaits {
    /// The fixed settle after the supply's current limit ends.
    pub supply_settle: BootWait,
    /// One command through the command port.
    pub command: BootWait,
    /// The data cache's invalidation of the mapped window.
    pub cache_sync: BootWait,
}

/// Why the PSRAM did not come up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsramFailure {
    /// MR2 holds a density code the boot does not know.
    UnsupportedDensity(u8),
    /// MR2's density differs from the board's PSRAM.
    DensityMismatch { found: u32 },
}

impl PsramFailure {
    /// The `reason=` word of the failure line.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnsupportedDensity(_) => "psram-density-unknown",
            Self::DensityMismatch { .. } => "psram-density-mismatch",
        }
    }
}

/// The owners the bring-up uses.
pub struct PsramOwners<'a, K, A, P, S> {
    /// The memory PLL, the supply and the controller's clocks.
    pub clock: &'a mut K,
    /// The PLLs' power in the PMU's active state.
    pub active: &'a mut A,
    /// The controllers, the command port and the PSRAM's MMU table.
    pub psram: &'a mut P,
    /// The cache sync engine.
    pub cache: &'a mut S,
}

/// The PSRAM as the boot leaves it: powered, configured and mapped.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the PSRAM is used through its mapped window"]
pub struct PsramReady {
    pub mapped: MappedRange,
}

/// The mode register pairs.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:156-232,237-276`
/// (mode-register transactions at addresses 0, 2, 4 and 8).
const MR0_MR1: u32 = 0x0;
const MR2_MR3: u32 = 0x2;
const MR4_MR5: u32 = 0x4;
const MR8_MR9: u32 = 0x8;

/// The command port's mode register commands: 16-bit command, 32-bit
/// address, 16 data bits.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:27-31,156-174,189-206`
/// (0x4040 read, 0xc0c0 write, 16-bit command/data and 32-bit address).
const REGISTER_READ: u16 = 0x4040;
const REGISTER_WRITE: u16 = 0xc0c0;

/// MR0: drive strength [1:0], read latency [4:2], latency type [5] (1
/// fixed); MR4: write latency [7:5]; MR8: burst length [1:0], burst type
/// [2], row-boundary crossing [3], X16 [6]; MR2: density [2:0].
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:70-131`.
mod mode {
    pub(super) const MR0_DRIVE_STRENGTH: u8 = 0b11;
    pub(super) const MR0_READ_LATENCY_SHIFT: u8 = 2;
    pub(super) const MR0_READ_LATENCY: u8 = 0b111 << MR0_READ_LATENCY_SHIFT;
    pub(super) const MR0_FIXED_LATENCY: u8 = 1 << 5;
    pub(super) const MR4_WRITE_LATENCY_SHIFT: u8 = 5;
    pub(super) const MR4_WRITE_LATENCY: u8 = 0b111 << MR4_WRITE_LATENCY_SHIFT;
    pub(super) const MR8_BURST_LENGTH: u8 = 0b11;
    pub(super) const MR8_BURST_TYPE: u8 = 1 << 2;
    pub(super) const MR8_ROW_BOUNDARY_CROSSING: u8 = 1 << 3;
    pub(super) const MR8_X16: u8 = 1 << 6;
    pub(super) const MR2_DENSITY: u8 = 0b111;
}

/// The bytes MR2's density code names.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:471-475`.
pub const fn density_bytes(code: u8) -> Option<u32> {
    const MIB: u32 = 1024 * 1024;
    match code {
        0x1 => Some(4 * MIB),
        0x3 => Some(8 * MIB),
        0x5 => Some(16 * MIB),
        0x7 => Some(32 * MIB),
        0x6 => Some(64 * MIB),
        _ => None,
    }
}

/// Bring the board's octal PSRAM up cold and map it into `window` from its
/// first page.
pub fn bring_up<K, A, P, S, C>(
    owners: PsramOwners<'_, K, A, P, S>,
    profile: OctalPsramProfile<K::SupplyLevel>,
    window: MmuWindow,
    cache: CacheGeometry<S::Map>,
    waits: PsramWaits,
    deadline: &BootDeadline<'_, C>,
) -> Result<PsramReady, BootFailure>
where
    K: MemoryClockRegisters,
    A: ActivePllPower,
    P: PsramControllerRegisters + PsramCommandRegisters + MmuRegisters,
    S: CacheSyncRegisters,
    C: Clock + ?Sized,
{
    let PsramOwners {
        clock,
        active,
        psram,
        cache: sync,
    } = owners;

    clock.open_controller_clock(Access::new());
    clock.reset_controller(Access::new(), true);
    clock.reset_controller(Access::new(), false);
    clock.select_memory_pll(Access::new());

    clock.limit_supply_current(Access::new(), true);
    clock.set_supply_level(Access::new(), profile.supply);
    clock.enable_supply_detector(Access::new());
    clock.power_supply(Access::new());
    clock.limit_supply_current(Access::new(), false);
    deadline.hold(waits.supply_settle)?;

    clock.set_memory_pll_references(Access::new());
    clock.power_memory_pll(Access::new());
    active.keep_memory_pll(Access::new());
    clock.open_function_clock(Access::new());

    psram.set_bus_divider(Access::new(), profile.divider);
    psram.enable_dll(Access::new());
    psram.configure_pads(Access::new());
    psram.set_chip_select_timing(Access::new(), profile.chip_select);

    let command = waits.command;
    let dummy = profile.register_dummy_bits;
    let (mr0, mr1) = read_pair(psram, MR0_MR1, dummy, command, deadline)?;
    let mr0 = (mr0
        & !(mode::MR0_DRIVE_STRENGTH | mode::MR0_READ_LATENCY | mode::MR0_FIXED_LATENCY))
        | ((profile.read_latency << mode::MR0_READ_LATENCY_SHIFT) & mode::MR0_READ_LATENCY)
        | mode::MR0_FIXED_LATENCY;
    write_pair(psram, MR0_MR1, mr0, mr1, command, deadline)?;
    let (mr4, mr5) = read_pair(psram, MR4_MR5, dummy, command, deadline)?;
    let mr4 = (mr4 & !mode::MR4_WRITE_LATENCY)
        | ((profile.write_latency << mode::MR4_WRITE_LATENCY_SHIFT) & mode::MR4_WRITE_LATENCY);
    write_pair(psram, MR4_MR5, mr4, mr5, command, deadline)?;
    let (mr8, mr9) = read_pair(psram, MR8_MR9, dummy, command, deadline)?;
    let mr8 = (mr8
        & !(mode::MR8_BURST_LENGTH
            | mode::MR8_BURST_TYPE
            | mode::MR8_ROW_BOUNDARY_CROSSING
            | mode::MR8_X16))
        | mode::MR8_BURST_LENGTH
        | mode::MR8_ROW_BOUNDARY_CROSSING;
    write_pair(psram, MR8_MR9, mr8, mr9, command, deadline)?;

    let (mr2, _) = read_pair(psram, MR2_MR3, dummy, command, deadline)?;
    let code = mr2 & mode::MR2_DENSITY;
    let found = density_bytes(code).ok_or(PsramFailure::UnsupportedDensity(code))?;
    if found != profile.size {
        return Err(PsramFailure::DensityMismatch { found }.into());
    }

    psram.configure_cache_access(Access::new(), profile.cache);
    let mapped = mmu::map(
        psram,
        window,
        MmuTarget::Psram,
        window.virtual_base,
        0,
        profile.size,
    )?;
    invalidate_data(
        sync,
        cache,
        mapped.virtual_start,
        mapped.len,
        waits.cache_sync,
        deadline,
    )?;
    Ok(PsramReady { mapped })
}

/// Read the mode register pair at `address`: (the register, its neighbour).
fn read_pair<P, C>(
    port: &mut P,
    address: u32,
    dummy_bits: u8,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<(u8, u8), BootFailure>
where
    P: PsramCommandRegisters,
    C: Clock + ?Sized,
{
    let command = PsramCommand {
        command: REGISTER_READ,
        command_bits: 16,
        address,
        address_bits: 32,
        output_bits: 0,
        input_bits: 16,
        dummy_bits,
    };
    transact(port, command, None, wait, deadline)?;
    let [low, high, ..] = port.input(Access::new()).to_le_bytes();
    Ok((low, high))
}

/// Write the mode register pair at `address`.
fn write_pair<P, C>(
    port: &mut P,
    address: u32,
    low: u8,
    high: u8,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure>
where
    P: PsramCommandRegisters,
    C: Clock + ?Sized,
{
    let command = PsramCommand {
        command: REGISTER_WRITE,
        command_bits: 16,
        address,
        address_bits: 32,
        output_bits: 16,
        input_bits: 0,
        dummy_bits: 0,
    };
    let word = u32::from(u16::from_le_bytes([low, high]));
    transact(port, command, Some(word), wait, deadline)
}

fn transact<P, C>(
    port: &mut P,
    command: PsramCommand,
    output: Option<u32>,
    wait: BootWait,
    deadline: &BootDeadline<'_, C>,
) -> Result<(), BootFailure>
where
    P: PsramCommandRegisters,
    C: Clock + ?Sized,
{
    // The phases change only while no command runs.
    deadline.poll(wait, || (!port.busy(Access::new())).then_some(()))?;
    port.set_octal_ddr_mode(Access::new());
    port.configure(Access::new(), command);
    if let Some(word) = output {
        port.set_output(Access::new(), word);
    }
    port.select_psram(Access::new());
    deadline.poll(wait, || port.start(Access::new()).ok())?;
    deadline.poll(wait, || (!port.busy(Access::new())).then_some(()))?;
    Ok(())
}
