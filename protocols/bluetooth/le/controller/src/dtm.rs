//! HCI Reset/Rand/DTM 1M service over an installed LE port.
//!
//! The composition owns the transport, entropy and `run_port`. A Reset leaves
//! this body, disables and uninstalls its port, then completes against a fresh
//! installation's idle lease. It cannot acknowledge a software-only reset of
//! a running test. Command capacity is one; the immutable response stays here
//! until the transport confirms delivery. Admission and event waits may be
//! cancelled and resumed without losing the HCI command or session owner.

use core::marker::PhantomData;

use oer_bluetooth_hci::{
    self as hci, HciCommandPacket, HciControllerResponse, LeDtmCommand, LeDtmCommandCompleteEvent,
    LeDtmCommandKind, LeDtmPhy, LeRandCommandCompleteEvent, LeReceiverTestCommand,
    LeTestEndCommand, LeTransmitterTestCommand,
    bt_hci::{
        PacketKind,
        cmd::{Cmd, Opcode, controller_baseband::Reset as HciReset, le::LeRand},
        param::Error as HciError,
    },
};
use oer_bluetooth_le_port::{
    self as port, Admitted, Command, Completion, DtmCommand, Event, EventResult, Idle,
    LeCapabilities, LeOperationId, LePhase, Receive, Request, ReturnedSession, Running, Session,
    Supported, TestPhy, Transmit, TxPower,
};
use oer_bluetooth_le_wire::{TestChannel, TestPayloadType};
use oer_radio_port::{Child, CorrelationIds, PoolPath, PortContext, Seat};

/// Entropy supplied by the composition, never synthesized from a timer.
pub trait Entropy {
    /// Fill one complete LE Rand return value, or report unavailable entropy.
    fn random(&mut self) -> Result<[u8; 8], EntropyUnavailable>;
}

/// Entropy was unavailable. No partial material is returned to the Host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntropyUnavailable;

/// One immutable packet, retained until the transport accepts it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Response {
    /// DTM, Reset or an error completion, with one command credit.
    Command(LeDtmCommandCompleteEvent),
    /// LE Rand; its Debug implementation redacts the random octets.
    Random(LeRandCommandCompleteEvent),
}
impl HciControllerResponse for Response {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Command(response) => response.as_bytes(),
            Self::Random(response) => response.as_bytes(),
        }
    }
}

/// A new command cannot replace a pending command or an undelivered response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Busy;

/// What the composition must do after accepting one command.
#[must_use]
pub enum Accepted<P: PoolPath> {
    /// An immediate response is retained by the service.
    Response,
    /// Call `submit`; only `Submitted::AwaitTerminal` needs `complete`.
    Port,
    /// Leave the body through normal disable/uninstall, then reinstall.
    Reset(Reset<P>),
}

/// Outstanding Reset, independent of the old body's loan.
#[must_use = "complete Reset only after a fresh joined installation"]
pub struct Reset<P: PoolPath> {
    epoch: u64,
    path: PhantomData<fn(P) -> P>,
}
impl<P: PoolPath> Reset<P> {
    /// A fresh idle lease of the same slot/path is required to acknowledge Reset.
    /// A late completion or the old body's lease cannot satisfy it.
    pub fn after_install(self, session: &Session<'_, P, Idle>) -> Result<Response, Self> {
        if session.epoch() == self.epoch {
            return Err(self);
        }
        Ok(status(RESET, None))
    }
}

const RESET: Opcode = HciReset::OPCODE;
const RAND: Opcode = LeRand::OPCODE;

