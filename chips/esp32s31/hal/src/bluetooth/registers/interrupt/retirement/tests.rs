use super::*;
use std::{cell::RefCell, collections::VecDeque, vec::Vec};

/// The bank and the task's registers as one model, each side reaching it
/// through its own control: the handler's and the task's steps alternate.
struct Model {
    faults: VecDeque<bool>,
    observed: crate::bluetooth::BluetoothInterruptPolicyObservation,
    sticky: bool,
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
            observed: crate::bluetooth::BluetoothInterruptPolicyObservation {
                primary: crate::bluetooth::BluetoothInterruptObservation {
                    enable_0: 0,
                    enable_1: 0,
                    status_0: 0,
                    status_1: 0,
                    nrt_0: 0,
                    nrt_1: 0,
                },
                task: crate::bluetooth::BluetoothTaskInterruptObservation {
                    enable: 0,
                    opaque_state: 0,
                },
            },
            sticky: false,
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
    fn observation(&self) -> crate::bluetooth::BluetoothInterruptObservation {
        self.0.borrow().observed.primary
    }

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
        model.observed.primary.enable_0 = 0;
        model.observed.primary.enable_1 = 0;
        model.writes.push("mask");
    }
    fn clear_pending(
        &mut self,
        _: crate::bluetooth::BluetoothTaskInterruptObservation,
    ) -> Result<(), crate::bluetooth::BluetoothInterruptPolicyFault> {
        let mut model = self.0.borrow_mut();
        assert!(!model.dynamic_enabled && !model.run_enabled);
        model.completion_pending = false;
        if !model.sticky {
            model.observed.primary.status_0 = 0;
            model.observed.primary.status_1 = 0;
            model.observed.primary.nrt_0 = 0;
            model.observed.primary.nrt_1 = 0;
        }
        model.writes.push("clear");
        Ok(())
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
        model.observed.task.enable = 0;
        if !model.sticky {
            model.observed.task.opaque_state = 0;
        }
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
    let task = model.borrow().observed.task;
    sample_idle(&mut Bank(model), busy(preflight), task)?;
    confirm_heads_absent(&mut Task(model))?;
    mask(&mut Bank(model));
    disable_run(&mut Task(model));
    let task = model.borrow().observed.task;
    release(&mut Bank(model), busy(after_fence), task)
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
    let task = model.borrow().observed.task;
    assert_eq!(sample_idle(&mut Bank(&model), busy(false), task), Ok(()));
    assert!(model.borrow().writes.is_empty());
    assert_eq!(
        sample_idle(&mut Bank(&model), busy(true), task),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.borrow().writes.is_empty());
}

#[test]
fn feature_task17_and_pending_are_cleared_before_release_is_confirmed() {
    let model = RefCell::new(Model::idle());
    {
        let mut m = model.borrow_mut();
        m.observed.primary.enable_0 =
            bank::irq_enable_0::SOURCE_0_MASK | bank::irq_enable_0::SOURCE_16_MASK;
        m.observed.primary.enable_1 = bank::irq_enable_1::SOURCE_4_MASK
            | bank::irq_enable_1::SOURCE_5_MASK
            | bank::irq_enable_1::SOURCE_13_MASK;
        m.observed.primary.status_1 = bank::irq_enable_1::SOURCE_4_MASK;
        m.observed.primary.nrt_0 = 0x100;
        m.observed.primary.nrt_1 = 0x2000;
        m.observed.task.enable =
            bank::interrupt_enable::SOURCE_17_MASK | bank::interrupt_enable::SCHEDULER_RUN_MASK;
        m.observed.task.opaque_state = bank::interrupt_enable::SOURCE_17_MASK;
    }
    assert_eq!(release_all(&model, false, false), Ok(()));
    assert_eq!(model.borrow().observed.confirm_released(), Ok(()));
}
#[test]
fn a_pending_lane_that_survives_ack_faults_with_the_full_post_release_image() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().sticky = true;
    model.borrow_mut().observed.primary.status_1 = bank::irq_enable_1::SOURCE_4_MASK;
    let result = release_all(&model, false, false);
    assert_eq!(
        result,
        Err(BluetoothControllerOutputReleaseError::InterruptPolicy(
            crate::bluetooth::BluetoothInterruptPolicyFault::ReleaseNotQuiet(
                model.borrow().observed
            )
        ))
    );
}

#[test]
fn opaque_bank1_pending_faults_without_acknowledgement() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().observed.primary.status_1 = bank::irq_enable_1::BIT_2_OPAQUE_MASK;
    let observed = model.borrow().observed;
    assert_eq!(
        release_all(&model, false, false),
        Err(BluetoothControllerOutputReleaseError::InterruptPolicy(
            crate::bluetooth::BluetoothInterruptPolicyFault::ReleaseNotQuiet(observed)
        ))
    );
    assert!(model.borrow().writes.is_empty());
}

#[test]
fn typed_register_masks_remove_the_cleanup_features_and_task17() {
    let mut memory = crate::bluetooth::registers::host::HostMemory::filled(0);
    let (mut task, mut interrupts, _) = memory.owners();
    let bank = &mut interrupts.peripherals.bluetooth_interrupt_bank;
    bank.modify_irq_enable_0(|w| {
        w.set_source_0(true);
        w.set_source_1(true);
        w.set_source_8(true);
        w.set_source_13(true);
        w.set_source_15(true);
        w.set_source_16(true);
        w.set_source_21(true);
        w.set_sources_27_28(oer_mmio::Bits::constant::<3>());
    });
    bank.modify_irq_enable_1(|w| {
        w.set_controller_time_overflow(true);
        w.set_source_3(true);
        w.set_source_4(true);
        w.set_source_5(true);
        w.set_source_6(true);
        w.set_source_8(true);
        w.set_source_9(true);
        w.set_source_12(true);
        w.set_source_13(true);
    });
    task.bluetooth
        .btmac_ble_phy_init
        .modify_interrupt_enable(|w| {
            w.set_source_17(true);
            w.set_scheduler_run(true);
        });
    interrupts.mask_output_dynamic_sources();
    task.disable_scheduler_run();
    let observed = crate::bluetooth::BluetoothInterruptPolicyObservation {
        primary: crate::bluetooth::observation::primary(
            &interrupts.peripherals.bluetooth_interrupt_bank,
        ),
        task: crate::bluetooth::observation::task(&task.bluetooth.btmac_ble_phy_init),
    };
    assert_eq!(observed.confirm_released(), Ok(()));
}
