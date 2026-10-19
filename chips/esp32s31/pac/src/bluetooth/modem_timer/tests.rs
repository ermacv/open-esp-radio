extern crate std;

use std::vec::Vec;

use super::{
    BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
    BluetoothModemLpTimerEpoch, BluetoothModemLpTimerHandlerRegisterObservation,
    BluetoothModemLpTimerInstant, BluetoothModemLpTimerInterruptObservation,
    ModemLpTimerHandlerRegisterDisposition, ModemLpTimerHandlerRegisterTransaction,
    ModemLpTimerInterruptDisposition, ModemLpTimerInterruptTransaction,
    ModemLpTimerLowPowerInitTransaction, ModemLpTimerRuntimeTransaction,
    ModemLpTimerStartTransaction, ModemLpTimerTransaction, execute_modem_lp_timer_compare_disable,
    execute_modem_lp_timer_compare_program, execute_modem_lp_timer_counter_sample,
    execute_modem_lp_timer_handler_registers, execute_modem_lp_timer_interrupt,
    execute_modem_lp_timer_low_power_init, execute_modem_lp_timer_prepare,
    execute_modem_lp_timer_start,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparationOperation {
    GeneratedTransaction,
    DeviceFence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartOperation {
    StartCounter,
    DeviceFence,
}

struct StartRecorder {
    operations: Vec<StartOperation>,
}

impl ModemLpTimerStartTransaction for StartRecorder {
    fn start_counter(&mut self) {
        self.operations.push(StartOperation::StartCounter);
    }

    fn fence(&mut self) {
        self.operations.push(StartOperation::DeviceFence);
    }
}

#[test]
fn runtime_counter_start_orders_the_command_before_publication() {
    let mut recorder = StartRecorder {
        operations: Vec::new(),
    };

    execute_modem_lp_timer_start(&mut recorder);

    assert_eq!(
        recorder.operations,
        [StartOperation::StartCounter, StartOperation::DeviceFence]
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LowPowerInitOperation {
    InitializeConfig,
    ClearControls04,
    ObserveControl2(bool),
    PublishControl1,
    Fence,
}

struct LowPowerInitRecorder {
    control_2: bool,
    operations: Vec<LowPowerInitOperation>,
}

impl ModemLpTimerLowPowerInitTransaction for LowPowerInitRecorder {
    fn initialize_config(&mut self) {
        self.operations
            .push(LowPowerInitOperation::InitializeConfig);
    }

    fn clear_controls_0_4(&mut self) {
        self.operations.push(LowPowerInitOperation::ClearControls04);
    }

    fn control_2_is_set(&mut self) -> bool {
        self.operations
            .push(LowPowerInitOperation::ObserveControl2(self.control_2));
        self.control_2
    }

    fn publish_control_1(&mut self) {
        self.operations.push(LowPowerInitOperation::PublishControl1);
    }

    fn fence(&mut self) {
        self.operations.push(LowPowerInitOperation::Fence);
    }
}

#[test]
fn low_power_init_skips_control_1_when_control_2_is_clear() {
    let mut recorder = LowPowerInitRecorder {
        control_2: false,
        operations: Vec::new(),
    };

    let observation = execute_modem_lp_timer_low_power_init(&mut recorder);

    assert_eq!(
        observation,
        BluetoothLowPowerRuntimeControlObservation::Control2Clear
    );
    assert_eq!(
        recorder.operations,
        [
            LowPowerInitOperation::InitializeConfig,
            LowPowerInitOperation::ClearControls04,
            LowPowerInitOperation::ObserveControl2(false),
            LowPowerInitOperation::Fence,
        ]
    );
}

#[test]
fn low_power_init_publishes_control_1_from_the_conditional_branch() {
    let mut recorder = LowPowerInitRecorder {
        control_2: true,
        operations: Vec::new(),
    };

    let observation = execute_modem_lp_timer_low_power_init(&mut recorder);

    assert_eq!(
        observation,
        BluetoothLowPowerRuntimeControlObservation::Control2SetControl1Published
    );
    assert_eq!(
        recorder.operations,
        [
            LowPowerInitOperation::InitializeConfig,
            LowPowerInitOperation::ClearControls04,
            LowPowerInitOperation::ObserveControl2(true),
            LowPowerInitOperation::PublishControl1,
            LowPowerInitOperation::Fence,
        ]
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptOperation {
    ReadStatus0038(u32),
    ReadValue006c(u32),
    TestControl0058Bit2(bool),
    SetControl0058Bit1FromFreshRead,
    Fence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandlerRegisterOperation {
    SampleState0024(bool),
    ClearState0024,
    SampleState002c(bool),
    ClearState002c,
    SampleFinalState0024,
    Fence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeOperation {
    ReadCounter(u32),
    SampleRollover(bool),
    ClearRollover,
    PublishSoftwarePending,
    DisableCompare,
    WriteCompare(u32),
    TriggerTimerCommand,
    Fence,
}

struct RuntimeRecorder {
    counters: [u32; 2],
    counter_index: usize,
    rollover: bool,
    operations: Vec<RuntimeOperation>,
}

impl RuntimeRecorder {
    fn new(first_counter: u32, fresh_counter: u32, rollover: bool) -> Self {
        Self {
            counters: [first_counter, fresh_counter],
            counter_index: 0,
            rollover,
            operations: Vec::new(),
        }
    }
}

impl ModemLpTimerRuntimeTransaction for RuntimeRecorder {
    fn read_counter(&mut self) -> u32 {
        let counter = self.counters[self.counter_index.min(1)];
        self.counter_index += 1;
        self.operations.push(RuntimeOperation::ReadCounter(counter));
        counter
    }

    fn rollover_low_byte_nonzero(&mut self) -> bool {
        self.operations
            .push(RuntimeOperation::SampleRollover(self.rollover));
        self.rollover
    }

    fn clear_rollover(&mut self) {
        self.operations.push(RuntimeOperation::ClearRollover);
    }

    fn publish_software_pending(&mut self) {
        self.operations
            .push(RuntimeOperation::PublishSoftwarePending);
    }

    fn disable_compare(&mut self) {
        self.operations.push(RuntimeOperation::DisableCompare);
    }

    fn write_compare(&mut self, image: crate::generated::BluetoothModemLpTimerCompareImage) {
        self.operations
            .push(RuntimeOperation::WriteCompare(image.get()));
    }

    fn trigger_timer_command(&mut self) {
        self.operations.push(RuntimeOperation::TriggerTimerCommand);
    }

    fn fence(&mut self) {
        self.operations.push(RuntimeOperation::Fence);
    }
}

struct HandlerRegisterRecorder {
    state_0024_low_byte_nonzero: bool,
    state_002c_low_byte_nonzero: bool,
    operations: Vec<HandlerRegisterOperation>,
}

impl HandlerRegisterRecorder {
    fn new(state_0024_low_byte_nonzero: bool, state_002c_low_byte_nonzero: bool) -> Self {
        Self {
            state_0024_low_byte_nonzero,
            state_002c_low_byte_nonzero,
            operations: Vec::new(),
        }
    }
}

impl ModemLpTimerHandlerRegisterTransaction for HandlerRegisterRecorder {
    fn sample_state_0024_low_byte_nonzero(&mut self) -> bool {
        self.operations
            .push(HandlerRegisterOperation::SampleState0024(
                self.state_0024_low_byte_nonzero,
            ));
        self.state_0024_low_byte_nonzero
    }

    fn clear_state_0024(&mut self) {
        self.operations
            .push(HandlerRegisterOperation::ClearState0024);
    }

    fn sample_state_002c_low_byte_nonzero(&mut self) -> bool {
        self.operations
            .push(HandlerRegisterOperation::SampleState002c(
                self.state_002c_low_byte_nonzero,
            ));
        self.state_002c_low_byte_nonzero
    }

    fn clear_state_002c(&mut self) {
        self.operations
            .push(HandlerRegisterOperation::ClearState002c);
    }

    fn sample_final_state_0024(&mut self) {
        self.operations
            .push(HandlerRegisterOperation::SampleFinalState0024);
    }

    fn fence(&mut self) {
        self.operations.push(HandlerRegisterOperation::Fence);
    }
}

struct InterruptRecorder {
    status_0038: u32,
    value_006c: u32,
    control_0058_bit_2: bool,
    operations: Vec<InterruptOperation>,
}

impl InterruptRecorder {
    fn new(status_0038: u32, value_006c: u32, control_0058_bit_2: bool) -> Self {
        Self {
            status_0038,
            value_006c,
            control_0058_bit_2,
            operations: Vec::new(),
        }
    }
}

impl ModemLpTimerInterruptTransaction for InterruptRecorder {
    fn read_status_0038(&mut self) -> u32 {
        self.operations
            .push(InterruptOperation::ReadStatus0038(self.status_0038));
        self.status_0038
    }

    fn read_value_006c(&mut self) -> u32 {
        self.operations
            .push(InterruptOperation::ReadValue006c(self.value_006c));
        self.value_006c
    }

    fn control_0058_bit_2_is_set(&mut self) -> bool {
        self.operations
            .push(InterruptOperation::TestControl0058Bit2(
                self.control_0058_bit_2,
            ));
        self.control_0058_bit_2
    }

    fn set_control_0058_bit_1_from_fresh_read(&mut self) {
        self.operations
            .push(InterruptOperation::SetControl0058Bit1FromFreshRead);
    }

    fn fence(&mut self) {
        self.operations.push(InterruptOperation::Fence);
    }
}

#[derive(Default)]
struct Recorder(Vec<PreparationOperation>);

impl ModemLpTimerTransaction for Recorder {
    fn prepare_hardware(&mut self) {
        self.0.push(PreparationOperation::GeneratedTransaction);
    }

    fn fence(&mut self) {
        self.0.push(PreparationOperation::DeviceFence);
    }
}

#[test]
fn source_127_preparation_fences_after_the_generated_transaction() {
    let mut recorder = Recorder::default();

    execute_modem_lp_timer_prepare(&mut recorder);

    assert_eq!(
        recorder.0,
        [
            PreparationOperation::GeneratedTransaction,
            PreparationOperation::DeviceFence,
        ]
    );
}

#[test]
fn source_127_zero_status_returns_after_exactly_one_read() {
    let mut recorder = InterruptRecorder::new(0, u32::MAX, true);

    let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

    assert_eq!(disposition, ModemLpTimerInterruptDisposition::Spurious);
    assert_eq!(recorder.operations, [InterruptOperation::ReadStatus0038(0)]);
}

#[test]
fn source_127_zero_value_dispatches_without_control_read() {
    let mut recorder = InterruptRecorder::new(1, 0, true);

    let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerInterruptDisposition::HandlerPending(
            BluetoothModemLpTimerInterruptObservation::Value006cZero
        )
    );
    assert_eq!(
        recorder.operations,
        [
            InterruptOperation::ReadStatus0038(1),
            InterruptOperation::ReadValue006c(0),
            InterruptOperation::Fence,
        ]
    );
}

#[test]
fn source_127_clear_control_2_dispatches_without_write() {
    let mut recorder = InterruptRecorder::new(1, 1, false);

    let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerInterruptDisposition::HandlerPending(
            BluetoothModemLpTimerInterruptObservation::Control2Clear
        )
    );
    assert_eq!(
        recorder.operations,
        [
            InterruptOperation::ReadStatus0038(1),
            InterruptOperation::ReadValue006c(1),
            InterruptOperation::TestControl0058Bit2(false),
            InterruptOperation::Fence,
        ]
    );
}

#[test]
fn source_127_set_control_2_publishes_control_1_before_dispatch() {
    let mut recorder = InterruptRecorder::new(1, 1, true);

    let disposition = execute_modem_lp_timer_interrupt(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerInterruptDisposition::HandlerPending(
            BluetoothModemLpTimerInterruptObservation::Control2SetControl1Published
        )
    );
    assert_eq!(
        recorder.operations,
        [
            InterruptOperation::ReadStatus0038(1),
            InterruptOperation::ReadValue006c(1),
            InterruptOperation::TestControl0058Bit2(true),
            InterruptOperation::SetControl0058Bit1FromFreshRead,
            InterruptOperation::Fence,
        ]
    );
}

#[test]
fn source_127_common_handler_rearms_after_idle_state_samples() {
    let mut recorder = HandlerRegisterRecorder::new(false, false);

    let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

    assert_eq!(disposition, ModemLpTimerHandlerRegisterDisposition::Rearmed);
    assert_eq!(
        recorder.operations,
        [
            HandlerRegisterOperation::SampleState0024(false),
            HandlerRegisterOperation::SampleState002c(false),
            HandlerRegisterOperation::SampleFinalState0024,
        ]
    );
}

#[test]
fn source_127_common_handler_acknowledges_queue_work_before_software_pending() {
    let mut recorder = HandlerRegisterRecorder::new(true, false);

    let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
            BluetoothModemLpTimerHandlerRegisterObservation {
                state_0024_low_byte_nonzero: true,
                state_002c_low_byte_nonzero: false,
            }
        )
    );
    assert_eq!(
        recorder.operations,
        [
            HandlerRegisterOperation::SampleState0024(true),
            HandlerRegisterOperation::ClearState0024,
            HandlerRegisterOperation::SampleState002c(false),
            HandlerRegisterOperation::Fence,
        ]
    );
}

#[test]
fn source_127_common_handler_acknowledges_epoch_work_before_software_pending() {
    let mut recorder = HandlerRegisterRecorder::new(false, true);

    let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
            BluetoothModemLpTimerHandlerRegisterObservation {
                state_0024_low_byte_nonzero: false,
                state_002c_low_byte_nonzero: true,
            }
        )
    );
    assert_eq!(
        recorder.operations,
        [
            HandlerRegisterOperation::SampleState0024(false),
            HandlerRegisterOperation::SampleState002c(true),
            HandlerRegisterOperation::ClearState002c,
            HandlerRegisterOperation::Fence,
        ]
    );
}

#[test]
fn source_127_common_handler_acknowledges_both_states_in_vendor_order() {
    let mut recorder = HandlerRegisterRecorder::new(true, true);

    let disposition = execute_modem_lp_timer_handler_registers(&mut recorder);

    assert_eq!(
        disposition,
        ModemLpTimerHandlerRegisterDisposition::SoftwarePending(
            BluetoothModemLpTimerHandlerRegisterObservation {
                state_0024_low_byte_nonzero: true,
                state_002c_low_byte_nonzero: true,
            }
        )
    );
    assert_eq!(
        recorder.operations,
        [
            HandlerRegisterOperation::SampleState0024(true),
            HandlerRegisterOperation::ClearState0024,
            HandlerRegisterOperation::SampleState002c(true),
            HandlerRegisterOperation::ClearState002c,
            HandlerRegisterOperation::Fence,
        ]
    );
}

#[test]
fn modem_lp_counter_without_rollover_returns_one_bounded_sample() {
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    epoch.advance_for_handler_registers(BluetoothModemLpTimerHandlerRegisterObservation {
        state_0024_low_byte_nonzero: false,
        state_002c_low_byte_nonzero: true,
    });
    let mut recorder = RuntimeRecorder::new(0x0000_1234, 0, false);

    let observation = execute_modem_lp_timer_counter_sample(&mut recorder, &mut epoch);

    assert_eq!(epoch.high_byte(), 1);
    assert_eq!(observation.instant().bits(), 0x0100_1234);
    assert!(!observation.rollover_was_acknowledged());
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::ReadCounter(0x0000_1234),
            RuntimeOperation::SampleRollover(false),
        ]
    );
}

