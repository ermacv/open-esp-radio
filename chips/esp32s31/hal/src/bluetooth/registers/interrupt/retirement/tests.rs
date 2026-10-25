use super::*;
use std::{cell::RefCell, collections::VecDeque, vec::Vec};

/// The bank and the task's registers as one model, each side reaching it
/// through its own control: the handler's and the task's steps alternate.
struct Model {
    faults: VecDeque<bool>,
    head: Option<BluetoothSchedulerHardwareListIndex>,
    dynamic_enabled: bool,
    run_enabled: bool,
    completion_pending: bool,
    output_live: bool,
    inspected: Vec<BluetoothSchedulerHardwareListIndex>,
    writes: Vec<&'static str>,
}

impl Model {
    fn idle() -> Self {
        Self {
            faults: [false, false].into(),
            head: None,
            dynamic_enabled: true,
            run_enabled: true,
            completion_pending: true,
            output_live: true,
            inspected: Vec::new(),
            writes: Vec::new(),
        }
    }
}

struct Bank<'a>(&'a RefCell<Model>);
struct Task<'a>(&'a RefCell<Model>);

impl BankControl for Bank<'_> {
    fn fault_pending(&mut self) -> bool {
        self.0
            .borrow_mut()
            .faults
            .pop_front()
            .expect("fresh fault observation")
    }
    fn mask_dynamic(&mut self) {
        let mut model = self.0.borrow_mut();
        model.dynamic_enabled = false;
        model.writes.push("mask");
    }
    fn clear_dynamic(&mut self) {
        let mut model = self.0.borrow_mut();
        assert!(!model.dynamic_enabled && !model.run_enabled);
        model.completion_pending = false;
        model.writes.push("clear");
    }
    fn release_output(&mut self) {
        let mut model = self.0.borrow_mut();
        assert!(!model.completion_pending);
        model.output_live = false;
        model.writes.push("release");
    }
    fn fence(&mut self) {
        self.0.borrow_mut().writes.push("bank fence");
    }
}

impl TaskControl for Task<'_> {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool {
        let mut model = self.0.borrow_mut();
        model.inspected.push(index);
        model.head == Some(index)
    }
    fn disable_run(&mut self) {
        let mut model = self.0.borrow_mut();
        model.run_enabled = false;
        model.writes.push("disable run");
    }
    fn fence(&mut self) {
        self.0.borrow_mut().writes.push("task fence");
    }
}

fn busy(busy: bool) -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::new(busy)
}

/// The whole release: the handler's sample, the task's heads, the
/// handler's mask, the task's RUN disable, the handler's release.
fn release_all(
    model: &RefCell<Model>,
    preflight: bool,
    after_fence: bool,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    sample_idle(&mut Bank(model), busy(preflight))?;
    confirm_heads_absent(&mut Task(model))?;
    mask(&mut Bank(model));
    disable_run(&mut Task(model));
    release(&mut Bank(model), busy(after_fence))
}

#[test]
fn live_scheduler_and_every_published_head_reject_without_mutation() {
    let model = RefCell::new(Model::idle());
    assert_eq!(
        release_all(&model, true, false),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.borrow().inspected.is_empty());
    assert!(model.borrow().writes.is_empty());
    for index in (0..16).filter_map(BluetoothSchedulerHardwareListIndex::new) {
        let model = RefCell::new(Model::idle());
        model.borrow_mut().head = Some(index);
        assert_eq!(
            release_all(&model, false, false),
            Err(BluetoothControllerOutputReleaseError::PublishedHead(index))
        );
        let model = model.borrow();
        assert_eq!(model.head, Some(index));
        assert!(model.writes.is_empty());
        assert!(model.output_live && model.dynamic_enabled && model.run_enabled);
    }
}

#[test]
fn faults_and_execution_racing_the_preflight_never_acknowledge_or_release() {
    for after_mask in [false, true] {
        let model = RefCell::new(Model::idle());
        model.borrow_mut().faults = if after_mask {
            [false, true].into()
        } else {
            [true].into()
        };
        assert_eq!(
            release_all(&model, false, false),
            Err(BluetoothControllerOutputReleaseError::InterruptFault)
        );
        let model = model.borrow();
        assert!(model.output_live && model.completion_pending);
        assert_eq!(model.writes.len(), if after_mask { 4 } else { 0 });
    }
    let model = RefCell::new(Model::idle());
    assert_eq!(
        release_all(&model, false, true),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    let model = model.borrow();
    assert!(model.output_live && model.completion_pending);
    assert!(!model.dynamic_enabled && !model.run_enabled);
}

#[test]
fn idle_output_release_keeps_the_reviewed_write_order_across_both_sides() {
    let model = RefCell::new(Model::idle());
    assert_eq!(release_all(&model, false, false), Ok(()));
    let model = model.borrow();
    assert_eq!(
        model.writes,
        [
            "mask",
            "bank fence",
            "disable run",
            "task fence",
            "clear",
            "release",
            "bank fence"
        ]
    );
    assert_eq!(model.inspected.len(), 16);
    assert!(!model.output_live && !model.completion_pending);
    assert!(model.head.is_none());
}

#[test]
fn the_idle_sample_writes_nothing_and_each_sample_is_fresh() {
    let model = RefCell::new(Model::idle());
    assert_eq!(sample_idle(&mut Bank(&model), busy(false)), Ok(()));
    assert!(model.borrow().writes.is_empty());
    assert_eq!(
        sample_idle(&mut Bank(&model), busy(true)),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.borrow().writes.is_empty());
}
