//! Exercise the standalone model library without its unit-test cfg.
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
    )
)]

use core::{future::Future, num::NonZeroU8, task::Poll};
use oer_ieee80211_wire::frame::FrameControl;
use std::{rc::Rc, task::Waker};

use oer_ieee80211_model::{
    MODEL_RX_EVENTS, ModelAir, ModelAnyBuffer, ModelBackend, ModelLarge, ModelResources,
    ModelRxMeta, ModelSmall, ModelTypes,
};
use oer_ieee80211_port::{
    AckHeader, Attempt, Channel, ChannelWidth, ClassFrame, HeaderSpec, InterfacePhase,
    LowerMacEvent, LowerMacPhase, LowerMacTerminal, MpduLength, Protection, RetryPlan, RetryStage,
    RxBuffer, SizedFrame, Station, SupportedPower, TxBufferPool, TxCompleted, TxFrame, TxOperation,
    TxPower, TxProtection, TxSource,
};
use oer_ieee80211_wire::{
    address::IndividualAddress,
    phy::{LegacyRate, PhyRate},
    qos::WmmAccessCategory,
};
use oer_radio_port::{
    Disabled, Ended, LocalId, Owner, PortBody, PortPhase, SlotIdentity, run_port, with_local_slot,
};

const STATION: [u8; 6] = [2, 0, 0, 0, 0, 1];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 2];

#[path = "model_feature/response.rs"]
mod response;

fn backend<Id: SlotIdentity>() -> (ModelBackend<Id>, Rc<ModelAir>) {
    ModelBackend::new(
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
        [
            IndividualAddress::new(STATION).unwrap(),
            IndividualAddress::new(PEER).unwrap(),
        ],
        std::rc::Rc::new(oer_time_virtual::VirtualClock::<8>::new()),
    )
}

fn exercise_model<P>(body: impl FnOnce(Rc<ModelAir>) -> P) -> bool
where
    for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>>, Output = ()>,
{
    run(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, air) = backend();
        let end = oer_radio_port::LocalExecution
            .scope(
                oer_radio_port::conformance::registration::guarded(
                    (&mut slot)
                        .register(backend)
                        .ok()
                        .expect("fresh local model slot"),
                ),
                async |frontend| run_port(frontend, body(air)).await,
            )
            .await;
        let stopped = matches!(&end, oer_radio_port::PortEnd::FailStopped { outcome, .. } if matches!(outcome.outcome(), oer_radio_port::Abandon::Stopped(_)));
        assert!(stopped || matches!(&end, oer_radio_port::PortEnd::Uninstalled { .. }));
        drop(end);
        slot.end_model_world();
        stopped
    }))
}

fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    core::pin::pin!(future).poll(&mut core::task::Context::from_waker(Waker::noop()))
}

fn run<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(Waker::noop());
    for _ in 0..1_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
    panic!("the model installation did not complete");
}

struct Receive(Rc<ModelAir>);

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Receive {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        _: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = enabled.phase();
            self.0
                .receive_continuously(&[1, 2, 3], ModelRxMeta::new(phase.channel(), -40, -90));
            let mut held = Vec::new();
            for _ in 0..MODEL_RX_EVENTS {
                let Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) =
                    poll_once(phase.next_event())
                else {
                    panic!("continuous RX must be ready within its capacity");
                };
                assert_eq!(frame.bytes(), &[1, 2, 3]);
                held.push(frame);
            }
            assert!(poll_once(phase.next_event()).is_pending());
            phase.recycle(held.pop().unwrap());
            // Recycling the continuous source restores capacity immediately,
            // without giving the driver another poll.
            let Poll::Ready(Ok(Ok(LowerMacEvent::Received { frame, .. }))) =
                poll_once(phase.next_event())
            else {
                panic!("the returned RX slot must be reusable");
            };
            held.push(frame);
            self.0.stop_continuous_receive();
            for frame in held {
                phase.recycle(frame);
            }
            assert!(poll_once(phase.next_event()).is_pending());
        }
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("all RX slots were recycled"))
            .await
        else {
            panic!("recycled RX must not poison the model");
        };
        Ended::uninstall(disabled, (), ())
    }
}

#[test]
fn continuous_receive_has_the_same_capacity_and_recycling_in_feature_builds() {
    assert!(!exercise_model(Receive));
}

