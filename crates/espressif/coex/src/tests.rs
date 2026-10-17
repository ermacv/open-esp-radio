use std::{cell::RefCell, rc::Rc};

use super::*;

mod failure;

type OperationTrace = Rc<RefCell<std::vec::Vec<&'static str>>>;

#[derive(Default)]
struct TimerModel {
    programmed: Option<(u8, u8, u8, u32, u32)>,
    enabled: u8,
    disabled: u8,
    operations: OperationTrace,
}

impl CoexTimerHardware for TimerModel {
    fn pti(&mut self, event: CoexEventId) -> CoexPti {
        CoexPtiTable::VENDOR.pti(event)
    }

    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        self.programmed = Some((
            index.value(),
            client.timer_client_value(),
            pti.value(),
            0,
            0,
        ));
        self.operations.borrow_mut().push("configure");
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        _index: CoexTimerIndex,
        primary: u32,
    ) -> Result<(), CoexError> {
        self.programmed.as_mut().unwrap().3 = primary;
        self.operations.borrow_mut().push("primary");
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        _index: CoexTimerIndex,
        secondary: u32,
    ) -> Result<(), CoexError> {
        self.programmed.as_mut().unwrap().4 = secondary;
        self.operations.borrow_mut().push("secondary");
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.enabled |= 1 << index.value();
        self.operations.borrow_mut().push("enable");
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.disabled |= 1 << index.value();
        self.enabled &= !(1 << index.value());
        Ok(())
    }

    fn force(&mut self, _: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }

    fn unforce(&mut self, _: CoexTimerIndex) -> Result<(), CoexError> {
        Ok(())
    }
}

struct ClockModel {
    clock: CoexTimerClock,
    samples: u8,
    operations: OperationTrace,
}

struct FailingClock;

impl CoexClockHardware for FailingClock {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        Err(CoexError::UnsupportedClock)
    }
}

impl CoexClockHardware for ClockModel {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        self.samples += 1;
        self.operations.borrow_mut().push("clock");
        Ok(self.clock)
    }
}

#[test]
fn reviewed_event_durations_match_the_vendor_parameter_object() {
    let durations = CoexEventDurations::reviewed_vendor();
    assert_eq!(
        durations.as_words(),
        &[25_000, 20_000, 5_000, 25_000, 50_000]
    );
    for (event, expected) in [
        (4, 25_000),
        (7, 20_000),
        (9, 5_000),
        (45, 25_000),
        (46, 50_000),
    ] {
        assert_eq!(
            durations.duration(CoexEventId::new(event).unwrap()),
            Some(expected)
        );
    }
    assert_eq!(durations.duration(CoexEventId::new(8).unwrap()), None);
}

#[test]
fn timer_map_rejects_ff_entries_and_reaches_every_timer() {
    let mapped: [u8; 6] = [1, 2, 36, 38, 45, 46];
    let indices = mapped.map(|event| {
        crate::timer_index(CoexEventId::new(event).unwrap())
            .unwrap()
            .value()
    });
    assert_eq!(indices, [0, 1, 4, 2, 3, 3]);
    assert_eq!(crate::timer_index(CoexEventId::new(3).unwrap()), None);
    assert_eq!(crate::timer_index(CoexEventId::new(47).unwrap()), None);
}

/// Event 48 selects the arbiter's grant-protect timer, which no policy
/// request may program or withdraw.
#[test]
fn the_grant_protect_event_has_no_policy_timer() {
    assert_eq!(crate::timer_index(CoexEventId::new(48).unwrap()), None);
}

#[test]
fn clock_conversion_matches_instruction_level_constants() {
    let fast = CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector8, 0, 40, true);
    assert_eq!(fast.tick_image(1_000).unwrap(), 32);
    assert_eq!(fast.tick_image(1_000_000).unwrap(), 32_768);
    let emulator = CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector8, 0, 40, false);
    assert_eq!(emulator.tick_image(1_000_000).unwrap(), 32_000);
    let slow = CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector1, 0, 40, true);
    assert_eq!(slow.tick_image(1).unwrap(), 524_288);
    let source_two =
        CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector2, 39, 40, true);
    assert_eq!(source_two.tick_image(1_000).unwrap(), 500);
    let source_four =
        CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector4, 49, 40, true);
    assert_eq!(source_four.tick_image(1_000).unwrap(), 800);
}

