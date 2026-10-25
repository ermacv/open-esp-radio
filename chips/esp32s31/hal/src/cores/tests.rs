//! The holder's elections, the stop of the other hart and the reset's
//! preparation over host memory: each test has its own holder and blocks.

use std::{cell::Cell, vec, vec::Vec};

use oer_esp32s31_pac::platform::{
    CoreControlRegisters, Uart0ClockRegisters,
    core_control::{HOST_SIZES, UART0_HOST_SIZE},
};
use oer_mmio::Host;

use super::{
    BootAddress, Election, Holder, HpCore, StopError, Stopped, Uart0Holder, prepare_reset,
    reset_with, restore_uart0_with, stop_with,
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
fn the_cores_owner_is_taken_once_and_holds_the_words_only_during_an_operation() {
    let holder = Holder::new();
    assert!(holder.take_owner());
    assert!(!holder.take_owner());
    assert!(holder.begin());
    assert!(!holder.begin(), "one operation at a time");
    holder.end();
    assert!(holder.begin());
}

#[test]
fn the_first_fatal_hart_wins_and_the_other_hart_loses() {
    let holder = Holder::new();
    assert_eq!(holder.elect(HpCore::Core1), Election::Won);
    assert_eq!(holder.elect(HpCore::Core0), Election::Lost);
    assert_eq!(holder.elect(HpCore::Core1), Election::Again);
    assert!(
        !holder.take_owner() && !holder.begin(),
        "the fatal path keeps the words"
    );
}

#[test]
fn a_fatal_path_takes_the_words_from_an_idle_owner_but_never_from_an_operation() {
    let holder = Holder::new();
    assert!(holder.take_owner());
    assert!(holder.begin());
    assert_eq!(holder.elect(HpCore::Core0), Election::Busy);
    holder.end();
    assert_eq!(holder.elect(HpCore::Core0), Election::Won);
    assert!(!holder.begin(), "the owner's next operation is refused");
}

#[test]
fn a_held_hart_1_is_stopped_without_a_stall_request() {
    let mut memory = Memory::new(0);
    memory.registers().hold_core1();
    let holder = Holder::new();
    let stopped = stop_with(&holder, || memory.registers(), HpCore::Core0, 4);
    assert_eq!(stopped, Ok(Stopped::Held));
    assert!(!memory.registers().stall_requested(HpCore::Core1));
}

#[test]
fn a_running_other_hart_is_stopped_once_the_hardware_reports_it_stalled() {
    let mut memory = Memory::new(u32::MAX);
    let holder = Holder::new();
    let stopped = stop_with(&holder, || memory.registers(), HpCore::Core0, 4);
    assert_eq!(stopped, Ok(Stopped::Stalled));
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
    let holder = Holder::new();
    let stopped = stop_with(&holder, || memory.registers(), HpCore::Core1, 4);
    assert_eq!(stopped, Ok(Stopped::Stalled));
    let registers = memory.registers();
    assert!(registers.stall_requested(HpCore::Core0));
    assert!(!registers.stall_requested(HpCore::Core1));
}

#[test]
fn a_stall_the_hardware_never_reports_is_unconfirmed_and_the_reset_still_holds_the_words() {
    let mut memory = Memory::new(0);
    let holder = Holder::new();
    let stopped = stop_with(&holder, || memory.registers(), HpCore::Core0, 4);
    assert_eq!(stopped, Err(StopError::Unconfirmed));
    // The words stay the fatal path's: its reset still prepares the ROM.
    let mut boot = Boot::default();
    assert!(reset_with(
        &holder,
        || memory.registers(),
        &mut boot,
        HpCore::Core0
    ));
    assert_eq!(boot.0, [0]);
}

#[test]
fn a_lost_or_busy_election_touches_no_word() {
    let captured = Cell::new(false);
    let holder = Holder::new();
    assert_eq!(holder.elect(HpCore::Core1), Election::Won);
    let mut memory = Memory::new(u32::MAX);
    let lost = stop_with(
        &holder,
        || {
            captured.set(true);
            memory.registers()
        },
        HpCore::Core0,
        4,
    );
    assert_eq!(lost, Err(StopError::Lost));
    let busy_holder = Holder::new();
    assert!(busy_holder.take_owner() && busy_holder.begin());
    let busy = stop_with(
        &busy_holder,
        || {
            captured.set(true);
            memory.registers()
        },
        HpCore::Core0,
        4,
    );
    assert_eq!(busy, Err(StopError::Busy));
    assert!(!captured.get());
}

#[test]
fn a_second_stop_on_the_same_hart_is_a_reentry() {
    let mut memory = Memory::new(u32::MAX);
    let holder = Holder::new();
    assert!(stop_with(&holder, || memory.registers(), HpCore::Core0, 4).is_ok());
    assert_eq!(
        stop_with(&holder, || memory.registers(), HpCore::Core0, 4),
        Err(StopError::Reentered)
    );
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
    let holder = Holder::new();
    assert!(stop_with(&holder, || memory.registers(), HpCore::Core1, 4).is_ok());
    let mut boot = Boot::default();
    assert!(reset_with(
        &holder,
        || memory.registers(),
        &mut boot,
        HpCore::Core1
    ));
    assert_eq!(boot.0, [0]);
    let mut other = Boot::default();
    assert!(!reset_with(
        &holder,
        || memory.registers(),
        &mut other,
        HpCore::Core0
    ));
    assert!(other.0.is_empty(), "the loser resets through the ROM alone");
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
    // The cores' owner runs an operation the fatal path interrupted.
    let holder = Holder::new();
    assert!(holder.take_owner() && holder.begin());
    let mut cores = Memory::new(0);
    let mut boot = Boot::default();
    assert!(!reset_with(
        &holder,
        || cores.registers(),
        &mut boot,
        HpCore::Core0
    ));
    let uart0 = Uart0Holder::new();
    let mut words = uart0_memory();
    assert!(restore_uart0_with(
        &uart0,
        || Uart0ClockRegisters::host(&mut words).expect("host memory holds the block"),
        HpCore::Core0
    ));
    assert!(uart0_gate_open(&words), "UART0's clock gate opened");
}

#[test]
fn uart0_is_restored_by_the_first_fatal_hart_and_again_only_on_that_hart() {
    let uart0 = Uart0Holder::new();
    let mut words = uart0_memory();
    assert!(restore_uart0_with(
        &uart0,
        || Uart0ClockRegisters::host(&mut words).expect("host memory holds the block"),
        HpCore::Core1
    ));
    let captured = Cell::new(false);
    let mut other = uart0_memory();
    assert!(!restore_uart0_with(
        &uart0,
        || {
            captured.set(true);
            Uart0ClockRegisters::host(&mut other).expect("host memory holds the block")
        },
        HpCore::Core0
    ));
    assert!(
        !captured.get(),
        "the other hart builds no owner of the word"
    );
    let mut again = uart0_memory();
    assert!(restore_uart0_with(
        &uart0,
        || Uart0ClockRegisters::host(&mut again).expect("host memory holds the block"),
        HpCore::Core1
    ));
    assert!(
        uart0_gate_open(&again),
        "a reset inside a reset restores it again"
    );
}
