use std::{collections::VecDeque, vec::Vec};

use oer_esp32s31_pac::{
    BluetoothControllerSramAddress, BluetoothSchedulerExecutionLockDisposition as Disposition,
    BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerHardwareListIndex,
};

use super::{BluetoothDiagnosticUnsettled, Control, step};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Busy,
    EnginesIdle,
    Publish,
    Observe,
}

#[derive(Default)]
struct Model {
    ops: Vec<Op>,
    /// `None` is a sample whose attempt budget ran out.
    busy: VecDeque<Option<bool>>,
    engines_idle: VecDeque<bool>,
    observations: VecDeque<Disposition>,
}

impl Control for Model {
    type Published = ();

    fn busy(&mut self) -> Result<bool, BluetoothDiagnosticUnsettled> {
        self.ops.push(Op::Busy);
        self.busy
            .pop_front()
            .unwrap_or(Some(false))
            .ok_or(BluetoothDiagnosticUnsettled)
    }
    fn engines_idle(&mut self) -> bool {
        self.ops.push(Op::EnginesIdle);
        self.engines_idle.pop_front().unwrap_or(true)
    }
    fn publish(&mut self, _: BluetoothSchedulerExecutionLockRequest) {
        self.ops.push(Op::Publish);
    }
    fn observe(&mut self) -> Result<Disposition, BluetoothDiagnosticUnsettled> {
        self.ops.push(Op::Observe);
        Ok(self
            .observations
            .pop_front()
            .unwrap_or(Disposition::ExecutionLockRetained))
    }
}

fn request() -> BluetoothSchedulerExecutionLockRequest {
    BluetoothSchedulerExecutionLockRequest::new(
        BluetoothControllerSramAddress::new(0x2f00_1000).expect("controller SRAM"),
        BluetoothSchedulerHardwareListIndex::new(0).expect("list zero"),
    )
}

#[test]
fn an_idle_scheduler_publishes_at_once_and_observes() {
    let mut model = Model::default();
    let mut published = None;
    assert_eq!(
        step(request(), &mut published, &mut model).unwrap(),
        Disposition::ExecutionLockRetained
    );
    assert_eq!(model.ops, [Op::Busy, Op::Publish, Op::Observe]);
}

#[test]
fn a_busy_scheduler_waits_for_idle_engines_before_publishing() {
    let mut model = Model {
        busy: [Some(true), Some(true)].into(),
        engines_idle: [false, true].into(),
        ..Model::default()
    };
    let mut published = None;
    assert_eq!(
        step(request(), &mut published, &mut model).unwrap(),
        Disposition::Pending
    );
    assert_eq!(model.ops, [Op::Busy, Op::EnginesIdle]);
    assert!(published.is_none());
    model.ops.clear();
    assert_eq!(
        step(request(), &mut published, &mut model).unwrap(),
        Disposition::ExecutionLockRetained
    );
    assert_eq!(
        model.ops,
        [Op::Busy, Op::EnginesIdle, Op::Publish, Op::Observe]
    );
}

#[test]
fn a_published_lock_only_observes_on_later_steps() {
    let mut model = Model {
        observations: [Disposition::Pending, Disposition::ReconcileCurrentHead].into(),
        ..Model::default()
    };
    let mut published = None;
    assert_eq!(
        step(request(), &mut published, &mut model).unwrap(),
        Disposition::Pending
    );
    model.ops.clear();
    assert_eq!(
        step(request(), &mut published, &mut model).unwrap(),
        Disposition::ReconcileCurrentHead
    );
    assert_eq!(model.ops, [Op::Observe]);
}

#[test]
fn an_unsettled_busy_sample_fails_the_step_without_publishing() {
    let mut model = Model {
        busy: [None].into(),
        ..Model::default()
    };
    let mut published = None;
    assert_eq!(
        step(request(), &mut published, &mut model),
        Err(BluetoothDiagnosticUnsettled)
    );
    assert_eq!(model.ops, [Op::Busy]);
    assert!(published.is_none());
}
