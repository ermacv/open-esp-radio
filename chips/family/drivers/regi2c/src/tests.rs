extern crate std;

use std::{cell::Cell, rc::Rc, vec::Vec};

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    StartRead(u8),
    StartWrite(u8, u8),
}

/// One-host register model whose commands stay busy for `latency` polls.
struct Model {
    registers: [u8; 4],
    latency: u8,
    busy: Rc<Cell<u8>>,
    operations: Vec<Operation>,
}

impl Model {
    fn new(latency: u8) -> Self {
        Self {
            registers: [0; 4],
            latency,
            busy: Rc::new(Cell::new(0)),
            operations: Vec::new(),
        }
    }
}

impl AnalogRegisterBus for Model {
    type Address = u8;

    fn try_start_read(&mut self, address: u8) -> Result<(), BusRefusal> {
        if self.busy.get() != 0 {
            return Err(BusRefusal::Busy);
        }
        self.busy.set(self.latency);
        self.operations.push(Operation::StartRead(address));
        Ok(())
    }

    fn try_finish_read(&self, address: u8) -> Result<u8, BusRefusal> {
        if self.busy.get() != 0 {
            Err(BusRefusal::Busy)
        } else {
            Ok(self.registers[usize::from(address)])
        }
    }

    fn try_start_write(&mut self, address: u8, value: u8) -> Result<(), BusRefusal> {
        if self.busy.get() != 0 {
            return Err(BusRefusal::Busy);
        }
        self.busy.set(self.latency);
        self.registers[usize::from(address)] = value;
        self.operations.push(Operation::StartWrite(address, value));
        Ok(())
    }

    fn try_finish_write(&self, _address: u8) -> Result<(), BusRefusal> {
        if self.busy.get() != 0 {
            Err(BusRefusal::Busy)
        } else {
            Ok(())
        }
    }
}

fn run<T>(clock: &Cell<u8>, mut poll: impl FnMut() -> Step<T>) -> T {
    for _ in 0..64 {
        if let Step::Ready(value) = poll() {
            return value;
        }
        clock.set(clock.get().saturating_sub(1));
    }
    panic!("the transaction did not complete");
}

#[test]
fn a_field_read_extracts_after_the_completed_read() {
    let mut model = Model::new(3);
    model.registers[2] = 0b0110_0000;
    let clock = model.busy.clone();
    let mut read = FieldRead::new(&mut model, AnalogField::new(2_u8, 6, 5).expect("field"));
    assert_eq!(
        run(&clock, || read.poll().expect("matching completion")),
        0b11
    );
    assert_eq!(model.operations, [Operation::StartRead(2)]);
}

#[test]
fn a_field_write_reads_modifies_and_writes_once() {
    let mut model = Model::new(2);
    model.registers[1] = 0b1010_1010;
    let clock = model.busy.clone();
    let mut write = FieldWrite::new(
        &mut model,
        AnalogField::new(1_u8, 3, 0).expect("field"),
        0b0101,
    )
    .expect("fits");
    run(&clock, || write.poll().expect("matching completion"));
    assert_eq!(
        model.operations,
        [
            Operation::StartRead(1),
            Operation::StartWrite(1, 0b1010_0101)
        ]
    );
}

#[test]
fn a_busy_host_delays_the_start_without_losing_the_command() {
    let mut model = Model::new(1);
    model.busy.set(5);
    let clock = model.busy.clone();
    let mut write =
        FieldWrite::new(&mut model, AnalogField::new(0_u8, 0, 0).expect("field"), 1).expect("fits");
    run(&clock, || write.poll().expect("matching completion"));
    assert_eq!(
        model.operations,
        [Operation::StartRead(0), Operation::StartWrite(0, 1)]
    );
}