enum Lease<'p, P: PoolPath> {
    Idle(Session<'p, P, Idle>),
    Running(Session<'p, P, Running>),
}
struct Pending<C: LeCapabilities> {
    command: LeDtmCommand,
    config: Option<Config<C>>,
    admitted: Option<Admitted>,
}
enum Config<C: LeCapabilities> {
    Receive(Supported<C, Receive>),
    Transmit(Supported<C, Transmit>),
    End,
}

/// Faults in terminal correlation or resource accounting stop this service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// An earlier accounting or reachability fault permanently stopped this body.
    Poisoned,
    /// No HCI command is waiting for a port terminal.
    NoCommand,
    /// A terminal names another operation or installation.
    Correlation,
    /// Terminal execution and returned lease/count disagree.
    Completion,
    /// The port lost events or ended before the mandatory terminal.
    Stream,
}

/// Whether admission produced a terminal obligation or an immediate HCI status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum Submitted {
    AwaitTerminal,
    Response,
}

/// The session observed when a pending command could not be submitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionSession {
    Missing,
    Idle,
    Running,
}

/// The configuration retained for a pending submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionConfiguration {
    Receive,
    Transmit,
    End,
}

/// A submission failure retains the service's command and session observations.
#[derive(Debug)]
pub enum SubmitError<F> {
    /// An earlier fault stopped the service; no further request was submitted.
    Poisoned,
    /// The backend retained the request in quarantine.
    Backend(oer_radio_port::Poisoned<F>),
    /// The receipt did not name the submitted operation and installation.
    Receipt,
    /// The retained command, configuration and session cannot form a request.
    InvalidState {
        command: LeDtmCommandKind,
        configuration: Option<SubmissionConfiguration>,
        session: SubmissionSession,
    },
}

/// Durable state of a one-credit HCI endpoint for a single DTM session.
pub struct Service<'p, P: PoolPath, C: LeCapabilities> {
    lease: Option<Lease<'p, P>>,
    seat: Seat<'p, port::CommandCapacity, Child<P, 0>>,
    power: Option<TxPower>,
    epoch: u64,
    slot: u8,
    transmitting: bool,
    ids: CorrelationIds,
    pending: Option<Pending<C>>,
    response: Option<Response>,
    retained: Option<port::Terminal<'p, P, ()>>,
    resetting: bool,
    faulted: bool,
}
impl<'p, P: PoolPath, C: LeCapabilities> Service<'p, P, C> {
    /// An RX-only endpoint needs no transmit-power value.
    pub fn new(
        session: Session<'p, P, Idle>,
        seat: Seat<'p, port::CommandCapacity, Child<P, 0>>,
        power: Option<TxPower>,
    ) -> Self {
        Self {
            epoch: session.epoch(),
            slot: session.index(),
            transmitting: false,
            lease: Some(Lease::Idle(session)),
            seat,
            power,
            ids: CorrelationIds::default(),
            pending: None,
            response: None,
            retained: None,
            resetting: false,
            faulted: false,
        }
    }

    /// Inspect the retained response across transport backpressure.
    pub fn response(&self) -> Option<&Response> {
        self.response.as_ref()
    }

    /// Release the single command credit after successful transport delivery.
    pub fn take_response(&mut self) -> Option<Response> {
        self.response.take()
    }

