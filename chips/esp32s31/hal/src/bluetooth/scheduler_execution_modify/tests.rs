use std::{collections::VecDeque, vec::Vec};

use oer_esp32s31_pac::{
    BluetoothSchedulerExecutionModifyDisposition as Disposition,
    BluetoothSchedulerHardwareListIndex,
};

use super::{BluetoothDiagnosticUnsettled, Control, Phase, State, step};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Busy,
    EnginesIdle,
    SelectProgress,
    Publish(bool),
    Repeats,
    Ready,
    SelectSettle,
    Settled,
    ClearStart,
    Rejected,
}

#[derive(Default)]
struct Model {
    ops: Vec<Op>,
    busy: VecDeque<bool>,
    engines_idle: VecDeque<bool>,
    repeats: VecDeque<bool>,
    ready: VecDeque<bool>,
    settled: VecDeque<bool>,
    /// Every diagnostic sample exhausts its attempt budget.
    unsettled: bool,
    rejected: bool,
}

impl Model {
    fn sample(
        &mut self,
        value: impl FnOnce(&mut Self) -> bool,
    ) -> Result<bool, BluetoothDiagnosticUnsettled> {
        if self.unsettled {
            Err(BluetoothDiagnosticUnsettled)
        } else {
            Ok(value(self))
        }
    }
}

impl Control for Model {
    type Published = ();

    fn busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.ops.push(Op::Busy);
        self.sample(|model| model.busy.pop_front().unwrap_or(false))
    }
    fn engines_idle(&mut self) -> bool {
        self.ops.push(Op::EnginesIdle);
        self.engines_idle.pop_front().unwrap_or(true)
    }
    fn select_progress(&mut self) {
        self.ops.push(Op::SelectProgress);
    }
    fn publish(&mut self, _: BluetoothSchedulerHardwareListIndex, list_deletion: bool) {
        self.ops.push(Op::Publish(list_deletion));
    }
    fn repeats(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.ops.push(Op::Repeats);
        self.sample(|model| model.repeats.pop_front().unwrap_or(false))
    }
    fn ready(&mut self) -> bool {
        self.ops.push(Op::Ready);
        self.ready.pop_front().unwrap_or(true)
    }
    fn select_settle(&mut self) {
        self.ops.push(Op::SelectSettle);
    }
    fn settled(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.ops.push(Op::Settled);
        self.sample(|model| model.settled.pop_front().unwrap_or(true))
    }
    fn clear_start(&mut self, (): ()) {
        self.ops.push(Op::ClearStart);
    }
    fn rejected(&mut self) -> bool {
        self.ops.push(Op::Rejected);
        self.rejected
    }
}

struct Request {
    phase: Phase,
    repeat: bool,
    published: Option<()>,
}

impl Request {
    fn new() -> Self {
        Self {
            phase: Phase::Preamble,
            repeat: false,
            published: None,
        }
    }

    fn step(&mut self, model: &mut Model, list_deletion: bool) -> Disposition {
        self.try_step(model, list_deletion).unwrap()
    }

    fn try_step(
        &mut self,
        model: &mut Model,
        list_deletion: bool,
    ) -> Result<Disposition, BluetoothDiagnosticUnsettled> {
        step(
            State {
                index: BluetoothSchedulerHardwareListIndex::ZERO,
                list_deletion,
                phase: &mut self.phase,
                repeat: &mut self.repeat,
                published: &mut self.published,
            },
            model,
        )
    }
}

#[test]
fn an_idle_scheduler_publishes_and_settles_in_the_vendor_order() {
    let mut model = Model::default();
    let mut request = Request::new();
    assert_eq!(request.step(&mut model, true), Disposition::Ready);
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::SelectProgress,
            Op::Publish(true),
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected,
        ]
    );
    assert!(request.published.is_some(), "the owner clears START");
}

#[test]
fn busy_engines_hold_the_request_and_a_busy_scheduler_waits_for_ready() {
    let mut model = Model {
        busy: VecDeque::from([true, true, true, true]),
        engines_idle: VecDeque::from([false, true]),
        ready: VecDeque::from([false, true]),
        ..Model::default()
    };
    let mut request = Request::new();
    assert_eq!(request.step(&mut model, false), Disposition::Pending);
    assert_eq!(model.ops, [Op::Busy, Op::EnginesIdle]);
    model.ops.clear();
    assert_eq!(request.step(&mut model, false), Disposition::Pending);
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::EnginesIdle,
            Op::SelectProgress,
            Op::Publish(false),
            Op::Repeats,
            Op::Busy,
            Op::Ready,
        ]
    );
    model.ops.clear();
    assert_eq!(request.step(&mut model, false), Disposition::Ready);
    assert_eq!(
        model.ops,
        [
            Op::Busy,
            Op::Ready,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected
        ]
    );
}

#[test]
fn a_conflict_repeats_the_request_after_it_settles() {
    let mut model = Model {
        repeats: VecDeque::from([true, false, false]),
        settled: VecDeque::from([false, true, true]),
        ..Model::default()
    };
    let mut request = Request::new();
    // The conflict is observed right after publication.
    assert_eq!(request.step(&mut model, false), Disposition::Pending);
    model.ops.clear();
    // A resumed observation selects the progress signal again.
    assert_eq!(request.step(&mut model, false), Disposition::Pending);
    assert_eq!(
        model.ops,
        [
            Op::SelectProgress,
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
        ]
    );
    model.ops.clear();
    assert_eq!(request.step(&mut model, false), Disposition::Ready);
    assert_eq!(
        model.ops,
        [
            Op::SelectSettle,
            Op::Settled,
            Op::ClearStart,
            Op::Busy,
            Op::SelectProgress,
            Op::Publish(false),
            Op::Repeats,
            Op::Busy,
            Op::SelectSettle,
            Op::Settled,
            Op::Rejected,
        ]
    );
}

#[test]
fn a_set_status_19_is_a_hardware_rejection() {
    let mut model = Model {
        rejected: true,
        ..Model::default()
    };
    assert_eq!(
        Request::new().step(&mut model, false),
        Disposition::HardwareRejected
    );
}

#[test]
fn an_unsettled_diagnostic_sample_fails_the_step_without_publishing() {
    let mut model = Model {
        unsettled: true,
        ..Model::default()
    };
    let mut request = Request::new();
    assert_eq!(
        request.try_step(&mut model, false),
        Err(BluetoothDiagnosticUnsettled)
    );
    assert_eq!(model.ops, [Op::Busy]);
    assert!(request.published.is_none());
}
