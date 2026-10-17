use std::vec::Vec;

use oer_esp32s31_pac::{
    Ieee802154AckTimeoutUnits as PacAckTimeoutUnits, Ieee802154CcaMode as PacCcaMode,
    Ieee802154MacControl as PacMacControl, Ieee802154MacPolicySnapshot as PacMacPolicySnapshot,
    Ieee802154MultipanEnableState as PacMultipanEnableState,
    Ieee802154PanIdentity as PacPanIdentity, Ieee802154RxStateCode, Ieee802154TxStateCode,
};

use oer_esp32s31_pac::{Ieee802154TaskRegisters, RadioPartitions};

use super::{
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154Hal, Ieee802154PacHal,
    Ieee802154Pti, Ieee802154RegisterBackend, Ieee802154StateSnapshot,
};
use crate::ieee802154::policy::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154PanIdentity,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    MaskEvents,
    MaskRxAborts,
    MaskTxAborts,
    AverageEdSampling,
    TxOnDelay,
    FrequencyCode(u8),
    CcaMode(u8),
    CcaThreshold(i8),
    MacControl(PacMacControl),
    AckTimeout(u16),
    PrimaryIdentity(PacPanIdentity),
    TxRxPti(u8),
    AckPti(u8),
    FoundationSnapshot,
    MacPolicySnapshot,
    SampleState,
    Fence,
}

struct FakeRegisters {
    operations: Vec<Operation>,
    state: Ieee802154StateSnapshot,
    foundation: Ieee802154FoundationSnapshot,
    policy: PacMacPolicySnapshot,
}

impl FakeRegisters {
    fn with_codes(rx: u8, tx: u8) -> Self {
        Self {
            operations: Vec::new(),
            state: Ieee802154StateSnapshot::new(
                Ieee802154RxStateCode::for_validation(rx).expect("three-bit RX state"),
                Ieee802154TxStateCode::for_validation(tx).expect("four-bit TX state"),
            ),
            foundation: Ieee802154FoundationSnapshot::new(
                true,
                true,
                true,
                true,
                Ieee802154Pti::new(3).expect("five-bit PTI"),
                Ieee802154Pti::new(3).expect("five-bit PTI"),
                true,
            ),
            policy: PacMacPolicySnapshot::new(
                Ieee802154FrequencyCode::new(48),
                PacCcaMode::CarrierAndEnergyDetection,
                -67,
                PacAckTimeoutUnits::new(109),
                PacMacControl::new(true, false, true, false, true, false),
                PacMultipanEnableState::new(true, false, false, false),
                PacPanIdentity::new(
                    0x1234,
                    0x5678,
                    [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87],
                ),
            ),
        }
    }
}

impl Ieee802154RegisterBackend for FakeRegisters {
    fn mask_all_events(&mut self) {
        self.operations.push(Operation::MaskEvents);
    }

    fn mask_all_rx_aborts(&mut self) {
        self.operations.push(Operation::MaskRxAborts);
    }

    fn mask_all_tx_aborts(&mut self) {
        self.operations.push(Operation::MaskTxAborts);
    }

    fn select_average_ed_sampling(&mut self) {
        self.operations.push(Operation::AverageEdSampling);
    }

    fn apply_txon_delay(&mut self) {
        self.operations.push(Operation::TxOnDelay);
    }

    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.operations.push(Operation::FrequencyCode(code.value()));
    }

    fn set_cca_mode(&mut self, mode: PacCcaMode) {
        self.operations.push(Operation::CcaMode(mode.field_value()));
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.operations.push(Operation::CcaThreshold(threshold));
    }

    fn set_mac_control(&mut self, control: PacMacControl) {
        self.operations.push(Operation::MacControl(control));
    }

    fn set_ack_timeout(&mut self, timeout: PacAckTimeoutUnits) {
        self.operations.push(Operation::AckTimeout(timeout.value()));
    }

    fn set_primary_pan_identity(&mut self, identity: PacPanIdentity) {
        self.operations.push(Operation::PrimaryIdentity(identity));
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.operations.push(Operation::TxRxPti(pti.value()));
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.operations.push(Operation::AckPti(pti.value()));
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.operations.push(Operation::FoundationSnapshot);
        self.foundation
    }

    fn mac_policy_snapshot(&mut self) -> PacMacPolicySnapshot {
        self.operations.push(Operation::MacPolicySnapshot);
        self.policy
    }

    fn sample_state(&mut self) -> Ieee802154StateSnapshot {
        self.operations.push(Operation::SampleState);
        self.state
    }

    fn order_device_accesses(&mut self) {
        self.operations.push(Operation::Fence);
    }
}

