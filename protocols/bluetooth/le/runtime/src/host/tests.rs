#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use super::*;
use core::{
    future::{Future, poll_fn},
    num::NonZeroU64,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_hci::{
    self as hci,
    bt_hci::cmd::{OpcodeGroup, controller_baseband::Reset as HciReset},
};
use oer_bluetooth_le_controller::dtm::EntropyUnavailable;
use oer_bluetooth_le_model::{Backend, Environment, ModelCaps, Resources};
use oer_radio_port::{conformance::registration::guarded, *};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    vec::Vec,
};

const STOP: Opcode = Opcode::new(OpcodeGroup::new(63), 0);
const RX: Opcode = hci::LE_RECEIVER_TEST_V1_OPCODE;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Step {
    Read(Opcode),
    Framing,
    Latch,
    Entropy,
    Consume(Opcode),
    Sending(Vec<u8>),
    Sent(Vec<u8>),
}

enum Input {
    Framing,
    Command(Opcode, &'static [u8]),
}
struct Random;
impl Entropy for Random {
    fn random(&mut self) -> Result<[u8; 8], EntropyUnavailable> {
        Ok([0x92; 8])
    }
}
struct Io {
    input: VecDeque<Input>,
    command: Option<(Opcode, &'static [u8])>,
    entropy: Random,
    trace: Rc<RefCell<Vec<Step>>>,
    blocked: Rc<Cell<bool>>,
    fail_send: bool,
}
impl Transport for Io {
    type Error = ();
    type Entropy = Random;
    async fn receive(&mut self) -> Result<Received, ()> {
        assert!(self.command.is_none(), "input is consumed once");
        match self
            .input
            .pop_front()
            .expect("finite script ends in Stop or Reset")
        {
            Input::Framing => {
                self.trace.borrow_mut().push(Step::Framing);
                Ok(Received::Framing)
            }
            Input::Command(opcode, parameters) => {
                self.trace.borrow_mut().push(Step::Read(opcode));
                self.command = Some((opcode, parameters));
                Ok(Received::Command)
            }
        }
    }
    fn command(&mut self) -> Result<(HciCommandPacket<'_>, &mut Random), ()> {
        let (opcode, parameters) = self.command.ok_or(())?;
        Ok((HciCommandPacket::new(opcode, parameters), &mut self.entropy))
    }
    fn consume(&mut self) -> Result<(), ()> {
        let (opcode, _) = self.command.take().ok_or(())?;
        self.trace.borrow_mut().push(Step::Consume(opcode));
        Ok(())
    }
    async fn entropy(&mut self) -> Result<(), ()> {
        self.trace.borrow_mut().push(Step::Entropy);
        Ok(())
    }
    async fn send(&mut self, response: &impl HciControllerResponse) -> Result<(), ()> {
        assert_eq!(response.kind(), PacketKind::Event);
        let bytes = response.as_bytes().to_vec();
        self.trace.borrow_mut().push(Step::Sending(bytes.clone()));
        if bytes.get(3..5) == Some(RX.to_raw().to_le_bytes().as_slice()) {
            if self.fail_send {
                return Err(());
            }
            poll_fn(|_| {
                if self.blocked.get() {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
        }
        // The exact immutable response survives output backpressure.
        assert_eq!(response.as_bytes(), bytes);
        self.trace.borrow_mut().push(Step::Sent(bytes));
        Ok(())
    }
}
struct Installed<'o, 'a, 'p: 'a, Id: SlotIdentity> {
    owner: &'o mut Owner<'a, 'p, Id, Backend<Id>, Enabled>,
    trace: Rc<RefCell<Vec<Step>>>,
}
impl<'p, Id: SlotIdentity> ControllerPort<'p, Root<Id>> for Installed<'_, '_, 'p, Id> {
    type Caps = ModelCaps;
    type Fault = oer_bluetooth_le_model::Fault;
    fn phase(
        &mut self,
    ) -> impl core::ops::DerefMut<
        Target: Sized + LePhase<'p, Root<Id>, (), Caps = ModelCaps, Fault = Self::Fault>,
    > {
        self.owner.phase()
    }
    async fn latch(&mut self) -> PortResult<(), ClockError, Self::Fault> {
        self.trace.borrow_mut().push(Step::Latch);
        self.owner.now().await.map(|result| result.map(|_| ()))
    }
}
#[derive(Clone, Copy)]
enum Case {
    Stop,
    InitialResponse,
    Reset,
    SendFailure,
}
struct Body {
    case: Case,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Body {
    type Output = Vec<Step>;
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id>, Disabled>,
        resources: Resources<'p, Id>,
    ) -> Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        let Resources {
            seats: [seat, ..],
            session,
        } = resources;
        let mut service = Service::<_, ModelCaps>::new(session, seat, None);
        let trace = Rc::new(RefCell::new(Vec::new()));
        let blocked = Rc::new(Cell::new(true));
        let mut input = VecDeque::from([
            Input::Framing,
            Input::Command(LeRand::OPCODE, &[]),
            Input::Command(LeRand::OPCODE, &[1]), // malformed Rand must not request entropy
            Input::Command(RX, &[19]),
        ]);
        match self.case {
            Case::Stop | Case::InitialResponse => input.extend([
                Input::Command(hci::LE_TEST_END_OPCODE, &[]),
                Input::Command(STOP, &[1]), // only an empty Stop leaves the loop
                Input::Command(STOP, &[]),
            ]),
            Case::Reset => input.push_back(Input::Command(HciReset::OPCODE, &[])),
            Case::SendFailure => {}
        }
        let mut io = Io {
            input,
            command: None,
            entropy: Random,
            trace: trace.clone(),
            blocked: blocked.clone(),
            fail_send: matches!(self.case, Case::SendFailure),
        };
        let initial = if matches!(self.case, Case::InitialResponse) {
            Some(Response::Command(
                hci::LeDtmCommandCompleteEvent::without_return_parameters(
                    HciReset::OPCODE,
                    hci::bt_hci::param::Status::SUCCESS,
                ),
            ))
        } else {
            None
        };
        let mut port = Installed {
            owner: &mut owner,
            trace: trace.clone(),
        };
        let outcome = if matches!(self.case, Case::SendFailure) {
            serve(&mut port, &mut service, &mut io, initial, STOP).await
        } else {
            let mut serving = pin!(serve(&mut port, &mut service, &mut io, initial, STOP));
            poll_fn(|cx| {
                assert!(serving.as_mut().poll(cx).is_pending());
                let trace = trace.borrow();
                if matches!(trace.last(), Some(Step::Sending(bytes)) if bytes.get(3..5) == Some(RX.to_raw().to_le_bytes().as_slice())) {
                    // No next command is read and no credit is emitted while blocked.
                    assert_eq!(trace.iter().filter(|step| **step == Step::Read(RX)).count(), 1);
                    Poll::Ready(())
                } else { cx.waker().wake_by_ref(); Poll::Pending }
            }).await;
            blocked.set(false);
            serving.await
        };
        match (self.case, outcome) {
            (Case::Stop | Case::InitialResponse, Ok(Disposition::Stop)) => {
                assert!(service.response().is_none())
            }
            (Case::Reset, Ok(Disposition::Reset(_))) => {
                assert!(
                    service.response().is_none(),
                    "no Reset acknowledgement before reinstall"
                );
                assert!(
                    service
                        .accept(HciCommandPacket::new(LeRand::OPCODE, &[]), &mut Random)
                        .is_err()
                );
            }
            (Case::SendFailure, Err(Error::Transport(()))) => {
                let response = service
                    .response()
                    .expect("undelivered packet keeps its credit");
                let bytes = response.as_bytes().to_vec();
                assert_eq!(bytes, completion(RX, 0, &[]));
                assert!(
                    service
                        .accept(HciCommandPacket::new(LeRand::OPCODE, &[]), &mut Random)
                        .is_err()
                );
                assert_eq!(service.response().unwrap().as_bytes(), bytes);
            }
            _ => panic!("unexpected service disposition"),
        }
        assert!(io.input.is_empty());
        let output = trace.borrow().clone();
        let owner = owner
            .disable(&mut |_| ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        Ended::uninstall(owner, (), output)
    }
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..10_000 {
        if let Poll::Ready(output) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            return output;
        }
    }
    panic!("finite model poll budget");
}
fn scenario(case: Case) -> Vec<Step> {
    run(with_local_slot(async |claim| {
        let env = Rc::new(Environment::new(NonZeroU64::new(1000).unwrap()));
        let mut slot = claim.place();
        let end = LocalExecution
            .scope(
                guarded(slot.register(Backend::new(env.clone())).ok().unwrap()),
                async |front| run_port(front, Body { case }).await,
            )
            .await;
        let PortEnd::Uninstalled { output, .. } = end else {
            panic!("normal model shutdown")
        };
        assert_eq!(env.active_radios(), 0);
        slot.end_model_world();
        output
    }))
}
fn completion(opcode: Opcode, status: u8, parameters: &[u8]) -> Vec<u8> {
    let [lo, hi] = opcode.to_raw().to_le_bytes();
    let mut bytes = std::vec![
        0x0e,
        u8::try_from(parameters.len().saturating_add(4)).unwrap(),
        1,
        lo,
        hi,
        status
    ];
    bytes.extend_from_slice(parameters);
    bytes
}
fn delivered(trace: &mut Vec<Step>, bytes: Vec<u8>) {
    trace.push(Step::Sending(bytes.clone()));
    trace.push(Step::Sent(bytes));
}
fn prefix() -> Vec<Step> {
    let mut expected = std::vec![Step::Framing];
    delivered(&mut expected, std::vec![0x10, 1, 0xff]);
    expected.extend([
        Step::Read(LeRand::OPCODE),
        Step::Latch,
        Step::Entropy,
        Step::Consume(LeRand::OPCODE),
    ]);
    delivered(&mut expected, completion(LeRand::OPCODE, 0, &[0x92; 8]));
    expected.extend([Step::Read(LeRand::OPCODE), Step::Consume(LeRand::OPCODE)]);
    delivered(&mut expected, completion(LeRand::OPCODE, 0x12, &[0; 8]));
    expected.extend([Step::Read(RX), Step::Consume(RX), Step::Latch]);
    expected
}
#[test]
fn framing_rand_dtm_end_and_stop_keep_transport_order_across_backpressure() {
    let mut expected = prefix();
    delivered(&mut expected, completion(RX, 0, &[]));
    expected.extend([
        Step::Read(hci::LE_TEST_END_OPCODE),
        Step::Consume(hci::LE_TEST_END_OPCODE),
        Step::Latch,
    ]);
    delivered(
        &mut expected,
        completion(hci::LE_TEST_END_OPCODE, 0, &[0, 0]),
    );
    expected.extend([Step::Read(STOP), Step::Consume(STOP)]);
    delivered(&mut expected, completion(STOP, 1, &[]));
    expected.extend([Step::Read(STOP), Step::Consume(STOP)]);
    assert_eq!(scenario(Case::Stop), expected);
}
#[test]
fn active_reset_leaves_the_loop_without_acknowledgement() {
    let mut expected = prefix();
    delivered(&mut expected, completion(RX, 0, &[]));
    expected.extend([
        Step::Read(HciReset::OPCODE),
        Step::Consume(HciReset::OPCODE),
    ]);
    assert_eq!(scenario(Case::Reset), expected);
}
#[test]
fn failed_output_retains_the_immutable_response_and_command_credit() {
    let mut expected = prefix();
    expected.push(Step::Sending(completion(RX, 0, &[])));
    assert_eq!(scenario(Case::SendFailure), expected);
}

#[test]
fn reinstall_response_is_delivered_before_reading_the_next_command() {
    let mut expected = Vec::new();
    delivered(&mut expected, completion(HciReset::OPCODE, 0, &[]));
    expected.extend(scenario(Case::Stop));
    assert_eq!(scenario(Case::InitialResponse), expected);
}
