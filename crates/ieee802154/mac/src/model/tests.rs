use alloc::{sync::Arc, task::Wake};
use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};

use oer_radio_port::{
    Call, Detached, DriverExit, Ended, InstallParts, Installed, Owner, PortBody, PortPhase,
    Retired, run_port,
};
use oer_radio_port::{EventsLost, Poisoned};

use super::*;
use crate::owner::{Ieee802154Clients, Transmission};

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Drive `future`, which the model never leaves pending, to its end.
fn ready<F: Future>(future: F) -> F::Output {
    match poll_once(pin!(future)) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the model answers at once"),
    }
}

fn channel(number: u8) -> Channel {
    Channel::new(number).unwrap_or_else(|_| panic!("an O-QPSK channel"))
}

/// An acknowledged data frame: frame control, sequence, PAN, short
/// destination and source.
const DATA: [u8; 9] = [0x61, 0x88, 0x01, 0xcd, 0xab, 0x02, 0x00, 0x01, 0x00];

fn transmission<'f>(frame: &'f [u8]) -> SupportedTransmit<'f, ModelCaps> {
    SupportedTransmit::new(Transmission {
        frame: crate::FrameView::new(frame).unwrap_or_else(|_| panic!("a MAC frame")),
        channel: channel(15),
        mode: crate::TxMode::CsmaCa { max_backoffs: 4 },
        power: None,
        max_frame_retries: 3,
        security: crate::TxSecurity::Radio,
        time_sync: None,
    })
    .unwrap_or_else(|_| panic!("the model performs every transmission"))
}

/// Run `body` over a fresh model and give back its world.
fn run<B: PortBody<OwnedModel>>(body: B) -> ModelHandle {
    let (model, handle) = OwnedModel::new();
    HANDLE.with(|slot| *slot.borrow_mut() = Some(handle.clone()));
    let oer_radio_port::PortEnd::Uninstalled { uninstalled, .. } = ready(run_port(model, body))
    else {
        panic!("the body uninstalls")
    };
    assert!(!uninstalled.abandoned());
    handle
}

#[test]
fn accounting_quarantine_does_not_prevent_a_confirmed_hardware_stop() {
    struct Body(ModelHandle);
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat, _] = resources.operation;
            let configured = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            let end = oer_radio_port::EventEnd::Quarantined { cause: ModelFault };
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let client = parts.clients.client(&configured);
                let permit = ready(client.operation(&mut seat));
                let token = client.transmit(permit, transmission(&DATA)).unwrap();
                self.0
                    .0
                    .borrow_mut()
                    .events
                    .push_back(ModelEvent::TransmitDone(
                        Interface::new(0),
                        RequestId::new(u32::MAX),
                        TxStatus::Success,
                    ));
                assert!(self.0.complete_transmit(token.id(), TxStatus::Success));
                for _ in 0..2 {
                    assert!(
                        matches!(ready(parts.events.next_event()), Err(actual) if actual == end)
                    );
                }
                assert_eq!(
                    parts.events.held.permits.in_flight(),
                    Some((Interface::new(0), token.id()))
                );
                assert!(poll_once(pin!(client.operation(&mut seat))).is_pending());
                assert_eq!(self.0.0.borrow().events.len(), 2);
                assert!(!self.0.abandoned());
            }
            assert!(matches!(ready(owner.phase().next_event()), Err(actual) if actual == end));
            let stopped = owner.fail_stop();
            assert!(matches!(
                stopped.outcome(),
                oer_radio_port::Abandon::Stopped(_)
            ));
            Ended::fail_stopped(stopped, ())
        }
    }

    let (installed, handle) = OwnedModel::new();
    let end = ready(run_port(installed, Body(handle.clone())));
    assert!(matches!(
        end,
        oer_radio_port::PortEnd::FailStopped {
            outcome: oer_radio_port::Abandon::Stopped(_),
            ..
        }
    ));
    let owners = Rc::strong_count(&handle.0);
    assert!(owners > 1, "the core retains the model's owner");
    drop(end);
    assert_eq!(Rc::strong_count(&handle.0), owners);
    assert_eq!(handle.0.borrow().events.len(), 2);
}