#[test]
fn modem_lp_counter_rollover_advances_epoch_before_fresh_sample_and_ack() {
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    let mut recorder = RuntimeRecorder::new(0x00ff_fffe, 0x0000_0003, true);

    let observation = execute_modem_lp_timer_counter_sample(&mut recorder, &mut epoch);

    assert_eq!(epoch.high_byte(), 1);
    assert_eq!(observation.instant().bits(), 0x0100_0003);
    assert!(observation.rollover_was_acknowledged());
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::ReadCounter(0x00ff_fffe),
            RuntimeOperation::SampleRollover(true),
            RuntimeOperation::ReadCounter(0x0000_0003),
            RuntimeOperation::ClearRollover,
            RuntimeOperation::PublishSoftwarePending,
            RuntimeOperation::TriggerTimerCommand,
            RuntimeOperation::Fence,
        ]
    );
}

#[test]
fn modem_lp_empty_queue_disables_compare_in_one_finite_step() {
    let mut recorder = RuntimeRecorder::new(0, 0, false);

    execute_modem_lp_timer_compare_disable(&mut recorder);

    assert_eq!(
        recorder.operations,
        [RuntimeOperation::DisableCompare, RuntimeOperation::Fence]
    );
}

#[test]
fn modem_lp_compare_requests_immediate_work_for_due_deadline() {
    let mut recorder = RuntimeRecorder::new(0x100, 0, false);

    let disposition = execute_modem_lp_timer_compare_program(
        &mut recorder,
        BluetoothModemLpTimerInstant::from_bits(0x102),
        BluetoothModemLpTimerEpoch::new(),
    );

    assert_eq!(
        disposition,
        BluetoothModemLpTimerCompareDisposition::Immediate
    );
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::DisableCompare,
            RuntimeOperation::ReadCounter(0x100),
            RuntimeOperation::SampleRollover(false),
            RuntimeOperation::PublishSoftwarePending,
            RuntimeOperation::TriggerTimerCommand,
            RuntimeOperation::Fence,
        ]
    );
}

