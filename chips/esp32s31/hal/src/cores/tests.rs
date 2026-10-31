//! The cells' elections, the stop of the other hart and the reset's
//! preparation over host memory: each test has its own cells and blocks.

use std::{vec, vec::Vec};

use oer_memory::ClaimCell;
use oer_mmio::Host;

use crate::platform::core_control::{
    CoreControlRegisters, HOST_SIZES, UART0_HOST_SIZE, Uart0ClockRegisters,
};

use super::{CoresError, Hart, StopError, operate, reset_with, restore_uart0_with, stop_with};

use oer_espressif_boot::drivers::cores::{CoreState, Stopped, park, prepare_reset, state};

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
            super::current_hart,
            |fatal| {
                assert_eq!((threshold.get(), mie.get()), (u8::MAX, false));
                assert_eq!(fatal.hart(), Ok(crate::platform::BOOT_HART));
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
        operate(&empty, |words| state(words, Hart::SECONDARIES[0])),
        Err(CoresError::NotInstalled)
    );
    let mut memory = Memory::new(0);
    let cell = memory.cell();
    assert_eq!(
        operate(&cell, |words| state(words, Hart::SECONDARIES[0])),
        Ok(CoreState::Released)
    );
    assert_eq!(
        operate(&cell, |words| park(words, Hart::SECONDARIES[0])),
        Ok(())
    );
    assert_eq!(
        operate(&cell, |words| state(words, Hart::SECONDARIES[0])),
        Ok(CoreState::Held)
    );
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Ok([None, Some(Stopped::Held)])
    );
    assert_eq!(
        operate(&cell, |words| state(words, Hart::SECONDARIES[0])),
        Err(CoresError::Stopped),
        "the fatal path keeps the words"
    );
}

#[test]
fn the_first_fatal_hart_wins_and_the_other_hart_loses() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, Hart::SECONDARIES[0].hart(), 4),
        Ok([Some(Stopped::Stalled), None])
    );
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Err(StopError::Lost)
    );
    assert_eq!(
        stop_with(&cell, Hart::SECONDARIES[0].hart(), 4),
        Err(StopError::Reentered)
    );
}

#[test]
fn a_fatal_path_never_takes_the_words_from_an_operation() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    let during = operate(&cell, |_| stop_with(&cell, crate::platform::BOOT_HART, 4));
    assert_eq!(during, Ok(Err(StopError::Busy)));
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Ok([None, Some(Stopped::Stalled)])
    );
}

#[test]
fn a_held_hart_1_is_stopped_without_a_stall_request() {
    let mut memory = Memory::new(0);
    memory.registers().hold(Hart::SECONDARIES[0]);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Ok([None, Some(Stopped::Held)])
    );
    drop(cell);
    assert!(
        !memory
            .registers()
            .stall_requested(Hart::SECONDARIES[0].hart())
    );
}

#[test]
fn a_running_other_hart_is_stopped_once_the_hardware_reports_it_stalled() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Ok([None, Some(Stopped::Stalled)])
    );
    drop(cell);
    let registers = memory.registers();
    assert!(registers.stall_requested(Hart::SECONDARIES[0].hart()));
    assert!(
        !registers.stall_requested(crate::platform::BOOT_HART),
        "a hart never stalls itself"
    );
}

#[test]
fn hart_1_s_fatal_path_stalls_hart_0() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, Hart::SECONDARIES[0].hart(), 4),
        Ok([Some(Stopped::Stalled), None])
    );
    drop(cell);
    let registers = memory.registers();
    assert!(registers.stall_requested(crate::platform::BOOT_HART));
    assert!(!registers.stall_requested(Hart::SECONDARIES[0].hart()));
}

#[test]
fn a_stall_the_hardware_never_reports_is_unconfirmed_and_the_reset_still_holds_the_words() {
    let mut memory = Memory::new(0);
    let cell = memory.cell();
    assert_eq!(
        stop_with(&cell, crate::platform::BOOT_HART, 4),
        Err(StopError::Unconfirmed)
    );
    // The words stay the fatal path's: its reset still prepares the ROM.
    assert_eq!(reset_with(&cell, crate::platform::BOOT_HART), Ok(()));
    drop(cell);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    assert_eq!(memory.0[4][0x8 / 4], 0x9fff_ffff);
}

#[test]
fn a_lost_or_busy_election_touches_no_word() {
    let mut memory = Memory::new(u32::MAX);
    let cell = memory.cell();
    assert!(stop_with(&cell, Hart::SECONDARIES[0].hart(), 4).is_ok());
    assert_eq!(
        reset_with(&cell, crate::platform::BOOT_HART),
        Err(StopError::Lost)
    );

    drop(cell);
    assert!(
        memory.0[4].iter().all(|word| *word == u32::MAX),
        "the losing hart never writes LP_SYS"
    );

    let mut idle = Memory::new(u32::MAX);
    let before = idle.0.clone();
    let cell = idle.cell();
    let during = operate(&cell, |_| reset_with(&cell, crate::platform::BOOT_HART));
    assert_eq!(during, Ok(Err(StopError::Busy)));
    drop(cell);
    assert_eq!(idle.0, before, "no word changed");
}