#[test]
fn the_operation_permit_comes_back_before_its_terminal_is_handed_out() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat0, mut seat1] = resources.operation;
            let zero = owner.setup().configure(resources.interface0.interface);
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            let handle = {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let events = &mut parts.events;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                let permit = ready(first.operation(&mut seat0));
                let work = first
                    .transmit(permit, transmission(&DATA))
                    .unwrap_or_else(|_| panic!("not poisoned"));
                let id = work.id();
                // The other interface waits for the one operation.
                let mut waiting = pin!(second.operation(&mut seat1));
                assert!(poll_once(waiting.as_mut()).is_pending());
                // A dropped handle does not keep the permit.
                drop(work);
                assert!(poll_once(waiting.as_mut()).is_pending());
                let handle = parts_handle();
                assert!(handle.complete_transmit(id, TxStatus::Success));
                let event = ready(events.next_event())
                    .unwrap_or_else(|_| panic!("not poisoned"))
                    .unwrap_or_else(|_| panic!("nothing lost"));
                // The permit is back before the terminal is seen.
                let Poll::Ready(permit) = poll_once(waiting.as_mut()) else {
                    panic!("the terminal released the permit");
                };
                assert!(matches!(
                    ModelEvents::view(&event),
                    PhaseEvent::TransmitDone {
                        interface,
                        id: done,
                        status: TxStatus::Success,
                        ..
                    } if interface == Interface::new(0) && done == id
                ));
                drop(permit);
                handle
            };
            let _ = handle;
            let owner = owner
                .disable(&mut |_| panic!("nothing is left"))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

std::thread_local! {
    static HANDLE: RefCell<Option<ModelHandle>> = const { RefCell::new(None) };
}

/// The world of the model the running test's body drives.
fn parts_handle() -> ModelHandle {
    HANDLE.with(|handle| {
        handle
            .borrow()
            .clone()
            .unwrap_or_else(|| panic!("a handle"))
    })
}

#[test]
fn takers_of_the_operation_are_served_in_arrival_order_across_interfaces() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat0, mut seat1] = resources.operation;
            let zero = owner.setup().configure(resources.interface0.interface);
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                let held = ready(first.operation(&mut seat0));
                // A taker that gives up its place before it is served
                // stalls no one: its future ends with this block.
                {
                    let mut gives_up = pin!(first.operation(&mut seat0));
                    assert!(poll_once(gives_up.as_mut()).is_pending());
                }
                let mut one_first = pin!(second.operation(&mut seat1));
                let mut zero_second = pin!(first.operation(&mut seat0));
                assert!(poll_once(one_first.as_mut()).is_pending());
                assert!(poll_once(zero_second.as_mut()).is_pending());
                // A permit dropped before a submission goes back.
                drop(held);
                assert!(poll_once(zero_second.as_mut()).is_pending());
                let Poll::Ready(permit) = poll_once(one_first.as_mut()) else {
                    panic!("the earliest waiting taker is served");
                };
                drop(permit);
                assert!(poll_once(zero_second.as_mut()).is_ready());
            }
            let owner = owner
                .disable(&mut |_| panic!("nothing is left"))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn disable_hands_leftover_terminals_to_the_sink_and_takes_their_permits_back() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat0, _] = resources.operation;
            let zero = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            let id = {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let client = parts.clients.client(&zero);
                let permit = ready(client.operation(&mut seat0));
                let scan = SupportedEnergyScan::new(channel(20), 128)
                    .unwrap_or_else(|| panic!("the model scans"));
                let work = client
                    .energy_scan(permit, scan)
                    .unwrap_or_else(|_| panic!("not poisoned"));
                work.id()
            };
            let mut left = Vec::new();
            let owner = owner
                .disable(&mut |event| {
                    left.push(event);
                    core::ops::ControlFlow::Continue(())
                })
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            assert!(matches!(
                left.as_slice(),
                [ModelEvent::EnergyScanDone(interface, done, None)]
                    if *interface == Interface::new(0) && *done == id
            ));
            // The next phase admits an operation at once.
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let client = parts.clients.client(&zero);
                assert!(poll_once(pin!(client.operation(&mut seat0))).is_ready());
            }
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn rejected_disable_terminal_and_later_events_remain_in_the_uninstalled_model() {
    struct Reject;
    impl PortBody<OwnedModel> for Reject {
        type Output = ();
        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat, _] = resources.operation;
            let configured = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.ok().expect("enabled");
            let handle = parts_handle();
            {
                let phase = owner.phase();
                let client = phase.clients.client(&configured);
                ready(client.receive(channel(11))).unwrap();
                let permit = ready(client.operation(&mut seat));
                let scan = SupportedEnergyScan::new(channel(11), 128).unwrap();
                let id = client.energy_scan(permit, scan).unwrap().id();
                assert!(handle.complete_energy_scan(id, Some(-70)));
                assert!(handle.receive(channel(11), Interface::new(0), &DATA, -40));
            }
            let mut offered = 0;
            let poisoned = owner
                .disable(&mut |terminal| {
                    offered += 1;
                    core::ops::ControlFlow::Break(terminal)
                })
                .await
                .err()
                .expect("the sink rejected its terminal");
            assert_eq!(offered, 1);
            assert_eq!(poisoned.cause(), ModelFault);
            Ended::uninstall(poisoned, (), ())
        }
    }
    let model = run(Reject);
    let state = model.0.borrow();
    assert_eq!(state.events.len(), 2);
    assert!(matches!(
        state.events[0],
        ModelEvent::EnergyScanDone(_, _, Some(-70))
    ));
    assert!(matches!(state.events[1], ModelEvent::Received(..)));
    assert_eq!(state.poisoned, Some(ModelFault));
}

