use super::*;
use crate::bluetooth::interrupt_policy::{CLEANUP_0, CLEANUP_1, FAULTS_0, FAULTS_1};
use crate::bluetooth::{BluetoothInterruptPolicyFault, BluetoothInterruptPolicyObservation};
use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;
use std::{cell::RefCell, vec::Vec};

/// Primary status is an ENA-masked view of retained causes. NRT is a
/// separate visible observation in this model, without claiming RAW semantics.
struct Model {
    raw: BluetoothInterruptPolicyObservation,
    task_pending: u32,
    acknowledged_fault: bool,
    sticky_pending: bool,
    sticky_enable: bool,
    fault_after_mask: Option<[u32; 2]>,
    nrt_after_release: Option<[u32; 2]>,
    nrt_after_sample: Option<[u32; 2]>,
    head: Option<BluetoothSchedulerHardwareListIndex>,
    run_enabled: bool,
    output_live: bool,
    inspected: Vec<BluetoothSchedulerHardwareListIndex>,
    events: Vec<&'static str>,
    clock: VirtualClock,
}
impl Model {
    fn idle() -> Self {
        Self {
            raw: BluetoothInterruptPolicyObservation {
                primary: crate::bluetooth::BluetoothInterruptObservation {
                    enable_0: CLEANUP_0 | FAULTS_0,
                    enable_1: CLEANUP_1 | FAULTS_1,
                    status_0: bank::irq_enable_0::SOURCE_21_MASK,
                    status_1: 0,
                    nrt_0: 0,
                    nrt_1: 0,
                },
                task_enable: bank::interrupt_enable::SCHEDULER_RUN_MASK,
            },
            task_pending: 0,
            acknowledged_fault: false,
            sticky_pending: false,
            sticky_enable: false,
            fault_after_mask: None,
            nrt_after_release: None,
            nrt_after_sample: None,
            head: None,
            run_enabled: true,
            output_live: true,
            inspected: Vec::new(),
            events: Vec::new(),
            clock: VirtualClock::new(),
        }
    }
    fn visible(&self) -> BluetoothInterruptPolicyObservation {
        let mut view = self.raw;
        view.primary.status_0 &= view.primary.enable_0;
        view.primary.status_1 &= view.primary.enable_1;
        view
    }
}
struct Bank<'a>(&'a RefCell<Model>);
struct Task<'a>(&'a RefCell<Model>);
impl BankControl for Bank<'_> {
    fn observation(&self) -> crate::bluetooth::BluetoothInterruptObservation {
        let mut model = self.0.borrow_mut();
        model.events.push("observe");
        model.visible().primary
    }
    fn fault_acknowledged(&self) -> bool {
        let mut model = self.0.borrow_mut();
        model.events.push("fault history");
        model.acknowledged_fault
    }
    fn acknowledge_nrt(&mut self) -> Result<(), BluetoothNrtInterruptFault> {
        let mut model = self.0.borrow_mut();
        model.events.extend(["sample nrt0", "sample nrt1"]);
        let snapshots = [model.raw.primary.nrt_0, model.raw.primary.nrt_1];
        if let Some([bank0, bank1]) = model.nrt_after_sample.take() {
            model.raw.primary.nrt_0 |= bank0;
            model.raw.primary.nrt_1 |= bank1;
        }
        acknowledge_nrt_if_admitted(snapshots, || {
            if snapshots == [0, 0] {
                return;
            }
            model.events.extend(["ack nrt0", "ack nrt1", "nrt fence"]);
            if !model.sticky_pending {
                model.raw.primary.status_0 &= !snapshots[0];
                model.raw.primary.status_1 &= !snapshots[1];
                model.raw.primary.nrt_0 &= !snapshots[0];
                model.raw.primary.nrt_1 &= !snapshots[1];
            }
        })
    }
    fn mask_dynamic(&mut self) {
        let mut model = self.0.borrow_mut();
        if !model.sticky_enable {
            model.raw.primary.enable_0 = 0;
            model.raw.primary.enable_1 = 0;
        }
        model.events.push("mask");
        if let Some([bank0, bank1]) = model.fault_after_mask.take() {
            model.clock.advance(Duration::from_micros(1)).unwrap();
            model.raw.primary.status_0 |= bank0;
            model.raw.primary.status_1 |= bank1;
        }
    }
    fn clear_pending(&mut self) {
        let mut model = self.0.borrow_mut();
        assert!(!model.run_enabled);
        // Fixed images clear retained non-fault causes even when STATUS is zero.
        if !model.sticky_pending {
            model.raw.primary.status_0 &= !CLEANUP_0;
            model.raw.primary.status_1 &= !CLEANUP_1;
            model.raw.primary.nrt_0 &= !CLEANUP_0;
            model.raw.primary.nrt_1 &= !CLEANUP_1;
        }
        model.events.push("clear");
    }
    fn release_output(&mut self) {
        let mut model = self.0.borrow_mut();
        model.output_live = false;
        model.events.push("release");
        if let Some([bank0, bank1]) = model.nrt_after_release.take() {
            model.raw.primary.nrt_0 |= bank0;
            model.raw.primary.nrt_1 |= bank1;
        }
    }
    fn fence(&mut self) {
        self.0.borrow_mut().events.push("bank fence");
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
        model.raw.task_enable = 0;
        // Model the sourced all-lane task WT publication; production never
        // reads opaque task status to claim hardware acknowledgement success.
        model.task_pending = 0;
        model.events.push("disable run");
    }
    fn fence(&mut self) {
        self.0.borrow_mut().events.push("task fence");
    }
}
fn busy(value: bool) -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::new(value)
}
fn release_all(
    model: &RefCell<Model>,
    preflight: bool,
    after_fence: bool,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    let task = model.borrow().raw.task_enable;
    sample_idle(&mut Bank(model), busy(preflight), task)?;
    confirm_heads_absent(&mut Task(model))?;
    mask(&mut Bank(model), task)?;
    disable_run(&mut Task(model));
    let task = model.borrow().raw.task_enable;
    release(&mut Bank(model), busy(after_fence), task)
}
fn policy_error(
    observed: BluetoothInterruptPolicyObservation,
) -> BluetoothControllerOutputReleaseError {
    BluetoothControllerOutputReleaseError::InterruptPolicy(
        BluetoothInterruptPolicyFault::ReleaseNotQuiet(observed),
    )
}
#[test]
fn live_scheduler_and_every_published_head_reject_without_mutation() {
    let model = RefCell::new(Model::idle());
    assert_eq!(
        release_all(&model, true, false),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(model.borrow().events.is_empty());
    for index in (0..16).filter_map(BluetoothSchedulerHardwareListIndex::new) {
        let model = RefCell::new(Model::idle());
        model.borrow_mut().head = Some(index);
        assert_eq!(
            release_all(&model, false, false),
            Err(BluetoothControllerOutputReleaseError::PublishedHead(index))
        );
        assert!(model.borrow().output_live && model.borrow().run_enabled);
        assert!(!model.borrow().events.contains(&"mask"));
    }
}
#[test]
fn a_fault_between_idle_and_mask_keeps_its_visible_source_image_without_writes() {
    for (bank0, bank1) in [(FAULTS_0, 0), (0, FAULTS_1)] {
        let model = RefCell::new(Model::idle());
        let task = model.borrow().raw.task_enable;
        sample_idle(&mut Bank(&model), busy(false), task).unwrap();
        confirm_heads_absent(&mut Task(&model)).unwrap();
        model
            .borrow()
            .clock
            .advance(Duration::from_micros(1))
            .unwrap();
        model.borrow_mut().raw.primary.status_0 |= bank0;
        model.borrow_mut().raw.primary.status_1 |= bank1;
        let observed = model.borrow().visible();
        assert_eq!(mask(&mut Bank(&model), task), Err(policy_error(observed)));
        assert!(!model.borrow().events.contains(&"mask"));
        assert!(!model.borrow().events.contains(&"clear"));
        assert!(model.borrow().output_live && model.borrow().run_enabled);
    }
}
#[test]
fn a_fault_arriving_after_the_mask_is_hidden_retained_and_never_blindly_cleared() {
    for pending in [[FAULTS_0, 0], [0, FAULTS_1]] {
        let model = RefCell::new(Model::idle());
        let before = model.borrow().clock.now();
        model.borrow_mut().fault_after_mask = Some(pending);
        assert_eq!(release_all(&model, false, false), Ok(()));
        let model = model.borrow();
        assert!(model.clock.now() > before);
        assert_eq!(
            [model.raw.primary.status_0, model.raw.primary.status_1],
            pending
        );
        assert_eq!(
            [
                model.visible().primary.status_0,
                model.visible().primary.status_1
            ],
            [0, 0]
        );
        assert_eq!(
            [
                model.raw.primary.enable_0,
                model.raw.primary.enable_1,
                model.raw.task_enable
            ],
            [0, 0, 0]
        );
    }
}
#[test]
fn acknowledged_faults_and_execution_after_the_fence_refuse_release() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().acknowledged_fault = true;
    assert_eq!(
        release_all(&model, false, false),
        Err(BluetoothControllerOutputReleaseError::InterruptFault)
    );
    assert!(!model.borrow().events.contains(&"mask"));
    let model = RefCell::new(Model::idle());
    assert_eq!(
        release_all(&model, false, true),
        Err(BluetoothControllerOutputReleaseError::SchedulerBusy)
    );
    assert!(!model.borrow().events.contains(&"clear"));
    assert!(model.borrow().output_live);
}
#[test]
fn visible_fault_checks_precede_mask_and_fixed_clear_follows_task_disable() {
    let model = RefCell::new(Model::idle());
    assert_eq!(release_all(&model, false, false), Ok(()));
    let model = model.borrow();
    assert_eq!(
        model.events,
        [
            "observe",
            "fault history",
            "sample nrt0",
            "sample nrt1",
            "observe",
            "fault history",
            "mask",
            "bank fence",
            "observe",
            "disable run",
            "task fence",
            "observe",
            "fault history",
            "clear",
            "release",
            "bank fence",
            "sample nrt0",
            "sample nrt1",
            "observe"
        ]
    );
    assert_eq!(model.inspected.len(), 16);
}

