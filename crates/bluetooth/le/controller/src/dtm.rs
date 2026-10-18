//! Direct Test Mode behind the LE test commands.
//!
//! A test runs alone: the Controller refuses it while advertising or
//! scanning is enabled. Test End first drains the event in progress, then
//! releases the backend's test role, then completes with the packet count.

use bt_hci::param::Error as HciError;
use oer_bluetooth_hci::{LeDtmCommand, LeDtmCommandCompleteEvent, LeDtmPhy, LeTestEndCommand};
use oer_bluetooth_ll::dtm::{
    DTM_MAX_PAYLOAD, DtmCalculation, DtmEpochExhausted, DtmEvent, DtmPayloadPattern, DtmSession,
    DtmStartError, DtmStop, DtmTest,
};
use oer_bluetooth_radio::{
    EventId, LeCapabilities, LeInstant, RadioOutcome, RadioRequest, RadioTiming, Supported,
    TestChannel, TestPhy, TxPower, Unplaced,
};

/// Event identities of the test session start here, apart from the roles.
const FIRST_EVENT_ID: u32 = 0x8000_0000;

#[derive(Debug)]
enum Phase {
    Idle,
    Running,
    /// Ending waits for the event in progress; Test End completes after it.
    Draining(Option<LeTestEndCommand>, EventId, bool),
    /// Ending releases the backend's test role.
    Releasing(Option<LeTestEndCommand>, u16),
}

#[derive(Debug)]
// CAPABILITY: bluetooth-le-receiver-test-transmitter-test
pub(crate) struct DtmRole {
    session: DtmSession,
    phase: Phase,
    /// The backend accepted a test event since the last release.
    holds_role: bool,
    completion: Option<LeDtmCommandCompleteEvent>,
}

/// The transmit power of every test event.
pub(crate) const DTM_TX_POWER: TxPower = TxPower::from_dbm(0);

/// What the DTM role asks of the radio.
pub(crate) enum DtmRadioWork<'s, K: LeCapabilities> {
    Request(RadioRequest<'s, K>),
    Cancel(oer_bluetooth_radio::EventId),
    None,
}

impl DtmRole {
    pub(crate) const fn new() -> Self {
        Self {
            session: DtmSession::new(DTM_TX_POWER, FIRST_EVENT_ID),
            phase: Phase::Idle,
            holds_role: false,
            completion: None,
        }
    }

    pub(crate) const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Handle one test command. Starting and ending an absent test complete at
    /// once; ending a running test completes later through
    /// [`Self::take_completion`].
    pub(crate) fn command<K: LeCapabilities>(
        &mut self,
        command: LeDtmCommand,
    ) -> Option<LeDtmCommandCompleteEvent> {
        match (command, &self.phase) {
            (LeDtmCommand::TestEnd(command), Phase::Idle) => {
                Some(command.into_ended_command_complete(0))
            }
            (LeDtmCommand::TestEnd(command), Phase::Running) => {
                self.end(Some(command));
                None
            }
            (command, Phase::Idle) => Some(self.start::<K>(command)),
            (command, _) => Some(busy(command)),
        }
    }

    /// End a running test without a Test End completion, as Reset does.
    pub(crate) fn abort(&mut self) {
        if let Phase::Running = self.phase {
            self.end(None);
        }
    }

    fn end(&mut self, command: Option<LeTestEndCommand>) {
        match self.session.end() {
            DtmStop::Stopped { received } => self.release(command, received),
            DtmStop::Draining(id) => self.phase = Phase::Draining(command, id, false),
        }
    }

    /// Release the backend's test role, or finish when none is held.
    fn release(&mut self, command: Option<LeTestEndCommand>, received: u16) {
        self.phase = Phase::Releasing(command, received);
        if !self.holds_role {
            self.finish();
        }
    }

    /// Whether the role has a request for the radio.
    pub(crate) fn wants_radio(&self) -> bool {
        match self.phase {
            Phase::Idle => false,
            Phase::Running => self.session.outstanding().is_none(),
            Phase::Draining(_, _, cancelled) => !cancelled,
            Phase::Releasing(..) => true,
        }
    }

    fn start<K: LeCapabilities>(&mut self, command: LeDtmCommand) -> LeDtmCommandCompleteEvent {
        let (test, opcode) = match command {
            LeDtmCommand::ReceiverTest(ref receive) => {
                let Ok(channel) = TestChannel::new(receive.channel().index()) else {
                    return rejected(command, HciError::INVALID_HCI_PARAMETERS);
                };
                let phy = match receive.phy() {
                    LeDtmPhy::Le1M => TestPhy::Le1M,
                    LeDtmPhy::Le2M => TestPhy::Le2M,
                    // A coded receiver accepts either coding.
                    LeDtmPhy::LeCoded | LeDtmPhy::LeCodedS2 => TestPhy::LeCodedS8,
                };
                (DtmTest::Receive { channel, phy }, receive.opcode())
            }
            LeDtmCommand::TransmitterTest(ref transmit) => {
                let Ok(channel) = TestChannel::new(transmit.channel().index()) else {
                    return rejected(command, HciError::INVALID_HCI_PARAMETERS);
                };
                let phy = match transmit.phy() {
                    LeDtmPhy::Le1M => TestPhy::Le1M,
                    LeDtmPhy::Le2M => TestPhy::Le2M,
                    LeDtmPhy::LeCoded => TestPhy::LeCodedS8,
                    LeDtmPhy::LeCodedS2 => TestPhy::LeCodedS2,
                };
                let pattern = DtmPayloadPattern::from_hci_selector(
                    transmit.payload_pattern().hci_parameter(),
                )
                .expect("the codec admits only the eight patterns");
                (
                    DtmTest::Transmit {
                        channel,
                        phy,
                        pattern,
                        length: transmit.payload_length(),
                    },
                    transmit.opcode(),
                )
            }
            LeDtmCommand::TestEnd(_) => unreachable!("Test End is handled by the caller"),
        };
        let phy = match test {
            DtmTest::Receive { phy, .. } | DtmTest::Transmit { phy, .. } => phy,
        };
        // The backend runs only the tests and PHYs `K` serves.
        if !K::CAPS.direct_test_mode || !K::CAPS.test_phys.contains(phy.phy()) {
            return rejected(command, HciError::UNSUPPORTED);
        }
        match self.session.start(test) {
            Ok(()) => {
                self.phase = Phase::Running;
                LeDtmCommandCompleteEvent::without_return_parameters(
                    opcode,
                    bt_hci::param::Status::SUCCESS,
                )
            }
            Err(DtmStartError::UnsupportedPhy) => rejected(command, HciError::UNSUPPORTED),
            Err(DtmStartError::Active) => busy(command),
        }
    }

