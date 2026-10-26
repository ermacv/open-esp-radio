//! The cells' elections, the stop of the other hart and the reset's
//! preparation over host memory: each test has its own cells and blocks.

use std::{vec, vec::Vec};

use oer_memory::ClaimCell;
use oer_mmio::Host;

use crate::platform::core_control::{
    CoreControlRegisters, HOST_SIZES, UART0_HOST_SIZE, Uart0ClockRegisters,
};

use super::{
    CoresError, Hart1, HpCore, StopError, Stopped, hart1, operate, park_hart1, prepare_reset,
    reset_with, restore_uart0_with, stop_with,
};

type HostRegisters<'a> = CoreControlRegisters<Host<'a>, Host<'a>, Host<'a>, Host<'a>, Host<'a>>;

#[test]
fn fatal_masks_before_lending_its_hart_and_halts_after_the_path() {
    use core::cell::{Cell, RefCell};
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

    let threshold = Cell::new(0_u8);
    let mie = Cell::new(true);
    let steps = RefCell::new(Vec::new());
    let terminal = catch_unwind(AssertUnwindSafe(|| {
        super::fatal_with(
            || {
                threshold.set(u8::MAX);
                steps.borrow_mut().push("threshold");
            },
            || {
                assert_eq!(threshold.get(), u8::MAX);
                mie.set(false);
                steps.borrow_mut().push("disable");
            },
            |fatal| {
                assert_eq!((threshold.get(), mie.get()), (u8::MAX, false));
                assert_eq!(fatal.hart(), HpCore::Core0);
                steps.borrow_mut().push("path");
            },
            || {
                assert_eq!((threshold.get(), mie.get()), (u8::MAX, false));
                steps.borrow_mut().push("halt");
                resume_unwind(std::boxed::Box::new(()))
            },
        )
    }));
    assert!(terminal.is_err(), "the terminal observer was reached");
    assert_eq!(*steps.borrow(), ["threshold", "disable", "path", "halt"]);
}

/// Host memory of the five blocks; the stall status reads `status_fill`.
struct Memory([Vec<u32>; 5]);

impl Memory {
    fn new(status_fill: u32) -> Self {
        let [stall, status, clocks, reset, system] = HOST_SIZES;
        Self([
            vec![0; stall / 4],
            vec![status_fill; status / 4],
            vec![0; clocks / 4],
            vec![0; reset / 4],
            vec![u32::MAX; system / 4],
        ])
    }

    fn registers(&mut self) -> HostRegisters<'_> {
        let [stall, status, clocks, reset, system] = &mut self.0;
        CoreControlRegisters::host(stall, status, clocks, reset, system)
            .expect("host memory holds the blocks")
    }

    /// A cell with the words installed.
    fn cell(&mut self) -> ClaimCell<HostRegisters<'_>> {
        let cell = ClaimCell::new();
        assert!(cell.install(self.registers()).is_ok());
        cell
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
    assert_eq!(reset_with(&cell, HpCore::Core0), Ok(()));
    drop(cell);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    assert_eq!(memory.0[4][0x8 / 4], 0x9fff_ffff);
}

#[test]
fn a_lost_or_busy_election_touches_no_word() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert!(stop_with(&cell, HpCore::Core1, 4).is_ok());
    assert_eq!(reset_with(&cell, HpCore::Core0), Err(StopError::Lost));

    drop(cell);
    assert!(
        memory.0[4].iter().all(|word| *word == u32::MAX),
        "the losing hart never writes LP_SYS"
    );

    let mut idle = Memory::new(u32::MAX);
    let before = idle.0.clone();
    let cell = idle.cell();
    let during = operate(&cell, |_| reset_with(&cell, HpCore::Core0));
    assert_eq!(during, Ok(Err(StopError::Busy)));
    drop(cell);
    assert_eq!(idle.0, before, "no word changed");
}

#[test]
fn the_reset_clears_the_boot_address_and_resets_and_stalls_the_other_hart() {
    let mut memory = Memory::new(0);
    prepare_reset(&mut memory.registers(), HpCore::Core0);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    let registers = memory.registers();
    assert!(registers.stall_requested(HpCore::Core1));
    assert!(!registers.stall_requested(HpCore::Core0));
    drop(registers);
    let [_, _, _, reset, _] = &memory.0;
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
    assert_eq!(reset_with(&cell, HpCore::Core1), Ok(()));
    assert_eq!(reset_with(&cell, HpCore::Core0), Err(StopError::Lost));
    drop(cell);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    assert_eq!(memory.0[4][0x8 / 4], 0x9fff_ffff);
}

#[test]
fn a_reset_without_installed_words_is_left_to_the_watchdog() {
    let cell = ClaimCell::<HostRegisters<'_>>::new();
    assert_eq!(
        reset_with(&cell, HpCore::Core0),
        Err(StopError::NotInstalled)
    );
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
        (
            reset_with(&core_cell, HpCore::Core0),
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