#[test]
fn the_radio_receives_while_any_interface_asks_on_one_shared_channel() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let zero = owner.setup().configure(resources.interface0.interface);
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                let handle = parts_handle();
                ready(first.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                assert_eq!(handle.receiving(), Some(channel(11)));
                // The same channel joins at once.
                ready(second.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                first.sleep();
                assert_eq!(handle.receiving(), Some(channel(11)));
                // Another channel waits until the receiving interface sleeps.
                ready(first.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                second.sleep();
                let mut other = pin!(second.receive(channel(25)));
                assert!(poll_once(other.as_mut()).is_pending());
                first.sleep();
                assert_eq!(poll_once(other.as_mut()), Poll::Ready(Ok(())));
                assert_eq!(handle.receiving(), Some(channel(25)));
                second.sleep();
                assert_eq!(handle.receiving(), None);
            }
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn a_window_is_a_wish_that_owes_no_event() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let zero = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let events = &mut parts.events;
                let client = parts.clients.client(&zero);
                let handle = parts_handle();
                let window = || {
                    SupportedScheduledReceive::new(
                        channel(12),
                        Ieee802154Instant::from_micros(1_000),
                        500,
                    )
                    .unwrap_or_else(|| panic!("the model schedules windows"))
                };
                // No window: nothing to wait for.
                assert_eq!(ready(client.window_ended()), Ok(()));
                // Windows are admitted at once, one after another, and the
                // next replaces the last.
                client.scheduled_receive(window());
                client.scheduled_receive(window());
                let mut ended = pin!(client.window_ended());
                assert!(poll_once(ended.as_mut()).is_pending());
                // The window ends on its own.
                assert!(handle.open_window(Interface::new(0)));
                assert!(handle.end_window(Interface::new(0)));
                assert_eq!(poll_once(ended.as_mut()), Poll::Ready(Ok(())));
                // A new wish ends a window too.
                client.scheduled_receive(window());
                ready(client.receive(channel(12))).unwrap_or_else(|_| panic!("not poisoned"));
                assert_eq!(ready(client.window_ended()), Ok(()));
                client.scheduled_receive(window());
                client.sleep();
                assert_eq!(ready(client.window_ended()), Ok(()));
                // No window owed an event.
                assert!(poll_once(pin!(events.next_event())).is_pending());
            }
            let owner = owner
                .disable(&mut |_| panic!("no window owes a terminal"))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn a_cancel_ends_the_operation_once() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [_, mut seat1] = resources.operation;
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let events = &mut parts.events;
                let client = parts.clients.client(&one);
                let permit = ready(client.operation(&mut seat1));
                let cca =
                    SupportedCca::new(channel(26)).unwrap_or_else(|| panic!("the model assesses"));
                let work = client
                    .clear_channel_assessment(permit, cca)
                    .unwrap_or_else(|_| panic!("not poisoned"));
                let id = work.id();
                assert_eq!(ready(work.cancel()), Ok(Ok(())));
                let event = ready(events.next_event())
                    .unwrap_or_else(|_| panic!("not poisoned"))
                    .unwrap_or_else(|_| panic!("nothing lost"));
                assert_eq!(
                    ModelEvents::view(&event),
                    PhaseEvent::ClearChannelAssessmentFailed {
                        interface: Interface::new(1),
                        id
                    }
                );
                // Nothing is in flight any more; the world cannot end it.
                assert!(!parts_handle().complete_cca(id, Some(true)));
                assert!(poll_once(pin!(client.operation(&mut seat1))).is_ready());
            }
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn a_radio_fault_ends_admitted_work_first_then_poisons() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat0, _] = resources.operation;
            let zero = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let events = &mut parts.events;
                let client = parts.clients.client(&zero);
                ready(client.receive(channel(15))).unwrap_or_else(|_| panic!("not poisoned"));
                let permit = ready(client.operation(&mut seat0));
                let work = client
                    .transmit(permit, transmission(&DATA))
                    .unwrap_or_else(|_| panic!("not poisoned"));
                let id = work.id();
                parts_handle().poison();
                let event = ready(events.next_event())
                    .unwrap_or_else(|_| panic!("the earlier terminal comes first"))
                    .unwrap_or_else(|_| panic!("nothing lost"));
                assert!(matches!(
                    ModelEvents::view(&event),
                    PhaseEvent::TransmitDone { id: done, status: TxStatus::Aborted, .. } if done == id
                ));
                assert!(matches!(
                    ready(events.next_event()),
                    Err(oer_radio_port::EventEnd::Drained { cause: ModelFault })
                ));
                // Nothing follows the poison, a reception included.
                assert!(!parts_handle().receive(channel(15), Interface::new(0), &DATA, -40));
                assert!(matches!(
                    ready(events.next_event()),
                    Err(oer_radio_port::EventEnd::Drained { cause: ModelFault })
                ));
                // The permit came back with the terminal; a submission is
                // poisoned and gives it back again.
                let permit = ready(client.operation(&mut seat0));
                assert!(client.transmit(permit, transmission(&DATA)).is_err());
                assert!(poll_once(pin!(client.operation(&mut seat0))).is_ready());
            }
            match owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
            {
                Ok(_) => panic!("a poisoned backend does not disable"),
                Err(poisoned) => oer_radio_port::Ended::uninstall(poisoned, (), ()),
            }
        }
    }
    run(Body);
}

