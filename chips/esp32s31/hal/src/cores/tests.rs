//! The cells' elections, the stop of the other hart and the reset's
//! preparation over host memory: each test has its own cells and blocks.

use std::{vec, vec::Vec};

use oer_memory::ClaimCell;
use oer_mmio::Host;

use crate::platform::core_control::{
    CoreControlRegisters, HOST_SIZES, UART0_HOST_SIZE, Uart0ClockRegisters,
};

use super::{
    BootAddress, CoresError, Hart1, HpCore, StopError, Stopped, hart1, operate, park_hart1,
    prepare_reset, reset_with, restore_uart0_with, stop_with,
};

type HostRegisters<'a> = CoreControlRegisters<Host<'a>, Host<'a>, Host<'a>, Host<'a>>;

/// Host memory of the four blocks; the stall status reads `status_fill`.
struct Memory([Vec<u32>; 4]);

impl Memory {
    fn new(status_fill: u32) -> Self {
        let [stall, status, clocks, reset] = HOST_SIZES;
        Self([
            vec![0; stall / 4],
            vec![status_fill; status / 4],
            vec![0; clocks / 4],
            vec![0; reset / 4],
        ])
    }

    fn registers(&mut self) -> HostRegisters<'_> {
        let [stall, status, clocks, reset] = &mut self.0;
        CoreControlRegisters::host(stall, status, clocks, reset)
            .expect("host memory holds the blocks")
    }

    /// A cell with the words installed.
    fn cell(&mut self) -> ClaimCell<HostRegisters<'_>> {
        let cell = ClaimCell::new();
        assert!(cell.install(self.registers()).is_ok());
        cell
    }
}

/// Records the boot addresses written.
#[derive(Default)]
struct Boot(Vec<u32>);

impl BootAddress for Boot {
    fn set(&mut self, entry: u32) {
        self.0.push(entry);
    }
}

#[test]
fn the_owner_s_operations_borrow_the_words_until_a_fatal_path_holds_them() {
    let empty = ClaimCell::<HostRegisters<'_>>::new();
    assert_eq!(
        operate(&empty, |words| hart1(words)),
        Err(CoresError::NotInstalled)
    );
    let mut memory = Memory::new(0);
    let cell = memory.cell();
    assert_eq!(operate(&cell, |words| hart1(words)), Ok(Hart1::Released));
    assert_eq!(operate(&cell, park_hart1), Ok(()));
    assert_eq!(operate(&cell, |words| hart1(words)), Ok(Hart1::Held));
    assert_eq!(stop_with(&cell, HpCore::Core0, 4), Ok(Stopped::Held));
    assert_eq!(
        operate(&cell, |words| hart1(words)),
        Err(CoresError::Stopped),
        "the fatal path keeps the words"
    );
}

#[test]
fn the_first_fatal_hart_wins_and_the_other_hart_loses() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(stop_with(&cell, HpCore::Core1, 4), Ok(Stopped::Stalled));
    assert_eq!(stop_with(&cell, HpCore::Core0, 4), Err(StopError::Lost));
    assert_eq!(
        stop_with(&cell, HpCore::Core1, 4),
        Err(StopError::Reentered)
    );
}

#[test]
fn a_fatal_path_never_takes_the_words_from_an_operation() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    let during = operate(&cell, |_| stop_with(&cell, HpCore::Core0, 4));
    assert_eq!(during, Ok(Err(StopError::Busy)));
    assert_eq!(stop_with(&cell, HpCore::Core0, 4), Ok(Stopped::Stalled));
}

#[test]
fn a_held_hart_1_is_stopped_without_a_stall_request() {
    let mut memory = Memory::new(0);
    memory.registers().hold_core1();
    let cell = memory.cell();
    assert_eq!(stop_with(&cell, HpCore::Core0, 4), Ok(Stopped::Held));
    drop(cell);
    assert!(!memory.registers().stall_requested(HpCore::Core1));
}

#[test]
fn a_running_other_hart_is_stopped_once_the_hardware_reports_it_stalled() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(stop_with(&cell, HpCore::Core0, 4), Ok(Stopped::Stalled));
    drop(cell);
    let registers = memory.registers();
    assert!(registers.stall_requested(HpCore::Core1));
    assert!(
        !registers.stall_requested(HpCore::Core0),
        "a hart never stalls itself"
    );
}

#[test]
fn hart_1_s_fatal_path_stalls_hart_0() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(stop_with(&cell, HpCore::Core1, 4), Ok(Stopped::Stalled));
    drop(cell);
    let registers = memory.registers();
    assert!(registers.stall_requested(HpCore::Core0));
    assert!(!registers.stall_requested(HpCore::Core1));
}

