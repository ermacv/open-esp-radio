use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Configure,
    Primary,
    Secondary,
    Enable,
    Disable,
}

#[derive(Default)]
struct Hardware {
    model: TimerModel,
    fail: Option<Step>,
    disables: usize,
}

impl Hardware {
    fn after_write(&mut self, step: Step) -> Result<(), CoexError> {
        if self.fail == Some(step) {
            self.fail = None;
            Err(CoexError::Hardware)
        } else {
            Ok(())
        }
    }
}

impl CoexTimerHardware for Hardware {
    fn pti(&mut self, event: CoexEventId) -> CoexPti {
        CoexPtiTable::VENDOR.pti(event)
    }

    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        self.model.configure_request(index, client, pti)?;
        self.after_write(Step::Configure)
    }
    fn set_primary_target(&mut self, index: CoexTimerIndex, image: u32) -> Result<(), CoexError> {
        self.model.set_primary_target(index, image)?;
        self.after_write(Step::Primary)
    }
    fn set_secondary_target(&mut self, index: CoexTimerIndex, image: u32) -> Result<(), CoexError> {
        self.model.set_secondary_target(index, image)?;
        self.after_write(Step::Secondary)
    }
    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.model.enable(index)?;
        self.after_write(Step::Enable)
    }
    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.disables += 1;
        self.model.disable(index)?;
        self.after_write(Step::Disable)
    }
    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.model.force(index)
    }
    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.model.unforce(index)
    }
}

fn request() -> CoexClientRequest {
    CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 2,
        duration: 3,
    }
}

fn clock() -> ClockModel {
    ClockModel {
        clock: CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector8, 0, 40, true),
        samples: 0,
        operations: OperationTrace::default(),
    }
}

#[test]
fn every_failed_programming_edge_retains_a_cleanup_obligation() {
    for step in [
        Step::Configure,
        Step::Primary,
        Step::Secondary,
        Step::Enable,
    ] {
        let mut core = CoexCore::new();
        let mut hw = Hardware {
            fail: Some(step),
            ..Hardware::default()
        };
        core.enable();
        assert_eq!(
            core.request_wifi(&mut hw, &mut clock(), request()),
            Err(CoexError::Hardware)
        );
        assert_ne!(
            core.status().uncertain_timers,
            0,
            "lost obligation after {step:?}"
        );
        if step == Step::Enable {
            assert_ne!(
                hw.model.enabled, 0,
                "model must reproduce write-before-error"
            );
        }
        let before = hw.model.operations.borrow().len();
        // Even toggling software enable must not hide unfinished hardware work.
        core.enable();
        assert_eq!(
            core.request_bluetooth(&mut hw, &mut clock(), request()),
            Err(CoexError::RecoveryRequired)
        );
        assert_eq!(hw.model.operations.borrow().len(), before);
        core.disable(&mut hw).unwrap();
        assert_eq!(hw.disables, 1);
        assert_eq!(hw.model.enabled, 0);
        assert_eq!(core.status().uncertain_timers, 0);
        assert!(!core.status().enabled);
        core.enable();
        assert!(core.request_wifi(&mut hw, &mut clock(), request()).is_ok());
    }
}

#[test]
fn a_clock_error_after_configuration_still_needs_cleanup() {
    let mut core = CoexCore::new();
    let mut hw = Hardware::default();
    core.enable();
    assert_eq!(
        core.request_wifi(&mut hw, &mut FailingClock, request()),
        Err(CoexError::UnsupportedClock)
    );
    assert_eq!(hw.model.operations.borrow().as_slice(), ["configure"]);
    assert_ne!(core.status().uncertain_timers, 0);
    core.disable(&mut hw).unwrap();
    assert_eq!(hw.disables, 1);
}

#[test]
fn failed_cleanup_is_not_mistaken_for_stopped_hardware() {
    let mut core = CoexCore::new();
    let mut hw = Hardware::default();
    core.enable();
    core.request_wifi(&mut hw, &mut clock(), request()).unwrap();
    hw.fail = Some(Step::Disable);
    assert_eq!(core.disable(&mut hw), Err(CoexError::Hardware));
    assert!(core.status().enabled);
    assert_ne!(core.status().uncertain_timers, 0);
    assert_eq!(
        core.request_wifi(&mut hw, &mut clock(), request()),
        Err(CoexError::RecoveryRequired)
    );
    core.disable(&mut hw).unwrap();
    assert_eq!(hw.disables, 2);
    assert_eq!(core.status().active_timers, 0);
    assert_eq!(core.status().uncertain_timers, 0);
}

#[test]
fn failed_release_of_an_untracked_timer_is_retried_by_shutdown() {
    let mut core = CoexCore::new();
    let mut hw = Hardware {
        fail: Some(Step::Disable),
        ..Hardware::default()
    };
    core.enable();
    assert_eq!(
        core.release(&mut hw, request().event),
        Err(CoexError::Hardware)
    );
    assert_eq!(core.status().active_timers, 0);
    assert_ne!(core.status().uncertain_timers, 0);
    core.disable(&mut hw).unwrap();
    assert_eq!(hw.disables, 2);
    assert_eq!(core.status().uncertain_timers, 0);
}
