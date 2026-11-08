//! The access-point role on the model's one port, through the public
//! contract: its admission beside the station's, its keys and beacon on
//! their own permissions, its stop beside an active station, and the shared
//! stop. Each case states the owners it expects back, apart from how the
//! model keeps them.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCommand, AccessPointCommands, AccessPointPort, AccessPointProgress,
    AccessPointReceive, AttemptKey, BeaconAttempt, BeaconPermission, BeaconRefusal, BeaconStamp,
    BeaconTimestamp, Configured, CurrentBeacon, GroupAddress, InstalledGroup, InstalledPairwise,
    NumberedKey, ReceiveFilter, Removed, RemovedKey, SingleEnd, SupportedFilter,
    SupportedTsfControl, TsfControl,
};
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;

/// The beacon's queue, from the contract's own policy.
const BEACON_QUEUE: WmmAccessCategory =
    oer_protocols_ieee80211_contracts_port::BEACON_ACCESS_CATEGORY;
/// The queue of the access point's data in these cases, apart from the
/// station's.
const AP_QUEUE: WmmAccessCategory = WmmAccessCategory::Video;

type ApCommand<'s> = AccessPointCommand<'s, Types<'s>, ModelCaps>;
type ApProgress<'s> = AccessPointProgress<'s, Types<'s>, ModelCaps>;
type ApPairwise<'s> = ModelPairwiseKey<'s, B<'s>, AccessPoint>;
type StaPairwise<'s> = ModelPairwiseKey<'s, B<'s>, Station>;
type Beacon<'s> = BeaconPermission<'s, ModelBeaconPath<B<'s>>>;
type Admitted<'s> = ModelAdmitted<LocalId<'s>, ModelCaps, ()>;

/// How a single MPDU's terminal ended, as these cases tell them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum End {
    Delivered,
    Withdrawn(WithdrawCause),
    Other,
}

/// One transmission terminal the sink held, its ticket and buffer back with
/// the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ended {
    /// A single MPDU of the ticket.
    Frame(u8, End),
    /// The beacon of the ticket; its permission is back too.
    Beacon(u8, End),
}

/// Take the transmission terminals out of the sink, returning tickets and
/// buffers to the harness and a beacon's permission to `beacon`. Every
/// other terminal stays in the sink, in its order.
fn settle<'s>(harness: &mut Harness<'s>, beacon: &mut Option<Beacon<'s>>) -> Vec<Ended> {
    let terminals = core::mem::take(&mut harness.sink.terminals);
    let mut ended = Vec::new();
    for terminal in terminals {
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                let (ticket, end) = give_back(harness, completed);
                ended.push(Ended::Frame(ticket, end));
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed,
                permission,
            }) => {
                assert!(
                    beacon.replace(permission).is_none(),
                    "one beacon permission per installation"
                );
                let (ticket, end) = give_back(harness, completed);
                ended.push(Ended::Beacon(ticket, end));
            }
            other => harness.sink.terminals.push(other),
        }
    }
    ended
}

fn give_back<'s>(
    harness: &mut Harness<'s>,
    completed: oer_protocols_ieee80211_contracts_port::SingleCompleted<'s, Types<'s>>,
) -> (u8, End) {
    let TerminalParts {
        ticket,
        payload,
        outcome,
        ..
    } = completed.into_parts(&mut harness.terminal_access);
    let index = ticket.index();
    harness.tickets.push(ticket);
    let (buffer, _) = payload.into_parts();
    let mut buffer = match buffer {
        ModelAnyBuffer::Small(buffer) => buffer,
        ModelAnyBuffer::SmallSent(sent) => {
            <Types<'s> as TxBufferPool<'s, ModelSmall>>::renew(sent).0
        }
        _ => panic!("these cases submit small buffers"),
    };
    buffer.renew_msdu().unwrap();
    harness.small.push(buffer);
    let end = match outcome {
        TxOutcome::Ended(SingleEnd::Delivered { .. }) => End::Delivered,
        TxOutcome::Withdrawn(cause) => End::Withdrawn(cause),
        _ => End::Other,
    };
    (index, end)
}

