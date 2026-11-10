//! The staged preparation refused at each of its steps, through the path
//! the chip's install takes.

extern crate std;

use super::*;
use core::cell::RefCell;
use std::vec;
use std::vec::Vec;

/// The refusal of the test's steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Refusal;

/// An owner of the test: its name, and the log it ends in.
#[derive(Debug)]
struct Owner<'a>(u8, &'a RefCell<Vec<u8>>);

impl Drop for Owner<'_> {
    fn drop(&mut self) {
        self.1.borrow_mut().push(self.0);
    }
}

/// Steps of the test: each makes its part from its input's name, the
/// input consumed; the step numbered `refuse` refuses.
struct TestSteps<'a> {
    refuse: u8,
    taken: u8,
    ended: &'a RefCell<Vec<u8>>,
}

impl<'a> TestSteps<'a> {
    fn run(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        let at = self.taken;
        self.taken += 1;
        let name = input.0;
        drop(input);
        if at == self.refuse {
            return Err(Refusal);
        }
        Ok(Owner(name + 100, self.ended))
    }
}

impl<'a> Steps for TestSteps<'a> {
    type Error = Refusal;
    type RxIn = Owner<'a>;
    type Rx = Owner<'a>;
    type DeviceIn = Owner<'a>;
    type Device = Owner<'a>;
    type SlotIn = Owner<'a>;
    type Slot = Owner<'a>;
    type ChainIn = Owner<'a>;
    type Chain = Owner<'a>;
    type LentIn = Owner<'a>;
    type Lent = Owner<'a>;
    type SharedIn = Owner<'a>;
    type Shared = Owner<'a>;
    fn bind(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
    fn device(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
    fn slot(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
    fn chain(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
    fn lent(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
    fn shared(&mut self, input: Owner<'a>) -> Result<Owner<'a>, Refusal> {
        self.run(input)
    }
}

/// The steps in order, each named by its input: bind 0, device 1, slots
/// 2..=6, chains 7..=8, lent 9, shared 10.
const STEPS: u8 = 11;

fn inputs(ended: &RefCell<Vec<u8>>) -> Inputs<TestSteps<'_>> {
    let owner = |name| Owner(name, ended);
    Inputs {
        rx: owner(0),
        device: owner(1),
        slots: [owner(2), owner(3), owner(4), owner(5), owner(6)],
        chains: [owner(7), owner(8)],
        lent: owner(9),
        shared: owner(10),
    }
}

/// Each place of a stage, in step order: the name of what it holds.
fn names(
    stage: &Stage<Owner<'_>, Owner<'_>, Owner<'_>, Owner<'_>, Owner<'_>, Owner<'_>>,
) -> Vec<Option<u8>> {
    let name = |owner: &Option<Owner<'_>>| owner.as_ref().map(|owner| owner.0);
    let mut all = vec![name(&stage.rx), name(&stage.device)];
    all.extend(stage.slots.iter().map(name));
    all.extend(stage.chains.iter().map(name));
    all.push(name(&stage.lent));
    all.push(name(&stage.shared));
    all
}

const fn step_of(at: u8) -> Step {
    match at {
        0 => Step::Bind,
        1 => Step::Device,
        2..=6 => Step::Slot((at - 2) as usize),
        7..=8 => Step::Chain((at - 7) as usize),
        9 => Step::Lent,
        _ => Step::Shared,
    }
}

/// A refusal at any step returns, whole: the step it refused at, the part
/// of every step before it, and the input of every step after it. Only the
/// refused step's input ended, as its contract names; nothing else ended
/// on the way.
#[test]
fn a_refusal_at_each_step_returns_the_reached_parts_and_the_remaining_inputs() {
    for refuse in 0..STEPS {
        let ended = RefCell::new(Vec::new());
        let mut steps = TestSteps {
            refuse,
            taken: 0,
            ended: &ended,
        };
        let inputs = inputs(&ended);
        let Err(refused) = prepare(&mut steps, inputs) else {
            panic!("step {refuse} refuses");
        };
        assert_eq!(refused.cause, Refusal);
        assert_eq!(refused.step, step_of(refuse));
        assert_eq!(steps.taken, refuse + 1, "no step runs after the refusal");
        // Each step's input ended as it made its part; the refused one's
        // too, and nothing else.
        assert_eq!(*ended.borrow(), (0..=refuse).collect::<Vec<_>>());
        let expected_reached: Vec<_> = (0..STEPS)
            .map(|at| (at < refuse).then_some(at + 100))
            .collect();
        assert_eq!(names(&refused.reached), expected_reached, "at {refuse}");
        let expected_remaining: Vec<_> = (0..STEPS).map(|at| (at > refuse).then_some(at)).collect();
        assert_eq!(names(&refused.remaining), expected_remaining, "at {refuse}");
    }
}

/// A preparation no step refuses makes every part, in order.
#[test]
fn a_preparation_no_step_refuses_makes_every_part() {
    let ended = RefCell::new(Vec::new());
    let mut steps = TestSteps {
        refuse: STEPS,
        taken: 0,
        ended: &ended,
    };
    let inputs = inputs(&ended);
    let Ok(parts) = prepare(&mut steps, inputs) else {
        panic!("no step refuses");
    };
    assert_eq!(steps.taken, STEPS);
    let mut made = vec![parts.rx.0, parts.device.0];
    made.extend(parts.slots.iter().map(|part| part.0));
    made.extend(parts.chains.iter().map(|part| part.0));
    made.extend([parts.lent.0, parts.shared.0]);
    assert_eq!(made, (100..100 + STEPS).collect::<Vec<_>>());
}
