//! The arbiter on a host port it plays itself.

use super::*;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, GateIssuer, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter, WidthSet,
};
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use oer_radio_port::{Mint, branded};
use std::vec::Vec;

mod regressions;

fn installed<R>(body: impl for<'id> FnOnce(Mint<'id, ArbiterPool>) -> R) -> R {
    let future = core::pin::pin!(branded(async |mint| body(mint)));
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("this fixture performs no async operation"),
    }
}

/// A 2.4 and 5 GHz backend.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct Caps;

impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: false,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: Some(0),
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

type TestArbiter<'id, R = NoReservation> = Arbiter<'id, Caps, R>;

fn channel(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

/// What the arbiter did, its tokens handed back as the port would.
#[derive(Debug, Eq, PartialEq)]
enum Seen<'id, Reserve> {
    Station(StationNotice<'id, Caps>),
    AccessPoint(AccessPointCommand<Caps>),
    CloseGate(Reserve),
    Retune(u8),
    OpenGate,
}

/// The gate issuer of the port the test plays.
struct Port<'id> {
    issuer: GateIssuer<'id>,
}

/// Take every action at `now`, playing the port: a closed gate and a
/// retune answer at once.
fn run<'id, R: ReservationMode>(
    arbiter: &mut TestArbiter<'id, R>,
    port: &mut Port<'id>,
    now: Instant,
) -> Vec<Seen<'id, R::Reserve>> {
    let mut seen = Vec::new();
    while let Some(action) = arbiter.poll_action(now) {
        match action {
            ArbiterAction::Station(notice) => seen.push(Seen::Station(notice)),
            ArbiterAction::AccessPoint(command) => seen.push(Seen::AccessPoint(command)),
            ArbiterAction::CloseGate { gate, reserve } => {
                arbiter.gate_closed(port.issuer.close(gate));
                seen.push(Seen::CloseGate(reserve));
            }
            ArbiterAction::Retune(pending) => {
                seen.push(Seen::Retune(pending.channel().channel().number()));
                arbiter.retuned(pending);
            }
            ArbiterAction::OpenGate(closed) => {
                arbiter.gate_opened(port.issuer.open(closed));
                seen.push(Seen::OpenGate);
            }
        }
    }
    seen
}

/// Refuse the next retune through the consuming API, retaining its successor.
fn refuse_next<'id>(
    mut arbiter: TestArbiter<'id>,
    port: &mut Port<'id>,
    now: Instant,
) -> (RetuneRefused<'id, Caps, NoReservation>, Vec<Seen<'id, ()>>) {
    let mut seen = Vec::new();
    loop {
        match arbiter.poll_action(now).expect("a retune is due") {
            ArbiterAction::Station(notice) => seen.push(Seen::Station(notice)),
            ArbiterAction::AccessPoint(command) => seen.push(Seen::AccessPoint(command)),
            ArbiterAction::CloseGate { gate, reserve } => {
                arbiter.gate_closed(port.issuer.close(gate));
                seen.push(Seen::CloseGate(reserve));
            }
            ArbiterAction::Retune(pending) => {
                seen.push(Seen::Retune(pending.channel().channel().number()));
                return (arbiter.retune_refused(pending), seen);
            }
            ArbiterAction::OpenGate(closed) => {
                arbiter.gate_opened(port.issuer.open(closed));
                seen.push(Seen::OpenGate);
            }
        }
    }
}

fn failed<'id>(arbiter: TestArbiter<'id>) -> FailedArbiter<'id, Caps> {
    match arbiter.finish(ArbiterEnd::PortFailed) {
        RetiredArbiter::PortFailed(failed) => failed,
        RetiredArbiter::Stopped(_) => panic!("failure has no restart authority"),
    }
}

fn failed_notices<'id>(failed: &mut FailedArbiter<'id, Caps>) -> Vec<Seen<'id, ()>> {
    core::iter::from_fn(|| failed.poll_notice())
        .map(|notice| match notice {
            ArbiterNotice::Station(notice) => Seen::Station(notice),
            ArbiterNotice::AccessPoint(command) => Seen::AccessPoint(command),
        })
        .collect()
}

fn alone<'id, R: ReservationMode>(
    mint: Mint<'id, ArbiterPool>,
) -> (TestArbiter<'id, R>, LeasePlace<'id>, Port<'id>) {
    let (gate, issuer, authority) = mint.into_pools();
    let (arbiter, place, _) = Arbiter::new(
        ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(),
        channel(1),
        authority,
        gate,
    );
    (arbiter, place, Port { issuer })
}

fn running(arbiter: &mut TestArbiter<'_, impl ReservationMode>) {
    arbiter.access_point(
        at(0),
        AccessPointFact::Running {
            channel: channel(1),
            next_tbtt: at(50),
            beacon_interval: crate::coordinator::BeaconInterval::new(Duration::from_millis(100))
                .unwrap(),
        },
    );
}

fn beside_access_point<'id, R: ReservationMode>(
    mint: Mint<'id, ArbiterPool>,
) -> (TestArbiter<'id, R>, LeasePlace<'id>, Port<'id>) {
    let (mut arbiter, place, port) = alone::<R>(mint);
    running(&mut arbiter);
    (arbiter, place, port)
}

fn scan(number: u8, dwell: u32) -> ChannelRequest<Caps> {
    ChannelRequest {
        channel: channel(number),
        purpose: Purpose::Scan {
            dwell: Duration::from_millis(dwell),
        },
    }
}

fn lease<'id>(answer: Answer<'id, Caps>) -> Lease<'id, Caps> {
    match answer {
        Answer::Lease(lease) => lease,
        Answer::Refused(refusal, _) => panic!("refused: {refusal:?}"),
    }
}

/// A place to compare notices with: places are equal.
fn place<'id>() -> LeasePlace<'id> {
    LeasePlace {
        private: PhantomData,
    }
}

#[test]
fn a_station_alone_leases_a_scan_channel_at_once_and_ends_it_in_place() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = alone::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 100), place));
        // The move there and the dwell; alone, the port stays.
        assert_eq!((lease.from(), lease.until()), (at(0), Some(at(102))));
        assert_eq!(lease.listen_until(), Some(at(102)));
        let id = lease.id();
        assert_eq!(
            run(&mut arbiter, &mut port, at(0)),
            [
                Seen::CloseGate(()),
                Seen::Retune(6),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseStarted(id)),
            ]
        );
        assert_eq!(arbiter.poll_deadline(), Some(at(102)));
        // Its end closes the gate, so every terminal of the lease comes
        // before it; the place comes back.
        assert_eq!(
            run(&mut arbiter, &mut port, at(102)),
            [
                Seen::CloseGate(()),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
        let _ = lease;
    });
}