#[test]
fn modem_lp_compare_programs_near_deadline_without_callback_loop() {
    let mut recorder = RuntimeRecorder::new(0x100, 0, false);

    let disposition = execute_modem_lp_timer_compare_program(
        &mut recorder,
        BluetoothModemLpTimerInstant::from_bits(0x200),
        BluetoothModemLpTimerEpoch::new(),
    );

    assert_eq!(
        disposition,
        BluetoothModemLpTimerCompareDisposition::Deadline
    );
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::DisableCompare,
            RuntimeOperation::ReadCounter(0x100),
            RuntimeOperation::SampleRollover(false),
            RuntimeOperation::WriteCompare(0x200),
            RuntimeOperation::TriggerTimerCommand,
            RuntimeOperation::Fence,
        ]
    );
}

#[test]
fn modem_lp_compare_uses_half_range_checkpoint_for_distant_deadline() {
    let mut recorder = RuntimeRecorder::new(0x100, 0, false);

    let disposition = execute_modem_lp_timer_compare_program(
        &mut recorder,
        BluetoothModemLpTimerInstant::from_bits(0x0100_0200),
        BluetoothModemLpTimerEpoch::new(),
    );

    assert_eq!(
        disposition,
        BluetoothModemLpTimerCompareDisposition::HalfRangeCheckpoint
    );
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::DisableCompare,
            RuntimeOperation::ReadCounter(0x100),
            RuntimeOperation::SampleRollover(false),
            RuntimeOperation::WriteCompare(0x0080_0100),
            RuntimeOperation::TriggerTimerCommand,
            RuntimeOperation::Fence,
        ]
    );
}

#[test]
fn modem_lp_compare_accounts_for_unacknowledged_rollover_locally() {
    let mut recorder = RuntimeRecorder::new(0x00ff_fffe, 0x0000_0003, true);
    let epoch = BluetoothModemLpTimerEpoch::new();

    let disposition = execute_modem_lp_timer_compare_program(
        &mut recorder,
        BluetoothModemLpTimerInstant::from_bits(0x0100_0010),
        epoch,
    );

    assert_eq!(
        disposition,
        BluetoothModemLpTimerCompareDisposition::Deadline
    );
    assert_eq!(epoch.high_byte(), 0);
    assert_eq!(
        recorder.operations,
        [
            RuntimeOperation::DisableCompare,
            RuntimeOperation::ReadCounter(0x00ff_fffe),
            RuntimeOperation::SampleRollover(true),
            RuntimeOperation::ReadCounter(0x0000_0003),
            RuntimeOperation::WriteCompare(0x10),
            RuntimeOperation::TriggerTimerCommand,
            RuntimeOperation::Fence,
        ]
    );
}