    /// Decode a complete HCI command and retain its owned semantic token.
    pub fn accept(
        &mut self,
        packet: HciCommandPacket<'_>,
        entropy: &mut impl Entropy,
    ) -> Result<Accepted<P>, Busy> {
        if self.pending.is_some() || self.response.is_some() || self.resetting {
            return Err(Busy);
        }
        let opcode = packet.opcode();
        if opcode == RESET {
            if !packet.parameters().is_empty() {
                return Ok(self.answer(status(opcode, Some(HciError::INVALID_HCI_PARAMETERS))));
            }
            self.resetting = true;
            return Ok(Accepted::Reset(Reset {
                epoch: self.epoch,
                path: PhantomData,
            }));
        }
        if self.faulted || self.lease.is_none() {
            return Ok(self.answer(status(opcode, Some(HciError::HARDWARE_FAILURE))));
        }
        if opcode == RAND {
            let response = if !packet.parameters().is_empty() {
                LeRandCommandCompleteEvent::error(HciError::INVALID_HCI_PARAMETERS)
            } else {
                match entropy.random() {
                    Ok(value) => LeRandCommandCompleteEvent::success(value),
                    Err(_) => LeRandCommandCompleteEvent::error(HciError::HARDWARE_FAILURE),
                }
            };
            return Ok(self.answer(Response::Random(response)));
        }
        let command = match LeDtmCommand::decode(packet) {
            Ok(command) => command,
            Err(error) => {
                let response = match error.into_command_complete() {
                    Ok(response) => Response::Command(response),
                    Err(_) => status(opcode, Some(HciError::UNKNOWN_CMD)),
                };
                return Ok(self.answer(response));
            }
        };
        let command = match (matches!(self.lease, Some(Lease::Idle(_))), command) {
            (true, LeDtmCommand::TestEnd(command)) => {
                return Ok(self.answer(Response::Command(command.into_ended_command_complete(0))));
            }
            (_, command) => command,
        };
        if !matches!(&command, LeDtmCommand::TestEnd(_))
            && matches!(self.lease, Some(Lease::Running(_)))
        {
            return Ok(self.answer(status(opcode, Some(HciError::CONTROLLER_BUSY))));
        }
        let config = match &command {
            LeDtmCommand::ReceiverTest(command) => {
                if command.phy() != LeDtmPhy::Le1M {
                    return Ok(self.answer(status(opcode, Some(HciError::UNSUPPORTED))));
                }
                let value = Receive {
                    channel: match channel(command.channel().index()) {
                        Ok(channel) => channel,
                        Err(error) => return Ok(self.answer(status(opcode, Some(error)))),
                    },
                    phy: TestPhy::Le1M,
                };
                match Supported::new(value) {
                    Ok(config) => Config::Receive(config),
                    Err(_) => return Ok(self.answer(status(opcode, Some(HciError::UNSUPPORTED)))),
                }
            }
            LeDtmCommand::TransmitterTest(command) => {
                if command.phy() != LeDtmPhy::Le1M {
                    return Ok(self.answer(status(opcode, Some(HciError::UNSUPPORTED))));
                }
                if command.opcode() == hci::LE_TRANSMITTER_TEST_V1_OPCODE
                    && command.payload_length() > 37
                {
                    return Ok(self.answer(status(opcode, Some(HciError::INVALID_HCI_PARAMETERS))));
                }
                let Some(power) = self.power else {
                    return Ok(self.answer(status(opcode, Some(HciError::CMD_DISALLOWED))));
                };
                let value = Transmit {
                    channel: match channel(command.channel().index()) {
                        Ok(channel) => channel,
                        Err(error) => return Ok(self.answer(status(opcode, Some(error)))),
                    },
                    phy: TestPhy::Le1M,
                    payload: match payload(command.payload_pattern().hci_parameter()) {
                        Ok(payload) => payload,
                        Err(error) => return Ok(self.answer(status(opcode, Some(error)))),
                    },
                    length: command.payload_length(),
                    power,
                };
                match Supported::new(value) {
                    Ok(config) => Config::Transmit(config),
                    Err(_) => return Ok(self.answer(status(opcode, Some(HciError::UNSUPPORTED)))),
                }
            }
            LeDtmCommand::TestEnd(_) => Config::End,
        };
        self.pending = Some(Pending {
            command,
            config: Some(config),
            admitted: None,
        });
        Ok(Accepted::Port)
    }

    fn answer(&mut self, response: Response) -> Accepted<P> {
        self.response = Some(response);
        Accepted::Response
    }