#[test]
fn beside_an_access_point_a_scan_goes_in_an_absence_with_the_air_reserved() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<AirReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 100), place));
        // 2 ms after the TBTT at 50 ms, 20 ms with its two retunes; the
        // station listens until the return's retune.
        assert_eq!((lease.from(), lease.until()), (at(52), Some(at(72))));
        assert_eq!(lease.listen_until(), Some(at(70)));
        assert_eq!(run(&mut arbiter, &mut port, at(0)), []);
        assert_eq!(arbiter.poll_deadline(), Some(at(52)));
        let id = lease.id();
        assert_eq!(
            run(&mut arbiter, &mut port, at(52)),
            [
                Seen::CloseGate(Some(at(72))),
                Seen::Retune(6),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseStarted(id)),
            ]
        );
        assert_eq!(arbiter.poll_deadline(), Some(at(70)));
        assert_eq!(
            run(&mut arbiter, &mut port, at(70)),
            [
                Seen::CloseGate(None),
                Seen::Retune(1),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
        let _ = lease;
    });
}

#[test]
fn an_explicitly_nonreserving_arbiter_can_use_a_longer_absence() {
    installed(|mint| {
        let (gate, issuer, authority) = mint.into_pools();
        let policy = ArbiterPolicy {
            absence: Duration::from_millis(40),
            ..ArbiterPolicy::DEFAULT
        };
        let (mut arbiter, place, _) = Arbiter::<Caps>::new(
            policy.checked(Duration::ZERO).unwrap(),
            channel(1),
            authority,
            gate,
        );
        let mut port = Port { issuer };
        running(&mut arbiter);
        let lease = lease(arbiter.request(at(0), scan(6, 100), place));
        assert_eq!(lease.until(), Some(at(92)));
        assert_eq!(
            run(&mut arbiter, &mut port, at(52)).first(),
            Some(&Seen::CloseGate(()))
        );
        let _ = lease;
    });
}

#[test]
fn a_released_lease_always_ends_or_is_cancelled() {
    installed(|mint| {
        // Before its start: cancelled.
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let waiting = lease(arbiter.request(at(0), scan(6, 10), place));
        arbiter.release(waiting);
        assert_eq!(
            run(&mut arbiter, &mut port, at(0)),
            [Seen::Station(StationNotice::LeaseCancelled(
                CancelReason::Released,
                self::place()
            ))]
        );

        // While its gate closes: cancel without moving off the home channel.
        let closing = lease(arbiter.request(at(0), scan(6, 10), self::place()));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(52)) else {
            panic!("the fixture expects a gate-closing action");
        };
        arbiter.release(closing);
        arbiter.gate_closed(port.issuer.close(gate));
        let seen = run(&mut arbiter, &mut port, at(52));
        assert_eq!(
            seen,
            [
                Seen::Station(StationNotice::LeaseCancelled(
                    CancelReason::Released,
                    self::place()
                )),
                Seen::OpenGate
            ]
        );
        assert_eq!(arbiter.channel(), Some(channel(1)));

        // While the port retunes onto its channel.
        let moving = lease(arbiter.request(at(100), scan(6, 10), self::place()));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(152)) else {
            panic!("the fixture expects a gate-closing action");
        };
        arbiter.gate_closed(port.issuer.close(gate));
        let Some(ArbiterAction::Retune(pending)) = arbiter.poll_action(at(152)) else {
            panic!("the closed gate yields its retune token");
        };
        assert_eq!(pending.channel(), channel(6));
        arbiter.release(moving);
        arbiter.retuned(pending);
        let seen = run(&mut arbiter, &mut port, at(152));
        assert_eq!(
            seen.last(),
            Some(&Seen::Station(StationNotice::LeaseEnded(self::place())))
        );
        assert_eq!(arbiter.channel(), Some(channel(1)));

        // On the home channel: its own cycle, then it ends.
        let home = lease(arbiter.request(
            at(200),
            ChannelRequest {
                channel: channel(1),
                purpose: Purpose::Connect,
            },
            self::place(),
        ));
        run(&mut arbiter, &mut port, at(200));
        arbiter.release(home);
        assert_eq!(
            run(&mut arbiter, &mut port, at(200)),
            [
                Seen::CloseGate(()),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
    });
}

#[test]
fn an_access_point_that_stops_during_an_absence_cancels_it_and_the_port_returns() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 10), place));
        run(&mut arbiter, &mut port, at(52));
        arbiter.access_point(at(55), AccessPointFact::Stopped);
        assert_eq!(
            run(&mut arbiter, &mut port, at(55)),
            [
                Seen::CloseGate(()),
                Seen::Retune(1),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseCancelled(
                    CancelReason::AccessPointStopped,
                    self::place()
                )),
            ]
        );
        let _ = lease;
    });
}

#[test]
fn a_refused_move_cancels_the_lease_and_the_port_returns() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 10), place));
        let (outcome, mut seen) = refuse_next(arbiter, &mut port, at(52));
        let RetuneRefused::Return {
            mut arbiter,
            pending,
        } = outcome
        else {
            panic!("an outbound refusal must return home")
        };
        while let Some(notice) = arbiter.poll_notice(at(52)) {
            match notice {
                ArbiterNotice::Station(notice) => seen.push(Seen::Station(notice)),
                ArbiterNotice::AccessPoint(command) => seen.push(Seen::AccessPoint(command)),
            }
        }
        seen.push(Seen::Retune(pending.channel().channel().number()));
        arbiter.retuned(pending);
        seen.extend(run(&mut arbiter, &mut port, at(52)));
        assert_eq!(
            seen,
            [
                Seen::CloseGate(()),
                Seen::Retune(6),
                Seen::Station(StationNotice::LeaseCancelled(
                    CancelReason::Refused,
                    self::place()
                )),
                Seen::Retune(1),
                Seen::OpenGate,
            ]
        );
        assert_eq!(arbiter.channel(), Some(channel(1)));
        let _ = lease;
    });
}

