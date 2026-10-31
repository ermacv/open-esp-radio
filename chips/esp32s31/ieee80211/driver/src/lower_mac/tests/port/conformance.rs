//! The shared lower-MAC conformance suites, run against the S31 backend over
//! the host device model. The model supplies what real air would: ACK and
//! timeout statuses, collisions and a held hardware completion. Air denial
//! has no S31 device model and stays excluded by name.
use super::*;
use oer_ieee80211_testing_conformance::{Environment, MpduState, outcomes};

mod barrier;
mod interface_commands;

/// One single-MPDU outcome case per fresh installation.
struct Outcomes;

/// What the device model answers the case's one publication with: a
/// completion status, or the hardware's TX-timeout edge.
fn answer(case: outcomes::Case) -> Option<((u8, u8), bool)> {
    use outcomes::{Case, Form};
    Some(match case {
        Case::Ack => ((0, 0), false),
        Case::NoResponse(Form::Single) => ((5, 0), false),
        // An RTS error with a collision detail.
        Case::Collision(Form::Single) => ((1, 1), false),
        Case::HardwareTimeout(Form::Single) => ((0, 0), true),
        _ => return None,
    })
}

type OutcomeTrace = outcomes::Trace<Ack, Ack, Infallible>;

struct OutcomeBody {
    hold: bool,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    /// The trace as observed at the terminal: a fail-stop may cancel the
    /// body before it returns.
    seen: Rc<RefCell<Option<OutcomeTrace>>>,
}

impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for OutcomeBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut owner) = owner.enable().await else {
            panic!("enable")
        };
        let mut resources = resources;
        {
            let mut phase = owner.phase();
            let permission = resources
                .station_commands
                .configure
                .with_context(Tag(42, None));
            phase.station().configure(
                permission,
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_)))) =
                phase.next_event().await
            else {
                panic!("configuration terminal")
            };
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            let admitted = outcomes::Identity {
                ticket: ticket.index(),
                buffer: buffer.index(),
            };
            submit_mpdu(&mut phase, buffer, ticket, None, None);
            let mut event = pin!(phase.next_event());
            let event = core::future::poll_fn(|cx| {
                let ready = event.as_mut().poll(cx);
                // A timed-out publication's abort settles in time.
                if ready.is_pending() && self.hold && self.observed.borrow().publications > 0 {
                    let _ = self.timer.0.advance_to_next_deadline();
                    cx.waker().wake_by_ref();
                }
                ready
            })
            .await;
            let completed = match event {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => completed,
                _ => panic!("TX terminal"),
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            let (buffer, _) = returned.payload.into_parts();
            let AnyBuffer::Sent(sent) = buffer else {
                panic!("a published MPDU returns Sent")
            };
            let returned_buffer = sent.index();
            let (mut renewed, _) = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::renew(sent);
            let sequence = oer_ieee80211_wire::sequence::SequenceNumber::from_sequence_control(
                u16::from_le_bytes([renewed.bytes_mut()[22], renewed.bytes_mut()[23]]),
            );
            *self.seen.borrow_mut() = Some(outcomes::Trace {
                admitted,
                returned: outcomes::Identity {
                    ticket: returned.ticket.index(),
                    buffer: returned_buffer,
                },
                state: MpduState::Sent,
                rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
                sequence,
                publications: 0,
                outcome: outcomes::Outcome::Single(returned.outcome),
            });
        }
        let Ok(owner) = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        else {
            panic!("disable")
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

impl outcomes::Harness for Outcomes {
    type Ack = Ack;
    type BlockAck = Ack;
    type AirDenied = Infallible;
    async fn exercise(&mut self, case: outcomes::Case) -> OutcomeTrace {
        let (status, hold) = answer(case).expect("a case this fixture models");
        let (parts, observed, timer) = installed(std::vec![status]);
        observed.borrow_mut().timeout_edge = hold;
        let seen = Rc::new(RefCell::new(None));
        let body = OutcomeBody {
            hold,
            timer,
            observed: observed.clone(),
            seen: seen.clone(),
        };
        let Report::Uninstalled { .. } = run_body(parts, body).await else {
            panic!("clean uninstall")
        };
        let mut output = seen.borrow_mut().take().expect("the terminal was observed");
        output.publications = observed.borrow().publications;
        output
    }
}

#[test]
fn the_s31_backend_passes_the_shared_single_mpdu_outcome_suite() {
    use outcomes::{Case, Form};
    // Every single-MPDU case the device model supplies, through the shared
    // verifier; the report counts them.
    let mut passed = 0;
    for case in [
        Case::Ack,
        Case::NoResponse(Form::Single),
        Case::Collision(Form::Single),
        Case::HardwareTimeout(Form::Single),
    ] {
        let trace = block_on(outcomes::Harness::exercise(&mut Outcomes, case));
        assert_eq!(
            outcomes::verify(case, trace).map_err(|why| (case, why)),
            Ok(())
        );
        passed += 1;
    }
    assert_eq!(passed, 4);
}

#[test]
fn an_executor_deadline_without_a_hardware_timeout_edge_keeps_the_dma_owner() {
    // No completion and no TX-timeout edge: the S31 slot cannot prove its
    // DMA stopped, so it quarantines the storage until a radio reset. The
    // operation's owners never return and the installation fails stop;
    // the shared HardwareTimeout case needs the hardware edge.
    let (parts, observed, timer) = installed(std::vec![(0, 0)]);
    observed.borrow_mut().hold_completion = true;
    let seen = Rc::new(RefCell::new(None));
    let body = OutcomeBody {
        hold: true,
        timer,
        observed: observed.clone(),
        seen: seen.clone(),
    };
    assert!(matches!(
        block_on(run_body(parts, body)),
        Report::FailStopped { .. } | Report::Abandoned
    ));
    assert!(seen.borrow().is_none(), "no terminal returns the owners");
    assert_eq!(observed.borrow().publications, 1);
}