/// The access point's unicast data frame to the peer, under `key` when
/// given, on `category`.
fn access_point_frame<'s>(
    harness: &mut Harness<'s>,
    key: Option<(&ApPairwise<'s>, u64)>,
    category: WmmAccessCategory,
) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(if key.is_some() { 0x4208 } else { 0x0208 }).unwrap(),
        0,
        IndividualAddress::new(PEER).unwrap(),
        ACCESS_POINT,
        ACCESS_POINT,
        0,
        None,
        None,
        None,
    )
    .and_then(AckHeader::new)
    .unwrap();
    let frame = TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    };
    let frame = ClassFrame::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: match key {
                Some((key, number)) => TxProtection::Ccmp(NumberedKey {
                    key: AttemptKey::Pairwise(key),
                    packet_number: CcmpPacketNumber::new(number).unwrap(),
                }),
                None => TxProtection::Open,
            },
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let attempt = Attempt {
        context: (),
        ticket,
        source,
        access_category: category,
        protection: Protection::None,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
    }
    .admit();
    harness.port.admit_access_point(attempt)
}

/// The access point's broadcast Beacon under its current permission.
fn beacon<'s>(harness: &mut Harness<'s>, permission: Beacon<'s>) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let current: CurrentBeacon<'_, 's, _> = harness.port.current_beacon(permission).unwrap();
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
        FrameControl::new(0x0080).unwrap(),
        0,
        GroupAddress::new([0xff; 6]).unwrap(),
        ACCESS_POINT,
        ACCESS_POINT,
        0,
        None,
        None,
        None,
    )
    .unwrap();
    let frame = TxFrame::Group {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        rate,
    };
    let frame = ClassFrame::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let sealed = BeaconAttempt {
        context: (),
        ticket,
        source,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
        permission: current,
        stamp: BeaconStamp::new(BeaconTimestamp::Caller).unwrap(),
    }
    .admit();
    harness.port.admit_beacon(sealed)
}

fn submit<'s>(harness: &mut Harness<'s>, attempt: Admitted<'s>) -> u8 {
    let ticket = attempt.operation.ticket().index();
    let now = harness.clock.now();
    harness
        .port
        .submit(attempt, now, &mut harness.sink)
        .ok()
        .unwrap();
    ticket
}

fn ap_command<'s>(harness: &mut Harness<'s>, command: ApCommand<'s>) -> ApProgress<'s> {
    let now = harness.clock.now();
    harness
        .port
        .access_point_command(command, now, &mut harness.sink)
        .unwrap()
}

fn ap_configure<'s>(
    permission: ConfigurePermission<
        's,
        AccessPoint,
        oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 2>,
    >,
    receive: AccessPointReceive<ModelCaps>,
) -> ApCommand<'s> {
    AccessPointCommand::Configure {
        permission,
        receive,
    }
}

fn bss() -> AccessPointReceive<ModelCaps> {
    AccessPointReceive::Bss {
        filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap(),
    }
}

fn tsf<'s>(control: TsfControl) -> ApCommand<'s> {
    AccessPointCommand::Tsf(SupportedTsfControl::new(control).unwrap())
}

/// Take the access point's configuration terminal from the sink.
fn take_ap_configured<'s>(
    harness: &mut Harness<'s>,
) -> Option<
    ConfigurePermission<
        's,
        AccessPoint,
        oer_protocols_radio_port::Child<ModelPool<LocalId<'s>, 9>, 2>,
    >,
> {
    let position = harness.sink.terminals.iter().position(|terminal| {
        matches!(
            terminal,
            LowerMacTerminal::Configured(Configured::AccessPoint(_))
        )
    })?;
    match harness.sink.terminals.remove(position) {
        LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => Some(permission),
        _ => None,
    }
}

/// Install the station's pairwise key and take it from its terminal.
fn station_key<'s>(
    harness: &mut Harness<'s>,
) -> (ModelAssociation<'s, B<'s>, Station>, StaPairwise<'s>) {
    let (start, _, _, [slot, _]) = harness.group.take().unwrap();
    let association = ModelKeys::<B<'s>, Station>::begin_association(
        start,
        oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
    );
    let (install, _) = harness.commands.install_pairwise(
        &association,
        slot,
        IndividualAddress::new(PEER).unwrap(),
        Ccmp128Key::new(&[5; 16]),
    );
    assert!(matches!(harness.command(install), CommandProgress::Done));
    let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key))) =
        harness.sink.terminals.pop()
    else {
        panic!("the station key's terminal");
    };
    (association, key)
}

