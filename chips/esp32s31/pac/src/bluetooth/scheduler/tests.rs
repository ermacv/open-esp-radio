use super::{
    BluetoothControllerSramAddress, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadControl, BluetoothSchedulerHardwareListHeadError,
    BluetoothSchedulerHardwareListHeadObservationControl,
    BluetoothSchedulerHardwareListHeadRetirementDisposition, BluetoothSchedulerHardwareListIndex,
    BluetoothSchedulerRunEventControl, classify_scheduler_hardware_list_head_retirement,
    execute_scheduler_hardware_list_head_observation,
    execute_scheduler_hardware_list_head_publication, execute_scheduler_run_event_publication,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationOperation {
    DescriptorFence,
    Publish {
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    },
    DeviceFence,
}

struct PublicationRecorder {
    operations: std::vec::Vec<PublicationOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservationOperation {
    Read(BluetoothSchedulerHardwareListIndex),
    DeviceFence,
}

struct ObservationRecorder {
    head: BluetoothSchedulerHardwareListHead,
    operations: std::vec::Vec<ObservationOperation>,
}

impl BluetoothSchedulerHardwareListHeadObservationControl for ObservationRecorder {
    fn read_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerHardwareListHead {
        self.operations.push(ObservationOperation::Read(index));
        self.head
    }

    fn order_after_observation(&mut self) {
        self.operations.push(ObservationOperation::DeviceFence);
    }
}

impl BluetoothSchedulerHardwareListHeadControl for PublicationRecorder {
    fn order_descriptor_before_publication(&mut self) {
        self.operations.push(PublicationOperation::DescriptorFence);
    }

    fn publish_head(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
        head: BluetoothSchedulerHardwareListHead,
    ) {
        self.operations
            .push(PublicationOperation::Publish { index, head });
    }

    fn order_after_publication(&mut self) {
        self.operations.push(PublicationOperation::DeviceFence);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunEventOperation {
    ClearStaleSource,
    EnableSource,
    DeviceFence,
}

struct RunEventRecorder {
    operations: std::vec::Vec<RunEventOperation>,
}

impl BluetoothSchedulerRunEventControl for RunEventRecorder {
    fn clear_scheduler_run_event_source(&mut self) {
        self.operations.push(RunEventOperation::ClearStaleSource);
    }

    fn enable_scheduler_run_event_source(&mut self) {
        self.operations.push(RunEventOperation::EnableSource);
    }

    fn order_after_scheduler_run_event(&mut self) {
        self.operations.push(RunEventOperation::DeviceFence);
    }
}

#[test]
fn scheduler_run_event_clears_stale_source_before_enabling_it() {
    let mut recorder = RunEventRecorder {
        operations: std::vec::Vec::new(),
    };

    execute_scheduler_run_event_publication(&mut recorder);

    assert_eq!(
        recorder.operations,
        [
            RunEventOperation::ClearStaleSource,
            RunEventOperation::EnableSource,
            RunEventOperation::DeviceFence,
        ]
    );
}

#[test]
fn non_empty_head_cannot_alias_the_empty_scheduler_state() {
    let empty_image = BluetoothControllerSramAddress::new(0x2f00_0000)
        .expect("controller SRAM window base is representable");
    assert_eq!(
        BluetoothSchedulerHardwareListHead::from_address(empty_image),
        Err(BluetoothSchedulerHardwareListHeadError::EncodesEmptyHead)
    );

    let item = BluetoothControllerSramAddress::new(0x2f00_0004)
        .expect("first non-empty item address is representable");
    assert_eq!(
        BluetoothSchedulerHardwareListHead::from_address(item)
            .expect("non-empty controller address is a valid list head")
            .address(),
        Some(item)
    );
}

#[test]
fn descriptor_visibility_precedes_head_publication() {
    let index = BluetoothSchedulerHardwareListIndex::ZERO;
    let head = BluetoothSchedulerHardwareListHead::from_address(
        BluetoothControllerSramAddress::new(0x2f00_0100)
            .expect("test item lies in controller SRAM"),
    )
    .expect("test item does not encode the empty head");
    let mut recorder = PublicationRecorder {
        operations: std::vec::Vec::new(),
    };

    let published = execute_scheduler_hardware_list_head_publication(&mut recorder, index, head);

    assert_eq!(published.index(), index);
    assert_eq!(published.head(), head);
    assert_eq!(
        recorder.operations,
        [
            PublicationOperation::DescriptorFence,
            PublicationOperation::Publish { index, head },
            PublicationOperation::DeviceFence,
        ]
    );
}

#[test]
fn post_completion_head_observation_is_fenced_and_bounded() {
    let index = BluetoothSchedulerHardwareListIndex::ZERO;
    let head = BluetoothSchedulerHardwareListHead::empty();
    let mut recorder = ObservationRecorder {
        head,
        operations: std::vec::Vec::new(),
    };

    let observed = execute_scheduler_hardware_list_head_observation(&mut recorder, index);

    assert_eq!(observed, head);
    assert_eq!(
        recorder.operations,
        [
            ObservationOperation::Read(index),
            ObservationOperation::DeviceFence,
        ]
    );
}

#[test]
fn head_retirement_distinguishes_empty_retained_and_changed_identity() {
    let expected = BluetoothSchedulerHardwareListHead::from_address(
        BluetoothControllerSramAddress::new(0x2f00_0100)
            .expect("expected item lies in controller SRAM"),
    )
    .expect("expected item is a nonempty head");
    let changed = BluetoothSchedulerHardwareListHead::from_address(
        BluetoothControllerSramAddress::new(0x2f00_0200)
            .expect("changed item lies in controller SRAM"),
    )
    .expect("changed item is a nonempty head");

    assert_eq!(
        classify_scheduler_hardware_list_head_retirement(
            expected,
            BluetoothSchedulerHardwareListHead::empty(),
        ),
        BluetoothSchedulerHardwareListHeadRetirementDisposition::Empty
    );
    assert_eq!(
        classify_scheduler_hardware_list_head_retirement(expected, expected),
        BluetoothSchedulerHardwareListHeadRetirementDisposition::ExpectedHeadStillPublished
    );
    assert_eq!(
        classify_scheduler_hardware_list_head_retirement(expected, changed),
        BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged
    );
}
