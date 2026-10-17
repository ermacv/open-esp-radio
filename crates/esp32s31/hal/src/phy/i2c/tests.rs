use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    vec::Vec,
};

use super::{
    BluetoothTxPowerControlAction, BluetoothTxPowerControlCompletion, BluetoothTxPowerControlError,
    BluetoothTxPowerControlI2cAccess, BluetoothTxPowerControlObservation,
    BluetoothTxPowerControlOperation, BluetoothTxPowerControlPrepareError,
    BluetoothTxPowerControlRegister, BluetoothTxPowerControlRestoreError,
    BluetoothTxPowerControlTransaction, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAddress,
    PhyI2cCommandMemoryInputs, PhyI2cConfigurationAction, PhyI2cConfigurationCommand,
    PhyI2cConfigurationError, PhyI2cConfigurationObservation, PhyI2cConfigurationOperation,
    PhyI2cConfigurationTransaction, PhyI2cHost, PhyI2cInitializationStageOneInputs,
    PhyI2cInitializationStageTwoError, PhyI2cParallelAccess, PhyI2cParallelWrite,
    configure_initialization_stage_two_with,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParallelEvent {
    SelectParallelMap,
    StartPair,
    RestoreRadioMap,
}

#[derive(Default)]
struct FakeParallelI2c {
    events: Vec<ParallelEvent>,
    initially_busy_host: Option<PhyI2cHost>,
    busy_after_start_host: Option<PhyI2cHost>,
    started: Cell<bool>,
}

impl PhyI2cParallelAccess for FakeParallelI2c {
    fn select_parallel_host_map(&mut self) {
        self.events.push(ParallelEvent::SelectParallelMap);
    }

    fn restore_radio_host_map(&mut self) {
        self.events.push(ParallelEvent::RestoreRadioMap);
    }

    fn is_busy(&self, host: PhyI2cHost) -> bool {
        if self.started.get() {
            self.busy_after_start_host == Some(host)
        } else {
            self.initially_busy_host == Some(host)
        }
    }

    fn start_pair(&mut self, _pair: PhyI2cParallelWrite) {
        self.started.set(true);
        self.events.push(ParallelEvent::StartPair);
    }
}

#[test]
fn retained_wake_i2c_stage_two_pairs_both_hosts_and_restores_the_radio_map() {
    let mut access = FakeParallelI2c::default();
    configure_initialization_stage_two_with(
        &mut access,
        PhyI2cCommandMemoryInputs::new(1, 2, 3, 4, 5, 6),
        10,
    )
    .unwrap();

    assert_eq!(
        access.events.first(),
        Some(&ParallelEvent::SelectParallelMap)
    );
    assert_eq!(access.events.last(), Some(&ParallelEvent::RestoreRadioMap));
    let pairs = access
        .events
        .iter()
        .filter(|event| **event == ParallelEvent::StartPair)
        .count();
    assert_eq!(pairs, 22);
}

#[test]
fn retained_wake_i2c_stage_two_restores_the_radio_map_on_busy_entry() {
    let mut access = FakeParallelI2c {
        initially_busy_host: Some(PhyI2cHost::Host1),
        ..FakeParallelI2c::default()
    };
    assert_eq!(
        configure_initialization_stage_two_with(
            &mut access,
            PhyI2cCommandMemoryInputs::new(1, 2, 3, 4, 5, 6),
            10,
        ),
        Err(PhyI2cInitializationStageTwoError::BusyAtStart {
            host: PhyI2cHost::Host1,
        })
    );
    assert_eq!(
        access.events,
        [
            ParallelEvent::SelectParallelMap,
            ParallelEvent::RestoreRadioMap,
        ]
    );
}

#[test]
fn retained_wake_i2c_stage_two_bounds_completion_and_restores_the_radio_map() {
    let mut access = FakeParallelI2c {
        busy_after_start_host: Some(PhyI2cHost::Host0),
        ..FakeParallelI2c::default()
    };
    assert_eq!(
        configure_initialization_stage_two_with(
            &mut access,
            PhyI2cCommandMemoryInputs::new(1, 2, 3, 4, 5, 6),
            2,
        ),
        Err(PhyI2cInitializationStageTwoError::CompletionTimeout {
            host: PhyI2cHost::Host0,
            pair: 0,
        })
    );
    assert_eq!(access.events.last(), Some(&ParallelEvent::RestoreRadioMap));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Read(BluetoothTxPowerControlRegister),
    Write(BluetoothTxPowerControlRegister, u8),
}

struct FakeI2c {
    restore: Option<(u8, u8)>,
    reads: RefCell<VecDeque<u8>>,
    operations: Vec<Operation>,
    busy_starts: u8,
}

impl FakeI2c {
    fn new(reads: impl IntoIterator<Item = u8>) -> Self {
        Self {
            restore: None,
            reads: RefCell::new(reads.into_iter().collect()),
            operations: Vec::new(),
            busy_starts: 0,
        }
    }

    fn accept_start(&mut self) -> Result<(), ()> {
        if self.busy_starts == 0 {
            Ok(())
        } else {
            self.busy_starts -= 1;
            Err(())
        }
    }
}

impl BluetoothTxPowerControlI2cAccess for FakeI2c {
    fn reserve_restore(&mut self) -> Result<(), BluetoothTxPowerControlPrepareError> {
        if self.restore.is_some() {
            Err(BluetoothTxPowerControlPrepareError::RestorePending)
        } else {
            self.restore = Some((0, 0));
            Ok(())
        }
    }

    fn capture_low(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        let Some((_, high)) = self.restore else {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        };
        self.restore = Some((value, high));
        Ok(())
    }

    fn capture_high(&mut self, value: u8) -> Result<(), BluetoothTxPowerControlRestoreError> {
        let Some((low, _)) = self.restore else {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        };
        self.restore = Some((low, value));
        Ok(())
    }

    fn restore_values(&self) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        self.restore
            .ok_or(BluetoothTxPowerControlRestoreError::RestoreNotPending)
    }

    fn finish_restore(&mut self) -> Result<(), BluetoothTxPowerControlRestoreError> {
        self.restore_values()?;
        self.restore = None;
        Ok(())
    }

    fn start_read(&mut self, register: BluetoothTxPowerControlRegister) -> Result<(), ()> {
        self.accept_start()?;
        self.operations.push(Operation::Read(register));
        Ok(())
    }

    fn start_write(
        &mut self,
        register: BluetoothTxPowerControlRegister,
        value: u8,
    ) -> Result<(), ()> {
        self.accept_start()?;
        self.operations.push(Operation::Write(register, value));
        Ok(())
    }

    fn observe_read(&self) -> Result<u8, ()> {
        self.reads.borrow_mut().pop_front().ok_or(())
    }

    fn observe_write(&self) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeConfigurationI2c {
    accepted_commands: u8,
    busy_starts: u8,
    pending_observations: u8,
    /// The address of the command in flight; completion edges must observe
    /// the same block's host.
    started: core::cell::Cell<Option<PhyI2cAddress>>,
    /// Every observed address, in order.
    observed: core::cell::RefCell<std::vec::Vec<PhyI2cAddress>>,
}

impl oer_radio_analog::AnalogRegisterBus for FakeConfigurationI2c {
    type Address = PhyI2cAddress;

    fn try_start_read(&mut self, address: PhyI2cAddress) -> Result<(), oer_radio_analog::Busy> {
        if self.busy_starts != 0 {
            self.busy_starts -= 1;
            return Err(oer_radio_analog::Busy);
        }
        self.accepted_commands += 1;
        self.started.set(Some(address));
        Ok(())
    }

    fn try_start_write(
        &mut self,
        address: PhyI2cAddress,
        _value: u8,
    ) -> Result<(), oer_radio_analog::Busy> {
        if self.busy_starts != 0 {
            self.busy_starts -= 1;
            return Err(oer_radio_analog::Busy);
        }
        self.accepted_commands += 1;
        self.started.set(Some(address));
        Ok(())
    }

    fn try_finish_read(&self, address: PhyI2cAddress) -> Result<u8, oer_radio_analog::Busy> {
        assert_eq!(self.started.get(), Some(address));
        self.observed.borrow_mut().push(address);
        if self.pending_observations == 0 {
            Ok(0xa0)
        } else {
            Err(oer_radio_analog::Busy)
        }
    }

    fn try_finish_write(&self, address: PhyI2cAddress) -> Result<(), oer_radio_analog::Busy> {
        assert_eq!(self.started.get(), Some(address));
        self.observed.borrow_mut().push(address);
        if self.pending_observations == 0 {
            Ok(())
        } else {
            Err(oer_radio_analog::Busy)
        }
    }
}

fn drive_configuration(
    transaction: &mut PhyI2cConfigurationTransaction,
    access: &mut FakeConfigurationI2c,
) {
    for _ in 0..64 {
        match transaction.action() {
            PhyI2cConfigurationAction::StartCommand => {
                transaction.start_with(access).unwrap();
            }
            PhyI2cConfigurationAction::AwaitCompletionEdge => {
                assert_eq!(
                    transaction.observe_with(access).unwrap(),
                    PhyI2cConfigurationObservation::EdgeConsumed
                );
            }
            PhyI2cConfigurationAction::Complete => return,
        }
    }
    panic!("PHY-I2C configuration exceeded its finite plan")
}

/// A configuration command completes on its own block's host: the SAR2
/// initialization of the host-zero block and the RC settings of a host-one
/// block observe the addresses they started, read and write alike.
#[test]
fn configuration_commands_observe_their_own_block() {
    for operation in [
        PhyI2cConfigurationOperation::Sar2Initialization,
        PhyI2cConfigurationOperation::RcCalibrationSettings,
    ] {
        let mut access = FakeConfigurationI2c::default();
        let mut transaction = PhyI2cConfigurationTransaction::new(operation);
        drive_configuration(&mut transaction, &mut access);
        assert_eq!(transaction.action(), PhyI2cConfigurationAction::Complete);
        let expected: std::vec::Vec<_> = (0..operation.command_count())
            .filter_map(|index| operation.command(index))
            .flat_map(|command| match command {
                PhyI2cConfigurationCommand::Modify(..) => {
                    [Some(command.address()), Some(command.address())]
                }
                _ => [Some(command.address()), None],
            })
            .flatten()
            .collect();
        assert_eq!(*access.observed.borrow(), expected);
    }
}

#[test]
fn configuration_operations_are_finite_and_retry_a_busy_start() {
    let mut access = FakeConfigurationI2c {
        busy_starts: 1,
        ..FakeConfigurationI2c::default()
    };
    let mut bias = PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::BiasRegisters);

    assert_eq!(
        bias.start_with(&mut access),
        Err(PhyI2cConfigurationError::BusyAtStart)
    );
    assert_eq!(bias.action(), PhyI2cConfigurationAction::StartCommand);
    drive_configuration(&mut bias, &mut access);
    assert_eq!(bias.action(), PhyI2cConfigurationAction::Complete);
    assert_eq!(access.accepted_commands, 2);

    let mut adc_rate = PhyI2cConfigurationTransaction::new(
        PhyI2cConfigurationOperation::ConfigureAdcRate(PhyAdcRate::High),
    );
    drive_configuration(&mut adc_rate, &mut access);
    assert_eq!(adc_rate.action(), PhyI2cConfigurationAction::Complete);

    let mut bbpll =
        PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::EnableBbpllCalibration);
    drive_configuration(&mut bbpll, &mut access);
    assert_eq!(bbpll.action(), PhyI2cConfigurationAction::Complete);

    access.accepted_commands = 0;
    let mut filter_dcap =
        PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::FilterDcap(
            PhyFilterDcapInputs::new(0x12, 0x34, 0x3a, 0x56, 0x87),
        ));

    drive_configuration(&mut filter_dcap, &mut access);
    assert_eq!(filter_dcap.action(), PhyI2cConfigurationAction::Complete);
    assert_eq!(access.accepted_commands, 18);
    assert_eq!(
        filter_dcap.observe_with(&access),
        Err(PhyI2cConfigurationError::AlreadyComplete)
    );

    access.accepted_commands = 0;
    let mut initialization =
        PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::InitializationStageOne(
            PhyI2cInitializationStageOneInputs::new(0x55, 0xfe),
        ));
    drive_configuration(&mut initialization, &mut access);
    assert_eq!(initialization.action(), PhyI2cConfigurationAction::Complete);
    assert_eq!(access.accepted_commands, 26);

    access.accepted_commands = 0;
    let mut sar2 =
        PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::Sar2Initialization);
    access.busy_starts = 1;
    assert_eq!(
        sar2.start_with(&mut access),
        Err(PhyI2cConfigurationError::BusyAtStart)
    );
    sar2.start_with(&mut access).unwrap();
    access.pending_observations = 1;
    assert_eq!(
        sar2.observe_with(&access),
        Ok(PhyI2cConfigurationObservation::StillPending)
    );
    assert_eq!(
        sar2.action(),
        PhyI2cConfigurationAction::AwaitCompletionEdge
    );
    access.pending_observations = 0;
    drive_configuration(&mut sar2, &mut access);
    assert_eq!(sar2.action(), PhyI2cConfigurationAction::Complete);
    assert_eq!(access.accepted_commands, 3);

    access.accepted_commands = 0;
    let mut rc_calibration =
        PhyI2cConfigurationTransaction::new(PhyI2cConfigurationOperation::RcCalibrationSettings);
    drive_configuration(&mut rc_calibration, &mut access);
    assert_eq!(rc_calibration.action(), PhyI2cConfigurationAction::Complete);
    assert_eq!(access.accepted_commands, 6);
}