#[test]
fn nrt_outside_fixed_cleanup_is_acknowledged_before_mask_and_final_confirmation() {
    let model = RefCell::new(Model::idle());
    let pending = [0x4, bank::irq_enable_1::BIT_2_OPAQUE_MASK];
    {
        let mut m = model.borrow_mut();
        [m.raw.primary.nrt_0, m.raw.primary.nrt_1] = pending;
        m.nrt_after_release = Some(pending);
    }
    assert_eq!(release_all(&model, false, false), Ok(()));
    let m = model.borrow();
    assert_eq!([m.raw.primary.nrt_0, m.raw.primary.nrt_1], [0, 0]);
    assert!(!m.output_live);
    assert_eq!(
        m.events,
        [
            "observe",
            "fault history",
            "sample nrt0",
            "sample nrt1",
            "ack nrt0",
            "ack nrt1",
            "nrt fence",
            "observe",
            "fault history",
            "mask",
            "bank fence",
            "observe",
            "disable run",
            "task fence",
            "observe",
            "fault history",
            "clear",
            "release",
            "bank fence",
            "sample nrt0",
            "sample nrt1",
            "ack nrt0",
            "ack nrt1",
            "nrt fence",
            "observe",
        ]
    );
}

fn forbidden_nrt_pairs() -> [[u32; 2]; 5] {
    // Each pair also includes an admitted lane in the other bank: admission
    // must refuse both acknowledgements, preserving the complete pair.
    [
        [bank::irq_clear_0::SOURCE_15_MASK, 0x4],
        [0x4, bank::irq_clear_1::CONTROLLER_TIME_OVERFLOW_MASK],
        [0x4, bank::irq_clear_1::SOURCE_8_MASK],
        [0x4, bank::irq_clear_1::SOURCE_9_MASK],
        [0x4, bank::irq_clear_1::SOURCE_12_MASK],
    ]
}