#[test]
fn the_reset_clears_the_boot_address_and_resets_and_stalls_the_other_hart() {
    let mut memory = Memory::new(0);
    prepare_reset(&mut memory.registers(), crate::platform::BOOT_HART);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    let registers = memory.registers();
    assert!(registers.stall_requested(Hart::SECONDARIES[0].hart()));
    assert!(!registers.stall_requested(crate::platform::BOOT_HART));
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
    assert!(stop_with(&cell, Hart::SECONDARIES[0].hart(), 4).is_ok());
    assert_eq!(reset_with(&cell, Hart::SECONDARIES[0].hart()), Ok(()));
    assert_eq!(
        reset_with(&cell, crate::platform::BOOT_HART),
        Err(StopError::Lost)
    );
    drop(cell);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    assert_eq!(memory.0[4][0x8 / 4], 0x9fff_ffff);
}

#[test]
fn a_reset_without_installed_words_is_left_to_the_watchdog() {
    let cell = ClaimCell::<HostRegisters<'_>>::new();
    assert_eq!(
        reset_with(&cell, crate::platform::BOOT_HART),
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
            reset_with(&core_cell, crate::platform::BOOT_HART),
            restore_uart0_with(&uart0, crate::platform::BOOT_HART),
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
    assert!(restore_uart0_with(&uart0, Hart::SECONDARIES[0].hart()));
    assert!(
        !restore_uart0_with(&uart0, crate::platform::BOOT_HART),
        "the other hart reaches no word"
    );
    assert!(
        restore_uart0_with(&uart0, Hart::SECONDARIES[0].hart()),
        "a reset after a stop restores it again"
    );
    drop(uart0);
    assert!(uart0_gate_open(&words));
}

#[test]
fn invalid_architectural_identity_is_retained_as_a_fault_after_masking() {
    use core::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    let masked = Cell::new(false);
    let terminal = catch_unwind(AssertUnwindSafe(|| {
        super::fatal_with(
            || (),
            || masked.set(true),
            || {
                assert!(masked.get());
                Err(super::Fault::InvalidHart { raw: u32::MAX })
            },
            |fatal| {
                assert_eq!(
                    fatal.hart(),
                    Err(super::Fault::InvalidHart { raw: u32::MAX })
                )
            },
            || resume_unwind(std::boxed::Box::new(())),
        )
    }));
    assert!(terminal.is_err());
}

#[test]
fn invalid_hart_reset_seals_without_a_false_key_or_a_peer_stall() {
    let mut memory = Memory::new(u32::MAX);
    let before = memory.0.clone();
    let cell = memory.cell();
    assert_eq!(super::reset_quarantined_with(&cell, u32::MAX), Ok(()));
    assert!(cell.lend(|_| ()).is_err(), "the owner remains quarantined");
    drop(cell);
    assert_eq!(
        memory.0[0], before[0],
        "unknown identity stalls no selected peer"
    );
    assert_eq!(memory.0[1], before[1]);
    assert_eq!(memory.0[2], before[2]);
    assert_eq!(memory.0[3], before[3]);
    assert_eq!(
        memory.0[4].iter().filter(|word| **word == 0).count(),
        Hart::SECONDARIES.len(),
        "secondary boot words are cleared"
    );
    assert_eq!(
        memory.0[4]
            .iter()
            .zip(&before[4])
            .filter(|(after, before)| after != before)
            .count(),
        Hart::SECONDARIES.len() + 1,
        "only boot words and the reset trigger change"
    );
}

#[test]
fn invalid_hart_reset_never_takes_over_a_borrowed_owner() {
    let mut memory = Memory::new(u32::MAX);
    let before = memory.0.clone();
    let cell = memory.cell();
    assert_eq!(
        cell.lend(|_| super::reset_quarantined_with(&cell, u32::MAX)),
        Ok(Err(StopError::InvalidHart { raw: u32::MAX }))
    );
    drop(cell);
    assert_eq!(memory.0, before);
}

mod launch {
    use super::*;
    use crate::cores::launch::{LaunchPermit, retire_with, start_with};
    use crate::cores::{SecondaryEntry, SecondaryHart, StartError};
    use crate::peripherals::reset_cause::{ResetCauseRegisters, ResetCauses};
    use core::cell::Cell;
    use oer_esp32s31_pac::{instances, interrupt_core::InterruptCore1};
    use oer_mmio::{Bits, Index};

    const HART: SecondaryHart = Hart::SECONDARIES[0];
    const ENTRY: u32 = 0x2f00_1000;

    fn entry() -> SecondaryEntry {
        #[allow(unsafe_code, reason = "a host test's entry is never executed")]
        // SAFETY: host memory only; no hart is released into this address.
        unsafe {
            SecondaryEntry::new::<1>(ENTRY)
        }
    }

    /// Hart 1's route bank, LP_SYS, the cache and hart 1's CLIC over host
    /// memory.
    struct Permit {
        routes: Vec<u32>,
        lp_sys: Vec<u32>,
        cache: Vec<u32>,
        clic: Vec<u32>,
    }

    impl Permit {
        /// Buses open, no wake stub, `connected` sources routed to line 17.
        fn new(connected: &[usize]) -> Self {
            let mut permit = Self {
                routes: vec![0; instances::interrupt_core1::SIZE / 4],
                lp_sys: vec![0; instances::lp_sys::SIZE / 4],
                cache: vec![0; instances::cache::SIZE / 4],
                clic: vec![0; instances::clic::SIZE / 4],
            };
            let mut bank = InterruptCore1::host(&mut permit.routes).expect("host bank");
            for &source in connected {
                bank.modify_route(Index::new(source).expect("a source"), |route| {
                    route.set_map(Bits::constant::<17>());
                });
            }
            permit
        }

        fn permit(&mut self) -> LaunchPermit<Host<'_>, Host<'_>, Host<'_>, Host<'_>> {
            LaunchPermit::host(
                &mut self.routes,
                &mut self.lp_sys,
                &mut self.cache,
                &mut self.clic,
            )
            .expect("host memory holds the permit")
        }

        fn routed(&mut self) -> usize {
            let bank = InterruptCore1::host(&mut self.routes).expect("host bank");
            (0..oer_esp32s31_pac::interrupt_core::route::COUNT)
                .filter(|&source| {
                    bank.read_route(Index::new(source).expect("a source"))
                        .map()
                        .get()
                        != 0
                })
                .count()
        }
    }

    /// Reset causes `code` for every hart.
    fn causes(code: u32) -> ResetCauses {
        let mut words = vec![code << 1; instances::lp_aon_clkrst::SIZE / 4];
        ResetCauses::read(ResetCauseRegisters::host(&mut words).expect("host words"))
    }

    /// Core control with hart 1 parked, as containment leaves it, and its stall
    /// status reading not stalled.
    fn held() -> Memory {
        let mut memory = Memory::new(0);
        memory.0[4].fill(0);
        let mut words = memory.registers();
        park(&mut words, HART);
        memory
    }

    fn boot_word(cell: &ClaimCell<HostRegisters<'_>>) -> u32 {
        operate(cell, |words| words.boot_address(HART)).expect("installed words")
    }

    fn hart_state(cell: &ClaimCell<HostRegisters<'_>>) -> CoreState {
        operate(cell, |words| state(words, HART)).expect("installed words")
    }

    #[test]
    fn a_held_hart_is_released_into_its_entry_after_its_routes_and_the_rom_slots() {
        let mut memory = held();
        let cell = memory.cell();
        let mut permit = Permit::new(&[3, 40]);
        let cleared = Cell::new(false);
        let mut released = start_with(&cell, permit.permit(), causes(0x17), entry(), || {
            // The ROM slots are cleared before the release writes anything.
            assert_eq!(boot_word(&cell), 0);
            assert_eq!(hart_state(&cell), CoreState::Held);
            cleared.set(true);
        })
        .expect("a held hart starts");
        assert!(cleared.get());
        assert_eq!(released.hart(), HART);
        assert_eq!(released.disconnected(), (2, Some(3)));
        assert_eq!(boot_word(&cell), ENTRY, "the boot word names the entry");
        assert_ne!(
            hart_state(&cell),
            CoreState::Held,
            "clocks open, reset released"
        );
        assert_eq!(retire_with(&cell, &mut released), Ok(()));
        assert_eq!(boot_word(&cell), 0, "the retired word reads back zero");
        drop(released);
        assert_eq!(permit.routed(), 0, "every route was disconnected");
    }

    #[test]
    fn the_released_harts_grant_moves_out_once_and_builds_only_on_hart_1() {
        let mut memory = held();
        let cell = memory.cell();
        let mut permit = Permit::new(&[]);
        let mut released = start_with(&cell, permit.permit(), causes(0x17), entry(), || ())
            .expect("a held hart starts");
        let grant = released.take_interrupts().expect("the grant is there once");
        assert!(
            released.take_interrupts().is_none(),
            "the grant moves out once"
        );
        assert_eq!(grant.hart(), 1);
        assert_eq!(
            grant.into_controller_on(0).err(),
            Some(crate::platform::ForeignHart {
                actual: 0,
                expected: 1
            }),
            "hart 0 would reach its own CLIC through hart 1's parts"
        );
    }

    #[test]
    fn hart_1_sets_its_own_level_bits_when_it_builds_its_controller() {
        use oer_espressif_interrupt_table::Controller as _;
        let mut memory = held();
        let cell = memory.cell();
        let mut permit = Permit::new(&[]);
        let mut released = start_with(&cell, permit.permit(), causes(0x17), entry(), || ())
            .expect("a held hart starts");
        let controller = released
            .take_interrupts()
            .expect("the grant")
            .into_controller_on(1)
            .expect("hart 1 builds its own controller");
        assert_eq!(controller.hart(), 1);
        drop(controller);
        drop(released);
        let (_, config) = oer_esp32s31_pac::clic::clic_host(&mut permit.clic).expect("host CLIC");
        assert_eq!(
            u32::from(crate::platform::interrupt_controller::facts::LEVEL_BITS),
            config.read_int_config().mnlbits().get()
        );
    }

    fn refused(
        memory: &mut Memory,
        permit: &mut Permit,
        causes: ResetCauses,
        expected: StartError,
    ) {
        let cell = memory.cell();
        let slots = Cell::new(false);
        let result = start_with(&cell, permit.permit(), causes, entry(), || slots.set(true));
        assert_eq!(result.err(), Some(expected));
        assert!(!slots.get(), "a refusal leaves the ROM slots alone");
        assert_eq!(boot_word(&cell), 0, "a refusal writes no boot word");
    }

    #[test]
    fn a_hart_that_is_not_held_is_refused_and_stays_untouched() {
        let mut memory = Memory::new(0);
        memory.0[4].fill(0);
        let mut permit = Permit::new(&[5]);
        refused(
            &mut memory,
            &mut permit,
            causes(0x17),
            StartError::NotHeld {
                state: CoreState::Released,
            },
        );
        assert_eq!(permit.routed(), 1, "no route is touched");
    }

    #[test]
    fn a_shut_fetch_bus_is_refused_with_the_hart_still_held() {
        let mut memory = held();
        let mut permit = Permit::new(&[]);
        permit.cache.fill(u32::MAX);
        refused(
            &mut memory,
            &mut permit,
            causes(0x17),
            StartError::FetchBusShut {
                ibus1: true,
                dbus0: true,
            },
        );
        let cell = memory.cell();
        assert_eq!(hart_state(&cell), CoreState::Held);
    }

    #[test]
    fn an_armed_wake_stub_after_a_power_down_reset_is_refused() {
        for cause in [5, 6] {
            let mut memory = held();
            let mut permit = Permit::new(&[7]);
            permit.lp_sys.fill(0x2f00_2000);
            refused(
                &mut memory,
                &mut permit,
                causes(cause),
                StartError::WakeStubArmed {
                    cause: cause as u8,
                    store8: 0x2f00_2000,
                },
            );
            assert_eq!(permit.routed(), 1, "no route is touched");
        }
        // Deep sleep (bit 0), an empty stub or another cause takes the boot word.
        for (cause, store8) in [(5, 0x2f00_2001), (5, 0), (6, 3), (0x17, 0x2f00_2000)] {
            let mut memory = held();
            let cell = memory.cell();
            let mut permit = Permit::new(&[]);
            permit.lp_sys.fill(store8);
            assert!(start_with(&cell, permit.permit(), causes(cause), entry(), || ()).is_ok());
        }
    }

    #[test]
    fn unavailable_words_refuse_before_any_write() {
        let empty = ClaimCell::<HostRegisters<'_>>::new();
        let mut permit = Permit::new(&[9]);
        assert_eq!(
            start_with(&empty, permit.permit(), causes(0x17), entry(), || ()).err(),
            Some(StartError::Cores(CoresError::NotInstalled))
        );
        assert_eq!(permit.routed(), 1);
    }
}

#[test]
fn a_reset_from_hart_one_lets_the_boot_hart_run_after_its_pulse() {
    let mut memory = Memory::new(0);
    let secondary = Hart::SECONDARIES[0].hart();
    prepare_reset(&mut memory.registers(), secondary);
    assert_eq!(memory.0[4][0x164 / 4], 0);
    let registers = memory.registers();
    // A stall request for the boot hart would survive the system reset.
    assert!(!registers.stall_requested(crate::platform::BOOT_HART));
    assert!(!registers.stall_requested(secondary));
    drop(registers);
    let [_, _, _, reset, _] = &memory.0;
    assert_eq!(
        reset.iter().filter(|word| **word != 0).count(),
        1,
        "only the boot hart's reset pulsed"
    );
}
