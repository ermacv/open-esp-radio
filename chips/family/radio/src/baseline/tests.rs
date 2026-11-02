use super::*;
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::{cell::RefCell, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    SysconWrite,
    SysconRead,
    SourceWrite,
    SourceRead,
    BusWrite,
    BusRead,
}
struct Port {
    baseline: u32,
    fail: Option<PowerRestoreCheckpoint>,
    steps: RefCell<Vec<(Instant, Step, u32)>>,
    time: RefCell<VirtualClock>,
}
impl Port {
    fn record(&self, step: Step, value: u32) {
        let clock = self.time.borrow();
        self.steps.borrow_mut().push((clock.now(), step, value));
        clock.advance(Duration::from_micros(1)).unwrap();
    }
}
impl PowerBaselinePort for Port {
    type Baseline = u32;
    fn restore_syscon(&mut self, baseline: u32) {
        self.record(Step::SysconWrite, baseline);
    }
    fn syscon_matches(&self, baseline: u32) -> bool {
        self.record(Step::SysconRead, baseline);
        self.fail != Some(PowerRestoreCheckpoint::ModemSyscon) && baseline == self.baseline
    }
    fn restore_source(&mut self, baseline: u32) {
        self.record(Step::SourceWrite, baseline);
    }
    fn source_matches(&self, baseline: u32) -> bool {
        self.record(Step::SourceRead, baseline);
        self.fail != Some(PowerRestoreCheckpoint::ModemSourceClocks) && baseline == self.baseline
    }
    fn restore_bus(&mut self, baseline: u32) {
        self.record(Step::BusWrite, baseline);
    }
    fn bus_matches(&self, baseline: u32) -> bool {
        self.record(Step::BusRead, baseline);
        self.fail != Some(PowerRestoreCheckpoint::ModemRegisterBusClock)
            && baseline == self.baseline
    }
}
#[test]
fn each_readback_precedes_the_next_write_and_failure_retains_the_suffix() {
    let order = [
        Step::SysconWrite,
        Step::SysconRead,
        Step::SourceWrite,
        Step::SourceRead,
        Step::BusWrite,
        Step::BusRead,
    ];
    for (fail, length) in [
        (None, 6),
        (Some(PowerRestoreCheckpoint::ModemSyscon), 2),
        (Some(PowerRestoreCheckpoint::ModemSourceClocks), 4),
        (Some(PowerRestoreCheckpoint::ModemRegisterBusClock), 6),
    ] {
        let mut port = Port {
            baseline: 42,
            fail,
            steps: RefCell::default(),
            time: RefCell::new(VirtualClock::default()),
        };
        let begin = port.time.borrow().now();
        assert_eq!(restore(&mut port, 42), fail.map_or(Ok(()), Err));
        assert_eq!(port.steps.borrow().len(), length);
        for ((at, step, baseline), expected) in port.steps.borrow().iter().zip(order) {
            assert_eq!(*step, expected);
            assert_eq!(*baseline, 42);
            assert!(*at >= begin);
        }
        assert_eq!(
            port.time.borrow().now().checked_duration_since(begin),
            Some(Duration::from_micros(length as u64))
        );
        port.fail = None;
        port.steps.borrow_mut().clear();
        assert_eq!(
            restore(&mut port, 42),
            Ok(()),
            "retry uses the same opaque baseline"
        );
        assert_eq!(
            port.steps
                .borrow()
                .iter()
                .map(|(_, step, _)| *step)
                .collect::<Vec<_>>(),
            order
        );
    }
}