#[test]
fn a_reception_overflow_is_reported_once_and_loses_no_terminal() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let [mut seat0, _] = resources.operation;
            let zero = owner.setup().configure(resources.interface0.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let events = &mut parts.events;
                let client = parts.clients.client(&zero);
                let handle = parts_handle();
                ready(client.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                for _ in 0..MODEL_RECEIVED_FRAMES + 2 {
                    assert!(handle.receive(channel(11), Interface::new(0), &DATA, -40));
                }
                let permit = ready(client.operation(&mut seat0));
                let id = client
                    .transmit(permit, transmission(&DATA))
                    .unwrap_or_else(|_| panic!("not poisoned"))
                    .id();
                assert!(handle.complete_transmit(id, TxStatus::NoAcknowledgement));
                assert!(handle.receive(channel(11), Interface::new(0), &DATA, -50));
                // The queued frames, one loss marker, then the terminal
                // produced after the loss; the frame after it found the
                // queue still full and is part of the reported gap.
                let mut order = Vec::new();
                loop {
                    match poll_once(pin!(events.next_event())) {
                        Poll::Ready(Ok(Ok(event))) => match ModelEvents::view(&event) {
                            PhaseEvent::Received(_) => order.push('r'),
                            PhaseEvent::TransmitDone { id: done, .. } => {
                                assert_eq!(done, id);
                                order.push('t');
                            }
                            other => panic!("unexpected {other:?}"),
                        },
                        Poll::Ready(Ok(Err(EventsLost))) => order.push('l'),
                        Poll::Ready(Err(_)) => panic!("not poisoned"),
                        Poll::Pending => break,
                    }
                }
                assert_eq!(order, ['r', 'r', 'r', 'r', 'l', 't']);
                // With room again, the next frame is queued.
                assert!(handle.receive(channel(11), Interface::new(0), &DATA, -45));
                assert!(matches!(
                    ready(events.next_event()),
                    Ok(Ok(ModelEvent::Received(..)))
                ));
                assert_eq!(resources.rssi.recent_rssi(), -45);
                // Leave frames, a loss and a scan's terminal behind.
                for _ in 0..MODEL_RECEIVED_FRAMES + 1 {
                    assert!(handle.receive(channel(11), Interface::new(0), &DATA, -40));
                }
                let permit = ready(client.operation(&mut seat0));
                let scan = SupportedEnergyScan::new(channel(11), 64)
                    .unwrap_or_else(|| panic!("the model scans"));
                let id = client
                    .energy_scan(permit, scan)
                    .unwrap_or_else(|_| panic!("not poisoned"))
                    .id();
                assert!(handle.complete_energy_scan(id, Some(-70)));
            }
            // The sink gets the terminal alone, which the portable view reads.
            let mut ended = Vec::new();
            let owner = owner
                .disable(&mut |event| {
                    ended.push(ModelEvents::view(&event).ends());
                    core::ops::ControlFlow::Continue(())
                })
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            assert_eq!(ended.len(), 1);
            assert!(ended[0].is_some());
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn pending_entries_hold_their_interface_slots_across_phases() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let ModelResources {
                interface0,
                interface1,
                ..
            } = resources;
            let zero = owner.setup().configure(interface0.interface);
            let one = owner.setup().configure(interface1.interface);
            let has = Has::<ModelCaps, SourceMatch>::from_const();
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            let [short, _] = interface1.short;
            let [extended, _] = interface0.extended;
            let (short, extended) = {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                second.set_pending_mode(has, AutoPendingMode::Enhanced);
                (
                    second.add_pending_short(short, [0x34, 0x12]),
                    first.add_pending_extended(extended, [1, 2, 3, 4, 5, 6, 7, 8]),
                )
            };
            let handle = parts_handle();
            assert_eq!(
                handle.interface(Interface::new(1)).pending_short[0],
                Some([0x34, 0x12])
            );
            assert_eq!(
                handle.interface(Interface::new(0)).pending_extended[0],
                Some([1, 2, 3, 4, 5, 6, 7, 8])
            );
            let mut owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"))
                .enable()
                .await
                .unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let second = parts.clients.client(&one);
                let first = parts.clients.client(&zero);
                let _slot = second.remove_pending_short(short);
                let _slot = first.remove_pending_extended(extended);
            }
            assert_eq!(handle.interface(Interface::new(1)).pending_short[0], None);
            assert_eq!(
                handle.interface(Interface::new(0)).pending_extended[0],
                None
            );
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn settings_reach_the_radio_while_disabled_and_while_enabled() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            {
                let mut setup = owner.setup();
                setup.set_transmit_power(
                    SupportedPower::new(5).unwrap_or_else(|_| panic!("within the levels")),
                );
                setup.set_promiscuous(Has::from_const(), true);
            }
            let zero = owner.setup().configure(resources.interface0.interface);
            let handle = parts_handle();
            // A request above the highest level radiates the highest.
            assert_eq!(handle.radio().transmit_power_dbm, Some(0));
            assert!(handle.radio().promiscuous);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                parts.control.set_cca_threshold(
                    SupportedCcaThreshold::new(-60).unwrap_or_else(|_| panic!("within range")),
                );
                parts.control.set_enhanced_ack_header_ies(
                    SupportedHeaderIes::new(&[1, 2, 3]).unwrap_or_else(|| panic!("short")),
                );
                let client = parts.clients.client(&zero);
                client.set_pan_id(0xabcd);
                client.set_mac_keys(2, [0; 16], [1; 16], [2; 16]);
                client.update_frame_counter(FrameCounterUpdate::SetIfLarger(7));
                client.update_frame_counter(FrameCounterUpdate::SetIfLarger(3));
                assert_eq!(client.frame_counter(), Some(7));
            }
            assert_eq!(handle.radio().cca_threshold_dbm, Some(-60));
            // The generator is off; its IEs are kept for when it is on.
            assert_eq!(handle.radio().enhanced_ack, None);
            assert_eq!(handle.radio().header_ies, [1, 2, 3]);
            assert_eq!(handle.interface(Interface::new(0)).pan_id, Some(0xabcd));
            let mut owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            let _slot = owner.setup().remove(zero);
            assert!(!handle.interface(Interface::new(0)).configured);
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn a_refused_enable_returns_the_disabled_owner() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            _: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            parts_handle().refuse_enable(true);
            match owner.enable().await {
                Err(oer_radio_port::Failed::Refused(owner, ModelRefusal)) => {
                    oer_radio_port::Ended::uninstall(owner, (), ())
                }
                _ => panic!("the radio refused"),
            }
        }
    }
    run(Body);
}