#[test]
fn refusal_returns_the_home_retune_without_taking_another_gate_action() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let _lease = lease(arbiter.request(at(0), scan(6, 10), place));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(52)) else {
            panic!("close the lease gate");
        };
        arbiter.gate_closed(port.issuer.close(gate));
        let Some(ArbiterAction::Retune(outbound)) = arbiter.poll_action(at(52)) else {
            panic!("outbound retune");
        };
        let RetuneRefused::Return {
            mut arbiter,
            pending: returning,
        } = arbiter.retune_refused(outbound)
        else {
            panic!("the return token is delivered directly")
        };
        assert_eq!(returning.channel(), channel(1));
        assert!(matches!(arbiter.port, super::Port::Retuning));
        assert!(matches!(
            arbiter.poll_notice(at(52)),
            Some(ArbiterNotice::Station(StationNotice::LeaseCancelled(
                CancelReason::Refused,
                _
            )))
        ));
        assert!(arbiter.poll_notice(at(52)).is_none());
        assert!(
            arbiter.poll_action(at(52)).is_none(),
            "a pending return cannot issue another retune or open"
        );
        arbiter.retuned(returning);
        let Some(ArbiterAction::OpenGate(closed)) = arbiter.poll_action(at(52)) else {
            panic!("return the same gate place");
        };
        arbiter.gate_opened(port.issuer.open(closed));
        assert_eq!(arbiter.channel(), Some(channel(1)));
        assert!(arbiter.poll_action(at(52)).is_none());
    });
}

#[test]
fn notices_without_a_phase_cannot_take_the_open_gate() {
    installed(|mint| {
        let (mut arbiter, place, _port) = alone::<NoReservation>(mint);
        let _lease = lease(arbiter.request(at(0), scan(6, 10), place));
        while arbiter.poll_notice(at(0)).is_some() {}
        assert!(matches!(
            arbiter.poll_action(at(0)),
            Some(ArbiterAction::CloseGate { .. })
        ));
    });
}

#[test]
fn a_failed_return_or_a_poisoned_port_stops_the_station_and_the_access_point() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let returning = lease(arbiter.request(at(0), scan(6, 10), place));
        run(&mut arbiter, &mut port, at(52));
        let (outcome, mut seen) = refuse_next(arbiter, &mut port, at(64));
        let RetuneRefused::Failed(mut failed) = outcome else {
            panic!("return must fail")
        };
        seen.extend(failed_notices(&mut failed));
        assert!(failed.poll_notice().is_none());
        assert!(seen.contains(&Seen::Station(StationNotice::LeaseCancelled(
            CancelReason::PortFailed,
            self::place()
        ))));
        assert!(seen.contains(&Seen::Station(StationNotice::Leave(
            LeaveReason::PortFailed
        ))));
        assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
        let _ = returning;
    });
    installed(|mint| {
        let (mut arbiter, place, _port) = beside_access_point::<NoReservation>(mint);
        let poisoned = lease(arbiter.request(at(0), scan(6, 10), place));
        let mut failed = failed(arbiter);
        let seen = failed_notices(&mut failed);
        assert!(seen.contains(&Seen::Station(StationNotice::Leave(
            LeaveReason::PortFailed
        ))));
        assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
        assert!(failed.poll_notice().is_none());
        let _ = poisoned;
    });
}

#[test]
fn a_gate_that_closes_too_late_for_two_retunes_and_the_shortest_listening_cancels_the_lease() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 10), place));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(52)) else {
            panic!("the fixture expects a gate-closing action");
        };
        // The attempts in flight end at 58 ms: 58 + 2 + 2 + 5 > 66.
        arbiter.gate_closed(port.issuer.close(gate));
        assert_eq!(
            run(&mut arbiter, &mut port, at(58)),
            [
                Seen::Station(StationNotice::LeaseCancelled(
                    CancelReason::Late,
                    self::place()
                )),
                Seen::OpenGate,
            ]
        );
        let _ = lease;
    });
}

#[test]
fn an_upstream_switch_moves_the_port_with_the_access_points_announcement() {
    installed(|mint| {
        let (mut arbiter, _place, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        // The upstream moves to channel 6 at 260 ms: the access point
        // announces it in its beacons before, and the port moves then.
        arbiter.upstream(
            at(10),
            UpstreamFact::SwitchAnnounced {
                target: channel(6),
                mode: ChannelSwitchMode::Continue,
                at: at(260),
            },
        );
        assert_eq!(
            run(&mut arbiter, &mut port, at(10)),
            [Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
                target: channel(6),
                mode: ChannelSwitchMode::Continue,
                count: 3,
            })]
        );
        // The access point's switch is due at its TBTT.
        arbiter.access_point(at(250), AccessPointFact::SwitchDue(channel(6)));
        assert_eq!(
            run(&mut arbiter, &mut port, at(250)),
            [
                Seen::CloseGate(()),
                Seen::Retune(6),
                Seen::OpenGate,
                Seen::Station(StationNotice::Moved(channel(6))),
            ]
        );
    });
}

#[test]
fn a_lost_upstream_is_searched_for_in_absences_and_its_loss_stops_the_access_point() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        arbiter.upstream(at(10), UpstreamFact::Lost);
        let search = |number| ChannelRequest {
            channel: channel(number),
            purpose: Purpose::Search {
                dwell: Duration::from_millis(10),
            },
        };
        let first = lease(arbiter.request(at(10), search(6), place));
        assert_eq!(first.from(), at(52));
        run(&mut arbiter, &mut port, at(52));
        let seen = run(&mut arbiter, &mut port, at(64));
        let Some(Seen::Station(StationNotice::LeaseEnded(place))) = seen.into_iter().last() else {
            panic!("the absence ends");
        };
        let _ = first;
        // The next absence follows one beacon interval later while dense.
        let second = lease(arbiter.request(at(64), search(11), place));
        assert_eq!(second.from(), at(152));
        arbiter.release(second);
        run(&mut arbiter, &mut port, at(64));
        // Past the loss policy's 30 s the access point stops, and the
        // station scans alone.
        let seen = run(&mut arbiter, &mut port, at(30_010));
        assert!(seen.contains(&Seen::AccessPoint(AccessPointCommand::Stop)));
    });
}