#[test]
fn bluetooth_and_wifi_requests_program_the_vendor_timer_clients() {
    let request = CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 2,
        duration: 3,
    };
    for (bluetooth, client) in [(true, 2), (false, 1)] {
        let mut core = CoexCore::new();
        let operations = OperationTrace::default();
        let mut hardware = TimerModel {
            operations: operations.clone(),
            ..TimerModel::default()
        };
        let mut clock = ClockModel {
            clock: CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector1, 0, 40, true),
            samples: 0,
            operations,
        };
        core.enable();
        if bluetooth {
            core.request_bluetooth(&mut hardware, &mut clock, request)
        } else {
            core.request_wifi(&mut hardware, &mut clock, request)
        }
        .unwrap();
        assert_eq!(hardware.programmed.map(|p| p.1), Some(client));
    }
}

#[test]
fn request_programs_then_enables_and_release_disables() {
    let mut core = CoexCore::new();
    let operations = OperationTrace::default();
    let mut hardware = TimerModel {
        operations: operations.clone(),
        ..TimerModel::default()
    };
    core.enable();
    let request = CoexClientRequest {
        event: CoexEventId::new(1).unwrap(),
        latency: 2,
        duration: 3,
    };
    let mut clock = ClockModel {
        clock: CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector1, 0, 40, true),
        samples: 0,
        operations: operations.clone(),
    };
    assert_eq!(
        core.request_wifi(&mut hardware, &mut clock, request)
            .unwrap(),
        CoexTimerIndex::new(0).unwrap()
    );
    assert_eq!(hardware.programmed, Some((0, 1, 5, 1_572_864, 1_048_576)));
    assert_eq!(clock.samples, 2);
    assert_eq!(
        operations.borrow().as_slice(),
        [
            "configure",
            "clock",
            "primary",
            "clock",
            "secondary",
            "enable"
        ]
    );
    assert_eq!(hardware.enabled, 1);
    assert_eq!(
        core.release(&mut hardware, request.event).unwrap(),
        CoexTimerIndex::new(0).unwrap()
    );
    assert_eq!(hardware.disabled, 1);
    assert_eq!(core.status().active_timers, 0);
}

#[test]
fn unmapped_events_return_vendor_invalid_event_without_hardware_effects() {
    let mut core = CoexCore::new();
    let mut hardware = TimerModel::default();
    let mut clock = ClockModel {
        clock: CoexTimerClock::from_hardware_fields(CoexClockSelector::Selector8, 0, 40, true),
        samples: 0,
        operations: OperationTrace::default(),
    };
    core.enable();
    let event = CoexEventId::new(0).unwrap();
    assert_eq!(
        core.request_wifi(
            &mut hardware,
            &mut clock,
            CoexClientRequest {
                event,
                latency: 0,
                duration: 100,
            },
        ),
        Err(CoexError::InvalidEvent)
    );
    assert_eq!(
        core.release(&mut hardware, event),
        Err(CoexError::InvalidEvent)
    );
    assert!(hardware.operations.borrow().is_empty());
    assert_eq!(clock.samples, 0);
}

#[test]
fn unsupported_clock_never_publishes_an_active_timer() {
    let mut core = CoexCore::new();
    let mut hardware = TimerModel::default();
    core.enable();

    assert_eq!(
        core.request_wifi(
            &mut hardware,
            &mut FailingClock,
            CoexClientRequest {
                event: CoexEventId::new(1).unwrap(),
                latency: 2,
                duration: 3,
            },
        ),
        Err(CoexError::UnsupportedClock)
    );
    assert_eq!(hardware.enabled, 0);
    assert_eq!(core.status().active_timers, 0);
}

#[test]
fn clock_configuration_domains_match_vendor_validation() {
    assert!(CoexClockSelector::Selector8.accepts_divisor(1));
    assert!(!CoexClockSelector::Selector8.accepts_divisor(2));
    assert!(CoexClockSelector::Selector4.accepts_divisor(50));
    assert!(!CoexClockSelector::Selector4.accepts_divisor(49));
    assert!(CoexClockSelector::Selector2.accepts_divisor(40));
    assert!(!CoexClockSelector::Selector2.accepts_divisor(39));
    assert!(CoexClockSelector::Selector1.accepts_divisor(3));
    assert!(!CoexClockSelector::Selector1.accepts_divisor(2));
    // The vendor bounds the divisor only from below.
    assert!(CoexClockSelector::Selector1.accepts_divisor(4097));
}

#[test]
fn recovered_status_bits_and_wifi_events_keep_their_vendor_values() {
    assert_eq!(crate::EXTERNAL_COEX_STATUS, 0x03);
    assert_eq!(crate::IEEE802154_COEX_STATUS, 0x01);
    for (event, value) in [
        (crate::wifi_events::BEACON, 0),
        (crate::wifi_events::SLICE, 1),
        (crate::wifi_events::CONNECTION, 46),
    ] {
        assert_eq!(event.value(), value);
        assert_eq!(CoexEventId::new(value), Some(event));
    }
}
