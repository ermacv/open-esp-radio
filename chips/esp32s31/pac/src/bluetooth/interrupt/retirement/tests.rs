use super::*;
use std::{collections::VecDeque, vec::Vec};

struct Model {
    faults: VecDeque<bool>,
    head: Option<BluetoothSchedulerHardwareListIndex>,
    dynamic_enabled: bool,
    run_enabled: bool,
    completion_pending: bool,
    output_live: bool,
    inspected: Vec<BluetoothSchedulerHardwareListIndex>,
    writes: usize,
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
            writes: 0,
        }
    }
}

impl Control for Model {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool {
        self.inspected.push(index);
        self.head == Some(index)
    }
    fn fault_pending(&mut self) -> bool {
        self.faults.pop_front().expect("fresh fault observation")
    }
    fn mask_dynamic(&mut self) {
        self.dynamic_enabled = false;
        self.writes += 1;
    }
    fn disable_run(&mut self) {
        self.run_enabled = false;
        self.writes += 1;
    }
    fn fence(&mut self) {}
    fn clear_dynamic(&mut self) {
        assert!(!self.dynamic_enabled && !self.run_enabled);
        self.completion_pending = false;
        self.writes += 1;
    }
    fn release_output(&mut self) {
        assert!(!self.completion_pending);
        self.output_live = false;
        self.writes += 1;
    }
}

fn busy(busy: bool) -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::new(busy)
}

fn quiesce_then_release(
    model: &mut Model,
    preflight: bool,
    after_fence: bool,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    quiesce(model, busy(preflight))?;
    release(model, busy(after_fence))
}

#[test]
fn live_scheduler_and_every_published_head_reject_without_mutation() {
    let mut model = Model::idle();
    assert_eq!(
        quiesce(&mut model, busy(true)),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.inspected.is_empty());
    assert_eq!(model.writes, 0);
    for index in 0..16 {
        let index = BluetoothSchedulerHardwareListIndex::new(index).unwrap();
        let mut model = Model::idle();
        model.head = Some(index);
        assert_eq!(
            quiesce(&mut model, busy(false)),
            Err(BluetoothControllerOutputReleaseError::PublishedHead(index))
        );
        assert_eq!(model.head, Some(index));
        assert_eq!(model.writes, 0);
        assert!(model.output_live && model.dynamic_enabled && model.run_enabled);
    }
}

#[test]
fn faults_and_execution_racing_the_preflight_never_acknowledge_or_release() {
    for after_mask in [false, true] {
        let mut model = Model::idle();
        model.faults = if after_mask {
            [false, true].into()
        } else {
            [true].into()
        };
        assert_eq!(
            quiesce_then_release(&mut model, false, false),
            Err(BluetoothControllerOutputReleaseError::InterruptFault)
        );
        assert!(model.output_live && model.completion_pending);
        assert_eq!(model.writes, if after_mask { 2 } else { 0 });
    }
    let mut model = Model::idle();
    assert_eq!(
        quiesce_then_release(&mut model, false, true),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.output_live && model.completion_pending);
    assert!(!model.dynamic_enabled && !model.run_enabled);
}

#[test]
fn idle_output_release_acknowledges_only_after_disabling_new_scheduler_work() {
    let mut model = Model::idle();
    assert_eq!(quiesce(&mut model, busy(false)), Ok(()));
    assert!(model.output_live && model.completion_pending);
    assert!(!model.dynamic_enabled && !model.run_enabled);
    assert_eq!(release(&mut model, busy(false)), Ok(()));
    assert_eq!(model.inspected.len(), 16);
    assert!(!model.output_live && !model.completion_pending);
    assert!(model.head.is_none());
}

#[test]
fn maintenance_admission_preserves_live_masks_and_pending_completion() {
    let mut model = Model::idle();
    assert_eq!(validate_idle(&mut model, busy(false)), Ok(()));
    assert_eq!(model.writes, 0);
    assert!(model.output_live && model.dynamic_enabled && model.run_enabled);
    assert!(model.completion_pending);
    // A second admission consumes its own fresh sample, never the first.
    assert_eq!(
        validate_idle(&mut model, busy(true)),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert_eq!(model.writes, 0);
}
