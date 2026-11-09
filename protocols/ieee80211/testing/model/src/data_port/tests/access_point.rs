//! The access-point role on the model's one port, through the public
//! contract: its admission beside the station's, its keys and beacon on
//! their own permissions, its stop beside an active station, and the shared
//! stop. Each case states the owners it expects back, apart from how the
//! model keeps them.

use super::*;
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCommand, AccessPointCommands, AccessPointPort, AccessPointProgress,
    AccessPointReceive, AccessPointRefusal, AttemptKey, BeaconAttempt, BeaconPermission,
    BeaconStamp, BeaconTimestamp, Configured, GroupAddress, InstalledGroup, InstalledPairwise,
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
pub(super) type StaPairwise<'s> = ModelPairwiseKey<'s, B<'s>, Station>;
pub(super) type Beacon<'s> = BeaconPermission<'s, ModelBeaconPath<B<'s>>>;
pub(super) type Admitted<'s> = ModelAdmitted<LocalId<'s>, ModelCaps, ()>;

/// How a single MPDU's terminal ended, as these cases tell them apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum End {
    Delivered,
    Withdrawn(WithdrawCause),
    Other,
}

/// One transmission terminal the sink held, its ticket and buffer back with
/// the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Ended {
    /// A single MPDU of the ticket.
    Frame(u8, End),
    /// The beacon of the ticket; its permission is back too.
    Beacon(u8, End),
}

/// Take the transmission terminals out of the sink, returning tickets and
/// buffers to the harness and a beacon's permission to `beacon`. Every
/// other terminal stays in the sink, in its order.
pub(super) fn settle<'s>(harness: &mut Harness<'s>, beacon: &mut Option<Beacon<'s>>) -> Vec<Ended> {
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

pub(super) fn give_back<'s>(
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
        permission,
        stamp: BeaconStamp::new(BeaconTimestamp::Caller).unwrap(),
    }
    .admit();
    harness.port.admit_beacon(sealed)
}

pub(super) fn submit<'s>(harness: &mut Harness<'s>, attempt: Admitted<'s>) -> u8 {
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
    tsf: Option<TsfControl>,
) -> ApCommand<'s> {
    AccessPointCommand::Configure {
        permission,
        receive,
        tsf: tsf.map(|control| SupportedTsfControl::new(control).unwrap()),
    }
}

fn bss() -> AccessPointReceive<ModelCaps> {
    AccessPointReceive::Bss {
        filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).unwrap(),
    }
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
pub(super) fn station_key<'s>(
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
pub(super) fn station_frame<'s>(
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
        let command = ap_configure(permissions.commands.configure, bss(), None);
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

/// Stop the started port of `harness` and reinstall it: the next
/// installation's port and resources, with the earlier installation's
/// tokens still with the caller.
pub(super) fn reinstall<'s>(mut harness: Harness<'s>) -> (Harness<'s>, ModelResources<'s, B<'s>>) {
    let waker = harness.waker();
    let now = harness.clock.now();
    assert!(matches!(
        harness
            .port
            .poll_stop(&mut Context::from_waker(&waker), now),
        Poll::Ready(Ok(()))
    ));
    let Harness {
        clock,
        air,
        port,
        mut installations,
        commands: _,
        sink,
        tickets,
        small,
        terminal_access,
        configure,
        gate,
        group,
        arenas,
        counter,
        access_point,
        station,
    } = harness;
    let Ok(ModelInstall {
        port,
        commands,
        resources,
    }) = port.reinstall(&mut installations)
    else {
        panic!("a retired port reinstalls");
    };
    assert_eq!(port.generation(), 1);
    (
        Harness {
            clock,
            air,
            installations,
            port,
            commands,
            sink,
            tickets,
            small,
            terminal_access,
            configure,
            gate,
            group,
            arenas,
            counter,
            access_point,
            station,
        },
        resources,
    )
}

