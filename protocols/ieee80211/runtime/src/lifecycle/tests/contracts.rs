//! The contracts of what the actor returns, the test playing the actor: an
//! owner of no operation the lifecycle sent ends it with its typed cause,
//! the first such owner stays with the retired lifecycle and later ones
//! stay in their queue; a stop that proved nothing ends it unconfirmed.

use super::*;
use oer_protocols_ieee80211_contracts_port::{Configured, LowerMacTerminal, Removed, RemovedKey};

/// What the test, as the actor, sends the lifecycle first.
#[derive(Clone, Copy)]
enum First {
    /// The access point's configuration terminal.
    Configured,
    /// The removal terminal of an access point's group key.
    KeyRemoved,
}

/// Run the lifecycle alone, the test playing the actor: it answers the
/// start, then returns two terminals no operation of the lifecycle asked
/// for, `first` before the other, and an optional stop's outcome; `check`
/// takes the lifecycle's end and the owners left in the return queue.
fn alone(first: First, check: impl for<'s> FnOnce(Completed<'s>, Vec<Returned<'s, P<'s>>>, bool)) {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::new());
        let mut installations = Installations::claim(claim);
        let (install, _air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let (parts, _storage, spare) = lifecycle_spare::<2>(open(), resources, commands);
        let mut to_queue = ClosingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = ClosingSpscQueue::<Parcel<'_, P<'_>>, ACTOR_PARCELS>::new();
        let mut from_queue = ClosingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = ClosingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = ClosingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            ClosingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = ClosingSpscQueue::<LinkReturn, 1>::new();
        let mut command_queue =
            WakingSpscQueue::<crate::station::ServiceCommand<ModelCaps24>, COMMANDS>::new();
        let (to_actor, to_actor_end) = to_queue.split();
        let (parcels, _parcels_end) = parcel_queue.split();
        let (from_actor_end, from_actor) = from_queue.split();
        let (returns_end, returns) = return_queue.split();
        let (link, _link_end) = link_queue.split();
        let (_groups_end, groups) = group_queue.split();
        let (_link_return_end, link_return) = link_return_queue.split();
        let (application, commands) = command_queue.split();
        let state = State::new();
        let (application, control) = Control::new(application, &state).unwrap();
        let mut lifecycle_ends = ActorEnds {
            to_actor,
            parcels,
            from_actor,
            returns,
            groups,
            link,
            link_return,
        };
        let mut output = application.events(|_| {});
        let mut io = LifecycleIo {
            commands,
            output: &mut output,
            control,
            stop: StopPolicy {
                drain: Duration::from_secs(1),
            },
        };
        let mut cx = Context::from_waker(Waker::noop());
        let completed = {
            let run = parts.run(&mut lifecycle_ends, &mut io, &clock, Observed::default());
            let mut run = core::pin::pin!(run);
            assert!(run.as_mut().poll(&mut cx).is_pending());
            assert!(matches!(
                to_actor_end.try_receive(),
                Some(ToActor::Start(_))
            ));
            assert!(from_actor_end.try_send(FromActor::Started(Ok(()))).is_ok());
            assert!(run.as_mut().poll(&mut cx).is_pending());
            let configure =
                LowerMacTerminal::Configured(Configured::AccessPoint(spare.access_point.configure));
            let (slot, permission) = spare.access_point_group;
            let removed = LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                slot,
                permission,
            }));
            let terminals = match first {
                First::Configured => [configure, removed],
                First::KeyRemoved => [removed, configure],
            };
            for terminal in terminals {
                assert!(
                    returns_end
                        .try_send(Returned::Terminal {
                            terminal,
                            grouped: false,
                        })
                        .is_ok()
                );
                assert!(from_actor_end.try_send(FromActor::Returned).is_ok());
            }
            let mut completed = None;
            for _ in 0..8 {
                if let Poll::Ready(done) = run.as_mut().poll(&mut cx) {
                    completed = Some(done);
                    break;
                }
            }
            completed.expect("a broken contract ends the lifecycle")
        };
        let mut left = Vec::new();
        while let Some(returned) = lifecycle_ends.returns.try_receive() {
            left.push(returned);
        }
        let announced = lifecycle_ends.from_actor.try_receive().is_some();
        drop(port);
        check(completed, left, announced);
    }));
}

