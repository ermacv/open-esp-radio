extern crate std;

use std::vec::Vec;

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
    busy: u8,
    operations: Vec<Operation>,
}

impl Model {
    fn new(latency: u8) -> Self {
        Self {
            registers: [0; 4],
            latency,
            busy: 0,
            operations: Vec::new(),
        }
    }

    fn tick(&mut self) {
        self.busy = self.busy.saturating_sub(1);
    }
}

impl AnalogRegisterBus for Model {
    type Address = u8;

    fn try_start_read(&mut self, address: u8) -> Result<(), Busy> {
        if self.busy != 0 {
            return Err(Busy);
        }
        self.busy = self.latency;
        self.operations.push(Operation::StartRead(address));
        Ok(())
    }

    fn try_finish_read(&self, address: u8) -> Result<u8, Busy> {
        if self.busy != 0 {
            Err(Busy)
        } else {
            Ok(self.registers[usize::from(address)])
        }
    }

    fn try_start_write(&mut self, address: u8, value: u8) -> Result<(), Busy> {
        if self.busy != 0 {
            return Err(Busy);
        }
        self.busy = self.latency;
        self.registers[usize::from(address)] = value;
        self.operations.push(Operation::StartWrite(address, value));
        Ok(())
    }

    fn try_finish_write(&self, _address: u8) -> Result<(), Busy> {
        if self.busy != 0 { Err(Busy) } else { Ok(()) }
    }
}

fn run<T>(model: &mut Model, mut poll: impl FnMut(&mut Model) -> Step<T>) -> T {
    for _ in 0..64 {
        if let Step::Ready(value) = poll(model) {
            return value;
        }
        model.tick();
    }
    panic!("the transaction did not complete");
}

#[test]
fn fields_are_bounded_to_one_byte() {
    assert!(AnalogField::new(0_u8, 7, 0).is_some());
    assert!(AnalogField::new(0_u8, 3, 4).is_none());
    assert!(AnalogField::new(0_u8, 8, 0).is_none());
}

#[test]
fn extraction_and_insertion_follow_the_vendor_leaves() {
    let field = AnalogField::new(0_u8, 5, 2).expect("field");
    assert_eq!(field.extract(0b1011_0110), 0b1101);
    assert_eq!(field.insert(0b1111_1111, 0b0010), Some(0b1100_1011));
    // A value wider than the field would leak above it in the vendor leaf.
    assert_eq!(field.insert(0, 0b1_0000), None);
    let whole = AnalogField::new(0_u8, 7, 0).expect("field");
    assert_eq!(whole.insert(0x12, 0xab), Some(0xab));
}

#[test]
fn a_field_read_extracts_after_the_completed_read() {
    let mut model = Model::new(3);
    model.registers[2] = 0b0110_0000;
    let mut read = FieldRead::new(AnalogField::new(2_u8, 6, 5).expect("field"));
    assert_eq!(run(&mut model, |bus| read.poll(bus)), 0b11);
    assert_eq!(model.operations, [Operation::StartRead(2)]);
}

#[test]
fn a_field_write_reads_modifies_and_writes_once() {
    let mut model = Model::new(2);
    model.registers[1] = 0b1010_1010;
    let mut write =
        FieldWrite::new(AnalogField::new(1_u8, 3, 0).expect("field"), 0b0101).expect("fits");
    run(&mut model, |bus| write.poll(bus));
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
    model.busy = 5;
    let mut write = FieldWrite::new(AnalogField::new(0_u8, 0, 0).expect("field"), 1).expect("fits");
    run(&mut model, |bus| write.poll(bus));
    assert_eq!(
        model.operations,
        [Operation::StartRead(0), Operation::StartWrite(0, 1)]
    );
}

#[test]
fn a_value_wider_than_its_field_is_rejected() {
    let field = AnalogField::new(0_u8, 1, 0).expect("field");
    assert_eq!(FieldWrite::new(field, 4).err(), Some(ValueTooWide));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParallelOperation {
    Select,
    Restore,
    Pair(u8),
}

struct ParallelModel {
    operations: Vec<ParallelOperation>,
    busy: [u8; 2],
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
        assert_eq!(self.busy, [0; 2], "a pair started while a host was busy");
        self.busy = self.latency;
        self.operations.push(ParallelOperation::Pair(pair));
    }

    fn is_busy(&self, host: ParallelHost) -> bool {
        self.busy[host as usize] != 0
    }
}

#[test]
fn parallel_pairs_start_after_both_hosts_are_idle_between_the_host_maps() {
    let mut model = ParallelModel {
        operations: Vec::new(),
        busy: [0; 2],
        latency: [2, 3],
    };
    let mut writes = ParallelWrites::new(|index| (index < 3).then_some(index as u8));
    let mut polls = 0;
    loop {
        let step = writes.poll(&mut model);
        model.busy = model.busy.map(|busy| busy.saturating_sub(1));
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
        busy: [0; 2],
        latency: [0; 2],
    };
    let mut writes = ParallelWrites::new(|_| None);
    while writes.poll(&mut model) == Step::Pending {}
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
    let mut configuration = Configuration::new(commands);
    run(&mut model, |bus| {
        configuration.poll(bus).expect("valid commands")
    });
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
    let mut configuration = Configuration::new(commands);
    assert_eq!(
        configuration.poll(&mut model),
        Err(InvalidCommand { index: 0 })
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