/// An unprotected station data frame on `category`.
fn station_frame_on<'s>(harness: &mut Harness<'s>, category: WmmAccessCategory) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let mut buffer = harness.small.pop().unwrap();
    buffer.bytes_mut().unwrap().fill(0);
    let rate = harness
        .port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .unwrap();
    let header = HeaderSpec::new(
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
    .and_then(AckHeader::new)
    .unwrap();
    let frame = TxFrame::Individual {
        frame: SizedFrame::new(header, MpduLength::new(40).unwrap()).unwrap(),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        }),
    };
    let frame = ClassFrame::new(frame).ok().unwrap();
    let source = prepare_model(
        TxSource::Fresh {
            buffer,
            frame,
            body: None,
            protection: TxProtection::Open,
        }
        .check()
        .ok()
        .unwrap(),
    )
    .ok()
    .unwrap();
    let attempt = Attempt {
        context: (),
        ticket,
        source,
        access_category: category,
        protection: Protection::None,
        power: SupportedPower::new(TxPower::Calibrated).unwrap(),
        coex_operation: TxOperation::Data,
    }
    .admit();
    harness.port.admit(attempt)
}

/// A station data frame under `key`, on the station's queue.
fn station_frame<'s>(
    harness: &mut Harness<'s>,
    key: &StaPairwise<'s>,
    number: u64,
) -> Admitted<'s> {
    let ticket = harness.tickets.pop().unwrap();
    let buffer = harness.small.pop().unwrap();
    harness.admitted_under_number(ticket, buffer, key, number)
}

#[test]
fn an_access_point_frame_goes_out_as_the_access_points_on_the_shared_port() {
    with_harness(|harness| {
        harness.start();
        let attempt = access_point_frame(harness, None, AP_QUEUE);
        let ticket = submit(harness, attempt);
        harness.service();
        let sent = harness.air.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].role, VifRole::AccessPoint);
        assert_eq!(sent[0].ticket, Some(ticket));
        let mut beacon = None;
        assert_eq!(
            settle(harness, &mut beacon),
            [Ended::Frame(ticket, End::Delivered)]
        );
        assert_eq!(
            harness.port.access_point_address(),
            IndividualAddress::new(ACCESS_POINT).unwrap()
        );
        assert_eq!(
            harness.port.address(),
            IndividualAddress::new(STATION).unwrap()
        );
    });
}

#[test]
fn a_station_configuration_neither_withdraws_nor_waits_for_access_point_frames() {
    with_harness(|harness| {
        harness.start();
        // A station frame on the air holds the station's configuration; a
        // second station frame and an access-point frame wait in another
        // queue.
        harness.air.hold_completion(CATEGORY, true);
        let first = harness.mpdu().unwrap();
        let first = submit(harness, first);
        harness.service();
        let waiting = WmmAccessCategory::Background;
        let queued = station_frame_on(harness, waiting);
        let queued = submit(harness, queued);
        let ap = access_point_frame(harness, None, waiting);
        let ap = submit(harness, ap);
        let configure = harness.configure().unwrap();
        let CommandProgress::Held(configure) = harness.command(configure) else {
            panic!("the station frame on the air holds the station's configuration");
        };
        // The station's queued frame ends withdrawn; the access point's goes
        // out while the station's configuration is still held.
        let mut beacon = None;
        assert_eq!(
            settle(harness, &mut beacon),
            [
                Ended::Frame(queued, End::Withdrawn(WithdrawCause::GateClosed)),
                Ended::Frame(ap, End::Delivered),
            ]
        );
        harness.air.hold_completion(CATEGORY, false);
        harness.service();
        assert!(matches!(harness.command(configure), CommandProgress::Done));
        assert!(harness.take_configured());
        assert_eq!(
            settle(harness, &mut beacon),
            [Ended::Frame(first, End::Delivered)]
        );
    });
}