#[test]
fn a_value_wider_than_its_field_is_rejected() {
    let field = AnalogField::new(0_u8, 1, 0).expect("field");
    assert_eq!(
        FieldWrite::new(&mut Model::new(0), field, 4).err(),
        Some(ValueTooWide)
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParallelOperation {
    Select,
    Restore,
    Pair(u8),
}

struct ParallelModel {
    operations: Vec<ParallelOperation>,
    busy: Rc<Cell<[u8; 2]>>,
    latency: [u8; 2],
}

impl ParallelAnalogBus for ParallelModel {
    type Pair = u8;

    fn select_parallel_host_map(&mut self) {
        self.operations.push(ParallelOperation::Select);
    }

    fn restore_host_map(&mut self) {
        self.operations.push(ParallelOperation::Restore);
    }

    fn start_pair(&mut self, pair: u8) {
        assert_eq!(
            self.busy.get(),
            [0; 2],
            "a pair started while a host was busy"
        );
        self.busy.set(self.latency);
        self.operations.push(ParallelOperation::Pair(pair));
    }

    fn is_busy(&self, host: Host) -> bool {
        self.busy.get()[host as usize] != 0
    }
}

#[test]
fn parallel_pairs_start_after_both_hosts_are_idle_between_the_host_maps() {
    let mut model = ParallelModel {
        operations: Vec::new(),
        busy: Rc::new(Cell::new([0; 2])),
        latency: [2, 3],
    };
    let clock = model.busy.clone();
    let mut writes = ParallelWrites::new(&mut model, |index| (index < 3).then_some(index as u8));
    let mut polls = 0;
    loop {
        let step = writes.poll().expect("matching completion");
        clock.set(clock.get().map(|busy| busy.saturating_sub(1)));
        polls += 1;
        if step == Step::Ready(()) {
            break;
        }
        assert!(polls < 100);
    }
    assert_eq!(
        model.operations,
        [
            ParallelOperation::Select,
            ParallelOperation::Pair(0),
            ParallelOperation::Pair(1),
            ParallelOperation::Pair(2),
            ParallelOperation::Restore,
        ]
    );
}

#[test]
fn an_empty_parallel_sequence_still_selects_and_restores_the_map() {
    let mut model = ParallelModel {
        operations: Vec::new(),
        busy: Rc::new(Cell::new([0; 2])),
        latency: [0; 2],
    };
    let mut writes = ParallelWrites::new(&mut model, |_| None);
    while writes.poll().expect("matching completion") == Step::Pending {}
    assert_eq!(
        model.operations,
        [ParallelOperation::Select, ParallelOperation::Restore]
    );
}

#[test]
fn a_configuration_runs_its_writes_and_field_writes_in_order() {
    let mut model = Model::new(2);
    model.registers[1] = 0xf0;
    let commands = |index: usize| match index {
        0 => Some(ConfigurationCommand::Write(0_u8, 0x7f)),
        1 => Some(ConfigurationCommand::Modify(
            AnalogField::new(1_u8, 3, 0).expect("field"),
            0x9,
        )),
        _ => None,
    };
    let clock = model.busy.clone();
    let mut configuration = Configuration::new(&mut model, commands);
    run(&clock, || configuration.poll().expect("valid commands"));
    assert_eq!(
        model.operations,
        [
            Operation::StartWrite(0, 0x7f),
            Operation::StartRead(1),
            Operation::StartWrite(1, 0xf9),
        ]
    );
}

#[test]
fn a_configuration_rejects_a_value_wider_than_its_field_before_the_bus() {
    let mut model = Model::new(0);
    let commands = |index: usize| {
        (index == 0)
            .then(|| ConfigurationCommand::Modify(AnalogField::new(0_u8, 0, 0).expect("field"), 2))
    };
    let mut configuration = Configuration::new(&mut model, commands);
    assert_eq!(
        configuration.poll(),
        Err(ConfigurationError::InvalidCommand(InvalidCommand {
            index: 0
        }))
    );
    assert!(model.operations.is_empty());
}

#[test]
fn a_field_write_transition_asks_for_one_read_and_one_write() {
    let field = AnalogField::new(2_u8, 5, 3).unwrap();
    let mut write = FieldWriteTransition::new(field, 0b101).unwrap();
    assert_eq!(write.action(), Action::Read { address: 2 });
    assert_eq!(
        write.advance(Completion::Written { address: 2 }),
        Err(TransitionError::WrongCompletion)
    );
    write
        .advance(Completion::Read {
            address: 2,
            value: 0xff,
        })
        .unwrap();
    assert_eq!(
        write.action(),
        Action::Write {
            address: 2,
            value: 0b1110_1111
        }
    );
    write.advance(Completion::Written { address: 2 }).unwrap();
    assert_eq!(write.action(), Action::Complete(()));
    assert_eq!(
        write.advance(Completion::Written { address: 2 }),
        Err(TransitionError::AlreadyComplete)
    );
}

#[test]
fn a_field_write_transition_rejects_a_read_of_another_address() {
    let field = AnalogField::new(2_u8, 5, 3).unwrap();
    let mut write = FieldWriteTransition::new(field, 0b101).unwrap();
    assert_eq!(
        write.advance(Completion::Read {
            address: 3,
            value: 0xff,
        }),
        Err(TransitionError::WrongCompletion)
    );
    // Refused, the transition still asks for its own read.
    assert_eq!(write.action(), Action::Read { address: 2 });
}

#[test]
fn a_field_read_transition_rejects_another_address() {
    let mut read = FieldReadTransition::new(AnalogField::new(1_u8, 3, 0).unwrap());
    assert_eq!(
        read.advance(Completion::Read {
            address: 2,
            value: 9
        }),
        Err(TransitionError::WrongCompletion)
    );
    read.advance(Completion::Read {
        address: 1,
        value: 0x9c,
    })
    .unwrap();
    assert_eq!(read.action(), Action::Complete(0xc));
}

#[test]
fn an_invalid_configuration_command_completes_without_a_bus_action() {
    let field = AnalogField::new(0_u8, 1, 0).unwrap();
    let commands = |index| match index {
        0 => Some(ConfigurationCommand::Write(1, 7)),
        1 => Some(ConfigurationCommand::Modify(field, 4)),
        _ => None,
    };
    let mut configuration = ConfigurationTransition::new(commands);
    assert_eq!(
        configuration.action(),
        Action::Write {
            address: 1,
            value: 7
        }
    );
    configuration
        .advance(Completion::Written { address: 1 })
        .unwrap();
    assert_eq!(configuration.invalid(), Some(InvalidCommand { index: 1 }));
    assert_eq!(
        configuration.action(),
        Action::Complete(Err(InvalidCommand { index: 1 }))
    );
}

#[test]
fn a_refused_driver_completion_is_retained_without_repeating_bus_actions() {
    struct Refusing(TransitionError);
    impl ByteTransition for Refusing {
        type Address = u8;
        type Output = ();
        fn action(&self) -> Action<u8, ()> {
            Action::Read { address: 0 }
        }
        fn advance(&mut self, _: Completion<u8>) -> Result<(), TransitionError> {
            Err(self.0)
        }
    }
    for error in [
        TransitionError::WrongCompletion,
        TransitionError::AlreadyComplete,
    ] {
        let mut bus = Model::new(0);
        let mut driver = Driver::new(&mut bus, Refusing(error));
        assert_eq!(driver.poll(), Ok(Step::Pending));
        assert_eq!(driver.poll(), Err(DriverError::Transition(error)));
        assert_eq!(driver.poll(), Err(DriverError::Transition(error)));
        assert_eq!(driver.poll(), Err(DriverError::Transition(error)));
        assert_eq!(bus.operations, [Operation::StartRead(0)]);
    }
}

#[test]
fn configuration_index_exhaustion_returns_the_exact_index_without_wrapping() {
    let field = AnalogField::new(0_u8, 3, 0).unwrap();
    for command in [
        ConfigurationCommand::Read(0),
        ConfigurationCommand::Write(0, 1),
        ConfigurationCommand::Modify(field, 1),
    ] {
        let commands = |_: usize| Some(command);
        let mut transition = ConfigurationTransition::new(commands);
        transition.step = transition.enter(usize::MAX);
        let completion = match command {
            ConfigurationCommand::Read(_) => Completion::Read {
                address: 0,
                value: 0,
            },
            ConfigurationCommand::Write(..) => Completion::Written { address: 0 },
            ConfigurationCommand::Modify(..) => {
                transition
                    .advance(Completion::Read {
                        address: 0,
                        value: 0xf0,
                    })
                    .unwrap();
                assert_eq!(
                    transition.action(),
                    Action::Write {
                        address: 0,
                        value: 0xf1
                    }
                );
                Completion::Written { address: 0 }
            }
        };
        let before = transition.action();
        assert_eq!(
            transition.advance(completion),
            Err(TransitionError::IndexExhausted { index: usize::MAX })
        );
        assert_eq!(transition.action(), before);
        let mut bus = Model::new(0);
        let mut driver = Driver::new(&mut bus, transition);
        assert_eq!(driver.poll(), Ok(Step::Pending));
        assert_eq!(
            driver.poll(),
            Err(DriverError::Transition(TransitionError::IndexExhausted {
                index: usize::MAX
            }))
        );
        assert_eq!(
            driver.poll(),
            Err(DriverError::Transition(TransitionError::IndexExhausted {
                index: usize::MAX
            }))
        );
        assert_eq!(bus.operations.len(), 1);
    }
}

#[test]
fn parallel_index_exhaustion_is_latched_without_restarting_a_pair_or_restoring() {
    let pairs = |_: usize| Some(7_u8);
    let mut transition = ParallelTransition::new(pairs);
    transition.phase = ParallelPhase::Await(usize::MAX, Host::Host1);
    let error = TransitionError::IndexExhausted { index: usize::MAX };
    assert_eq!(transition.advance(), Err(error));
    assert_eq!(transition.pair_index(), Some(usize::MAX));
    let mut bus = ParallelModel {
        operations: Vec::new(),
        busy: Rc::new(Cell::new([0; 2])),
        latency: [0; 2],
    };
    let mut writes = ParallelWrites {
        bus: &mut bus,
        transition,
        error: None,
    };
    assert_eq!(writes.poll(), Err(error));
    assert_eq!(writes.poll(), Err(error));
    assert!(bus.operations.is_empty());
}

/// A bus that serves no address, counting what it was asked.
#[derive(Default)]
struct NoRoute {
    asked: Cell<u8>,
}

impl NoRoute {
    fn refuse(&self) -> BusRefusal {
        self.asked.set(self.asked.get() + 1);
        BusRefusal::Unrouted
    }
}

impl AnalogRegisterBus for NoRoute {
    type Address = u8;

    fn try_start_read(&mut self, _address: u8) -> Result<(), BusRefusal> {
        Err(self.refuse())
    }

    fn try_finish_read(&self, _address: u8) -> Result<u8, BusRefusal> {
        Err(self.refuse())
    }

    fn try_start_write(&mut self, _address: u8, _value: u8) -> Result<(), BusRefusal> {
        Err(self.refuse())
    }

    fn try_finish_write(&self, _address: u8) -> Result<(), BusRefusal> {
        Err(self.refuse())
    }
}

#[test]
fn an_unrouted_address_ends_the_transaction_and_stays_refused() {
    let mut bus = NoRoute::default();
    let mut read = FieldRead::new(&mut bus, AnalogField::new(0_u8, 7, 0).expect("field"));
    assert_eq!(read.poll(), Err(DriverError::Unrouted));
    assert_eq!(read.poll(), Err(DriverError::Unrouted));
    drop(read);
    assert_eq!(bus.asked.get(), 1, "no later poll reaches the bus");
}

#[test]
fn a_configuration_field_write_refuses_the_read_of_another_byte() {
    let commands = |index: usize| {
        (index == 0).then(|| {
            ConfigurationCommand::Modify(AnalogField::new(1_u8, 3, 0).expect("field"), 0x9)
        })
    };
    let mut transition = ConfigurationTransition::new(commands);
    assert_eq!(transition.action(), Action::Read { address: 1 });
    assert_eq!(
        transition.advance(Completion::Read {
            address: 2,
            value: 0xf0
        }),
        Err(TransitionError::WrongCompletion)
    );
    assert_eq!(transition.action(), Action::Read { address: 1 });
}

#[test]
fn a_parallel_pair_waits_for_host_0_then_host_1_before_the_next_pair() {
    let mut transition = ParallelTransition::new(|index: usize| (index < 2).then_some(index));
    let mut actions = Vec::new();
    loop {
        let action = transition.action();
        actions.push(action);
        if action == ParallelAction::Complete {
            break;
        }
        transition.advance().expect("a successor index");
    }
    assert_eq!(
        actions,
        [
            ParallelAction::SelectParallelMap,
            ParallelAction::StartPair(0),
            ParallelAction::AwaitIdle(Host::Host0),
            ParallelAction::AwaitIdle(Host::Host1),
            ParallelAction::StartPair(1),
            ParallelAction::AwaitIdle(Host::Host0),
            ParallelAction::AwaitIdle(Host::Host1),
            ParallelAction::RestoreMap,
            ParallelAction::Complete,
        ]
    );
}
