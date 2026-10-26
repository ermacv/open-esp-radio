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
    SamplePending,
    ClearPending,
    EnableRollover,
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

    fn write_compare(&mut self, image: u32) {
        self.operations.push(RuntimeOperation::WriteCompare(image));
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

    fn sample_pending(&mut self) -> super::BluetoothModemLpTimerPendingObservation {
        self.0.push(PreparationOperation::SamplePending);
        super::BluetoothModemLpTimerPendingObservation {
            state_0024: 7,
            state_002c: 3,
        }
    }
    fn clear_pending(&mut self) {
        self.0.push(PreparationOperation::ClearPending);
    }
    fn enable_rollover(&mut self) {
        self.0.push(PreparationOperation::EnableRollover);
    }
    fn fence(&mut self) {
        self.0.push(PreparationOperation::DeviceFence);
    }
}

#[test]
fn source_127_preparation_fences_after_the_generated_transaction() {
    let mut recorder = Recorder::default();

    let stale = execute_modem_lp_timer_prepare(&mut recorder);
    assert_eq!(stale.state_0024(), 7);
    assert_eq!(stale.state_002c(), 3);

    assert_eq!(
        recorder.0,
        [
            PreparationOperation::GeneratedTransaction,
            PreparationOperation::SamplePending,
            PreparationOperation::ClearPending,
            PreparationOperation::EnableRollover,
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

#[test]
fn a_delayed_plan_retains_arrivals_and_only_the_handler_requests_immediate_work() {
    use super::super::host::HostMemory;
    use oer_esp32s31_pac::modem_pwr::{word_0024_opaque, word_0048_opaque};
    use oer_time::{Clock, Duration, Instant};
    use oer_time_virtual::VirtualClock;

    for delay in [8, 9, 10, 11] {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(10));
        let mut memory = HostMemory::filled(0);
        memory.timer[word_0048_opaque::OFFSET / 4] = clock.now().as_micros() as u32;
        memory.timer[word_0024_opaque::OFFSET / 4] = 1;
        let mut epoch = BluetoothModemLpTimerEpoch::new();
        let work = {
            let (_, _, mut timer) = memory.owners();
            timer.take_software_work(&mut epoch).unwrap()
        };
        let before = memory.clone();
        let plan = work.plan(Some(BluetoothModemLpTimerInstant::from_bits(20)));
        assert_eq!(memory, before, "P2 planning cannot write timer registers");
        clock.advance(Duration::from_micros(delay)).unwrap();
        memory.timer[word_0048_opaque::OFFSET / 4] = clock.now().as_micros() as u32;
        // A fresh pending arrival after the snapshot is not acknowledged by apply.
        memory.timer[word_0024_opaque::OFFSET / 4] = 7;
        let (_, _, mut timer) = memory.owners();
        assert!(timer.apply_plan(plan, &mut epoch, None).is_none());
        assert_eq!(timer.runtime_control.read_word_0024_opaque().low_byte(), 1);
        let work = timer.take_software_work(&mut epoch).unwrap();
        assert!(work.dispatch_requested());
        assert_eq!(work.instant().bits(), clock.now().as_micros() as u32);
        assert!(timer.take_software_work(&mut epoch).is_none());
    }
}

#[test]
fn a_rollover_between_plan_and_apply_advances_the_handler_epoch_once() {
    use super::super::host::HostMemory;
    use oer_esp32s31_pac::modem_pwr::{word_002c_opaque, word_0024_opaque, word_0048_opaque};

    let mut memory = HostMemory::filled(0);
    memory.timer[word_0048_opaque::OFFSET / 4] = 0x00ff_fffe;
    memory.timer[word_0024_opaque::OFFSET / 4] = 1;
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    let work = {
        let (_, _, mut timer) = memory.owners();
        timer.take_software_work(&mut epoch).unwrap()
    };
    assert_eq!(work.epoch().high_byte(), 0);
    let plan = work.plan(Some(BluetoothModemLpTimerInstant::from_bits(0x0100_0010)));
    // Hardware rolls over and asserts pending while P2 holds only values.
    memory.timer[word_0048_opaque::OFFSET / 4] = 3;
    memory.timer[word_002c_opaque::OFFSET / 4] = 1;
    memory.timer[word_0024_opaque::OFFSET / 4] = 1;
    let (_, _, mut timer) = memory.owners();
    assert!(timer.apply_plan(plan, &mut epoch, None).is_none());
    assert_eq!(
        timer
            .runtime_control
            .read_word_006c_opaque()
            .bits_0_31_opaque(),
        0x10
    );
    assert_ne!(timer.runtime_control.read_word_002c_opaque().low_byte(), 0);
    assert_ne!(timer.runtime_control.read_word_0024_opaque().low_byte(), 0);
    let work = timer.take_software_work(&mut epoch).unwrap();
    assert_eq!(work.instant().bits(), 0x0100_0003);
    assert_eq!(work.epoch().high_byte(), 1);
    assert_eq!(epoch.high_byte(), 1);
    assert!(work.dispatch_requested());
    assert!(timer.take_software_work(&mut epoch).is_none());
    assert_eq!(epoch.high_byte(), 1);
}

#[test]
fn past_and_close_plans_follow_vendor_disable_sample_pending_command_order() {
    for deadline in [0xff, 0x100, 0x101, 0x102] {
        let mut recorder = RuntimeRecorder::new(0x100, 0, false);
        assert_eq!(
            execute_modem_lp_timer_compare_program(
                &mut recorder,
                BluetoothModemLpTimerInstant::from_bits(deadline),
                BluetoothModemLpTimerEpoch::new()
            ),
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
}

#[test]
fn entries_during_p2_work_retain_dispatch_and_epoch_until_the_plan_returns() {
    use super::super::host::HostMemory;
    use oer_esp32s31_pac::modem_pwr::{
        word_002c_opaque, word_0024_opaque, word_0038_opaque, word_0048_opaque,
    };

    let mut memory = HostMemory::filled(0);
    memory.timer[word_0024_opaque::OFFSET / 4] = 1;
    let mut epoch = BluetoothModemLpTimerEpoch::new();
    let work = {
        let (_, _, mut timer) = memory.owners();
        timer.take_software_work(&mut epoch).unwrap()
    };
    let plan = work.plan(Some(BluetoothModemLpTimerInstant::from_bits(0x0100_0010)));
    // Another source entry arrives while P2 is still processing the snapshot.
    memory.timer[word_0038_opaque::OFFSET / 4] = 1;
    memory.timer[word_0024_opaque::OFFSET / 4] = 1;
    memory.timer[word_002c_opaque::OFFSET / 4] = 1;
    memory.timer[word_0048_opaque::OFFSET / 4] = 3;
    let (_, _, mut timer) = memory.owners();
    let mut deferred = None;
    timer.retain_interrupt(&mut epoch, &mut deferred);
    assert_eq!(epoch.high_byte(), 1);
    assert_eq!(deferred, Some(true));
    assert_eq!(timer.runtime_control.read_word_0024_opaque().low_byte(), 0);
    assert_eq!(timer.runtime_control.read_word_002c_opaque().low_byte(), 0);
    // Reentry with no new state cannot erase the retained dispatch or count
    // the same rollover again.
    timer.retain_interrupt(&mut epoch, &mut deferred);
    assert_eq!(epoch.high_byte(), 1);
    assert_eq!(deferred, Some(true));
    let work = timer.apply_plan(plan, &mut epoch, deferred).unwrap();
    assert!(work.dispatch_requested());
    assert_eq!(work.instant().bits(), 0x0100_0003);
    assert_eq!(work.epoch().high_byte(), 1);
}

#[test]
fn prepare_exposes_stale_pending_and_clears_both_whole_words() {
    use super::super::host::HostMemory;
    use oer_esp32s31_pac::modem_pwr::{word_002c_opaque, word_0024_opaque};
    for (software, rollover) in [(0, 0), (7, 0), (0, 3), (7, 3)] {
        let mut memory = HostMemory::filled(0);
        memory.timer[word_0024_opaque::OFFSET / 4] = 0xaabb_cc00 | software;
        memory.timer[word_002c_opaque::OFFSET / 4] = 0xddee_ff00 | rollover;
        {
            let (_, _, mut timer) = memory.owners();
            let stale = timer.prepare_registers();
            assert_eq!(u32::from(stale.state_0024()), software);
            assert_eq!(u32::from(stale.state_002c()), rollover);
        }
        assert_eq!(memory.timer[word_0024_opaque::OFFSET / 4], 0);
        assert_eq!(memory.timer[word_002c_opaque::OFFSET / 4], 0);
    }
}

#[derive(Debug, Eq, PartialEq)]
enum StopOp {
    DisableDelivery,
    StopCounter,
    Fence,
    SamplePending,
    ReadStatus,
    ClearRollover,
    ClearPending,
}
struct StopRecorder {
    clock: oer_time_virtual::VirtualClock,
    counter_running: bool,
    stopped_counter: u64,
    delivery_enabled: bool,
    source_127_high: bool,
    pending: super::BluetoothModemLpTimerPendingObservation,
    status: u32,
    ops: Vec<StopOp>,
}
impl StopRecorder {
    fn new(pending: super::BluetoothModemLpTimerPendingObservation, status: u32) -> Self {
        Self {
            clock: oer_time_virtual::VirtualClock::new(),
            counter_running: true,
            stopped_counter: 0,
            delivery_enabled: true,
            source_127_high: status != 0,
            pending,
            status,
            ops: Vec::new(),
        }
    }

    fn counter(&self) -> u64 {
        use oer_time::Clock;
        if self.counter_running {
            self.clock.now().as_micros()
        } else {
            self.stopped_counter
        }
    }

    fn is_quiet(&self) -> bool {
        !self.counter_running
            && !self.delivery_enabled
            && !self.source_127_high
            && self.status == 0
            && self.pending == Default::default()
    }
}

impl super::ModemLpTimerStopTransaction for StopRecorder {
    fn disable_delivery(&mut self) {
        self.ops.push(StopOp::DisableDelivery);
        self.delivery_enabled = false;
        self.status = 0;
        self.source_127_high = false;
    }
    fn stop_counter(&mut self) {
        self.ops.push(StopOp::StopCounter);
        self.stopped_counter = self.counter();
        self.counter_running = false;
    }
    fn sample_pending(&mut self) -> super::BluetoothModemLpTimerPendingObservation {
        self.ops.push(StopOp::SamplePending);
        self.pending
    }
    fn read_status(&mut self) -> u32 {
        self.ops.push(StopOp::ReadStatus);
        self.status
    }
    fn clear_rollover(&mut self) {
        self.ops.push(StopOp::ClearRollover);
        self.pending.state_002c = 0;
    }
    fn clear_pending(&mut self) {
        self.ops.push(StopOp::ClearPending);
        self.pending = Default::default();
        self.status = 0;
        self.source_127_high = false;
    }
    fn fence(&mut self) {
        self.ops.push(StopOp::Fence);
    }
}

#[test]
fn shutdown_returns_pending_then_leaves_the_counter_bytes_status_and_output_quiet() {
    use oer_esp32s31_pac::modem_pwr::word_0038_opaque::Bits0_31Opaque;
    // SOURCE: HW6_S31_LP_QUIET_20261026, hw6-20261026-reset-02.
    for (software, rollover, status) in [
        (9, 0, Bits0_31Opaque::CompareSoftwarePending),
        (0, 5, Bits0_31Opaque::RolloverPending),
        (9, 5, Bits0_31Opaque::BothMeasuredPending),
    ] {
        let pending = super::BluetoothModemLpTimerPendingObservation {
            state_0024: software,
            state_002c: rollover,
        };
        let mut recorder = StopRecorder::new(pending, status.bits());
        recorder
            .clock
            .advance(oer_time::Duration::from_micros(3000))
            .unwrap();
        let counter_before_stop = recorder.counter();
        assert!(!recorder.is_quiet());
        assert_eq!(super::execute_modem_lp_timer_stop(&mut recorder), pending);
        assert!(recorder.is_quiet());
        recorder
            .clock
            .advance(oer_time::Duration::from_micros(4096))
            .unwrap();
        assert_eq!(recorder.counter(), counter_before_stop);
        assert_eq!(
            recorder.ops,
            [
                StopOp::DisableDelivery,
                StopOp::StopCounter,
                StopOp::Fence,
                StopOp::SamplePending,
                StopOp::ClearPending,
                StopOp::Fence,
            ]
        );
    }
}

#[test]
fn shutdown_samples_both_bytes_before_clearing_both_whole_words() {
    use super::super::host::HostMemory;
    use oer_esp32s31_pac::modem_pwr::{word_002c_opaque, word_0024_opaque};
    let mut memory = HostMemory::filled(0);
    memory.timer[word_0024_opaque::OFFSET / 4] = 0xaabb_cc07;
    memory.timer[word_002c_opaque::OFFSET / 4] = 0xddee_ff03;
    {
        let (_, _, mut timer) = memory.owners();
        let pending = timer.stop_for_shutdown();
        assert_eq!(pending.state_0024(), 7);
        assert_eq!(pending.state_002c(), 3);
    }
    assert_eq!(memory.timer[word_0024_opaque::OFFSET / 4], 0);
    assert_eq!(memory.timer[word_002c_opaque::OFFSET / 4], 0);
}

#[test]
fn stopped_rollover_is_acknowledged_once_without_arming_or_dispatch() {
    use oer_esp32s31_pac::modem_pwr::word_0038_opaque::Bits0_31Opaque;
    for status in [Bits0_31Opaque::Inactive, Bits0_31Opaque::RolloverPending] {
        let pending = super::BluetoothModemLpTimerPendingObservation {
            state_0024: 0,
            state_002c: 5,
        };
        let mut recorder = StopRecorder::new(pending, status.bits());
        let mut epoch = BluetoothModemLpTimerEpoch::new();
        assert_eq!(
            super::execute_modem_lp_timer_stopped_interrupt(&mut recorder, &mut epoch),
            Ok(
                super::BluetoothModemLpTimerStoppedInterruptObservation::RolloverAcknowledged(
                    pending
                )
            )
        );
        assert_eq!(epoch.high_byte(), 1);
        assert_eq!(
            recorder.ops,
            [
                StopOp::ReadStatus,
                StopOp::SamplePending,
                StopOp::ClearRollover,
                StopOp::Fence
            ]
        );
        recorder.ops.clear();
        assert_eq!(
            super::execute_modem_lp_timer_stopped_interrupt(&mut recorder, &mut epoch),
            Ok(super::BluetoothModemLpTimerStoppedInterruptObservation::Quiet)
        );
        assert_eq!(epoch.high_byte(), 1);
        assert_eq!(recorder.ops, [StopOp::ReadStatus, StopOp::SamplePending]);
    }
}

#[test]
fn stopped_software_pending_and_unknown_status_fault_without_acknowledgement() {
    for (software, status) in [(7, 0), (0, u32::MAX)] {
        let pending = super::BluetoothModemLpTimerPendingObservation {
            state_0024: software,
            state_002c: 3,
        };
        let mut recorder = StopRecorder::new(pending, status);
        let mut epoch = BluetoothModemLpTimerEpoch::new();
        let error =
            super::execute_modem_lp_timer_stopped_interrupt(&mut recorder, &mut epoch).unwrap_err();
        assert_eq!(
            error,
            if software != 0 {
                super::BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending(pending)
            } else {
                super::BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus(status)
            }
        );
        assert_eq!(recorder.pending, pending);
        assert_eq!(epoch.high_byte(), 0);
        assert_eq!(recorder.ops, [StopOp::ReadStatus, StopOp::SamplePending]);
    }
}