#[test]
fn an_access_point_configure_permission_of_an_earlier_installation_is_refused_with_it() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let stale = harness.access_point.take().unwrap();
        let command = ap_configure(stale.commands.configure, bss(), Some(TsfControl::Restart));
        let AccessPointProgress::Refused(
            AccessPointCommand::Configure { permission, .. },
            AccessPointRefusal::StalePermission {
                issued: 0,
                installation: 1,
            },
        ) = ap_command(&mut harness, command)
        else {
            panic!("an earlier installation's configure permission is refused");
        };
        assert_eq!(permission.generation(), 0, "the permission comes back");
        assert_eq!(
            harness.air.access_point(),
            ModelAccessPointInterface::default(),
            "nothing was configured"
        );
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        // The current installation's permission configures.
        let command = ap_configure(resources.access_point.commands.configure, bss(), None);
        assert!(matches!(
            ap_command(&mut harness, command),
            AccessPointProgress::Done
        ));
        assert_eq!(take_ap_configured(&mut harness).unwrap().generation(), 1);
    }));
}

#[test]
fn a_beacon_sealed_before_a_reinstallation_is_refused_at_submit_with_every_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        // The beacon is prepared and sealed under the first installation.
        let permissions = harness.access_point.take().unwrap();
        let sealed = beacon(&mut harness, permissions.beacon);
        let ticket = sealed.operation.ticket().index();
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let now = harness.clock.now();
        assert_eq!(
            harness.port.submit(sealed, now, &mut harness.sink),
            Err(SubmitRefusal::StalePermission {
                issued: 0,
                installation: 1,
            })
        );
        let mut returned = None;
        assert_eq!(
            settle(&mut harness, &mut returned),
            [Ended::Beacon(
                ticket,
                End::Withdrawn(WithdrawCause::StalePermission)
            )],
            "its terminal returns ticket, buffer and permission"
        );
        assert_eq!(returned.unwrap().generation(), 0);
        assert!(harness.air.sent().is_empty(), "nothing went on the air");
        // The current installation's beacon goes.
        let current = beacon(&mut harness, resources.access_point.beacon);
        let current = submit(&mut harness, current);
        harness.service();
        let mut returned = None;
        assert_eq!(
            settle(&mut harness, &mut returned),
            [Ended::Beacon(current, End::Delivered)]
        );
        assert_eq!(returned.unwrap().generation(), 1);
    }));
}

/// The command a refusal for a token of installation 0 on installation 1
/// returned.
fn refused_stale<'s>(progress: ApProgress<'s>) -> ApCommand<'s> {
    match progress {
        AccessPointProgress::Refused(
            command,
            AccessPointRefusal::StalePermission {
                issued: 0,
                installation: 1,
            },
        ) => command,
        _ => panic!("a token of the earlier installation is refused"),
    }
}

#[test]
fn every_access_point_key_token_of_an_earlier_installation_is_refused_with_its_owners() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let stale = harness.access_point.take().unwrap();
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        let current = resources.access_point;
        let stale_association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            stale.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            current.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [stale_pairwise, _] = stale.pairwise;
        let [pairwise, _] = current.pairwise;
        let [stale_group, _] = stale.group;
        let [group, _] = current.group;
        // An earlier slot under the current association.
        let (install, _) = harness.commands.install_access_point_pairwise(
            &association,
            stale_pairwise,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        let progress = ap_command(&mut harness, install);
        assert!(matches!(
            refused_stale(progress),
            AccessPointCommand::InstallPairwise(_)
        ));
        // A current slot under the earlier association.
        let (install, _) = harness.commands.install_access_point_pairwise(
            &stale_association,
            pairwise,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        let progress = ap_command(&mut harness, install);
        assert!(matches!(
            refused_stale(progress),
            AccessPointCommand::InstallPairwise(_)
        ));
        // The earlier group permission with a current slot, and the
        // current permission with an earlier slot.
        let install = harness.commands.install_access_point_group(
            &association,
            stale.group_permission,
            group,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        let progress = ap_command(&mut harness, install);
        assert!(matches!(
            refused_stale(progress),
            AccessPointCommand::InstallGroup(_)
        ));
        let install = harness.commands.install_access_point_group(
            &association,
            current.group_permission,
            stale_group,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        let progress = ap_command(&mut harness, install);
        assert!(matches!(
            refused_stale(progress),
            AccessPointCommand::InstallGroup(_)
        ));
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        assert_eq!(harness.air.group_writes(), 0, "no key was written");
    }));
}