#[test]
fn an_access_point_frame_on_the_air_holds_only_the_access_points_configuration() {
    with_harness(|harness| {
        harness.start();
        let mut permissions = harness.access_point.take().unwrap();
        harness.air.hold_completion(AP_QUEUE, true);
        let ap = access_point_frame(harness, None, AP_QUEUE);
        let ap = submit(harness, ap);
        harness.service();
        // The station configures while the access point's frame is out.
        let configure = harness.configure().unwrap();
        assert!(matches!(harness.command(configure), CommandProgress::Done));
        assert!(harness.take_configured());
        let station = harness.mpdu().unwrap();
        let station = submit(harness, station);
        harness.service();
        let mut beacon = None;
        assert_eq!(
            settle(harness, &mut beacon),
            [Ended::Frame(station, End::Delivered)]
        );
        let command = ap_configure(permissions.commands.configure, bss());
        let AccessPointProgress::Held(command) = ap_command(harness, command) else {
            panic!("the access point's frame on the air holds its configuration");
        };
        assert!(take_ap_configured(harness).is_none());
        harness.air.hold_completion(AP_QUEUE, false);
        harness.service();
        assert_eq!(
            settle(harness, &mut beacon),
            [Ended::Frame(ap, End::Delivered)]
        );
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        permissions.commands.configure = take_ap_configured(harness).unwrap();
        assert_eq!(
            harness.air.access_point().receive,
            Some(ReceiveFilter::BSS_MEMBER)
        );
        harness.access_point = Some(permissions);
    });
}

#[test]
fn a_beacon_goes_out_on_its_queue_and_its_terminal_returns_the_permission() {
    with_harness(|harness| {
        harness.start();
        let mut permissions = harness.access_point.take().unwrap();
        // The beacon's queue is busy: the beacon waits while another
        // queue's frame goes.
        harness.air.hold(BEACON_QUEUE, true);
        let admitted = beacon(harness, permissions.beacon);
        let ticket = submit(harness, admitted);
        let other = access_point_frame(harness, None, AP_QUEUE);
        let other = submit(harness, other);
        harness.service();
        let mut returned = None;
        assert_eq!(
            settle(harness, &mut returned),
            [Ended::Frame(other, End::Delivered)]
        );
        assert!(returned.is_none(), "the permission stays with the beacon");
        harness.air.hold(BEACON_QUEUE, false);
        harness.service();
        assert_eq!(
            settle(harness, &mut returned),
            [Ended::Beacon(ticket, End::Delivered)]
        );
        let sent = harness.air.sent();
        let last = sent.last().unwrap();
        assert_eq!(last.role, VifRole::AccessPoint);
        assert_eq!(last.ticket, Some(ticket));
        // The returned permission admits the next beacon.
        permissions.beacon = returned.take().unwrap();
        let again = beacon(harness, permissions.beacon);
        let again = submit(harness, again);
        harness.service();
        assert_eq!(
            settle(harness, &mut returned),
            [Ended::Beacon(again, End::Delivered)]
        );
        permissions.beacon = returned.unwrap();
        harness.access_point = Some(permissions);
    });
}

#[test]
fn a_beacon_the_stopped_radio_refuses_returns_its_permission_at_once() {
    with_harness(|harness| {
        let permissions = harness.access_point.take().unwrap();
        let admitted = beacon(harness, permissions.beacon);
        let ticket = admitted.operation.ticket().index();
        let now = harness.clock.now();
        assert_eq!(
            harness.port.submit(admitted, now, &mut harness.sink),
            Err(SubmitRefusal::NotEnabled)
        );
        let mut returned = None;
        assert_eq!(
            settle(harness, &mut returned),
            [Ended::Beacon(
                ticket,
                End::Withdrawn(WithdrawCause::Disabled)
            )]
        );
        assert!(returned.is_some());
    });
}

#[test]
fn a_beacon_permission_of_an_earlier_installation_is_refused_before_a_beacon_is_built() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        let stale = harness.access_point.take().unwrap().beacon;
        let Harness {
            port,
            mut installations,
            ..
        } = harness;
        let Ok(ModelInstall {
            port, resources, ..
        }) = port.reinstall(&mut installations)
        else {
            panic!("a retired port reinstalls");
        };
        assert_eq!(port.generation(), 1);
        let Err(refused) = port.current_beacon(stale) else {
            panic!("an earlier installation's permission is refused");
        };
        assert_eq!(
            refused.reason,
            BeaconRefusal::StalePermission {
                permission: 0,
                installation: 1,
            }
        );
        assert_eq!(
            refused.permission.generation(),
            0,
            "the permission comes back"
        );
        let current = port
            .current_beacon(resources.access_point.beacon)
            .expect("the current installation's permission is checked");
        assert_eq!(current.into_permission().generation(), 1);
    }));
}