#[test]
fn a_failed_upstream_join_resumes_the_original_loss_deadline() {
    installed(|mint| {
        let (mut arbiter, _place, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        arbiter.upstream(at(10), UpstreamFact::Lost);
        assert_eq!(arbiter.poll_deadline(), Some(at(30_010)));
        arbiter.upstream(at(20), UpstreamFact::Found(channel(1)));
        assert_eq!(arbiter.poll_deadline(), None);

        // A join can outlast the loss policy. Its failure makes that
        // original deadline due immediately, without another 30 s wait.
        arbiter.upstream(at(40_000), UpstreamFact::JoinFailed);
        assert_eq!(arbiter.poll_deadline(), Some(at(30_010)));
        assert_eq!(
            run(&mut arbiter, &mut port, at(40_000)),
            [Seen::AccessPoint(AccessPointCommand::Stop)]
        );
        assert_eq!(arbiter.poll_deadline(), None);
        assert!(run(&mut arbiter, &mut port, at(40_000)).is_empty());
    });
}

#[test]
fn a_failed_upstream_join_does_not_restart_dense_search() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.policy.search.dense_period = Duration::from_millis(200);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        arbiter.upstream(at(10), UpstreamFact::Lost);
        arbiter.upstream(at(20), UpstreamFact::Found(channel(1)));
        arbiter.upstream(at(410), UpstreamFact::JoinFailed);
        let search = |number| ChannelRequest {
            channel: channel(number),
            purpose: Purpose::Search {
                dwell: Duration::from_millis(10),
            },
        };
        let first = lease(arbiter.request(at(410), search(6), place));
        assert_eq!(first.from(), at(452));
        run(&mut arbiter, &mut port, at(452));
        let seen = run(&mut arbiter, &mut port, at(464));
        let Some(Seen::Station(StationNotice::LeaseEnded(place))) = seen.into_iter().last() else {
            panic!("the search absence returns its place");
        };
        let second = lease(arbiter.request(at(464), search(11), place));
        assert_eq!(second.from(), at(1452));
        arbiter.release(second);
        run(&mut arbiter, &mut port, at(464));
    });
}

fn connect(number: u8) -> ChannelRequest<Caps> {
    ChannelRequest {
        channel: channel(number),
        purpose: Purpose::Connect,
    }
}

#[test]
fn a_join_away_from_the_access_point_starts_with_its_announced_move_and_it_stays_there() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), connect(6), place));
        // Five beacons announce the move; the switch is due at the TBTT after
        // the last of them.
        assert_eq!(lease.from(), at(550));
        assert_eq!(lease.until(), None);
        let id = lease.id();
        assert_eq!(
            run(&mut arbiter, &mut port, at(0)),
            [Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
                target: channel(6),
                mode: ChannelSwitchMode::Continue,
                count: 5,
            })]
        );
        // Nothing moves before the access point's switch.
        assert_eq!(run(&mut arbiter, &mut port, at(549)), []);
        assert_eq!(arbiter.channel(), Some(channel(1)));
        arbiter.access_point(at(550), AccessPointFact::SwitchDue(channel(6)));
        assert_eq!(
            run(&mut arbiter, &mut port, at(550)),
            [
                Seen::CloseGate(()),
                Seen::Retune(6),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseStarted(id)),
            ]
        );
        // The join fails: the station gives its lease back, and the access
        // point stays on the channel it announced.
        arbiter.release(lease);
        assert_eq!(
            run(&mut arbiter, &mut port, at(600)),
            [
                Seen::CloseGate(()),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
        assert_eq!(arbiter.channel(), Some(channel(6)));
    });
}

#[test]
fn a_second_join_elsewhere_within_the_follow_interval_is_refused_and_the_access_point_stays() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), connect(6), place));
        run(&mut arbiter, &mut port, at(0));
        arbiter.access_point(at(550), AccessPointFact::SwitchDue(channel(6)));
        run(&mut arbiter, &mut port, at(550));
        arbiter.release(lease);
        let place = match run(&mut arbiter, &mut port, at(600)).pop() {
            Some(Seen::Station(StationNotice::LeaseEnded(place))) => place,
            seen => panic!("{seen:?}"),
        };
        // The join failed, so the interval doubled: within 60 s of the last
        // announcement, refused with when to retry, and nothing is announced.
        let place = match arbiter.request(at(1_000), connect(11), place) {
            Answer::Refused(Refusal::TooSoon { retry_at }, place) => {
                assert_eq!(retry_at, at(60_000));
                place
            }
            answer => panic!("{answer:?}"),
        };
        assert_eq!(run(&mut arbiter, &mut port, at(1_000)), []);
        assert_eq!(arbiter.channel(), Some(channel(6)));
        // After it, the access point follows again.
        let _ = lease_after(&mut arbiter, place);
        assert!(matches!(
            run(&mut arbiter, &mut port, at(60_000)).as_slice(),
            [Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
                count: 5,
                ..
            })]
        ));
    });
}

#[test]
fn an_unrepresentable_follow_holdoff_refuses_without_a_move_or_lost_place() {
    installed(|mint| {
        let mut policy = ArbiterPolicy::DEFAULT;
        policy.follow.min_follow_interval = Duration::from_micros(u64::MAX);
        policy.follow.max_follow_interval = Duration::from_micros(u64::MAX);
        let (gate, issuer, authority) = mint.into_pools();
        let (mut arbiter, place, _) = Arbiter::<Caps, NoReservation>::new(
            policy.checked(Duration::ZERO).unwrap(),
            channel(1),
            authority,
            gate,
        );
        let mut port = Port { issuer };
        running(&mut arbiter);
        let mut place = failed_follow(&mut arbiter, &mut port, place, 6, at(1), at(550));
        let last_follow = arbiter.coordinator.last_follow_for_test();
        let next_id = arbiter.next_id;
        for now in [at(1_000), at(2_000)] {
            place = match arbiter.request(now, connect(11), place) {
                Answer::Refused(Refusal::ScheduleExhausted, place) => place,
                answer => panic!("holdoff overflow must not authorize a move: {answer:?}"),
            };
            assert!(run(&mut arbiter, &mut port, now).is_empty());
            assert!(arbiter.primary.is_none());
            assert_eq!(arbiter.channel(), Some(channel(6)));
            assert_eq!(arbiter.coordinator.last_follow_for_test(), last_follow);
            assert_eq!(arbiter.next_id, next_id);
        }
        // The same returned admission place still permits a local join.
        let local = lease(arbiter.request(at(3_000), connect(6), place));
        let started = run(&mut arbiter, &mut port, at(3_000));
        assert_eq!(
            started
                .iter()
                .filter(|event| matches!(event, Seen::Station(StationNotice::LeaseStarted { .. })))
                .count(),
            1,
            "{started:?}"
        );
        assert!(
            !started
                .iter()
                .any(|event| matches!(event, Seen::Retune(_) | Seen::AccessPoint(_))),
            "{started:?}"
        );
        arbiter.release(local);
        assert!(matches!(
            run(&mut arbiter, &mut port, at(3_000)).last(),
            Some(Seen::Station(StationNotice::LeaseEnded(_)))
        ));
    });
}

