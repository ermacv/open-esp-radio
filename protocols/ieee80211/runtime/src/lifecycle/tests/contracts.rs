//! The contracts of what the actor returns, the test playing the actor: an
//! owner of no operation the lifecycle sent ends it with its typed cause,
//! the first such owner stays with the retired lifecycle and later ones
//! stay in their queue; a stop that proved nothing ends it unconfirmed.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    Configured, LowerMacTerminal, SampledTsfPermission, VifRole,
};

/// What the test, as the actor, sends the lifecycle first.
#[derive(Clone, Copy)]
enum First {
    /// The access point's configuration terminal.
    AccessPoint,
    /// A TSF sample of the station's interface.
    Tsf,
}

/// A sample of the station's TSF for `permission`.
fn tsf_sample<'s>(
    permission: oer_protocols_ieee80211_contracts_port::TsfPermission<
        's,
        oer_protocols_ieee80211_contracts_port::Station,
        <Types<'s> as PhaseTypes<'s>>::StationCommandPath,
    >,
) -> LowerMacTerminal<'s, Types<'s>> {
    LowerMacTerminal::TsfSampled {
        permission: SampledTsfPermission::Station(permission),
        sample: oer_protocols_ieee80211_contracts_port::TsfSample {
            tsf: oer_protocols_ieee80211_contracts_port::VifTsf::new(
                VifRole::Station,
                oer_protocols_ieee80211_contracts_port::tsf::TsfInstant::from_micros(0),
            ),
            local: oer_protocols_ieee80211_contracts_port::Ieee80211Stamp {
                at: oer_protocols_ieee80211_contracts_port::Ieee80211Instant::from_micros(0),
                generation: oer_protocols_radio_port::GenerationSource::new(1).current(),
            },
            uncertainty: oer_protocols_ieee80211_contracts_port::Ieee80211Duration::from_micros(0),
            generation: oer_protocols_radio_port::GenerationSource::new(1).current(),
        },
    }
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
        let mut to_queue = WakingSpscQueue::<ToActor<'_, P<'_>>, TO>::new();
        let mut parcel_queue = WakingSpscQueue::<Parcel<'_, P<'_>>, ACTOR_PARCELS>::new();
        let mut from_queue = WakingSpscQueue::<FromActor<'_, P<'_>>, FROM>::new();
        let mut return_queue = WakingSpscQueue::<Returned<'_, P<'_>>, RETURNS>::new();
        let mut link_queue = WakingSpscQueue::<ActorLink<'_, P<'_>>, 1>::new();
        let mut group_queue =
            WakingSpscQueue::<GroupPending<'_, ActorEpochPath<'_, P<'_>>>, 1>::new();
        let mut link_return_queue = WakingSpscQueue::<LinkReturn, 1>::new();
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
            let tsf = tsf_sample(spare.station_tsf);
            let terminals = match first {
                First::AccessPoint => [configure, tsf],
                First::Tsf => [tsf, configure],
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
    alone(First::AccessPoint, |completed, left, announced| {
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
                terminal: LowerMacTerminal::TsfSampled { .. },
                ..
            }]
        ));
        assert!(announced, "the later owner's announcement waits too");
    });
}

/// A TSF sample no request asked for ends the lifecycle on its contract,
/// the sample's permission kept with the retired lifecycle.
#[test]
fn an_unsolicited_tsf_sample_ends_the_lifecycle_and_keeps_its_permission() {
    alone(First::Tsf, |completed, left, announced| {
        assert_eq!(
            completed.end,
            LifecycleEnd::Contract(LifecycleViolation::Returned(
                BackendContractViolation::UnexpectedTsfSample
            ))
        );
        assert!(matches!(
            completed.retired.rejected,
            Some(Rejected::Terminal(LowerMacTerminal::TsfSampled {
                permission: SampledTsfPermission::Station(_),
                ..
            }))
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
