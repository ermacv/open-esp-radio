use super::*;
use oer_ieee80211_lower_mac::{Configured, SampledTsfPermission, VifRole};

enum ContractCase {
    Unsolicited,
    Disable { prior: bool, overflow: bool },
}

struct ContractBody(ContractCase);

impl PortBody<ModelBackend<ModelCaps24, ServiceContexts>> for ContractBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, ModelBackend<ModelCaps24, ServiceContexts>, Disabled>,
        resources: ModelResources<'id>,
    ) -> Ended<'id, ModelBackend<ModelCaps24, ServiceContexts>, ()> {
        let ModelResources {
            connection_epoch,
            station_commands,
            access_point_commands,
            terminal_access,
            tickets,
            small,
            large,
            station_association,
            access_point_association,
            station_pairwise,
            station_group,
            station_group_permission,
            access_point_pairwise,
            access_point_group,
            access_point_group_permission,
            gate,
            arbiter_authority,
            ..
        } = resources;
        let (arbiter, place, absence) = PortArbiter::<ModelBackend<ModelCaps24, ServiceContexts>>::new(
            ArbiterPolicy::DEFAULT.checked(<ModelBackend<ModelCaps24, ServiceContexts> as LowerMacBackend>::PUBLICATION_BOUND).unwrap(),
            SupportedChannel::new(channel6()).unwrap(), arbiter_authority, gate,
        );
        let mut tickets = tickets.into_iter();
        let station_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
        let data_tickets: [_; 4] = core::array::from_fn(|_| tickets.next().unwrap());
        let station = Station::new_open(
            config(StaSecurity::Open).0,
            StationResources::<_, 4, 4, 1> {
                configure: station_commands.configure,
                epochs: oer_ieee80211_sta::epoch::LinkEpochSource::new(connection_epoch),
                tickets: station_tickets,
                buffers: <[_; 4]>::try_from(small).ok().unwrap(),
                place,
                absence_place: absence,
                keys: oer_ieee80211_lower_mac::association::reunite_vacant(
                    station_association,
                    oer_ieee80211_lower_mac::association::VacantKeyBank::new(
                        station_pairwise,
                        station_group,
                    ),
                ),
                group_permission: station_group_permission,
            },
        );
        let mut service = StationService::<ModelBackend<ModelCaps24, ServiceContexts>, _, 1>::new(
            station,
            StaData::new(
                data_tickets,
                <[_; 4]>::try_from(large).ok().unwrap(),
                MODEL_TX_QUEUE_DEPTH,
            ),
            arbiter,
            EspressifRateSelection::<ModelCaps24>::new(),
            terminal_access,
        );
        let access_point_association =
            <ModelTypes<ServiceContexts> as PhaseTypes>::AccessPointKeys::begin_association(
                access_point_association,
                oer_ieee80211_mac::security::Spp::NotNegotiated,
            );
        let [ap_pairwise, _] = access_point_pairwise;
        let [ap_group, _] = access_point_group;
        if let ContractCase::Disable { prior, overflow } = self.0 {
            let mut owner = owner.enable().await.ok().unwrap();
            let (bank, places) = service
                .state
                .station
                .finish(oer_ieee80211_sta::machine::StaDisconnectReason::Requested);
            let RetiredArbiter::Stopped(stopped) = service.arbiter.finish(ArbiterEnd::Stopped)
            else {
                panic!()
            };
            let (arbiter, places) = stopped
                .restart(SupportedChannel::new(channel6()).unwrap(), places)
                .ok()
                .unwrap()
                .into_parts();
            service.arbiter = arbiter;
            service.state.station = bank.restart(places).into_parts().0;
            let Some(StaAction::Configure(receive, pending)) =
                service.state.station.poll_action(Instant::EPOCH)
            else {
                panic!("the restart reclaims receive configuration")
            };
            let first = LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                slot: ap_pairwise,
                association: access_point_association,
            }));
            if overflow {
                CUSTODY_LIMIT.set(Some(1));
            }
            let custody = if prior {
                service
                    .state
                    .terminal(&mut service.arbiter, Instant::EPOCH, first)
                    .err()
                    .unwrap()
                    .into()
            } else {
                owner.phase().inject_terminal(first);
                TerminalCustody::Clear
            };
            {
                let mut phase = owner.phase();
                phase.inject_terminal(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(
                    RemovedKey::Group {
                        slot: ap_group,
                        permission: access_point_group_permission,
                    },
                )));
                phase.station().configure(pending, receive);
            }
            let (ended, custody) = service
                .state
                .disable(
                    &mut service.arbiter,
                    owner,
                    &VirtualClock::<1>::new(),
                    custody,
                )
                .await;
            CUSTODY_LIMIT.set(None);
            assert!(service.state.station.is_reclaiming());
            let end = end_disabled(ended, custody);
            let (owner, custody) = match end {
                ServiceEnd::Failed {
                    owner,
                    fault: ServiceFault::Contract(reason),
                    custody,
                } if !overflow => {
                    assert_eq!(
                        reason,
                        BackendContractViolation::UnexpectedAccessPointTerminal
                    );
                    (Ok(owner), custody)
                }
                ServiceEnd::Poisoned {
                    owner,
                    fault: ServiceFault::Contract(reason),
                    custody,
                } if overflow => {
                    assert_eq!(
                        reason,
                        BackendContractViolation::UnexpectedAccessPointTerminal
                    );
                    (Err(owner), custody)
                }
                _ => panic!("the first violation and its complete bank must be returned"),
            };
            let TerminalCustody::Quarantined(bank) = custody else {
                panic!()
            };
            let (reason, first, mut later) = bank.into_parts();
            assert_eq!(
                reason,
                BackendContractViolation::UnexpectedAccessPointTerminal
            );
            let RejectedTerminal::Sealed(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(
                RemovedKey::Pairwise { slot, .. },
            ))) = first
            else {
                panic!("the first owner was discarded")
            };
            assert_eq!(slot.index(), 0);
            if overflow {
                assert!(matches!(
                    later.pop(),
                    Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(
                        RemovedKey::Group { .. }
                    )))
                ));
                assert!(later.pop().is_none());
            } else {
                let Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                    slot,
                    permission,
                }))) = later.pop()
                else {
                    panic!("later key owners were discarded")
                };
                assert_eq!(slot.index(), 0);
                assert_eq!(permission.index(), 0);
                assert!(matches!(
                    later.pop(),
                    Some(LowerMacTerminal::Configured(Configured::Station(_)))
                ));
                assert!(later.pop().is_none());
            }
            return match owner {
                Ok(owner) => owner.uninstall((), ()),
                Err(owner) => Ended::uninstall(owner, (), ()),
            };
        }
        for (terminal, expected) in [
            (
                LowerMacTerminal::TsfSampled {
                    permission: SampledTsfPermission::Station(station_commands.tsf),
                    sample: oer_ieee80211_lower_mac::TsfSample {
                        tsf: oer_ieee80211_lower_mac::VifTsf::new(
                            VifRole::Station,
                            oer_ieee80211_mac::tsf::TsfInstant::from_micros(0),
                        ),
                        local: oer_ieee80211_lower_mac::Ieee80211Stamp {
                            at: oer_ieee80211_lower_mac::Ieee80211Instant::from_micros(0),
                            generation: oer_radio_port::GenerationSource::new(1).current(),
                        },
                        uncertainty: oer_ieee80211_lower_mac::Ieee80211Duration::from_micros(0),
                        generation: oer_ieee80211_lower_mac::TsfRelation::new(
                            1,
                            oer_ieee80211_mac::tsf::TsfDuration::from_micros(0),
                        )
                        .generation(),
                    },
                },
                BackendContractViolation::UnexpectedTsfSample,
            ),
            (
                LowerMacTerminal::Configured(Configured::AccessPoint(
                    access_point_commands.configure,
                )),
                BackendContractViolation::UnexpectedAccessPointTerminal,
            ),
            (
                LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                    slot: ap_pairwise,
                    association: access_point_association,
                })),
                BackendContractViolation::UnexpectedAccessPointTerminal,
            ),
            (
                LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                    slot: ap_group,
                    permission: access_point_group_permission,
                })),
                BackendContractViolation::UnexpectedAccessPointTerminal,
            ),
        ] {
            assert_eq!(
                service
                    .state
                    .terminal(&mut service.arbiter, Instant::EPOCH, terminal)
                    .map_err(|fault| fault.reason),
                Err(expected)
            );
        }
        assert_eq!(service.state.station.lost_tokens(), 0);
        Ended::uninstall(owner, (), ())
    }
}