/// An access point's configuration terminal, which no station request
/// reserved, ends the lifecycle on its contract: the retired lifecycle
/// keeps that owner, and the terminal after it stays in its queue with its
/// announcement.
#[test]
fn an_unsolicited_access_point_terminal_ends_the_lifecycle_and_keeps_its_owner() {
    alone(First::Configured, |completed, left, announced| {
        assert_eq!(
            completed.end,
            LifecycleEnd::Contract(LifecycleViolation::Returned(
                BackendContractViolation::UnexpectedAccessPointTerminal
            ))
        );
        assert!(completed.end.is_terminal());
        assert!(matches!(
            completed.retired.rejected,
            Some(Rejected::Terminal(LowerMacTerminal::Configured(
                Configured::AccessPoint(_)
            )))
        ));
        assert!(matches!(
            left.as_slice(),
            [Returned::Terminal {
                terminal: LowerMacTerminal::KeyRemoved(Removed::AccessPoint(_)),
                ..
            }]
        ));
        assert!(announced, "the later owner's announcement waits too");
    });
}

/// An access point's key removal no request asked for ends the lifecycle
/// on its contract, the removed slot and permission kept with the retired
/// lifecycle.
#[test]
fn an_unsolicited_access_point_key_removal_ends_the_lifecycle_and_keeps_its_owners() {
    alone(First::KeyRemoved, |completed, left, announced| {
        assert_eq!(
            completed.end,
            LifecycleEnd::Contract(LifecycleViolation::Returned(
                BackendContractViolation::UnexpectedAccessPointTerminal
            ))
        );
        assert!(matches!(
            completed.retired.rejected,
            Some(Rejected::Terminal(LowerMacTerminal::KeyRemoved(
                Removed::AccessPoint(RemovedKey::Group { .. })
            )))
        ));
        assert!(matches!(
            left.as_slice(),
            [Returned::Terminal {
                terminal: LowerMacTerminal::Configured(Configured::AccessPoint(_)),
                ..
            }]
        ));
        assert!(announced);
    });
}

/// A closed reception of the actor is its own refusal, never a full queue:
/// the lifecycle ends on it at once, retries nothing, and keeps the message
/// it could not send whole.
#[test]
fn a_closed_actor_reception_ends_the_lifecycle_named_and_keeps_its_message() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::new());
        let mut installations = Installations::claim(claim);
        let (install, _air) = ModelPort::install(
            &mut installations,
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(BSSID).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let (parts, _storage, _spare) = lifecycle_spare::<2>(open(), resources, commands);
        let mut to_queue = ClosingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = ClosingSpscQueue::<Parcel<'_, P<'_>>, ACTOR_PARCELS>::new();
        let mut from_queue = ClosingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = ClosingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = ClosingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            ClosingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = ClosingSpscQueue::<LinkReturn, 1>::new();
        let mut command_queue =
            WakingSpscQueue::<crate::station::ServiceCommand<ModelCaps24>, COMMANDS>::new();
        let (to_actor, mut to_actor_end) = to_queue.split();
        let (parcels, _parcels_end) = parcel_queue.split();
        let (_from_actor_end, from_actor) = from_queue.split();
        let (_returns_end, returns) = return_queue.split();
        let (link, _link_end) = link_queue.split();
        let (_groups_end, groups) = group_queue.split();
        let (_link_return_end, link_return) = link_return_queue.split();
        let (application, commands) = command_queue.split();
        // The actor's side closed its reception before the lifecycle ran.
        embassy_futures::block_on(to_actor_end.close());
        let state = State::new();
        let (application, control) = Control::new(application, &state).unwrap();
        let mut lifecycle_ends = ActorEnds {
            to_actor,
            parcels,
            from_actor,
            returns,
            groups,
            link,
            link_return,
        };
        let mut output = application.events(|_| {});
        let mut io = LifecycleIo {
            commands,
            output: &mut output,
            control,
            stop: StopPolicy {
                drain: Duration::from_secs(1),
            },
        };
        let mut cx = Context::from_waker(Waker::noop());
        let completed = {
            let run = parts.run(&mut lifecycle_ends, &mut io, &clock, Observed::default());
            let mut run = core::pin::pin!(run);
            match run.as_mut().poll(&mut cx) {
                Poll::Ready(done) => done,
                Poll::Pending => panic!("a closed reception ends the lifecycle at once"),
            }
        };
        assert_eq!(
            completed.end,
            LifecycleEnd::Contract(LifecycleViolation::ActorClosed)
        );
        let mut unsent = completed.retired.unsent;
        assert!(matches!(
            unsent.pop_front(),
            Some(Unsent::ToActor(ToActor::Start(_)))
        ));
        assert!(unsent.is_empty());
        assert!(to_actor_end.try_receive().is_none());
        drop(port);
    }));
}
