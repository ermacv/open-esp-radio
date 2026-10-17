use std::{collections::VecDeque, vec, vec::Vec};

use super::{BluetoothDiagnosticUnsettled, BluetoothSchedulerStop, Control, Progress, step};

struct Model {
    /// `None` is a sample whose attempt budget ran out.
    busy: VecDeque<Option<bool>>,
    c0: bool,
    c1: bool,
    trace: Vec<&'static str>,
}

impl Model {
    fn read_busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.trace.push("busy");
        self.busy
            .pop_front()
            .unwrap()
            .ok_or(BluetoothDiagnosticUnsettled)
    }
}

impl Control for Model {
    type Stopped = ();

    fn busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.read_busy()
    }
    fn preamble(&mut self) {
        self.trace.extend(["mask", "disable-run", "fence"]);
    }
    fn commands_ready(&mut self) -> bool {
        self.trace.push("command-0");
        if !self.c0 {
            return false;
        }
        self.trace.push("command-1");
        self.c1
    }
    fn request(&mut self) {
        self.trace.extend(["request", "fence"]);
    }
    fn confirm_stopped(&mut self) -> Result<Option<()>, BluetoothDiagnosticUnsettled> {
        if self.read_busy()? {
            return Ok(None);
        }
        self.trace.push("fence");
        Ok(Some(()))
    }
}

fn model(busy: &[bool], c0: bool, c1: bool) -> Model {
    Model {
        busy: busy.iter().copied().map(Some).collect(),
        c0,
        c1,
        trace: vec![],
    }
}

#[test]
fn idle_has_no_lifecycle_side_effect() {
    let mut hw = model(&[false], false, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw).unwrap(),
        Progress::Stopped(())
    ));
    assert_eq!(hw.trace, ["busy", "fence"]);
}

#[test]
fn silence_stop_waits_for_preamble_and_publishes_only_once() {
    let mut hw = model(&[true, true], false, false);
    let Progress::Pending(stop) = step(BluetoothSchedulerStop::default(), &mut hw).unwrap() else {
        panic!()
    };
    assert_eq!(
        hw.trace,
        ["busy", "mask", "disable-run", "fence", "busy", "command-0"]
    );
    hw.c0 = true;
    hw.c1 = true;
    hw.busy.extend([Some(true), Some(true)]);
    hw.trace.clear();
    let Progress::Pending(stop) = step(stop, &mut hw).unwrap() else {
        panic!()
    };
    assert_eq!(
        hw.trace,
        ["busy", "command-0", "command-1", "request", "fence", "busy"]
    );
    hw.busy.push_back(Some(false));
    hw.trace.clear();
    assert!(matches!(
        step(stop, &mut hw).unwrap(),
        Progress::Stopped(())
    ));
    assert_eq!(hw.trace, ["busy", "fence"]);
}

#[test]
fn completion_racing_preamble_skips_command_reads() {
    let mut hw = model(&[true, false, false], false, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw).unwrap(),
        Progress::Stopped(())
    ));
    assert_eq!(
        hw.trace,
        [
            "busy",
            "mask",
            "disable-run",
            "fence",
            "busy",
            "request",
            "fence",
            "busy",
            "fence"
        ]
    );
}

#[test]
fn second_command_not_ready_keeps_owner_without_request() {
    let mut hw = model(&[true, true], true, false);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw).unwrap(),
        Progress::Pending(_)
    ));
    assert!(!hw.trace.contains(&"request"));
}

#[test]
fn unsettled_busy_sample_fails_the_step_before_any_request() {
    let mut hw = model(&[true], false, false);
    hw.busy.push_back(None);
    assert!(matches!(
        step(BluetoothSchedulerStop::default(), &mut hw),
        Err(BluetoothDiagnosticUnsettled)
    ));
    assert_eq!(hw.trace, ["busy", "mask", "disable-run", "fence", "busy"]);
}