/// Join channel `number` at `now` beside the access point, its move due
/// at `due`, then give the lease back without connecting; the place comes
/// back.
fn failed_follow<'id>(
    arbiter: &mut TestArbiter<'id>,
    port: &mut Port<'id>,
    place: LeasePlace<'id>,
    number: u8,
    now: Instant,
    due: Instant,
) -> LeasePlace<'id> {
    let lease = lease(arbiter.request(now, connect(number), place));
    run(arbiter, port, now);
    arbiter.access_point(due, AccessPointFact::SwitchDue(channel(number)));
    run(arbiter, port, due);
    arbiter.release(lease);
    match run(arbiter, port, due).pop() {
        Some(Seen::Station(StationNotice::LeaseEnded(place))) => place,
        seen => panic!("{seen:?}"),
    }
}

fn retry_at<'id>(answer: Answer<'id, Caps>) -> (Instant, LeasePlace<'id>) {
    match answer {
        Answer::Refused(Refusal::TooSoon { retry_at }, place) => (retry_at, place),
        answer => panic!("{answer:?}"),
    }
}

#[test]
fn the_follow_interval_doubles_after_each_failed_join_and_resets_once_connected() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        // Announced at 0: the next within 30 s; failed, so 60 s from the next.
        let place = failed_follow(&mut arbiter, &mut port, place, 6, at(0), at(550));
        let (retry, place) = retry_at(arbiter.request(at(1_000), connect(11), place));
        assert_eq!(retry, at(60_000));
        let place = failed_follow(&mut arbiter, &mut port, place, 11, at(60_000), at(60_600));
        let (retry, place) = retry_at(arbiter.request(at(61_000), connect(1), place));
        assert_eq!(retry, at(180_000));
        // The doubling stops at 480 s.
        let mut now = 180_000;
        let mut place = place;
        let mut number = 1;
        for _ in 0..4 {
            place = failed_follow(
                &mut arbiter,
                &mut port,
                place,
                number,
                at(now),
                at(now + 600),
            );
            let (retry, back) =
                retry_at(arbiter.request(at(now + 1_000), connect(number ^ 7), place));
            place = back;
            now = u32::try_from(retry.as_micros() / 1000).unwrap();
            number ^= 7;
        }
        assert_eq!(now - 180_000 - 240_000 - 480_000 * 2, 480_000);
        // Connected: back to 30 s.
        let lease = lease(arbiter.request(at(now), connect(number), place));
        run(&mut arbiter, &mut port, at(now));
        arbiter.access_point(at(now + 600), AccessPointFact::SwitchDue(channel(number)));
        run(&mut arbiter, &mut port, at(now + 600));
        arbiter.upstream(at(now + 700), UpstreamFact::Connected(channel(number)));
        arbiter.release(lease);
        let place = match run(&mut arbiter, &mut port, at(now + 800)).pop() {
            Some(Seen::Station(StationNotice::LeaseEnded(place))) => place,
            seen => panic!("{seen:?}"),
        };
        let (retry, _) = retry_at(arbiter.request(at(now + 1_000), connect(number ^ 7), place));
        assert_eq!(retry, at(now + 30_000));
    });
}

#[test]
fn a_lease_names_the_running_access_points_channel() {
    installed(|mint| {
        let (mut arbiter, place, _port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 10), place));
        assert_eq!(lease.access_point(), Some(channel(1)));
    });
    installed(|mint| {
        let (mut alone, place, _port) = alone::<NoReservation>(mint);
        assert_eq!(lease_of(&mut alone, place).access_point(), None);
    });
}

fn lease_of<'id>(arbiter: &mut TestArbiter<'id>, place: LeasePlace<'id>) -> Lease<'id, Caps> {
    lease(arbiter.request(at(0), scan(6, 10), place))
}

fn lease_after<'id>(arbiter: &mut TestArbiter<'id>, place: LeasePlace<'id>) -> Lease<'id, Caps> {
    lease(arbiter.request(at(60_000), connect(11), place))
}

#[test]
fn a_join_where_the_access_point_may_not_follow_stops_it_by_the_default_policy() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let five = SupportedChannel::new(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap()).unwrap();
        let lease = lease(arbiter.request(
            at(0),
            ChannelRequest {
                channel: five,
                purpose: Purpose::Connect,
            },
            place,
        ));
        // The access point stops; the station joins alone, at once.
        assert_eq!(
            run(&mut arbiter, &mut port, at(0)),
            [
                Seen::AccessPoint(AccessPointCommand::Stop),
                Seen::CloseGate(()),
                Seen::Retune(36),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseStarted(lease.id())),
            ]
        );
    });
}

/// A station connected on channel 1 beside an access point there, its
/// primary lease running.
fn connected<'id>(
    mint: Mint<'id, ArbiterPool>,
) -> (
    TestArbiter<'id>,
    Lease<'id, Caps>,
    AbsencePlace<'id>,
    Port<'id>,
) {
    connected_with_policy(mint, ArbiterPolicy::DEFAULT)
}

fn connected_with_policy<'id>(
    mint: Mint<'id, ArbiterPool>,
    policy: ArbiterPolicy,
) -> (
    TestArbiter<'id>,
    Lease<'id, Caps>,
    AbsencePlace<'id>,
    Port<'id>,
) {
    let (gate, issuer, authority) = mint.into_pools();
    let mut port = Port { issuer };
    let (mut arbiter, place, absence) = Arbiter::new(
        policy.checked(Duration::ZERO).unwrap(),
        channel(1),
        authority,
        gate,
    );
    running(&mut arbiter);
    let home = lease(arbiter.request(
        at(0),
        ChannelRequest {
            channel: channel(1),
            purpose: Purpose::Connect,
        },
        place,
    ));
    let id = home.id();
    assert_eq!(
        run(&mut arbiter, &mut port, at(0)).last(),
        Some(&Seen::Station(StationNotice::LeaseStarted(id)))
    );
    (arbiter, home, absence, port)
}

fn absence<'id>(answer: AbsenceAnswer<'id, Caps>) -> Lease<'id, Caps> {
    match answer {
        AbsenceAnswer::Lease(lease) => lease,
        AbsenceAnswer::Refused(refusal, _) => panic!("refused: {refusal:?}"),
    }
}

fn absence_place<'id>() -> AbsencePlace<'id> {
    AbsencePlace {
        private: PhantomData,
    }
}

