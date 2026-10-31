//! Recording ports test order and read bounds, independently of MMIO layout.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

use super::*;
use core::cell::RefCell;
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Stall(usize),
    Run(usize),
    Hold(usize),
    Clear(usize),
    Reset(usize),
    Held(usize),
    Status(usize),
    Fence,
    CpuClock(usize),
    ClicClock(usize),
    Release(usize),
    BootWord(usize, u32),
}
struct Fake {
    events: RefCell<Vec<Event>>,
    held: [bool; 3],
    samples: RefCell<std::collections::VecDeque<bool>>,
    clock: VirtualClock,
    boot_words: [u32; 3],
    stuck_boot_word: Option<u32>,
}
impl Fake {
    fn new(samples: impl IntoIterator<Item = bool>) -> Self {
        Self {
            events: RefCell::new(Vec::new()),
            held: [false; 3],
            samples: RefCell::new(samples.into_iter().collect()),
            clock: VirtualClock::starting_at(Instant::EPOCH),
            boot_words: [0; 3],
            stuck_boot_word: None,
        }
    }
    fn event(&self, event: Event) {
        self.events.borrow_mut().push(event);
        self.clock
            .advance(Duration::from_micros(1))
            .expect("test time fits");
    }
}
impl Registers for Fake {
    type Secondary = usize;
    const SECONDARIES: &'static [usize] = &[1, 2];
    fn request_stall(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Stall(hart));
    }
    fn hold(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Hold(hart));
    }
    fn clear_boot_address(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Clear(hart));
    }
}
impl RuntimeRegisters for Fake {
    type Hart = usize;
    type Stops = [Option<Stopped>; 3];
    const HARTS: &'static [usize] = &[0, 1, 2];
    fn hart(hart: usize) -> usize {
        hart
    }
    fn save_stop(stops: &mut Self::Stops, hart: usize, stopped: Stopped) {
        *stops.get_mut(hart).expect("fake hart exists") = Some(stopped);
    }
    fn fence(&self, _: Access<'_>) {
        self.event(Event::Fence);
    }
    fn held(&self, _: Access<'_>, hart: usize) -> bool {
        self.event(Event::Held(hart));
        *self.held.get(hart).expect("fake hart exists")
    }
    fn is_stalled(&self, _: Access<'_>, hart: usize) -> bool {
        self.event(Event::Status(hart));
        self.samples
            .borrow_mut()
            .pop_front()
            .expect("sample budget")
    }
    fn request_peer_stall(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Stall(hart));
    }
    fn request_run(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Run(hart));
    }
    fn reset_core(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Reset(hart));
    }
}
impl LaunchRegisters for Fake {
    fn enable_cpu_clock(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::CpuClock(hart));
    }
    fn enable_clic_clock(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::ClicClock(hart));
    }
    fn release_reset(&mut self, _: Access<'_>, hart: usize) {
        self.event(Event::Release(hart));
    }
    fn set_boot_address(&mut self, _: Access<'_>, hart: usize, entry: u32) {
        self.event(Event::BootWord(hart, entry));
        *self.boot_words.get_mut(hart).expect("fake hart exists") =
            self.stuck_boot_word.unwrap_or(entry);
    }
    fn boot_address(&self, _: Access<'_>, hart: usize) -> u32 {
        *self.boot_words.get(hart).expect("fake hart exists")
    }
}

#[test]
fn release_confirms_the_run_and_writes_the_boot_word_last() {
    let mut fake = Fake::new([true, false]);
    assert_eq!(release(&mut fake, 1, 0x2f00_1000, 3), Ok(()));
    assert_eq!(
        *fake.events.borrow(),
        [
            Event::Run(1),
            Event::Fence,
            Event::Status(1),
            Event::Status(1),
            Event::Fence,
            Event::CpuClock(1),
            Event::ClicClock(1),
            Event::Release(1),
            Event::Fence,
            Event::BootWord(1, 0x2f00_1000),
            Event::Fence,
        ]
    );
}

#[test]
fn an_unconfirmed_run_writes_no_clock_reset_or_boot_word() {
    let mut fake = Fake::new([true, true, true]);
    assert_eq!(release(&mut fake, 1, 0x2f00_1000, 3), Err(Unconfirmed));
    assert!(
        fake.events
            .borrow()
            .iter()
            .all(|event| matches!(event, Event::Run(_) | Event::Fence | Event::Status(_)))
    );
    assert_eq!(fake.boot_words, [0; 3]);
}

