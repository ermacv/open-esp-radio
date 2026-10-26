use crate::modem::{BluetoothLowPowerClockObservation, ModemSysconBluetoothObservation};

use crate::{
    domain::{LowPowerClockError, RadioDomain},
    root::{Ieee802154RadioPartition, ProtocolPartitions, RadioHardware, WifiPartition},
};

use super::{
    BluetoothClockCheckpoint, ClockedOwner, ControllerHalBorrow, ControllerPublicAddress,
    ControllerRandomAddress, RxMemoryListInitialPublication, RxPacketControl,
    TaskOwnerReuniteError, clock_checkpoint, execute_rx_memory_list_initial_publication,
};

/// The other partitions of a concurrent split.
type Others = (
    WifiPartition,
    Ieee802154RadioPartition,
    crate::domain::CoexAbsent<crate::domain::WifiClient>,
    crate::domain::CoexAbsent<crate::domain::BluetoothClient>,
    crate::domain::CoexAbsent<crate::domain::Ieee802154Client>,
);

/// A concurrent split whose Bluetooth partition is assumed clocked.
fn clocked() -> (
    RadioDomain<crate::power::test_clocks::CountingBackend>,
    Others,
    ClockedOwner,
) {
    let (shared, partitions) =
        RadioHardware::for_host_test().split::<crate::power::test_clocks::CountingBackend, _>(());
    let ProtocolPartitions {
        wifi,
        wifi_coex,
        bluetooth,
        bluetooth_coex,
        ieee802154,
        ieee802154_coex,
    } = partitions;
    (
        shared,
        (wifi, ieee802154, wifi_coex, bluetooth_coex, ieee802154_coex),
        ClockedOwner::for_host_test(bluetooth),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RxListPublicationStep {
    PacketControl(RxPacketControl),
    CurrentHead,
    NextHeadCleared,
    InitialControlReset,
}

#[derive(Default)]
struct RecordingRxListPublication {
    steps: std::vec::Vec<RxListPublicationStep>,
}

impl RxMemoryListInitialPublication for RecordingRxListPublication {
    fn prepare_packet_control(&mut self, policy: RxPacketControl) {
        self.steps
            .push(RxListPublicationStep::PacketControl(policy));
    }

    fn reset_initial_control(&mut self) {
        assert_eq!(
            self.steps.last(),
            Some(&RxListPublicationStep::NextHeadCleared)
        );
        self.steps.push(RxListPublicationStep::InitialControlReset);
    }

    fn publish_current_head(&mut self) {
        self.steps.push(RxListPublicationStep::CurrentHead);
    }

    fn clear_next_head(&mut self) {
        self.steps.push(RxListPublicationStep::NextHeadCleared);
    }
}

#[test]
fn receive_list_publication_resets_control_after_both_pointer_operations() {
    let mut transaction = RecordingRxListPublication::default();

    execute_rx_memory_list_initial_publication(
        &mut transaction,
        RxPacketControl::ControllerDefault,
    );

    assert_eq!(
        transaction.steps,
        [
            RxListPublicationStep::PacketControl(RxPacketControl::ControllerDefault),
            RxListPublicationStep::CurrentHead,
            RxListPublicationStep::NextHeadCleared,
            RxListPublicationStep::InitialControlReset,
        ]
    );
}

#[test]
fn software_connection_policy_precedes_rx_and_is_replaced_when_the_role_changes() {
    let mut transaction = RecordingRxListPublication::default();
    // First connection event, recurring event (including PHY restoration),
    // then a return to advertising/scanning on the same powered task epoch.
    for policy in [
        RxPacketControl::SoftwareConnection,
        RxPacketControl::SoftwareConnection,
        RxPacketControl::ControllerDefault,
    ] {
        let before = transaction.steps.len();
        execute_rx_memory_list_initial_publication(&mut transaction, policy);
        assert_eq!(
            &transaction.steps[before..],
            [
                RxListPublicationStep::PacketControl(policy),
                RxListPublicationStep::CurrentHead,
                RxListPublicationStep::NextHeadCleared,
                RxListPublicationStep::InitialControlReset,
            ],
        );
    }
}

#[test]
fn public_and_hci_random_forms_converge_on_one_controller_identity() {
    let public = ControllerPublicAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let random = ControllerRandomAddress::from_hci_wire_bytes([6, 5, 4, 3, 2, 1]);

    assert_eq!(
        public.controller_wire_octets(),
        random.controller_wire_octets()
    );
    assert_eq!(public.canonical_bytes(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(random.hci_wire_bytes(), [6, 5, 4, 3, 2, 1]);
}

#[test]
fn untouched_task_owner_returns_the_partition_to_the_neutral_root() {
    let (shared, (wifi, ieee802154, wifi_coex, bluetooth_coex, ieee802154_coex), clocked) =
        clocked();
    let (task, interrupts) = clocked.separate_interrupt_owner();
    let clocked = task
        .into_clocked(interrupts)
        .expect("an untouched task owner can be reunited");
    let partitions = ProtocolPartitions {
        wifi,
        wifi_coex,
        bluetooth: clocked.into_partition_for_host_test(),
        bluetooth_coex,
        ieee802154,
        ieee802154_coex,
    };

    // Reuniting the split proves that the finite HAL borrows neither moved
    // nor duplicated any partition.
    let Ok(_root) = RadioHardware::reunite(shared, partitions) else {
        panic!("the untouched partitions reunite");
    };
}

#[test]
fn mutable_controller_borrow_arms_fail_stop_reunion() {
    let (_shared, _partitions, clocked) = clocked();
    let (mut task, interrupts) = clocked.separate_interrupt_owner();
    {
        let _controller = task.borrow_bluetooth_controller();
    }

    let failure = match task.into_clocked(interrupts) {
        Ok(_) => panic!("hardware rollback is required after a mutable HAL borrow"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        TaskOwnerReuniteError::HardwareLifecycleNotRestored
    );
    let _retained_owners = failure.into_parts();
}

#[test]
fn non_pristine_interrupt_history_blocks_reunion() {
    let (_shared, _partitions, clocked) = clocked();
    let (task, mut interrupts) = clocked.separate_interrupt_owner();

    // This private state mutation isolates the ownership rule without
    // issuing target MMIO from a host test. The actual prepare/release
    // methods are the only production constructors of this dirty setup.
    interrupts.reunitable = false;

    let failure = match task.into_clocked(interrupts) {
        Ok(_) => panic!("interrupt MMIO history requires verified rollback"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        TaskOwnerReuniteError::InterruptLifecycleNotRestored
    );
    let _retained_owners = failure.into_parts();
}

#[test]
fn unfinished_controller_time_latch_blocks_reunion() {
    let (_shared, _partitions, clocked) = clocked();
    let (mut task, interrupts) = clocked.separate_interrupt_owner();

    // The HAL latch state is the only record of a published request; a
    // cancelled async step must not let the partition be reunited.
    task.time_latch.begin_for_test();
    assert!(task.time_latch.in_flight());

    let failure = match task.into_clocked(interrupts) {
        Ok(_) => panic!("an unfinished latch request requires draining"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        TaskOwnerReuniteError::ControllerTimeLatchInFlight
    );
    let _retained_owners = failure.into_parts();
}

#[test]
fn clock_readback_names_the_first_failed_checkpoint() {
    let clocks = ModemSysconBluetoothObservation {
        controller_clocks_enabled: true,
        apb_clocks_enabled: true,
        controller_resets_released: true,
    };
    let low_power = BluetoothLowPowerClockObservation {
        exclusive_main_xtal_selected: true,
        bluetooth_divider_configured: true,
    };
    assert_eq!(clock_checkpoint(clocks, low_power), None);
    let cases = [
        (
            ModemSysconBluetoothObservation {
                controller_clocks_enabled: false,
                apb_clocks_enabled: false,
                ..clocks
            },
            low_power,
            BluetoothClockCheckpoint::ControllerClocks,
        ),
        (
            ModemSysconBluetoothObservation {
                apb_clocks_enabled: false,
                ..clocks
            },
            low_power,
            BluetoothClockCheckpoint::ApbClocks,
        ),
        (
            ModemSysconBluetoothObservation {
                controller_resets_released: false,
                ..clocks
            },
            BluetoothLowPowerClockObservation { ..low_power },
            BluetoothClockCheckpoint::ControllerReset,
        ),
        (
            clocks,
            BluetoothLowPowerClockObservation {
                exclusive_main_xtal_selected: false,
                ..low_power
            },
            BluetoothClockCheckpoint::LowPowerClockSource,
        ),
        (
            clocks,
            BluetoothLowPowerClockObservation {
                bluetooth_divider_configured: false,
                ..low_power
            },
            BluetoothClockCheckpoint::LowPowerClockDivider,
        ),
    ];
    for (clocks, low_power, checkpoint) in cases {
        assert_eq!(clock_checkpoint(clocks, low_power), Some(checkpoint));
    }
}

#[test]
fn an_unselected_low_power_clock_cannot_be_deselected() {
    let (mut shared, _partitions, clocked) = clocked();
    let (task, _interrupts) = clocked.separate_interrupt_owner();
    let radio = &mut shared;
    // Rejected before any register access, so the validation root is safe.
    assert_eq!(
        radio.deselect_bluetooth_low_power_clock(&task.registers),
        Err(LowPowerClockError::NotSelected)
    );
    assert!(!radio.bluetooth_low_power_clock_selected());
}

#[test]
fn a_scanner_starts_only_on_the_scanning_list_of_its_own_epoch() {
    use super::{RxMemoryListPublished, ScanStartError};
    use crate::bluetooth::BluetoothMemoryListSelector;

    let (_shared, _partitions, clocked) = clocked();
    let (task, interrupts) = clocked.separate_interrupt_owner();
    let ended = task.epoch;
    let clocked = match task.into_clocked(interrupts) {
        Ok(clocked) => clocked,
        Err(_) => panic!("an untouched task reunites"),
    };
    let (mut task, _interrupts) = clocked.separate_interrupt_owner();
    let current = task.epoch;
    assert_ne!(ended, current, "a new task owner starts a new epoch");
    let head = crate::dma_address::model_reference(0x2f00_1000);
    let published = |scanning, epoch| RxMemoryListPublished {
        selector: BluetoothMemoryListSelector::One,
        head,
        scanning,
        epoch,
    };
    let mut controller = task.borrow_bluetooth_controller();
    // A publication of the epoch that ended no longer names the head the
    // Controller holds, so it is refused before any register is written.
    assert_eq!(
        controller.publish_scan_start(&published(true, ended)).err(),
        Some(ScanStartError::StaleList)
    );
    assert_eq!(
        controller
            .publish_scan_start(&published(false, current))
            .err(),
        Some(ScanStartError::NotScanningList)
    );
}

#[test]
fn stopping_a_planning_timer_preserves_original_and_deferred_dispatch_on_refusal() {
    use super::{BluetoothModemLpTimerEpoch, ModemLpTimerSoftwarePendingOwner};
    for (dispatch_requested, deferred_dispatch) in
        [(true, None), (false, Some(true)), (true, Some(false))]
    {
        let (_, partitions) = crate::root::RadioHardware::for_host_test()
            .split::<crate::power::test_clocks::CountingBackend, _>(());
        let (mut task, _) =
            ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
        let pending = ModemLpTimerSoftwarePendingOwner {
            timer: task.modem_lp_timer.take().unwrap(),
            epoch: BluetoothModemLpTimerEpoch::new(),
            dispatch_requested,
            deferred_dispatch,
        };
        // This fictitious root has no accessible MMIO: refusal must precede
        // any hardware operation and return all software facts unchanged.
        let refused = pending
            .stop_for_shutdown()
            .err()
            .expect("dispatch refuses stop");
        assert_eq!(refused.dispatch_requested, dispatch_requested);
        assert_eq!(refused.deferred_dispatch, deferred_dispatch);
    }
}