#[test]
fn validated_values_check_the_capabilities() {
    assert!(SupportedPower::<ModelCaps>::new(-30).is_err());
    assert_eq!(
        SupportedPower::<ModelCaps>::new(13).map(|power| power.dbm()),
        Ok(12)
    );
    assert!(SupportedCcaThreshold::<ModelCaps>::new(-30).is_err());
    assert!(SupportedHeaderIes::<ModelCaps>::new(&[0; 17]).is_none());
    struct Plain;
    impl Ieee802154Caps for Plain {
        const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
            operations: RadioCapabilities::NONE,
            ..ModelCaps::CAPS
        };
    }
    assert!(Has::<Plain, Promiscuous>::new().is_none());
    assert!(SupportedEnergyScan::<Plain>::new(channel(11), 1).is_none());
    assert!(SupportedCca::<Plain>::new(channel(11)).is_none());
    let refused = SupportedTransmit::<Plain>::new(Transmission {
        frame: crate::FrameView::new(&DATA).unwrap_or_else(|_| panic!("a MAC frame")),
        channel: channel(15),
        mode: crate::TxMode::CsmaCa { max_backoffs: 4 },
        power: None,
        max_frame_retries: 3,
        security: crate::TxSecurity::Radio,
        time_sync: None,
    });
    let Err((_, missing)) = refused else {
        panic!("a plain radio sends direct frames only");
    };
    assert_eq!(
        missing,
        RadioCapabilities::CSMA_CA
            .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
            .union(RadioCapabilities::TRANSMIT_RETRIES)
    );
    // Only what the radio lacks: a scheduler without CCA misses CCA alone.
    struct Scheduler;
    impl Ieee802154Caps for Scheduler {
        const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
            operations: RadioCapabilities::SCHEDULED_TRANSMIT,
            ..ModelCaps::CAPS
        };
    }
    let refused = SupportedTransmit::<Scheduler>::new(Transmission {
        frame: crate::FrameView::new(&[0x41, 0x88, 0x01, 0xcd, 0xab, 0xff, 0xff, 0x01, 0x00])
            .unwrap_or_else(|_| panic!("a MAC frame")),
        channel: channel(15),
        mode: crate::TxMode::Scheduled {
            at: Ieee802154Instant::from_micros(10),
            cca: true,
        },
        power: None,
        max_frame_retries: 0,
        security: crate::TxSecurity::Radio,
        time_sync: None,
    });
    let Err((_, missing)) = refused else {
        panic!("the scheduler has no CCA");
    };
    assert_eq!(missing, RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT);
}