#[test]
fn a_connected_stations_absence_waits_for_its_readiness_and_returns_home() {
    installed(|mint| {
        let (mut arbiter, home, place, mut port) = connected(mint);
        // At 45 ms the preparation window ends at 55 ms, past the slot at
        // 52 ms: the absence follows the next TBTT.
        let late = absence(arbiter.request_absence(at(45), scan(6, 10), &home, place));
        assert_eq!(late.from(), at(152));
        // The station never said it was ready: cancelled at its start.
        assert_eq!(
            run(&mut arbiter, &mut port, at(152)),
            [Seen::Station(StationNotice::AbsenceCancelled(
                CancelReason::NotReady,
                absence_place()
            ))]
        );
        let _ = late;
        let ready = absence(arbiter.request_absence(at(200), scan(6, 10), &home, absence_place()));
        assert_eq!(ready.from(), at(252));
        arbiter.ready(ready.id());
        assert_eq!(
            run(&mut arbiter, &mut port, at(252)),
            [
                Seen::CloseGate(()),
                Seen::Retune(6),
                Seen::OpenGate,
                Seen::Station(StationNotice::AbsenceStarted(ready.id())),
            ]
        );
        assert_eq!(
            run(&mut arbiter, &mut port, at(264)),
            [
                Seen::CloseGate(()),
                Seen::Retune(1),
                Seen::OpenGate,
                Seen::Station(StationNotice::AbsenceEnded(absence_place())),
            ]
        );
        let _ = (ready, home);
    });
}

#[test]
fn an_absence_needs_the_running_primary_lease() {
    installed(|mint| {
        let (gate, _issuer, authority) = mint.into_pools();
        let (mut arbiter, place, absence) = Arbiter::<Caps>::new(
            ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(),
            channel(1),
            authority,
            gate,
        );
        let waiting = lease(arbiter.request(
            at(0),
            ChannelRequest {
                channel: channel(1),
                purpose: Purpose::Connect,
            },
            place,
        ));
        // Not started yet: no absence from it.
        assert!(matches!(
            arbiter.request_absence(at(0), scan(6, 10), &waiting, absence),
            AbsenceAnswer::Refused(AbsenceRefusal::NotConnected, _)
        ));
        let _ = waiting;
    });
}

#[test]
fn a_release_of_the_primary_lease_during_an_absence_ends_the_absence_first() {
    installed(|mint| {
        let (mut arbiter, home, place, mut port) = connected(mint);
        let away = absence(arbiter.request_absence(at(0), scan(6, 10), &home, place));
        arbiter.ready(away.id());
        run(&mut arbiter, &mut port, at(52));
        arbiter.release(home);
        let seen = run(&mut arbiter, &mut port, at(55));
        let ends: Vec<_> = seen
            .into_iter()
            .filter(|seen| matches!(seen, Seen::Station(_)))
            .collect();
        assert_eq!(
            ends,
            [
                Seen::Station(StationNotice::AbsenceCancelled(
                    CancelReason::Released,
                    absence_place()
                )),
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
        let _ = away;
    });
}

#[test]
fn a_switch_during_an_absence_cancels_it_and_replaces_the_primary_lease() {
    installed(|mint| {
        let (mut arbiter, home, place, mut port) = connected(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        let away = absence(arbiter.request_absence(at(0), scan(6, 10), &home, place));
        arbiter.ready(away.id());
        run(&mut arbiter, &mut port, at(52));
        // The upstream moves to channel 11, during the absence.
        arbiter.upstream(
            at(53),
            UpstreamFact::SwitchAnnounced {
                target: channel(11),
                mode: ChannelSwitchMode::Continue,
                at: at(55),
            },
        );
        // The access point announces the switch; it is due at 55 ms.
        arbiter.access_point(at(55), AccessPointFact::SwitchDue(channel(11)));
        let seen = run(&mut arbiter, &mut port, at(55));
        let notices: Vec<_> = seen
            .into_iter()
            .filter(|seen| matches!(seen, Seen::Station(_)))
            .collect();
        assert!(matches!(
            &notices[..],
            [
                Seen::Station(StationNotice::AbsenceCancelled(CancelReason::Switch, _)),
                Seen::Station(StationNotice::Moved(moved)),
            ] if *moved == channel(11)
        ));
        assert_eq!(arbiter.channel(), Some(channel(11)));
        // The station exchanges its primary lease: the old one goes, a new one
        // of the new channel comes, under a new name.
        let old = home.id();
        let home = arbiter.exchange(home).unwrap();
        assert_ne!(home.id(), old);
        assert_eq!(home.channel(), channel(11));
        // The old name is gone: a release of the new lease ends it.
        arbiter.release(home);
        assert!(
            run(&mut arbiter, &mut port, at(56))
                .into_iter()
                .any(|seen| matches!(seen, Seen::Station(StationNotice::LeaseEnded(..))))
        );
        let _ = away;
    });
}

#[test]
fn an_exchange_without_a_move_gives_the_same_lease_back() {
    installed(|mint| {
        let (mut arbiter, home, _, _) = connected(mint);
        let id = home.id();
        let home = arbiter.exchange(home).unwrap_err();
        assert_eq!(home.id(), id);
        assert_eq!(home.channel(), channel(1));
    });
}

#[test]
fn returning_the_unique_place_proves_the_lease_ended_and_admits_the_next_lease() {
    installed(|mint| {
        let (mut arbiter, home, _, mut port) = connected(mint);
        arbiter.release(home);
        let place = run(&mut arbiter, &mut port, at(56))
            .into_iter()
            .find_map(|event| match event {
                Seen::Station(StationNotice::LeaseEnded(place)) => Some(place),
                _ => None,
            })
            .expect("the sole lease returns its place without an ID");
        let answer = arbiter.request(
            at(57),
            ChannelRequest {
                channel: channel(1),
                purpose: Purpose::Connect,
            },
            place,
        );
        assert!(matches!(answer, Answer::Lease(_)));
    });
}

#[test]
fn an_absence_reserves_one_publication_for_each_gate_and_the_return_starts_early() {
    installed(|mint| {
        let (gate, issuer, authority) = mint.into_pools();
        let mut port = Port { issuer };
        let publication = Duration::from_millis(3);
        let (mut arbiter, place, absence_place) = Arbiter::<Caps>::new(
            ArbiterPolicy::DEFAULT.checked(publication).unwrap(),
            channel(1),
            authority,
            gate,
        );
        running(&mut arbiter);
        let home = lease(arbiter.request(
            at(0),
            ChannelRequest {
                channel: channel(1),
                purpose: Purpose::Connect,
            },
            place,
        ));
        run(&mut arbiter, &mut port, at(0));
        let away = absence(arbiter.request_absence(at(0), scan(6, 5), &home, absence_place));
        // 5 ms listening and two moves, each with 3 ms publication + 2 ms retune.
        assert_eq!(
            away.until().unwrap().checked_duration_since(away.from()),
            Some(Duration::from_millis(15))
        );
        assert_eq!(
            away.until()
                .unwrap()
                .checked_duration_since(away.listen_until().unwrap()),
            Some(Duration::from_millis(5))
        );
    });
}

#[test]
fn a_gate_wait_has_no_expired_deadline() {
    installed(|mint| {
        let (mut arbiter, _, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        arbiter.upstream(
            at(10),
            UpstreamFact::SwitchAnnounced {
                target: channel(6),
                mode: ChannelSwitchMode::Continue,
                at: at(260),
            },
        );
        let _ = run(&mut arbiter, &mut port, at(10));
        arbiter.access_point(at(250), AccessPointFact::SwitchDue(channel(6)));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(250)) else {
            panic!()
        };
        assert!(arbiter.poll_action(at(250)).is_none());
        assert!(
            arbiter
                .poll_deadline()
                .is_none_or(|deadline| deadline > at(250)),
            "expired timer would spin while gate is closing"
        );
        let _ = gate;
    });
}

#[test]
fn a_move_keeps_the_next_coordinator_retune_until_its_gate_opens() {
    for after_retune in [false, true] {
        installed(|mint| {
            let (mut arbiter, _, mut port) = beside_access_point::<NoReservation>(mint);
            arbiter.access_point(at(50), AccessPointFact::SwitchDue(channel(6)));
            let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(50)) else {
                panic!("the first move closes its gate");
            };
            arbiter.gate_closed(port.issuer.close(gate));
            let Some(ArbiterAction::Retune(pending)) = arbiter.poll_action(at(50)) else {
                panic!("the first move is admitted");
            };
            if after_retune {
                arbiter.retuned(pending);
                arbiter.access_point(at(51), AccessPointFact::SwitchDue(channel(11)));
            } else {
                arbiter.access_point(at(51), AccessPointFact::SwitchDue(channel(11)));
                arbiter.retuned(pending);
            }
            let events = run(&mut arbiter, &mut port, at(52));
            assert!(events.contains(&Seen::Retune(11)), "{events:?}");
            assert!(events.contains(&Seen::Station(StationNotice::Moved(channel(6)))));
            assert!(events.contains(&Seen::Station(StationNotice::Moved(channel(11)))));
            assert_eq!(arbiter.channel(), Some(channel(11)));
            assert_eq!(arbiter.home, channel(11));
            assert_eq!(arbiter.poll_deadline(), None);
        });
    }
}

#[test]
fn an_old_channel_tbtt_during_a_move_does_not_change_the_return_channel() {
    installed(|mint| {
        let (mut arbiter, home, place, mut port) = connected(mint);
        arbiter.access_point(at(50), AccessPointFact::SwitchDue(channel(6)));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(50)) else {
            panic!("the move closes its gate");
        };
        arbiter.access_point(
            at(51),
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: at(150),
                beacon_interval: crate::coordinator::BeaconInterval::new(Duration::from_millis(
                    100,
                ))
                .unwrap(),
            },
        );
        arbiter.gate_closed(port.issuer.close(gate));
        run(&mut arbiter, &mut port, at(52));
        assert_eq!(arbiter.home, channel(6));
        let home = arbiter.exchange(home).unwrap();
        let away = absence(arbiter.request_absence(at(53), scan(11, 5), &home, place));
        arbiter.ready(away.id());
        run(&mut arbiter, &mut port, away.from());
        run(&mut arbiter, &mut port, away.until().unwrap());
        assert_eq!(arbiter.channel(), Some(channel(6)));
    });
}