#[test]
fn removing_an_access_point_key_of_an_earlier_installation_is_refused_with_its_owners() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [pairwise_slot, _] = permissions.pairwise;
        let [group_slot, _] = permissions.group;
        let (install, _) = harness.commands.install_access_point_pairwise(
            &association,
            pairwise_slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        assert!(matches!(
            ap_command(&mut harness, install),
            AccessPointProgress::Done
        ));
        let install = harness.commands.install_access_point_group(
            &association,
            permissions.group_permission,
            group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        assert!(matches!(
            ap_command(&mut harness, install),
            AccessPointProgress::Done
        ));
        let mut installed = core::mem::take(&mut harness.sink.terminals).into_iter();
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(pairwise))) =
            installed.next()
        else {
            panic!("the pairwise terminal");
        };
        let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
            key: group,
            permission,
        })) = installed.next()
        else {
            panic!("the group terminal");
        };
        let (mut harness, resources) = reinstall(harness);
        harness.start();
        // The current installation's keys, installed on its own tokens.
        let current = resources.access_point;
        let current_association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            current.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [current_pairwise_slot, _] = current.pairwise;
        let [current_group_slot, _] = current.group;
        let (install, _) = harness.commands.install_access_point_pairwise(
            &current_association,
            current_pairwise_slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        assert!(matches!(
            ap_command(&mut harness, install),
            AccessPointProgress::Done
        ));
        let install = harness.commands.install_access_point_group(
            &current_association,
            current.group_permission,
            current_group_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[9; 16]),
        );
        assert!(matches!(
            ap_command(&mut harness, install),
            AccessPointProgress::Done
        ));
        let mut installed = core::mem::take(&mut harness.sink.terminals).into_iter();
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(
            current_pairwise,
        ))) = installed.next()
        else {
            panic!("the current pairwise terminal");
        };
        let Some(LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
            key: current_group,
            permission: current_permission,
        })) = installed.next()
        else {
            panic!("the current group terminal");
        };
        // Each removal names exactly one token of the earlier installation,
        // and is refused for it with every owner back.
        let AccessPointCommand::RemovePairwise {
            association,
            key: current_pairwise,
        } = refused_stale(ap_command(
            &mut harness,
            AccessPointCommand::RemovePairwise {
                association,
                key: current_pairwise,
            },
        ))
        else {
            panic!("an earlier association does not remove a current key");
        };
        let AccessPointCommand::RemovePairwise {
            association: current_association,
            key: pairwise,
        } = refused_stale(ap_command(
            &mut harness,
            AccessPointCommand::RemovePairwise {
                association: current_association,
                key: pairwise,
            },
        ))
        else {
            panic!("an earlier key is not removed under the current association");
        };
        let AccessPointCommand::RemoveGroup {
            permission,
            key: current_group,
        } = refused_stale(ap_command(
            &mut harness,
            AccessPointCommand::RemoveGroup {
                permission,
                key: current_group,
            },
        ))
        else {
            panic!("an earlier group permission does not remove a current key");
        };
        let AccessPointCommand::RemoveGroup {
            permission: current_permission,
            key: group,
        } = refused_stale(ap_command(
            &mut harness,
            AccessPointCommand::RemoveGroup {
                permission: current_permission,
                key: group,
            },
        ))
        else {
            panic!("an earlier group key is not removed with the current permission");
        };
        assert!(harness.sink.terminals.is_empty(), "no terminal is owed");
        assert_eq!(
            (association.owner.generation(), permission.generation()),
            (0, 0)
        );
        // The current tokens still remove the current keys.
        assert!(matches!(
            ap_command(
                &mut harness,
                AccessPointCommand::RemovePairwise {
                    association: current_association,
                    key: current_pairwise,
                },
            ),
            AccessPointProgress::Done
        ));
        assert!(matches!(
            ap_command(
                &mut harness,
                AccessPointCommand::RemoveGroup {
                    permission: current_permission,
                    key: current_group,
                },
            ),
            AccessPointProgress::Done
        ));
        assert_eq!(harness.sink.terminals.len(), 2, "both removals end");
        drop((association, permission, pairwise, group));
    }));
}