/// A backend whose only resource is a model pool of the one operation
/// permit, as `ModelHeld` keeps it, for the shared pool rules.
struct PoolProbe;

struct NoEvents;

impl PortPhase for NoEvents {
    type Event = ();
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<(), EventsLost>, oer_radio_port::EventEnd<ModelFault>>> + '_
    {
        core::future::pending()
    }
}

impl PortBackend for PoolProbe {
    type Event<'id> = ();
    type Terminal<'id> = ();
    type Phase<'a, 'id>
        = PhaseParts<NoEvents, (), ()>
    where
        Self: 'a,
        'id: 'a;
    type Local<'id> = ();
    type Driver = ();
    type Domain = Ieee802154Radio;
    type Fault = ModelFault;
    type Refusal = ModelRefusal;
    type Setup<'a, 'id>
        = ()
    where
        Self: 'a,
        'id: 'a;

    type Pools = oer_radio_port::Permits<crate::owner::Operation, 1, 3>;
    type Resources<'id> = (
        ModelPermitPool<'id, crate::owner::Operation>,
        [oer_radio_port::Seat<'id, crate::owner::Operation>; 3],
    );
    type Held<'id> = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = ();

    fn enable<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,
    ) -> impl Future<Output = oer_radio_port::EnableResult<ModelRefusal, ModelFault>> + 'a
    where
        'id: 'a,
    {
        core::future::ready(Ok(Ok(())))
    }

    fn disable<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a (),
        _: &'a mut Self::Local<'id>,
        _: &'a mut dyn FnMut(()) -> core::ops::ControlFlow<()>,
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        core::future::ready(Ok(()))
    }

    fn now(
        &self,
        _: Call,
    ) -> impl Future<
        Output = oer_radio_port::PortResult<
            Ieee802154Instant,
            oer_radio_port::ClockError,
            ModelFault,
        >,
    > + '_ {
        core::future::ready(Ok(Ok(Ieee802154Instant::from_micros(0))))
    }

    fn resources<'id>(&mut self, mint: Mint<'id, Self::Pools>) -> InstallParts<'id, Self> {
        let oer_radio_port::PermitTokens { supply, seats, .. } = mint.into_pools();
        InstallParts {
            held: (),
            local: (),
            resources: (ModelPermitPool::new(supply), seats),
        }
    }

    fn setup<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,
    ) -> Self::Setup<'a, 'id>
    where
        'id: 'a,
    {
    }

    fn phase<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a (),
        _: &'a mut Self::Local<'id>,
    ) -> Self::Phase<'a, 'id>
    where
        'id: 'a,
    {
        PhaseParts {
            events: NoEvents,
            control: (),
            clients: (),
        }
    }

    async fn drive<'p, 'id>(
        driver: oer_radio_port::DriverLoan<'p, Self::Driver>,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> DriverExit<Self::State, Self::Fault> {
        DriverExit::Retired {
            state: (),
            proof: retire.requested(driver).await,
        }
    }

    fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<> {
        core::future::pending()
    }

    fn detach<'id>(&mut self, _: oer_radio_port::Detach<'_>, _: &Self::Held<'id>) {}

    fn uninstall<'id>(
        self,
        _: Self::Held<'id>,
        _: Self::Local<'id>,
        _: Retired<'id, (Self::Driver, Self::State)>,
        _: Detached<'id>,
        _: Self::Uninstall,
    ) {
    }

    fn abandon(&mut self, call: oer_radio_port::Quarantine) -> oer_radio_port::Abandon {
        oer_radio_port::Abandon::Stopped(call.stopped())
    }
}