#[test]
fn a_due_move_cancels_a_waiting_absence_before_closing_its_gate() {
    installed(|mint| {
        let (mut arbiter, home, place, mut port) = connected(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        arbiter.upstream(
            at(10),
            UpstreamFact::SwitchAnnounced {
                target: channel(11),
                mode: ChannelSwitchMode::Continue,
                at: at(260),
            },
        );
        run(&mut arbiter, &mut port, at(10));
        let away = absence(arbiter.request_absence(at(240), scan(6, 5), &home, place));
        assert_eq!(away.from(), at(252));
        arbiter.ready(away.id());
        arbiter.access_point(at(250), AccessPointFact::SwitchDue(channel(11)));
        let events = run(&mut arbiter, &mut port, at(250));
        assert_eq!(
            events,
            [
                Seen::Station(StationNotice::AbsenceCancelled(
                    CancelReason::Switch,
                    absence_place()
                )),
                Seen::CloseGate(()),
                Seen::Retune(11),
                Seen::OpenGate,
                Seen::Station(StationNotice::Moved(channel(11))),
            ]
        );
        assert_eq!(arbiter.poll_deadline(), None);
        assert!(run(&mut arbiter, &mut port, away.from()).is_empty());
    });
}

#[test]
fn loss_deadline_waits_for_the_gate_terminal_then_expires_once() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
        let _lease = lease(arbiter.request(at(0), scan(6, 5), place));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(52)) else {
            panic!("the scan must close the gate");
        };
        arbiter.upstream(at(53), UpstreamFact::Lost);
        let due = at(53).saturating_add(arbiter.policy.loss.after);
        assert!(arbiter.poll_action(due).is_none());
        assert_eq!(
            arbiter.poll_deadline(),
            None,
            "only the gate terminal can progress"
        );
        arbiter.gate_closed(port.issuer.close(gate));
        let events = run(&mut arbiter, &mut port, due);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Seen::AccessPoint(AccessPointCommand::Stop)))
                .count(),
            1
        );
        assert_eq!(arbiter.poll_deadline(), None);
        assert!(run(&mut arbiter, &mut port, due).is_empty());
    });
}