#[test]
fn the_access_points_keys_install_and_remove_on_its_own_permissions() {
    with_harness(|harness| {
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [pairwise_slot, spare_pairwise] = permissions.pairwise;
        let [group_slot, spare_group] = permissions.group;
        let (install, identity) = harness.commands.install_access_point_pairwise(
            &association,
            pairwise_slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        assert!(matches!(
            ap_command(harness, install),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(pairwise))) =
            harness.sink.terminals.pop()
        else {
            panic!("the access point's pairwise terminal");
        };
        assert!(ModelKeys::<B<'_>, AccessPoint>::pairwise_identity(&pairwise) == identity);
        let install = harness.commands.install_access_point_group(
            &association,
            permissions.group_permission,
            group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        assert!(matches!(
            ap_command(harness, install),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
            key: group,
            permission,
        })) = harness.sink.terminals.pop()
        else {
            panic!("the access point's group terminal");
        };
        // A protected frame under the access point's key goes out under it.
        let attempt = access_point_frame(harness, Some((&pairwise, 1)), AP_QUEUE);
        let ticket = submit(harness, attempt);
        harness.service();
        let sent = harness.air.sent();
        assert_eq!(sent.last().unwrap().key.map(|(_, live)| live), Some(true));
        let mut beacon = None;
        assert_eq!(
            settle(harness, &mut beacon),
            [Ended::Frame(ticket, End::Delivered)]
        );
        // Both removals return their slots, the group permission and the
        // association.
        assert!(matches!(
            ap_command(
                harness,
                AccessPointCommand::RemoveGroup {
                    permission,
                    key: group
                }
            ),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
            slot: group_slot,
            permission,
        }))) = harness.sink.terminals.pop()
        else {
            panic!("the group removal's terminal");
        };
        assert!(matches!(
            ap_command(
                harness,
                AccessPointCommand::RemovePairwise {
                    association,
                    key: pairwise
                }
            ),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
            slot: pairwise_slot,
            association,
        }))) = harness.sink.terminals.pop()
        else {
            panic!("the pairwise removal's terminal");
        };
        // The complete bank ends the association.
        let bank = oer_protocols_ieee80211_contracts_port::association::VacantKeyBank::new(
            [pairwise_slot, spare_pairwise],
            [group_slot, spare_group],
        );
        let _vacant = ModelKeys::<B<'_>, AccessPoint>::end_association(association, bank);
        drop(permission);
    });
}