#[test]
fn the_operation_pool_serves_takers_in_arrival_order() {
    struct Body;
    impl PortBody<PoolProbe> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            owner: Owner<'p, 'id, PoolProbe, oer_radio_port::Disabled>,
            (pool, mut seats): (
                ModelPermitPool<'id, crate::owner::Operation>,
                [oer_radio_port::Seat<'id, crate::owner::Operation>; 3],
            ),
        ) -> Ended<'id, PoolProbe, ()> {
            oer_radio_port::conformance::permits_serve_takers_in_arrival_order(
                &pool,
                &mut seats,
                [],
            );
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    assert!(matches!(
        ready(run_port(Installed::new(PoolProbe, ()), Body)),
        oer_radio_port::PortEnd::Uninstalled {
            uninstalled: (),
            output: ()
        }
    ));
}

/// A waker that counts its wakes.
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_change_of_wish_wakes_waiting_receives_and_a_window_preempts() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let zero = owner.setup().configure(resources.interface0.interface);
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                let counter = Arc::new(Counter(AtomicUsize::new(0)));
                let waker = Waker::from(counter.clone());
                let mut context = Context::from_waker(&waker);
                ready(first.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                let mut waiting = pin!(second.receive(channel(20)));
                assert!(waiting.as_mut().poll(&mut context).is_pending());
                // The receiving interface moves to the waiting one's
                // channel: the waiting receive is woken and admitted.
                ready(first.receive(channel(20))).unwrap_or_else(|_| panic!("not poisoned"));
                assert!(counter.0.load(Ordering::Relaxed) > 0);
                assert_eq!(waiting.as_mut().poll(&mut context), Poll::Ready(Ok(())));
                // A window on another channel is admitted at once and keeps
                // its time: while open it takes the radio to its channel,
                // and the radio returns to the receive after it.
                let window = SupportedScheduledReceive::new(
                    channel(25),
                    Ieee802154Instant::from_micros(1_000),
                    500,
                )
                .unwrap_or_else(|| panic!("the model schedules windows"));
                second.scheduled_receive(window);
                let handle = parts_handle();
                assert_eq!(handle.receiving(), Some(channel(20)));
                assert!(handle.open_window(Interface::new(1)));
                assert_eq!(handle.receiving(), Some(channel(25)));
                assert!(handle.receive(channel(25), Interface::new(1), &DATA, -60));
                assert!(handle.end_window(Interface::new(1)));
                assert_eq!(handle.receiving(), Some(channel(20)));
            }
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn interfaces_moving_to_one_channel_admit_each_other() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let zero = owner.setup().configure(resources.interface0.interface);
            let one = owner.setup().configure(resources.interface1.interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let first = parts.clients.client(&zero);
                let second = parts.clients.client(&one);
                ready(first.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                ready(second.receive(channel(11))).unwrap_or_else(|_| panic!("not poisoned"));
                // Both PANs move to 15: the first waits while the second
                // receives on 11, and stops receiving there itself.
                let mut moving = pin!(first.receive(channel(15)));
                assert!(poll_once(moving.as_mut()).is_pending());
                // The second sees no interface receiving elsewhere.
                ready(second.receive(channel(15))).unwrap_or_else(|_| panic!("not poisoned"));
                assert_eq!(poll_once(moving.as_mut()), Poll::Ready(Ok(())));
                assert_eq!(parts_handle().receiving(), Some(channel(15)));
            }
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn removing_an_interface_keeps_its_pending_entries() {
    struct Body;
    impl PortBody<OwnedModel> for Body {
        type Output = ();

        async fn run<'p, 'id>(
            self,
            mut owner: Owner<'p, 'id, OwnedModel, oer_radio_port::Disabled>,
            resources: ModelResources<'id>,
        ) -> Ended<'id, OwnedModel, ()> {
            let zero = owner.setup().configure(resources.interface0.interface);
            let [slot, _] = resources.interface0.short;
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            let entry = {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                parts.clients.client(&zero).add_pending_short(slot, [7, 7])
            };
            let mut owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            let interface = owner.setup().remove(zero);
            let handle = parts_handle();
            // The entry's token still holds its row.
            assert_eq!(
                handle.interface(Interface::new(0)).pending_short[0],
                Some([7, 7])
            );
            assert!(!handle.interface(Interface::new(0)).configured);
            let zero = owner.setup().configure(interface);
            let mut owner = owner.enable().await.unwrap_or_else(|_| panic!("enabled"));
            {
                let mut phase = owner.phase();
                let parts = &mut *phase;
                let _slot = parts.clients.client(&zero).remove_pending_short(entry);
            }
            assert_eq!(handle.interface(Interface::new(0)).pending_short[0], None);
            let owner = owner
                .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                .await
                .unwrap_or_else(|_| panic!("not poisoned"));
            oer_radio_port::Ended::uninstall(owner, (), ())
        }
    }
    run(Body);
}