    /// Reserve capacity and synchronously submit. Cancellation before admission
    /// retains the original command and lease here; after admission this is a no-op.
    pub async fn submit<F>(
        &mut self,
        phase: &impl LePhase<'p, P, (), Caps = C, Fault = F>,
    ) -> Result<Submitted, SubmitError<F>> {
        if self.faulted {
            return Err(SubmitError::Poisoned);
        }
        let Some(pending) = self.pending.as_mut() else {
            return Ok(Submitted::Response);
        };
        if pending.admitted.is_some() {
            return Ok(Submitted::AwaitTerminal);
        }
        // Borrow the command through the wait: cancellation cannot consume it.
        let capacity = phase.take(&mut self.seat).await;
        let opcode = pending.command.kind().opcode();
        let command = match (self.lease.take(), pending.config.take()) {
            (Some(Lease::Idle(session)), Some(Config::Receive(config))) => {
                DtmCommand::Receive(session, config)
            }
            (Some(Lease::Idle(session)), Some(Config::Transmit(config))) => {
                DtmCommand::Transmit(session, config)
            }
            (Some(Lease::Running(session)), Some(Config::End)) => DtmCommand::End(session),
            (lease, config) => {
                let error = SubmitError::InvalidState {
                    command: pending.command.kind(),
                    configuration: config.as_ref().map(|config| match config {
                        Config::Receive(_) => SubmissionConfiguration::Receive,
                        Config::Transmit(_) => SubmissionConfiguration::Transmit,
                        Config::End => SubmissionConfiguration::End,
                    }),
                    session: match &lease {
                        None => SubmissionSession::Missing,
                        Some(Lease::Idle(_)) => SubmissionSession::Idle,
                        Some(Lease::Running(_)) => SubmissionSession::Running,
                    },
                };
                self.lease = lease;
                pending.config = config;
                self.faulted = true;
                phase.release(capacity);
                return Err(error);
            }
        };
        let expected = Admitted {
            id: self.ids.next::<LeOperationId>(),
            epoch: self.epoch,
        };
        let request = Request {
            id: expected.id,
            capacity,
            command: Command::DirectTest(command),
            context: PortContext::new(()),
        };
        match phase.submit(request) {
            Ok(Ok(admitted)) => {
                pending.admitted = Some(expected);
                if admitted != expected {
                    self.faulted = true;
                    return Err(SubmitError::Receipt);
                }
                Ok(Submitted::AwaitTerminal)
            }
            Ok(Err(refused)) => {
                phase.release(refused.request.capacity);
                let Command::DirectTest(command) = refused.request.command;
                self.lease = Some(match command {
                    DtmCommand::Receive(session, _) | DtmCommand::Transmit(session, _) => {
                        Lease::Idle(session)
                    }
                    DtmCommand::End(session) => Lease::Running(session),
                });
                self.pending = None;
                self.response = Some(status(opcode, Some(HciError::CMD_DISALLOWED)));
                Ok(Submitted::Response)
            }
            Err(fault) => {
                self.pending = None;
                self.faulted = true;
                self.response = Some(status(opcode, Some(HciError::HARDWARE_FAILURE)));
                Err(SubmitError::Backend(fault))
            }
        }
    }