#[test]
fn a_stall_the_hardware_never_reports_is_unconfirmed_and_the_reset_still_holds_the_words() {
    let mut memory = Memory::new(0);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, HpCore::Core0, 4),
        Err(StopError::Unconfirmed)
    );
    // The words stay the fatal path's: its reset still prepares the ROM.
    let mut boot = Boot::default();
    assert_eq!(reset_with(&cell, &mut boot, HpCore::Core0), Ok(()));
    assert_eq!(boot.0, [0]);
}

#[test]
fn a_lost_or_busy_election_touches_no_word() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert!(stop_with(&cell, HpCore::Core1, 4).is_ok());
    let mut boot = Boot::default();
    assert_eq!(
        reset_with(&cell, &mut boot, HpCore::Core0),
        Err(StopError::Lost)
    );
    assert!(boot.0.is_empty(), "the loser prepares nothing");
    drop(cell);

    let mut idle = Memory::new(u32::MAX);
    let before = idle.0.clone();
    let cell = idle.cell();
    let mut boot = Boot::default();
    let during = operate(&cell, |_| reset_with(&cell, &mut boot, HpCore::Core0));
    assert_eq!(during, Ok(Err(StopError::Busy)));
    assert!(boot.0.is_empty());
    drop(cell);
    assert_eq!(idle.0, before, "no word changed");
}

#[test]
fn the_reset_clears_the_boot_address_and_resets_and_stalls_the_other_hart() {
    let mut memory = Memory::new(0);
    let mut boot = Boot::default();
    prepare_reset(&mut memory.registers(), &mut boot, HpCore::Core0);
    assert_eq!(boot.0, [0]);
    let registers = memory.registers();
    assert!(registers.stall_requested(HpCore::Core1));
    assert!(!registers.stall_requested(HpCore::Core0));
    drop(registers);
    let [_, _, _, reset] = &memory.0;
    assert_eq!(
        reset.iter().filter(|word| **word != 0).count(),
        1,
        "one core's reset pulsed"
    );
}

#[test]
fn a_reset_after_the_stop_keeps_its_words_and_one_without_them_touches_nothing() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert!(stop_with(&cell, HpCore::Core1, 4).is_ok());
    let mut boot = Boot::default();
    assert_eq!(reset_with(&cell, &mut boot, HpCore::Core1), Ok(()));
    assert_eq!(boot.0, [0]);
    let mut other = Boot::default();
    assert_eq!(
        reset_with(&cell, &mut other, HpCore::Core0),
        Err(StopError::Lost)
    );
    assert!(
        other.0.is_empty(),
        "the loser leaves the reset to the watchdog"
    );
}

#[test]
fn a_reset_without_installed_words_is_left_to_the_watchdog() {
    let cell = ClaimCell::<HostRegisters<'_>>::new();
    let mut boot = Boot::default();
    assert_eq!(
        reset_with(&cell, &mut boot, HpCore::Core0),
        Err(StopError::NotInstalled)
    );
    assert!(boot.0.is_empty());
}

/// Host memory of UART0's clock block, every bit clear: a restore opens its
/// gate.
fn uart0_memory() -> Vec<u32> {
    vec![0; UART0_HOST_SIZE / 4]
}

fn uart0_gate_open(words: &[u32]) -> bool {
    words.iter().any(|word| *word != 0)
}

#[test]
fn a_reset_that_cannot_hold_the_core_control_words_still_restores_uart0() {
    let mut cores = Memory::new(0);
    let core_cell = cores.cell();
    let mut words = uart0_memory();
    let uart0 = ClaimCell::new();
    assert!(
        uart0
            .install(Uart0ClockRegisters::host(&mut words).expect("host memory holds the block"))
            .is_ok()
    );
    // The cores' owner runs an operation the fatal path interrupted.
    let during = operate(&core_cell, |_| {
        let mut boot = Boot::default();
        (
            reset_with(&core_cell, &mut boot, HpCore::Core0),
            restore_uart0_with(&uart0, HpCore::Core0),
        )
    });
    assert_eq!(during, Ok((Err(StopError::Busy), true)));
    drop(uart0);
    assert!(uart0_gate_open(&words), "UART0's clock gate opened");
}

#[test]
fn uart0_is_restored_by_the_first_fatal_hart_and_again_only_on_that_hart() {
    let mut words = uart0_memory();
    let uart0 = ClaimCell::new();
    assert!(
        uart0
            .install(Uart0ClockRegisters::host(&mut words).expect("host memory holds the block"))
            .is_ok()
    );
    assert!(restore_uart0_with(&uart0, HpCore::Core1));
    assert!(
        !restore_uart0_with(&uart0, HpCore::Core0),
        "the other hart reaches no word"
    );
    assert!(
        restore_uart0_with(&uart0, HpCore::Core1),
        "a reset after a stop restores it again"
    );
    drop(uart0);
    assert!(uart0_gate_open(&words));
}