struct ReceiveLoss(Rc<ModelAir>);

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for ReceiveLoss {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        use embassy_futures::yield_now;
        use oer_ieee80211_model::MODEL_EVENTS;
        use oer_ieee80211_port::{StationPhase, StationReceive};
        use oer_radio_port::EventsLost;

        let mut owner = owner.enable().await.ok().unwrap();
        {
            let mut phase = owner.phase();
            let meta = ModelRxMeta::new(phase.channel(), -40, -90);
            // Overflow actual receive capacity; do not write the loss latch.
            for _ in 0..MODEL_RX_EVENTS + 1 {
                self.0.receive(&[1], meta);
            }
            phase
                .station()
                .configure(resources.station_commands.configure, StationReceive::Off);
            phase.close_gate(resources.gate);
            yield_now().await;
            self.0.receive_continuously(&[2], meta);
            let mut terminals = Vec::new();
            let mut losses = 0;
            let mut streaming = 0;
            for _ in 0..MODEL_EVENTS + 4 {
                match phase.next_event().await.unwrap() {
                    Err(EventsLost) => losses += 1,
                    Ok(LowerMacEvent::Received { frame, .. }) => {
                        if frame.bytes() == [2] {
                            assert_eq!(losses, 1, "continuous RX cannot pass pending loss");
                            streaming += 1;
                        }
                        phase.recycle(frame);
                    }
                    Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                        oer_ieee80211_port::Configured::Station(_),
                    ))) => terminals.push(1),
                    Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(_))) => {
                        terminals.push(2)
                    }
                    _ => panic!("unexpected event"),
                }
                // Return real RX capacity before the next observation, as a
                // concurrently running driver would do under continuous RX.
                yield_now().await;
            }
            assert_eq!(
                losses, 1,
                "one coalesced indication, not one per lost frame"
            );
            assert_eq!(terminals, [1, 2], "ordinary terminal FIFO is preserved");
            assert!(streaming > 0);
            self.0.stop_continuous_receive();
            assert!(poll_once(phase.next_event()).is_pending());
        }
        let owner = owner
            .disable(&mut |_| panic!("all owners returned"))
            .await
            .ok()
            .unwrap();
        Ended::uninstall(owner, (), ())
    }
}

#[test]
fn receive_overflow_is_reported_before_continuous_rx_without_reordering_terminals() {
    assert!(!exercise_model(ReceiveLoss));
}

struct Owners;

impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for Owners {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, ModelBackend<Id>, Disabled>,
        mut resources: ModelResources<'id, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let Ok(mut enabled) = owner.enable().await else {
            panic!("enable")
        };
        {
            let mut phase = enabled.phase();
            macro_rules! submit {
                ($class:ty, $buffer:expr) => {{
                    let rate = phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap();
                    let header = AckHeader::new(
                        HeaderSpec::new(
                            FrameControl::new(0x0108).unwrap(),
                            0,
                            IndividualAddress::new(PEER).unwrap(),
                            STATION,
                            PEER,
                            0,
                            None,
                            None,
                            None,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let frame = ClassFrame::new(TxFrame::Individual {
                        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
                        plan: RetryPlan::new(RetryStage {
                            rate,
                            transmissions: NonZeroU8::MIN,
                            protection: Protection::None,
                        }),
                    })
                    .ok()
                    .unwrap();
                    let source = TxSource::Fresh {
                        buffer: $buffer,
                        frame,
                        body: None,
                        protection: TxProtection::Open,
                    };
                    let source = <ModelTypes<ModelBackend<Id>> as TxBufferPool<$class>>::prepare::<
                        Station,
                        _,
                    >(source.check().ok().unwrap())
                    .ok()
                    .unwrap();
                    let ticket = resources.tickets.pop().unwrap();
                    let index = ticket.index();
                    phase
                        .station()
                        .submit(
                            (Attempt {
                                context: (),
                                ticket,
                                source,
                                access_category: WmmAccessCategory::BestEffort,
                                protection: Protection::None,
                                power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                                coex_operation: TxOperation::Data,
                            })
                            .admit(),
                        )
                        .unwrap();
                    index
                }};
            }
            let first = submit!(ModelSmall, resources.small.pop().unwrap());
            let second = submit!(ModelLarge, resources.large.pop().unwrap());
            for (ticket_index, large) in [(first, false), (second, true)] {
                let event = phase.next_event().await.unwrap().unwrap();
                let LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(TxCompleted::Single(
                    terminal,
                ))) = event
                else {
                    panic!("one Single terminal per submission");
                };
                let oer_ieee80211_port::TerminalParts {
                    ticket, payload, ..
                } = terminal.into_parts(&mut resources.terminal_access);
                let (buffer, _) = oer_ieee80211_port::TxMpdu::into_parts(payload);
                assert_eq!(ticket.index(), ticket_index);
                assert_eq!(matches!(buffer, ModelAnyBuffer::LargeSent(_)), large);
                if !large {
                    assert!(matches!(buffer, ModelAnyBuffer::SmallSent(_)));
                }
            }
            assert!(poll_once(phase.next_event()).is_pending());
        }
        let Ok(disabled) = enabled
            .disable(&mut |_| panic!("both terminals were returned"))
            .await
        else {
            panic!("disable")
        };
        Ended::uninstall(disabled, (), ())
    }
}

#[test]
fn feature_builds_keep_each_accepted_ticket_with_its_original_buffer() {
    assert!(!exercise_model(|_| Owners));
}
