use super::*;
use oer_radio_port::{Abandon, PortEnd};

struct Witness(Rc<Cell<usize>>);
impl Drop for Witness {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct Contexts;
impl ModelContext for Contexts {
    type Body = ModelBody;
    type Context<'id> = Witness;
    type StationConfigureContext<'id> = Witness;
    type AccessPointConfigureContext<'id> = ();
}
type Backend = ModelBackend<ModelCaps, Contexts>;

#[derive(Clone, Copy)]
enum End {
    Drained,
    SubmissionFailed,
    Quarantined,
}
struct Body {
    end: End,
    posted: bool,
    drops: Rc<Cell<usize>>,
}
impl PortBody<Backend> for Body {
    type Output = ();
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, Backend, ()> {
        let mut owner = owner.enable().await.ok().unwrap();
        let ModelResources {
            mut tickets,
            mut small,
            mut terminal_access,
            station_commands,
            gate,
            ..
        } = resources;
        let air = owner.phase().held.air.clone();
        air.hold(WmmAccessCategory::BestEffort, true);
        let ticket = tickets.remove(0);
        let ticket_id = ticket.index();
        {
            let mut phase = owner.phase();
            phase.station().configure(
                station_commands
                    .configure
                    .with_context(Witness(self.drops.clone())),
                StationReceive::Off,
            );
            if matches!(self.end, End::SubmissionFailed) {
                air.fail_after_terminals();
            }
            let source = TxSource::Fresh {
                buffer: small.remove(0),
                frame: ClassFrame::new(data_frame(&phase, false)).ok().unwrap(),
                body: None,
                protection: TxProtection::Open,
            }
            .check()
            .ok()
            .unwrap();
            let source = <ModelTypes<Contexts> as TxBufferPool<ModelSmall>>::prepare::<
                Station,
                ModelCaps,
            >(source)
            .ok()
            .unwrap();
            let submitted = phase.station().submit(
                (oer_ieee80211_lower_mac::Attempt {
                    context: Witness(self.drops.clone()),
                    ticket,
                    source,
                    access_category: WmmAccessCategory::BestEffort,
                    protection: Protection::None,
                    power: SupportedPower::new(TxPower::Calibrated).unwrap(),
                    coex: SupportedCoex::new(CoexPriority::Normal).unwrap(),
                })
                .admit(),
            );
            assert_eq!(
                submitted.is_err(),
                matches!(self.end, End::SubmissionFailed)
            );
            phase.close_gate(gate);
        }
        if self.posted {
            yield_now().await;
        }
        assert_eq!(self.drops.get(), 0, "the backend still owns both contexts");
        match self.end {
            End::Drained | End::SubmissionFailed => air.fail_after_terminals(),
            End::Quarantined => air.poison(),
        }
        let mut returned = Vec::new();
        loop {
            // Every poll uses a new phase loan; the end belongs to the install.
            match owner.phase().next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(terminal))) => match terminal {
                    LowerMacTerminal::Configured(Configured::Station(permission)) => {
                        let (_permission, witness) = permission.into_parts();
                        drop(witness);
                        returned.push("configure");
                    }
                    LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
                        let parts = terminal.into_parts(&mut terminal_access);
                        assert_eq!(parts.ticket.index(), ticket_id);
                        assert!(matches!(parts.outcome, TxOutcome::Withdrawn(_)));
                        drop(parts);
                        returned.push("tx");
                    }
                    LowerMacTerminal::GateClosed(_) => returned.push("gate"),
                    _ => panic!("unexpected terminal"),
                },
                Err(end) => {
                    assert_eq!(
                        matches!(end, EventEnd::Quarantined { .. }),
                        matches!(self.end, End::Quarantined)
                    );
                    // A later failure cannot replace the final variant/cause.
                    air.poison();
                    for _ in 0..2 {
                        assert!(
                            matches!(owner.phase().next_event().await, Err(again) if again == end)
                        );
                    }
                    break;
                }
                _ => panic!("no RX or lost event in this fixture"),
            }
        }
        match self.end {
            End::Drained | End::SubmissionFailed => {
                assert_eq!(returned, ["configure", "tx", "gate"]);
                assert_eq!(self.drops.get(), 2);
                let disabled = owner
                    .disable(&mut |_| panic!("Drained returned every terminal"))
                    .await;
                let Err(poisoned) = disabled else {
                    panic!("the fault persists")
                };
                Ended::uninstall(poisoned, (), ())
            }
            End::Quarantined => {
                assert!(returned.is_empty());
                assert_eq!(self.drops.get(), 0);
                Ended::fail_stopped(owner.fail_stop(), ())
            }
        }
    }
}

#[test]
fn event_end_returns_every_ordinary_terminal_or_retains_every_quarantined_owner() {
    for end in [End::Drained, End::SubmissionFailed, End::Quarantined] {
        for posted in [false, true] {
            let (backend, air) = Backend::new(
                Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
                [
                    IndividualAddress::new(STATION).unwrap(),
                    IndividualAddress::new(ACCESS_POINT).unwrap(),
                ],
            );
            let drops = Rc::new(Cell::new(0));
            let ended = block_on(run_port(
                backend,
                Body {
                    end,
                    posted,
                    drops: drops.clone(),
                },
            ));
            match end {
                End::Drained | End::SubmissionFailed => {
                    assert!(matches!(ended, PortEnd::Uninstalled { .. }));
                    assert_eq!(air.disable_calls(), 1);
                    assert_eq!(drops.get(), 2);
                }
                End::Quarantined => {
                    assert!(matches!(
                        ended,
                        PortEnd::FailStopped {
                            outcome: Abandon::Stopped(_),
                            ..
                        }
                    ));
                    drop(ended);
                    assert_eq!(air.disable_calls(), 0);
                    assert_eq!(
                        drops.get(),
                        0,
                        "dropping the quarantine receipt releases no owners"
                    );
                }
            }
        }
    }
}