#[test]
fn a_retuned_absence_does_not_charge_its_outbound_move_twice() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let lease = lease(arbiter.request(at(0), scan(6, 100), place));
        assert_eq!(lease.until(), Some(at(72)));
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(52)) else {
            panic!("close");
        };
        arbiter.gate_closed(port.issuer.close(gate));
        let Some(ArbiterAction::Retune(pending)) = arbiter.poll_action(at(52)) else {
            panic!("retune");
        };
        arbiter.retuned(pending);
        let events = run(&mut arbiter, &mut port, at(69));
        assert_eq!(
            events,
            [
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseStarted(lease.id())),
            ]
        );
        assert_eq!(arbiter.poll_deadline(), Some(at(70)));
        let events = run(&mut arbiter, &mut port, at(70));
        assert_eq!(
            events,
            [
                Seen::CloseGate(()),
                Seen::Retune(1),
                Seen::OpenGate,
                Seen::Station(StationNotice::LeaseEnded(self::place())),
            ]
        );
        assert_eq!(arbiter.channel(), Some(channel(1)));
    });
}

#[test]
fn a_cancelled_starting_absence_delivers_its_pending_leave_and_both_places() {
    for before_deadline in [false, true] {
        installed(|mint| {
            let mut policy = ArbiterPolicy::DEFAULT;
            policy.follow.unservable = coordinator::ApUnservable::LeaveUpstream;
            let (mut arbiter, home, place, mut port) = connected_with_policy(mint, policy);
            let away = absence(arbiter.request_absence(at(0), scan(6, 10), &home, place));
            arbiter.ready(away.id());
            let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(away.from())
            else {
                panic!("close");
            };
            arbiter.upstream(
                at(53),
                UpstreamFact::SwitchAnnounced {
                    target: SupportedChannel::new(Channel::ghz5(36, ChannelWidth::Mhz20).unwrap())
                        .unwrap(),
                    mode: ChannelSwitchMode::Continue,
                    at: at(100),
                },
            );
            arbiter.gate_closed(port.issuer.close(gate));
            let now = if before_deadline {
                at(53)
            } else {
                away.until().unwrap()
            };
            let events = run(&mut arbiter, &mut port, now);
            let reason = CancelReason::Switch;
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        Seen::Station(StationNotice::Leave(LeaveReason::Unservable))
                    ))
                    .count(),
                1
            );
            let notices: Vec<_> = events
                .into_iter()
                .filter(|event| matches!(event, Seen::Station(_)))
                .collect();
            assert_eq!(
                notices,
                [
                    Seen::Station(StationNotice::AbsenceCancelled(reason, absence_place())),
                    Seen::Station(StationNotice::Leave(LeaveReason::Unservable)),
                ]
            );
            assert_eq!(arbiter.channel(), Some(channel(1)));
            arbiter.release(home);
            assert!(
                run(&mut arbiter, &mut port, now)
                    .contains(&Seen::Station(StationNotice::LeaseEnded(self::place())))
            );
            assert!(run(&mut arbiter, &mut port, now).is_empty());
        });
    }
}

#[test]
fn an_absence_returns_before_its_finite_primary_lease_expires() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let home = lease(arbiter.request(at(0), scan(1, 60), place));
        run(&mut arbiter, &mut port, at(0));
        let away = absence(arbiter.request_absence(at(0), scan(6, 100), &home, absence_place()));
        assert_eq!(away.until(), home.until());
        arbiter.ready(away.id());
        run(&mut arbiter, &mut port, away.from());
        assert_eq!(arbiter.poll_deadline(), away.listen_until());
        let seen = run(&mut arbiter, &mut port, away.listen_until().unwrap());
        assert!(seen.contains(&Seen::Station(StationNotice::AbsenceEnded(absence_place()))));
        let seen = run(&mut arbiter, &mut port, home.until().unwrap());
        assert!(seen.contains(&Seen::Station(StationNotice::LeaseEnded(self::place()))));
        assert_eq!(arbiter.poll_deadline(), None);
    });
}

#[test]
fn an_absence_starting_after_primary_expiry_returns_its_place_and_deadline() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let home = lease(arbiter.request(at(0), scan(1, 40), place));
        run(&mut arbiter, &mut port, at(0));
        assert!(
            matches!(arbiter.request_absence(at(0), scan(6, 10), &home, absence_place()), AbsenceAnswer::Refused(AbsenceRefusal::PrimaryEnds { at: end }, _) if Some(end) == home.until())
        );
        assert!(arbiter.absence.is_none());
        assert_eq!(arbiter.poll_deadline(), home.until());
    });
}

#[test]
fn beacon_slots_match_the_tbtt_grid_and_guard_without_iteration() {
    let schedule = ApSchedule {
        channel: channel(1),
        next_tbtt: at(50),
        beacon_interval: BeaconInterval::new(Duration::from_millis(100)).unwrap(),
    };
    for earliest in [0, 49, 50, 51, 52, 53, 151, 152, 153, 10_053] {
        let expected = (0..200)
            .map(|n| at(52 + n * 100))
            .find(|&at| at >= self::at(earliest))
            .unwrap();
        assert_eq!(
            slot(schedule, at(earliest), Duration::from_millis(2)),
            Some(expected)
        );
        assert_eq!(
            schedule.switch_at(at(earliest), 0),
            slot(schedule, at(earliest), Duration::ZERO)
        );
    }
    let schedule = ApSchedule {
        channel: channel(1),
        next_tbtt: Instant::from_micros(0),
        beacon_interval: BeaconInterval::new(Duration::from_micros(1)).unwrap(),
    };
    assert_eq!(
        slot(schedule, Instant::from_micros(u64::MAX), Duration::ZERO),
        Some(Instant::from_micros(u64::MAX))
    );
    let schedule = ApSchedule {
        beacon_interval: BeaconInterval::new(Duration::from_micros(2)).unwrap(),
        ..schedule
    };
    assert_eq!(
        slot(schedule, Instant::from_micros(u64::MAX), Duration::ZERO),
        None
    );
    let schedule = ApSchedule {
        next_tbtt: Instant::from_micros(u64::MAX),
        ..schedule
    };
    assert_eq!(slot(schedule, at(0), Duration::from_micros(1)), None);
    assert_eq!(schedule.switch_at(at(0), 1), None);
    assert!(BeaconInterval::new(Duration::ZERO).is_none());
}

#[test]
fn an_unrepresentable_beacon_slot_returns_the_admission_place() {
    installed(|mint| {
        let (mut arbiter, place, _) = beside_access_point::<NoReservation>(mint);
        arbiter.access_point(
            at(0),
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: Instant::from_micros(u64::MAX),
                beacon_interval: BeaconInterval::new(Duration::from_millis(100)).unwrap(),
            },
        );
        assert!(matches!(
            arbiter.request(at(0), scan(6, 10), place),
            Answer::Refused(Refusal::ScheduleExhausted, _)
        ));
        assert!(arbiter.primary.is_none());
    });
}