#[test]
fn unsolicited_tsf_and_access_point_terminals_retain_their_real_permissions() {
    let (backend, _) = ModelBackend::<ModelCaps24, ServiceContexts>::new(
        channel6(),
        [
            IndividualAddress::new(LOCAL).unwrap(),
            IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        ],
    );
    assert!(matches!(
        block_on(run_port(backend, ContractBody(ContractCase::Unsolicited))),
        oer_radio_port::PortEnd::Uninstalled { .. }
    ));
}

#[test]
fn disable_quarantines_the_first_rejected_owner_and_every_later_terminal() {
    for prior in [false, true] {
        let (backend, _) = ModelBackend::<ModelCaps24, ServiceContexts>::new(
            channel6(),
            [
                IndividualAddress::new(LOCAL).unwrap(),
                IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
            ],
        );
        assert!(matches!(
            block_on(run_port(
                backend,
                ContractBody(ContractCase::Disable {
                    prior,
                    overflow: false
                })
            )),
            oer_radio_port::PortEnd::Uninstalled { .. }
        ));
    }
}

#[test]
fn full_custody_returns_the_next_terminal_to_poisoned_port_teardown() {
    let (backend, _) = ModelBackend::<ModelCaps24, ServiceContexts>::new(
        channel6(),
        [
            IndividualAddress::new(LOCAL).unwrap(),
            IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap(),
        ],
    );
    assert!(matches!(
        block_on(run_port(
            backend,
            ContractBody(ContractCase::Disable {
                prior: false,
                overflow: true
            })
        )),
        oer_radio_port::PortEnd::Uninstalled { .. }
    ));
}