fn drive(
    transaction: &mut BluetoothTxPowerControlTransaction,
    access: &mut FakeI2c,
) -> BluetoothTxPowerControlCompletion {
    for _ in 0..32 {
        match transaction.action() {
            BluetoothTxPowerControlAction::StartCommand => {
                transaction.start_with(access).unwrap();
            }
            BluetoothTxPowerControlAction::AwaitCompletionEdge => {
                assert_eq!(
                    transaction.observe_with(access).unwrap(),
                    BluetoothTxPowerControlObservation::EdgeConsumed
                );
            }
            BluetoothTxPowerControlAction::Complete(completion) => return completion,
        }
    }
    panic!("Bluetooth TX-power control transaction exceeded its finite plan")
}

#[test]
fn prepare_configure_and_restore_preserve_analog_order_and_snapshot() {
    let mut access = FakeI2c::new([0xa5, 0xd5]);
    let mut prepare =
        BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::PrepareRestore);
    assert_eq!(
        drive(&mut prepare, &mut access),
        BluetoothTxPowerControlCompletion::RestorePrepared
    );
    assert_eq!(access.restore, Some((0xa5, 0x15)));

    access.operations.clear();
    *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
    let mut configure = BluetoothTxPowerControlTransaction::new(
        BluetoothTxPowerControlOperation::ConfigureCalibration,
    );
    assert_eq!(
        drive(&mut configure, &mut access),
        BluetoothTxPowerControlCompletion::CalibrationConfigured
    );
    assert_eq!(
        access.operations,
        [
            Operation::Write(BluetoothTxPowerControlRegister::Low0, 2),
            Operation::Write(BluetoothTxPowerControlRegister::Low1, 2),
            Operation::Read(BluetoothTxPowerControlRegister::High0),
            Operation::Write(BluetoothTxPowerControlRegister::High0, 0xc2),
            Operation::Read(BluetoothTxPowerControlRegister::High1),
            Operation::Write(BluetoothTxPowerControlRegister::High1, 0x82),
        ]
    );

    access.operations.clear();
    *access.reads.borrow_mut() = [0xc0, 0x80].into_iter().collect();
    let mut restore =
        BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::Restore);
    assert_eq!(
        drive(&mut restore, &mut access),
        BluetoothTxPowerControlCompletion::Restored
    );
    assert_eq!(
        access.operations,
        [
            Operation::Write(BluetoothTxPowerControlRegister::Low0, 0xa5),
            Operation::Write(BluetoothTxPowerControlRegister::Low1, 0xa5),
            Operation::Read(BluetoothTxPowerControlRegister::High0),
            Operation::Write(BluetoothTxPowerControlRegister::High0, 0xd5),
            Operation::Read(BluetoothTxPowerControlRegister::High1),
            Operation::Write(BluetoothTxPowerControlRegister::High1, 0x95),
        ]
    );
    assert_eq!(access.restore, None);
}

#[test]
fn prepare_rejects_an_owned_restore_before_starting_i2c() {
    let mut access = FakeI2c::new([]);
    access.restore = Some((0, 0));
    let mut transaction =
        BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::PrepareRestore);
    assert_eq!(
        transaction.start_with(&mut access),
        Err(BluetoothTxPowerControlError::RestorePending)
    );
    assert!(access.operations.is_empty());
}

#[test]
fn prepare_retries_a_busy_first_command_without_reserving_restore_twice() {
    let mut access = FakeI2c::new([0xa5, 0xd5]);
    access.busy_starts = 1;
    let mut transaction =
        BluetoothTxPowerControlTransaction::new(BluetoothTxPowerControlOperation::PrepareRestore);

    assert_eq!(
        transaction.start_with(&mut access),
        Err(BluetoothTxPowerControlError::BusyAtStart)
    );
    assert_eq!(
        transaction.action(),
        BluetoothTxPowerControlAction::StartCommand
    );
    assert!(access.operations.is_empty());

    assert_eq!(
        drive(&mut transaction, &mut access),
        BluetoothTxPowerControlCompletion::RestorePrepared
    );
    assert_eq!(access.restore, Some((0xa5, 0x15)));
}