#[test]
fn an_access_point_frame_under_a_key_of_an_earlier_installation_is_refused_at_submit() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let mut harness = Harness::new(claim);
        harness.start();
        let permissions = harness.access_point.take().unwrap();
        let association = ModelKeys::<B<'_>, AccessPoint>::begin_association(
            permissions.association,
            oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        );
        let [slot, _] = permissions.pairwise;
        let (install, _) = harness.commands.install_access_point_pairwise(
            &association,
            slot,
            IndividualAddress::new(PEER).unwrap(),
            Ccmp128Key::new(&[8; 16]),
        );
        assert!(matches!(
            ap_command(&mut harness, install),
            AccessPointProgress::Done
        ));
        let Some(LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key))) =
            harness.sink.terminals.pop()
        else {
            panic!("the access point's pairwise terminal");
        };
        let sealed = access_point_frame(&mut harness, Some((&key, 1)), AP_QUEUE);
        let ticket = sealed.operation.ticket().index();
        let (mut harness, _resources) = reinstall(harness);
        harness.start();
        let now = harness.clock.now();
        assert_eq!(
            harness.port.submit(sealed, now, &mut harness.sink),
            Err(SubmitRefusal::StalePermission {
                issued: 0,
                installation: 1,
            })
        );
        let mut beacon = None;
        assert_eq!(
            settle(&mut harness, &mut beacon),
            [Ended::Frame(
                ticket,
                End::Withdrawn(WithdrawCause::StalePermission)
            )]
        );
        assert!(harness.air.sent().is_empty());
        drop(key);
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
        let command = ap_configure(
            permissions.commands.configure,
            bss(),
            Some(TsfControl::Restart),
        );
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        let ap_configure_permission = take_ap_configured(harness).unwrap();
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

        // A station frame waits in a free queue as the access point stops.
        let station_queued = station_frame_on(harness, WmmAccessCategory::Background);
        let station_queued = submit(harness, station_queued);
        // The access point stops: its configuration Off waits for its frame
        // and ends its queued beacon at once, with the permission; the
        // station's queued frame goes out.
        let command = ap_configure(ap_configure_permission, AccessPointReceive::Off, None);
        let AccessPointProgress::Held(command) = ap_command(harness, command) else {
            panic!("the access point's frame on the air holds its stop");
        };
        harness.service();
        // Terminals of different queues come in no promised order.
        let ended = settle(harness, &mut beacon_back);
        assert_eq!(ended.len(), 2);
        assert!(ended.contains(&Ended::Beacon(
            queued_beacon,
            End::Withdrawn(WithdrawCause::GateClosed)
        )));
        assert!(ended.contains(&Ended::Frame(station_queued, End::Delivered)));
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
        // Its TSF stops last, with its configure permission.
        let command = ap_configure(
            ap_configure_permission,
            AccessPointReceive::Off,
            Some(TsfControl::Stop),
        );
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        assert_eq!(harness.air.access_point().tsf, ModelAccessPointTsf::Stopped);
        let ap_configure_permission = take_ap_configured(harness).unwrap();
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
        let AccessPointProgress::Refused(install, AccessPointRefusal::NotEnabled) =
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
        let command = ap_configure(permissions.commands.configure, bss(), None);
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        permissions.commands.configure = take_ap_configured(harness).unwrap();
        let command = ap_configure(
            permissions.commands.configure,
            bss(),
            Some(TsfControl::Restart),
        );
        assert!(matches!(
            ap_command(harness, command),
            AccessPointProgress::Done
        ));
        permissions.commands.configure = take_ap_configured(harness).unwrap();
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
        let command = ap_configure(
            permissions.commands.configure,
            AccessPointReceive::Off,
            None,
        );
        let AccessPointProgress::Refused(
            AccessPointCommand::Configure { permission, .. },
            AccessPointRefusal::NotEnabled,
        ) = ap_command(harness, command)
        else {
            panic!("a stopped port refuses the configuration with its permission");
        };
        permissions.commands.configure = permission;
        permissions.beacon = returned.unwrap();
        harness.access_point = Some(permissions);
    });
}