#[test]
fn the_access_point_stops_alone_beside_an_active_station_with_its_frame_on_the_air() {
    with_harness(|harness| {
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        // The station runs: configured, its key installed, its frames out.
        let configure = harness.configure().unwrap();
        assert!(matches!(harness.command(configure), CommandProgress::Done));
        assert!(harness.take_configured());
        let (_station_association, station_key) = station_key(harness);
        // The access point runs: configured, TSF restarted, its keys in.
        let command = ap_configure(permissions.commands.configure, bss());
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        let ap_configure_permission = take_ap_configured(harness).unwrap();
        assert!(matches!(
            ap_command(harness, tsf(TsfControl::Restart)),
            AccessPointProgress::Done
        ));
        assert!(matches!(
            harness.air.access_point().tsf,
            ModelAccessPointTsf::Running { .. }
        ));
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [pairwise_slot, spare_pairwise] = permissions.pairwise;
        let [group_slot, spare_group] = permissions.group;
        let (install, _) = harness.commands.install_access_point_pairwise(
            &association,
            pairwise_slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        assert!(matches!(
            ap_command(harness, install),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(pairwise))) =
            harness.sink.terminals.pop()
        else {
            panic!("the access point's pairwise terminal");
        };
        let install = harness.commands.install_access_point_group(
            &association,
            permissions.group_permission,
            group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        assert!(matches!(
            ap_command(harness, install),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
            key: group,
            permission: group_permission,
        })) = harness.sink.terminals.pop()
        else {
            panic!("the access point's group terminal");
        };
        // An access-point frame under its key is on the air, a beacon
        // queued behind a busy beacon queue, a station frame on the air.
        harness.air.hold_completion(AP_QUEUE, true);
        let ap_frame = access_point_frame(harness, Some((&pairwise, 1)), AP_QUEUE);
        let ap_frame = submit(harness, ap_frame);
        harness.air.hold(BEACON_QUEUE, true);
        let queued_beacon = beacon(harness, permissions.beacon);
        let queued_beacon = submit(harness, queued_beacon);
        harness.air.hold_completion(CATEGORY, true);
        let station_on_air = station_frame(harness, &station_key, 1);
        let station_on_air = submit(harness, station_on_air);
        harness.service();
        let mut beacon_back = None;
        assert!(settle(harness, &mut beacon_back).is_empty());

        // The access point stops: its configuration Off waits for its frame
        // and ends its queued beacon at once, with the permission.
        let command = ap_configure(ap_configure_permission, AccessPointReceive::Off);
        let AccessPointProgress::Held(command) = ap_command(harness, command) else {
            panic!("the access point's frame on the air holds its stop");
        };
        assert_eq!(
            settle(harness, &mut beacon_back),
            [Ended::Beacon(
                queued_beacon,
                End::Withdrawn(WithdrawCause::GateClosed)
            )]
        );
        assert!(beacon_back.is_some(), "the beacon permission is back");
        // The station is not held by the access point's stop: its frame
        // ends, a new one goes, and its own command runs.
        harness.air.hold_completion(CATEGORY, false);
        harness.service();
        assert_eq!(
            settle(harness, &mut beacon_back),
            [Ended::Frame(station_on_air, End::Delivered)]
        );
        let station_next = station_frame(harness, &station_key, 2);
        let station_next = submit(harness, station_next);
        harness.service();
        assert_eq!(
            settle(harness, &mut beacon_back),
            [Ended::Frame(station_next, End::Delivered)]
        );
        // The access point's frame ends; its configuration completes.
        harness.air.hold_completion(AP_QUEUE, false);
        harness.service();
        assert_eq!(
            settle(harness, &mut beacon_back),
            [Ended::Frame(ap_frame, End::Delivered)]
        );
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        let ap_configure_permission = take_ap_configured(harness).unwrap();
        assert_eq!(harness.air.access_point().receive, None);
        // Its keys come off, its TSF stops.
        assert!(matches!(
            ap_command(
                harness,
                AccessPointCommand::RemoveGroup {
                    permission: group_permission,
                    key: group
                }
            ),
            AccessPointProgress::Done
        ));
        assert!(matches!(
            ap_command(
                harness,
                AccessPointCommand::RemovePairwise {
                    association,
                    key: pairwise
                }
            ),
            AccessPointProgress::Done
        ));
        assert!(matches!(
            ap_command(harness, tsf(TsfControl::Stop)),
            AccessPointProgress::Done
        ));
        assert_eq!(harness.air.access_point().tsf, ModelAccessPointTsf::Stopped);
        let mut removed = core::mem::take(&mut harness.sink.terminals).into_iter();
        let Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
            slot: group_slot,
            permission: group_permission,
        }))) = removed.next()
        else {
            panic!("the group removal's terminal");
        };
        let Some(LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
            slot: pairwise_slot,
            association,
        }))) = removed.next()
        else {
            panic!("the pairwise removal's terminal");
        };
        assert!(removed.next().is_none());
        // Every access-point owner is back: the complete bank ends its
        // association, the permissions are held again.
        let bank = oer_protocols_ieee80211_contracts_port::association::VacantKeyBank::new(
            [pairwise_slot, spare_pairwise],
            [group_slot, spare_group],
        );
        let _vacant = ModelKeys::<B<'_>, AccessPoint>::end_association(association, bank);
        drop((
            ap_configure_permission,
            beacon_back.unwrap(),
            group_permission,
        ));
        // The station keeps working on its key after the access point left.
        let station_after = station_frame(harness, &station_key, 3);
        let station_after = submit(harness, station_after);
        harness.service();
        let sent = harness.air.sent();
        let last = sent.last().unwrap();
        assert_eq!(last.role, VifRole::Station);
        assert_eq!(last.key.map(|(_, live)| live), Some(true));
        let mut none = None;
        assert_eq!(
            settle(harness, &mut none),
            [Ended::Frame(station_after, End::Delivered)]
        );
        assert_eq!(harness.tickets.len(), MODEL_TICKETS, "every ticket is back");
        assert_eq!(
            harness.small.len(),
            MODEL_SMALL_BUFFERS,
            "every buffer is back"
        );
    });
}

