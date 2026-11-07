use core::cell::Cell;
use std::rc::Rc;

use super::*;

struct Hardware {
    state: bool,
    observations: &'static [bool],
    next_observation: Cell<usize>,
    writes: std::vec::Vec<bool>,
}

impl Hardware {
    fn new(state: bool, observations: &'static [bool]) -> Self {
        Self {
            state,
            observations,
            next_observation: Cell::new(0),
            writes: std::vec::Vec::new(),
        }
    }
}

impl MacRuntimeLowRateHardware for Hardware {
    fn phy_low_rate_enabled(&self) -> bool {
        let index = self.next_observation.get();
        if let Some(observed) = self.observations.get(index) {
            self.next_observation.set(index + 1);
            *observed
        } else {
            self.state
        }
    }

    fn configure_phy_low_rate(&mut self, enabled: bool) {
        self.state = enabled;
        self.writes.push(enabled);
    }
}

#[test]
fn dropped_session_restores_the_disabled_entry_state() {
    let mut hardware = Hardware::new(false, &[]);
    {
        let mut session = MacLowRateSession::activate(&mut hardware).unwrap();
        assert_eq!(session.previous_state(), MacLowRateState::Disabled);
        assert!(session.hardware_mut().state);
    }
    assert!(!hardware.state);
    assert_eq!(hardware.writes, [true, false]);
}

#[test]
fn already_enabled_entry_state_is_preserved() {
    let mut hardware = Hardware::new(true, &[]);
    let session = MacLowRateSession::activate(&mut hardware).unwrap();
    assert_eq!(session.previous_state(), MacLowRateState::Enabled);
    assert!(session.restore().is_ok());
    assert!(hardware.state);
    assert_eq!(hardware.writes, [true, true]);
}

#[test]
fn failed_activation_rolls_back_before_returning_the_owner() {
    let mut hardware = Hardware::new(false, &[false, false, false]);
    assert_eq!(
        MacLowRateSession::activate(&mut hardware).err(),
        Some(MacLowRateTransitionError {
            transition: MacLowRateTransition::Activate,
            expected: MacLowRateState::Enabled,
            observed: MacLowRateState::Disabled,
        })
    );
    assert!(!hardware.state);
    assert_eq!(hardware.writes, [true, false]);
}

#[test]
fn probe_reports_first_restore_mismatch_only_after_retry_restores_state() {
    let mut hardware = Hardware::new(false, &[false, true, true, false]);
    assert_eq!(
        probe_phy_low_rate_gate(&mut hardware),
        Err(MacLowRateTransitionError {
            transition: MacLowRateTransition::Restore,
            expected: MacLowRateState::Disabled,
            observed: MacLowRateState::Enabled,
        })
    );
    assert!(!hardware.state);
    assert_eq!(hardware.writes, [true, false, false]);
}

#[test]
fn persistent_probe_mismatch_panics_after_four_bounded_restore_attempts() {
    struct PersistentMismatchHardware {
        observations: Cell<usize>,
        writes: Rc<Cell<usize>>,
    }

    impl MacRuntimeLowRateHardware for PersistentMismatchHardware {
        fn phy_low_rate_enabled(&self) -> bool {
            let observation = self.observations.get();
            self.observations.set(observation + 1);
            // Disabled at entry; every later readback claims Enabled so
            // activation succeeds and every restore attempt mismatches.
            observation != 0
        }

        fn configure_phy_low_rate(&mut self, _enabled: bool) {
            self.writes.set(self.writes.get() + 1);
        }
    }

    let writes = Rc::new(Cell::new(0));
    let diagnostic = Rc::clone(&writes);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut hardware = PersistentMismatchHardware {
            observations: Cell::new(0),
            writes: diagnostic,
        };
        let _ = probe_phy_low_rate_gate(&mut hardware);
    }));

    assert!(panic.is_err());
    // One activation write plus two explicit and two Drop restore writes.
    assert_eq!(writes.get(), 5);
}