    /// Await only the already admitted command's terminal. Dropping this wait
    /// preserves its token and the backend's ownership; a later call resumes it.
    pub async fn complete<F>(
        &mut self,
        phase: &mut impl LePhase<'p, P, (), Caps = C, Fault = F>,
    ) -> Result<(), Fault> {
        if self.faulted {
            return Err(Fault::Poisoned);
        }
        let receipt = self
            .pending
            .as_ref()
            .and_then(|p| p.admitted)
            .ok_or(Fault::NoCommand)?;
        let event = match phase.next_event().await {
            Ok(Ok(Event::Terminal(terminal))) => terminal,
            _ => {
                self.faulted = true;
                return Err(Fault::Stream);
            }
        };
        if event.admitted != receipt || receipt.epoch != self.epoch {
            self.retained = Some(event);
            self.faulted = true;
            return Err(Fault::Correlation);
        }
        let Some(mut pending) = self.pending.take() else {
            self.retained = Some(event);
            self.faulted = true;
            return Err(Fault::NoCommand);
        };
        let opcode = pending.command.kind().opcode();
        let ending = pending.command.kind() == LeDtmCommandKind::TestEnd;
        let Completion::DirectTest { session, count } = &event.completion;
        let executed = matches!(event.result, EventResult::Executed { .. });
        let identity_valid = match session {
            ReturnedSession::Idle(session) => {
                session.epoch() == self.epoch && session.index() == self.slot
            }
            ReturnedSession::Running(session) => {
                session.epoch() == self.epoch && session.index() == self.slot
            }
            ReturnedSession::Quarantined => true,
        };
        let valid = identity_valid
            && if executed {
                if ending {
                    matches!(session, ReturnedSession::Idle(_))
                        && count
                            .is_some_and(|c| !self.transmitting || (c.packets == 0 && !c.saturated))
                } else {
                    matches!(session, ReturnedSession::Running(_)) && count.is_none()
                }
            } else if event.result == EventResult::Indeterminate {
                matches!(session, ReturnedSession::Quarantined) && count.is_none()
            } else if event.result == EventResult::Aborted || !ending {
                count.is_none() && matches!(session, ReturnedSession::Idle(_))
            } else {
                count.is_none() && matches!(session, ReturnedSession::Running(_))
            };
        if !valid {
            self.retained = Some(event);
            self.faulted = true;
            return Err(Fault::Completion);
        }
        let completed = if executed {
            match (pending.command, count) {
                (LeDtmCommand::TestEnd(command), Some(count)) => {
                    Completed::End(command, count.packets)
                }
                (LeDtmCommand::ReceiverTest(command), None) => Completed::Receive(command),
                (LeDtmCommand::TransmitterTest(command), None) => Completed::Transmit(command),
                (command, _) => {
                    pending.command = command;
                    self.pending = Some(pending);
                    self.retained = Some(event);
                    self.faulted = true;
                    return Err(Fault::Completion);
                }
            }
        } else {
            Completed::Status(if event.result == EventResult::Indeterminate {
                HciError::HARDWARE_FAILURE
            } else {
                HciError::CMD_DISALLOWED
            })
        };
        let Completion::DirectTest { session, .. } = event.completion;
        self.lease = match session {
            ReturnedSession::Idle(session) => Some(Lease::Idle(session)),
            ReturnedSession::Running(session) => Some(Lease::Running(session)),
            ReturnedSession::Quarantined => {
                self.faulted = true;
                None
            }
        };
        // Each executed variant owns its command token and, for End, the
        // validated count. Formatting cannot require an optional value.
        self.response = Some(match completed {
            Completed::Receive(command) => {
                self.transmitting = false;
                Response::Command(command.into_started_command_complete())
            }
            Completed::Transmit(command) => {
                self.transmitting = true;
                Response::Command(command.into_started_command_complete())
            }
            Completed::End(command, packets) => {
                Response::Command(command.into_ended_command_complete(packets))
            }
            Completed::Status(error) => status(opcode, Some(error)),
        });
        self.pending = None;
        Ok(())
    }
}

enum Completed {
    Receive(LeReceiverTestCommand),
    Transmit(LeTransmitterTestCommand),
    End(LeTestEndCommand, u16),
    Status(HciError),
}

fn channel(value: u8) -> Result<TestChannel, HciError> {
    TestChannel::new(value).map_err(|_| HciError::INVALID_HCI_PARAMETERS)
}
fn payload(value: u8) -> Result<TestPayloadType, HciError> {
    TestPayloadType::new(value).map_err(|_| HciError::INVALID_HCI_PARAMETERS)
}
fn status(opcode: Opcode, error: Option<HciError>) -> Response {
    Response::Command(LeDtmCommandCompleteEvent::without_return_parameters(
        opcode,
        error.map_or(hci::bt_hci::param::Status::SUCCESS, HciError::to_status),
    ))
}

#[cfg(test)]
mod tests;