    /// The next radio request of the test.
    pub(crate) fn next_request<'s, K: LeCapabilities>(
        &mut self,
        now: LeInstant,
        timing: RadioTiming,
        payload: &'s mut [u8; DTM_MAX_PAYLOAD],
    ) -> Result<DtmRadioWork<'s, K>, crate::EpochExhausted> {
        Ok(match &mut self.phase {
            Phase::Idle => DtmRadioWork::None,
            Phase::Running => {
                match self.session.next_request(now, timing, payload).map_err(
                    |DtmEpochExhausted { calculation }| {
                        crate::EpochExhausted::at(
                            crate::PlanningRole::DirectTest,
                            crate::PlanningOperation::Event,
                            match calculation {
                                DtmCalculation::Admission => crate::PlanningCalculation::Admission,
                                DtmCalculation::InitialAnchor => {
                                    crate::PlanningCalculation::FirstAnchor
                                }
                                DtmCalculation::NextAnchor => {
                                    crate::PlanningCalculation::NextAnchor
                                }
                                DtmCalculation::ReachableAnchor => {
                                    crate::PlanningCalculation::ReachableAnchor
                                }
                                DtmCalculation::SlotAlignment => {
                                    crate::PlanningCalculation::SlotAlignment
                                }
                                DtmCalculation::AirWindow => {
                                    crate::PlanningCalculation::WindowGeometry
                                }
                                DtmCalculation::Reservation => {
                                    crate::PlanningCalculation::Reservation
                                }
                            },
                        )
                    },
                )? {
                    Some(event) => match place::<K>(event, now) {
                        Ok(request) => DtmRadioWork::Request(request),
                        // A window past the horizon is skipped as a refused
                        // one is; the test was checked against `K` when it
                        // started.
                        Err(Unplaced::Unsupported | Unplaced::BeyondHorizon) => {
                            self.session.refused();
                            DtmRadioWork::None
                        }
                        Err(Unplaced::OutsideEpoch) => {
                            self.session.refused();
                            return Err(crate::EpochExhausted::at(
                                crate::PlanningRole::DirectTest,
                                crate::PlanningOperation::Event,
                                crate::PlanningCalculation::Reservation,
                            ));
                        }
                    },
                    None => DtmRadioWork::None,
                }
            }
            Phase::Draining(_, id, cancelled) => {
                if *cancelled {
                    DtmRadioWork::None
                } else {
                    *cancelled = true;
                    DtmRadioWork::Cancel(*id)
                }
            }
            Phase::Releasing(..) => DtmRadioWork::Request(RadioRequest::EndTest),
        })
    }

    /// The backend's answer to the last request of this role.
    pub(crate) fn request_done(&mut self, accepted: bool) {
        match self.phase {
            Phase::Running if accepted => self.holds_role = true,
            Phase::Running => self.session.refused(),
            // A cancelled event ends either way; its outcome follows.
            Phase::Draining(..) | Phase::Idle => {}
            Phase::Releasing(..) => {
                self.holds_role = false;
                self.finish();
            }
        }
    }

    pub(crate) fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        self.session.observe(outcome);
        if let Phase::Draining(..) = self.phase
            && let Some(received) = self.session.drained()
        {
            let Phase::Draining(command, ..) = core::mem::replace(&mut self.phase, Phase::Idle)
            else {
                unreachable!()
            };
            self.release(command, received);
        }
    }

    fn finish(&mut self) {
        if let Phase::Releasing(command, received) =
            core::mem::replace(&mut self.phase, Phase::Idle)
        {
            self.completion = command.map(|command| command.into_ended_command_complete(received));
        }
    }

    pub(crate) fn take_completion(&mut self) -> Option<LeDtmCommandCompleteEvent> {
        self.completion.take()
    }
}

fn busy(command: LeDtmCommand) -> LeDtmCommandCompleteEvent {
    rejected(command, HciError::CONTROLLER_BUSY)
}

fn rejected(command: LeDtmCommand, error: HciError) -> LeDtmCommandCompleteEvent {
    LeDtmCommandCompleteEvent::without_return_parameters(command.kind().opcode(), error.to_status())
}

/// The request of one planned test event, placed at `now`.
fn place<K: LeCapabilities>(
    event: DtmEvent<'_>,
    now: LeInstant,
) -> Result<RadioRequest<'_, K>, Unplaced> {
    Ok(match event {
        DtmEvent::Transmit(test) => RadioRequest::TestTransmit(Supported::placed(test, now)?),
        DtmEvent::Receive(test) => RadioRequest::TestReceive(Supported::placed(test, now)?),
    })
}