#[test]
fn an_access_point_command_held_at_its_barrier_holds_no_station_command() {
    with_harness(|harness| {
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [group_slot, _] = permissions.group;
        let install = harness.commands.install_access_point_group(
            &association,
            permissions.group_permission,
            group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        harness.air.hold_group_commands(true);
        let AccessPointProgress::Held(install) = ap_command(harness, install) else {
            panic!("held group commands hold the access point's installation");
        };
        // The station's command runs meanwhile.
        let configure = harness.configure().unwrap();
        assert!(matches!(harness.command(configure), CommandProgress::Done));
        assert!(harness.take_configured());
        // Past its bound, the held installation is the access point's own
        // named stall; the port keeps it.
        harness.advance(MODEL_HELD_COMMAND_BOUND);
        let now = harness.clock.now();
        let progress = harness
            .port
            .access_point_command(install, now, &mut harness.sink);
        assert!(matches!(
            progress,
            Err(ModelPortFault::AccessPointCommandStalled {
                kind: oer_protocols_ieee80211_contracts_port::AccessPointCommandKind::InstallGroup,
                ..
            })
        ));
    });
}

#[test]
fn a_stop_during_a_held_key_installation_returns_the_command_and_refuses_it_after() {
    with_harness(|harness| {
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [group_slot, _] = permissions.group;
        let install = harness.commands.install_access_point_group(
            &association,
            permissions.group_permission,
            group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        harness.air.hold_group_commands(true);
        let AccessPointProgress::Held(install) = ap_command(harness, install) else {
            panic!("held group commands hold the installation");
        };
        let waker = harness.waker();
        let now = harness.clock.now();
        assert!(matches!(
            harness
                .port
                .poll_stop(&mut Context::from_waker(&waker), now),
            Poll::Ready(Ok(()))
        ));
        // The stopped port refuses the installation with its owners; no
        // terminal was owed.
        let AccessPointProgress::Refused(install, CommandRefusal::NotEnabled) =
            ap_command(harness, install)
        else {
            panic!("a stopped port refuses the command with its owners");
        };
        assert!(harness.sink.terminals.is_empty());
        assert!(matches!(install, AccessPointCommand::InstallGroup(_)));
    });
}

#[test]
fn the_shared_stop_ends_the_beacon_and_the_access_points_interface() {
    with_harness(|harness| {
        harness.start();
        let mut permissions = harness.access_point.take().unwrap();
        let command = ap_configure(permissions.commands.configure, bss());
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        permissions.commands.configure = take_ap_configured(harness).unwrap();
        assert!(matches!(
            ap_command(harness, tsf(TsfControl::Restart)),
            AccessPointProgress::Done
        ));
        // A beacon on the air and a station frame queued behind a busy
        // queue when the shared owner stops.
        harness.air.hold_completion(BEACON_QUEUE, true);
        let on_air = beacon(harness, permissions.beacon);
        let on_air = submit(harness, on_air);
        harness.air.hold(CATEGORY, true);
        let station = harness.mpdu().unwrap();
        let station = submit(harness, station);
        harness.service();
        let waker = harness.waker();
        let mut cx = Context::from_waker(&waker);
        let now = harness.clock.now();
        assert!(harness.port.poll_stop(&mut cx, now).is_pending());
        harness.service();
        let mut returned = None;
        assert_eq!(
            settle(harness, &mut returned),
            [Ended::Frame(
                station,
                End::Withdrawn(WithdrawCause::Disabled)
            )]
        );
        // The beacon on the air ends within the publication bound.
        harness.advance(MODEL_PUBLICATION_BOUND);
        let now = harness.clock.now();
        let _ = harness.port.poll_stop(&mut cx, now);
        harness.service();
        let ended = settle(harness, &mut returned);
        assert!(matches!(ended.as_slice(), [Ended::Beacon(ticket, _)] if *ticket == on_air));
        assert!(matches!(
            harness.port.poll_stop(&mut cx, now),
            Poll::Ready(Ok(()))
        ));
        assert!(returned.is_some(), "the beacon permission is back");
        assert_eq!(
            harness.air.access_point(),
            ModelAccessPointInterface::default(),
            "the stopped MAC receives nothing for the access point and runs no TSF"
        );
        // The access point's later commands are refused with their owners.
        let command = ap_configure(permissions.commands.configure, AccessPointReceive::Off);
        let AccessPointProgress::Refused(
            AccessPointCommand::Configure { permission, .. },
            CommandRefusal::NotEnabled,
        ) = ap_command(harness, command)
        else {
            panic!("a stopped port refuses the configuration with its permission");
        };
        permissions.commands.configure = permission;
        permissions.beacon = returned.unwrap();
        harness.access_point = Some(permissions);
    });
}