#[test]
fn nrt_fault_and_overflow_lanes_still_refuse_the_idle_sample_without_writes() {
    for pending in forbidden_nrt_pairs() {
        let model = RefCell::new(Model::idle());
        {
            let mut m = model.borrow_mut();
            [m.raw.primary.nrt_0, m.raw.primary.nrt_1] = pending;
        }
        assert_eq!(
            release_all(&model, false, false),
            Err(policy_error(model.borrow().visible()))
        );
        assert_eq!(model.borrow().events, ["observe"]);
        assert!(model.borrow().output_live && model.borrow().run_enabled);
    }
}

#[test]
fn both_release_nrt_transactions_refuse_fault_and_overflow_without_either_ack() {
    for pending in forbidden_nrt_pairs() {
        for final_check in [false, true] {
            let model = RefCell::new(Model::idle());
            let task = model.borrow().raw.task_enable;
            sample_idle(&mut Bank(&model), busy(false), task).unwrap();
            if final_check {
                mask(&mut Bank(&model), task).unwrap();
                disable_run(&mut Task(&model));
                model.borrow_mut().nrt_after_release = Some(pending);
            } else {
                let mut m = model.borrow_mut();
                [m.raw.primary.nrt_0, m.raw.primary.nrt_1] = pending;
            }
            // A later diagnostic read includes another arrival; the fault must
            // retain the exact refused pair instead of replacing its image.
            model.borrow_mut().nrt_after_sample = Some([0x80, 0]);
            let result = if final_check {
                release(&mut Bank(&model), busy(false), 0)
            } else {
                mask(&mut Bank(&model), task)
            };
            let mut observed = model.borrow().visible();
            [observed.primary.nrt_0, observed.primary.nrt_1] = pending;
            assert_eq!(result, Err(policy_error(observed)));
            assert!(!model.borrow().events.contains(&"ack nrt0"));
            assert!(!model.borrow().events.contains(&"ack nrt1"));
            if !final_check {
                assert!(!model.borrow().events.contains(&"mask"));
            }
        }
    }
}