#[test]
fn work_permit_moves_into_admission_and_only_its_terminal_releases_it() {
    use crate::owner::{Operation, OperationPermits};
    use oer_radio_port::{Permits, branded};
    ready(branded::<Permits<Operation, 1, 2>, _>(async |mint| {
        let (permits, [mut first, mut second]) =
            OperationPermits::new(mint.into_pools(), |supply, _| ModelPermitPool::new(supply));
        let token = ready(permits.pool().take(&mut first));
        drop(permits.guard(token));
        let token = ready(permits.pool().take(&mut second));
        let id = RequestId::new(41);
        permits.admit(Interface::new(1), id, permits.guard(token));
        {
            let mut waiting = core::pin::pin!(permits.pool().take(&mut first));
            assert!(
                waiting
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        let mismatch = match permits.complete(RequestId::new(42)) {
            Err(mismatch) => mismatch,
            Ok(_) => panic!("another terminal cannot release the held operation"),
        };
        assert_eq!(mismatch.held, Some((Interface::new(1), id)));
        assert_eq!(permits.in_flight(), mismatch.held);
        let completed = permits.complete(id).expect("matching terminal");
        assert_eq!(completed.id(), id);
        assert_eq!(completed.interface(), Interface::new(1));
        assert!(permits.complete(id).is_err());
        let token = ready(permits.pool().take(&mut first));
        drop(permits.guard(token));
    }));
}