#[test]
fn typed_operations_reach_the_backend_without_register_images() {
    let mut hal = Ieee802154Hal::from_register_backend(FakeRegisters::with_codes(0, 0));
    hal.mask_all_events();
    hal.mask_all_rx_aborts();
    hal.mask_all_tx_aborts();
    hal.select_average_ed_sampling();
    hal.apply_txon_delay();
    hal.set_frequency_code(Ieee802154FrequencyCode::new(15));
    hal.set_cca_mode(Ieee802154CcaMode::CarrierAndEnergyDetection);
    hal.set_cca_threshold_code(-67);
    let control = Ieee802154MacControl::new(true, false, true, false, true, false);
    hal.set_mac_control(control);
    hal.set_ack_timeout(Ieee802154AckTimeout::from_microseconds(1_729).expect("bounded timeout"));
    let identity = Ieee802154PanIdentity::new(
        0x1234,
        0x5678,
        [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87],
    );
    hal.set_primary_pan_identity(identity);
    hal.set_txrx_pti(Ieee802154Pti::new(8).expect("five-bit PTI"));
    hal.set_ack_pti(Ieee802154Pti::new(3).expect("five-bit PTI"));
    hal.order_device_accesses();
    let registers = hal.into_register_backend();

    assert_eq!(
        registers.operations,
        [
            Operation::MaskEvents,
            Operation::MaskRxAborts,
            Operation::MaskTxAborts,
            Operation::AverageEdSampling,
            Operation::TxOnDelay,
            Operation::FrequencyCode(15),
            Operation::CcaMode(3),
            Operation::CcaThreshold(-67),
            Operation::MacControl(PacMacControl::new(true, false, true, false, true, false,)),
            Operation::AckTimeout(109),
            Operation::PrimaryIdentity(PacPanIdentity::new(
                0x1234,
                0x5678,
                [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87],
            )),
            Operation::TxRxPti(8),
            Operation::AckPti(3),
            Operation::Fence,
        ]
    );
}

#[test]
fn policy_readback_is_converted_to_hal_semantics_once() {
    let mut hal = Ieee802154Hal::from_register_backend(FakeRegisters::with_codes(0, 0));
    let snapshot = hal.mac_policy_snapshot();

    assert_eq!(snapshot.frequency_code(), 48);
    assert_eq!(
        snapshot.cca_mode(),
        Ieee802154CcaMode::CarrierAndEnergyDetection
    );
    assert_eq!(snapshot.cca_threshold_code(), -67);
    assert_eq!(snapshot.ack_timeout().units(), 109);
    assert_eq!(
        snapshot.control(),
        Ieee802154MacControl::new(true, false, true, false, true, false)
    );
    assert_eq!(
        snapshot.multipan_enable_state(),
        PacMultipanEnableState::new(true, false, false, false)
    );
    assert_eq!(snapshot.identity().pan_id(), 0x1234);
    assert_eq!(
        hal.into_register_backend().operations,
        [Operation::MacPolicySnapshot]
    );
}

#[test]
fn foundation_readback_is_delegated_without_event_status_access() {
    let mut hal = Ieee802154Hal::from_register_backend(FakeRegisters::with_codes(0, 0));
    let snapshot = hal.foundation_snapshot();

    assert!(snapshot.events_masked());
    assert!(snapshot.rx_aborts_masked());
    assert!(snapshot.tx_aborts_masked());
    assert!(snapshot.ed_uses_average());
    assert_eq!(snapshot.txrx_pti().value(), 3);
    assert_eq!(snapshot.ack_pti().value(), 3);
    assert_eq!(
        hal.into_register_backend().operations,
        [Operation::FoundationSnapshot]
    );
}

#[test]
fn state_predicate_samples_once_and_makes_no_idle_claim() {
    let mut hal = Ieee802154Hal::from_register_backend(FakeRegisters::with_codes(0, 0));
    assert!(hal.state_codes_are_zero());
    let registers = hal.into_register_backend();
    assert_eq!(registers.operations, [Operation::SampleState]);

    let mut hal = Ieee802154Hal::from_register_backend(FakeRegisters::with_codes(0, 1));
    assert!(!hal.state_codes_are_zero());
    let registers = hal.into_register_backend();
    assert_eq!(registers.operations, [Operation::SampleState]);
}

#[test]
fn production_hal_borrows_the_ieee802154_task_partition() {
    let RadioPartitions { ieee802154, .. } = RadioPartitions::for_validation();
    let (mut task, mut interrupts) = Ieee802154TaskRegisters::new(ieee802154);
    {
        let mut hal = Ieee802154PacHal::from_owned(&mut task, &mut interrupts);

        // Construct the real closed backend and exercise only its
        // portable ordering boundary; host tests must not perform MMIO.
        hal.order_device_accesses();
    }

    // Reuniting proves the combined borrow consumed neither disjoint
    // ownership half.
    let _partition = task.into_partition(interrupts);
}