#[test]
fn an_nrt_arrival_after_the_final_sample_is_not_acknowledged_and_blocks_release() {
    let model = RefCell::new(Model::idle());
    let task = model.borrow().raw.task_enable;
    mask(&mut Bank(&model), task).unwrap();
    disable_run(&mut Task(&model));
    {
        let mut m = model.borrow_mut();
        // The final sampled pair is nonzero. Only that pair may be cleared.
        m.nrt_after_release = Some([0x4, 0]);
        m.nrt_after_sample = Some([0x80, 0]);
    }
    assert_eq!(
        release(&mut Bank(&model), busy(false), 0),
        Err(policy_error(model.borrow().visible()))
    );
    assert_eq!(model.borrow().raw.primary.nrt_0, 0x80);
}
#[test]
fn masking_hides_but_retains_pending_and_fixed_w1c_clears_it_without_reopening() {
    let model = RefCell::new(Model::idle());
    let task = model.borrow().raw.task_enable;
    assert_ne!(model.borrow().visible().primary.status_0, 0);
    mask(&mut Bank(&model), task).unwrap();
    assert_eq!(model.borrow().visible().primary.status_0, 0);
    assert_ne!(model.borrow().raw.primary.status_0, 0);
    disable_run(&mut Task(&model));
    release(&mut Bank(&model), busy(false), 0).unwrap();
    assert_eq!(model.borrow().raw.primary.status_0, 0);
    assert_eq!(
        [
            model.borrow().raw.primary.enable_0,
            model.borrow().raw.primary.enable_1
        ],
        [0, 0]
    );
}
#[test]
fn feature_task17_and_pending_receive_mask_and_fixed_ack_before_release() {
    let model = RefCell::new(Model::idle());
    {
        let mut m = model.borrow_mut();
        m.raw.primary.status_0 = CLEANUP_0;
        m.raw.primary.status_1 = CLEANUP_1;
        m.raw.primary.nrt_0 = bank::irq_enable_0::SOURCE_8_MASK;
        m.raw.primary.nrt_1 = bank::irq_enable_1::SOURCE_13_MASK;
        m.raw.task_enable |= bank::interrupt_enable::SOURCE_17_MASK;
        m.task_pending = bank::interrupt_enable::SOURCE_17_MASK;
    }
    assert_eq!(release_all(&model, false, false), Ok(()));
    let m = model.borrow();
    assert_eq!(m.task_pending, 0);
    assert_eq!(
        m.raw.primary.status_0
            | m.raw.primary.status_1
            | m.raw.primary.nrt_0
            | m.raw.primary.nrt_1
            | m.raw.task_enable,
        0
    );
}
#[test]
fn a_visible_pending_lane_that_survives_ack_faults_with_the_full_image() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().sticky_pending = true;
    model.borrow_mut().raw.primary.nrt_1 = bank::irq_enable_1::SOURCE_4_MASK;
    assert_eq!(
        release_all(&model, false, false),
        Err(policy_error(model.borrow().visible()))
    );
}
#[test]
fn nonzero_enable_readback_faults_with_original_words() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().sticky_enable = true;
    assert_eq!(
        release_all(&model, false, false),
        Err(policy_error(model.borrow().visible()))
    );
}
#[test]
fn an_opaque_enabled_cause_refuses_cleanup_without_acknowledgement() {
    let model = RefCell::new(Model::idle());
    model.borrow_mut().raw.primary.enable_1 |= bank::irq_enable_1::BIT_2_OPAQUE_MASK;
    model.borrow_mut().raw.primary.status_1 = bank::irq_enable_1::BIT_2_OPAQUE_MASK;
    assert_eq!(
        release_all(&model, false, false),
        Err(policy_error(model.borrow().visible()))
    );
    assert!(!model.borrow().events.contains(&"clear"));
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
    interrupts.mask_output_dynamic_sources(false, 0).unwrap();
    task.disable_scheduler_run();
    let observed = crate::bluetooth::BluetoothInterruptPolicyObservation {
        primary: crate::bluetooth::observation::primary(
            &interrupts.peripherals.bluetooth_interrupt_bank,
        ),
        task_enable: crate::bluetooth::observation::task_enable(&task.bluetooth.btmac_ble_phy_init),
    };
    assert_eq!(observed.confirm_released(), Ok(()));
}