#[test]
fn retirement_clears_fences_and_refuses_a_nonzero_read_back() {
    let mut fake = Fake::new([]);
    fake.boot_words[1] = 0x2f00_1000;
    assert_eq!(retire_boot_address(&mut fake, 1), Ok(()));
    assert_eq!(*fake.events.borrow(), [Event::BootWord(1, 0), Event::Fence]);
    let mut fake = Fake::new([]);
    fake.stuck_boot_word = Some(0x2f00_1000);
    assert_eq!(
        retire_boot_address(&mut fake, 1),
        Err(BootWordKept {
            readback: 0x2f00_1000
        })
    );
}

#[test]
fn confirmation_checks_the_last_allowed_read_and_fences_only_success() {
    let mut fake = Fake::new([false, false, true]);
    assert_eq!(stall(&mut fake, 2, 3), Ok(()));
    assert_eq!(
        *fake.events.borrow(),
        [
            Event::Stall(2),
            Event::Fence,
            Event::Status(2),
            Event::Status(2),
            Event::Status(2),
            Event::Fence
        ]
    );
    assert_eq!(fake.clock.now().as_micros(), 6);
    let mut fake = Fake::new([false, false, false, true]);
    assert_eq!(stall(&mut fake, 2, 3), Err(Unconfirmed));
    assert_eq!(fake.samples.borrow().len(), 1, "no fourth read");
    assert_eq!(fake.events.borrow().last(), Some(&Event::Status(2)));
    let mut fake = Fake::new([]);
    assert_eq!(stall(&mut fake, 2, 0), Err(Unconfirmed));
    assert_eq!(*fake.events.borrow(), [Event::Stall(2), Event::Fence]);
}
#[test]
fn run_waits_for_false_and_the_default_budget_counts_completed_reads() {
    let mut fake = Fake::new([true, false]);
    assert_eq!(run(&mut fake, 1, 2), Ok(()));
    assert_eq!(
        *fake.events.borrow(),
        [
            Event::Run(1),
            Event::Fence,
            Event::Status(1),
            Event::Status(1),
            Event::Fence
        ]
    );
    let mut fake = Fake::new(core::iter::repeat_n(false, STALL_POLLS as usize));
    assert_eq!(stall(&mut fake, 1, STALL_POLLS), Err(Unconfirmed));
    assert_eq!(fake.clock.now().as_micros(), u64::from(STALL_POLLS) + 2);
}
#[test]
fn status_samples_held_before_stalled_and_park_has_no_confirmation() {
    let mut fake = Fake::new([true, false]);
    fake.held[1] = true;
    assert_eq!(state(&fake, 1), CoreState::Held);
    fake.held[1] = false;
    assert_eq!(state(&fake, 1), CoreState::Stalled);
    assert_eq!(state(&fake, 1), CoreState::Released);
    fake.events.borrow_mut().clear();
    park(&mut fake, 2);
    assert_eq!(*fake.events.borrow(), [Event::Stall(2), Event::Hold(2)]);
}
#[test]
fn stop_skips_self_and_held_peers_and_stops_on_first_unconfirmed_peer() {
    let mut fake = Fake::new([false, true]);
    fake.held[0] = true;
    assert_eq!(
        stop_other(&mut fake, 1, 2),
        Ok([Some(Stopped::Held), None, Some(Stopped::Stalled)])
    );
    assert_eq!(
        *fake.events.borrow(),
        [
            Event::Held(0),
            Event::Held(2),
            Event::Stall(2),
            Event::Fence,
            Event::Status(2),
            Event::Status(2),
            Event::Fence
        ]
    );
    let mut fake = Fake::new([false]);
    assert_eq!(stop_other(&mut fake, 0, 1), Err(Unconfirmed));
    assert_eq!(
        *fake.events.borrow(),
        [
            Event::Held(1),
            Event::Stall(1),
            Event::Fence,
            Event::Status(1)
        ]
    );
}
#[test]
fn reset_clears_all_entries_before_peer_pulses_from_every_hart() {
    for this in 0..3 {
        let mut fake = Fake::new([]);
        prepare_reset(&mut fake, this);
        let mut expected = std::vec![Event::Clear(1), Event::Clear(2)];
        for hart in 0..3 {
            if hart != this {
                expected.extend([Event::Reset(hart), Event::Stall(hart)]);
            }
        }
        assert_eq!(*fake.events.borrow(), expected);
    }
    let mut fake = Fake::new([]);
    clear_boot_addresses(&mut fake);
    assert_eq!(
        *fake.events.borrow(),
        [Event::Clear(1), Event::Clear(2)],
        "invalid identity selects no peer"
    );
}
