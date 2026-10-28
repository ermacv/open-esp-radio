extern crate std;

use core::{cell::RefCell, future::Future};
use std::{rc::Rc, vec, vec::Vec};

use embassy_futures::{
    block_on,
    select::{Either, select},
    yield_now,
};
use oer_esp32s31_hal::ieee80211::init::{
    MacColdStartConfig, MacColdStartOutcome, MacSlowClockCalibration,
};
use oer_esp32s31_hal::{
    ieee80211::client::{WifiClocked, WifiCold},
    root::RadioHardware,
};
use oer_esp32s31_phy::{PhyCalibrationPath, PhyState};
use oer_esp32s31_radio_client::{
    ClientCells, Leaving, PhyFault, RcSlow, RegistrationFault, RequestKind, claim_for_host_test,
    wifi::{
        RetuneReport, TrackReport, WifiJoin, WifiJoined, WifiLeave, WifiLeft, WifiRetune,
        WifiRetuned, WifiReturned, WifiTrack, WifiTracked,
    },
};
use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_time::{Clock as _, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::*;

/// The model's failed preparation.
pub(super) const PREPARE_CAUSE: PoisonCause =
    PoisonCause::Registration(RegistrationFault::Step(PhyFault::EdgeTimedOut));
/// The preparation poison every later request names.
pub(super) const PREPARE_POISON: Poison = Poison {
    origin: PoisonOrigin::Prepare,
    cause: PREPARE_CAUSE,
};
/// The model's failed tracking tick.
const TICK_CAUSE: PoisonCause = PoisonCause::Tracking(PhyFault::EdgeTimedOut);

pub(super) const fn wifi_poison(request: RequestKind, cause: PoisonCause) -> Poison {
    Poison::request(oer_esp32s31_hal::domain::RadioClient::Wifi, request, cause)
}
const TRACK_POISON: Poison = wifi_poison(
    RequestKind::Track,
    PoisonCause::Channel(PhyFault::UnexpectedBinding),
);
const RETUNE_POISON: Poison = wifi_poison(
    RequestKind::Retune,
    PoisonCause::Channel(PhyFault::EdgeTimedOut),
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Prepare(Result<PhyPreparation, Fault>),
    Enter,
    Track,
    Retune,
    Exit,
    CloseRf,
    Tick,
    #[cfg(feature = "bluetooth")]
    BleEnter,
    #[cfg(feature = "bluetooth")]
    BleTrack,
    #[cfg(feature = "bluetooth")]
    BleExit,
}

#[derive(Default)]
struct Script {
    wifi: bool,
    #[cfg(feature = "bluetooth")]
    bluetooth: bool,
    #[cfg(feature = "bluetooth")]
    ble_installation: u32,
    #[cfg(feature = "bluetooth")]
    ble_tracking: bool,
    #[cfg(feature = "bluetooth")]
    ble_stopped: bool,
    #[cfg(feature = "bluetooth")]
    ble_fault: bool,
    #[cfg(feature = "bluetooth")]
    ble_available: bool,
    registered: bool,
    rf_open: bool,
    fail_prepare: bool,
    refuse_enter: bool,
    client_fails_enter: bool,
    poison_track: bool,
    poison_retune: bool,
    /// The periodic tick of this number (from 1) fails.
    fail_tick: Option<usize>,
    ticks: usize,
    not_last: bool,
    close_recoverable: bool,
    deadline: Option<Instant>,
    log: Vec<Event>,
}

#[derive(Clone)]
struct Model(Rc<RefCell<Script>>);

impl Model {
    fn log(&self) -> Vec<Event> {
        self.0.borrow().log.clone()
    }
    fn push(&self, event: Event) {
        self.0.borrow_mut().log.push(event);
    }
}

impl PhyDomain for Model {
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault> {
        let mut script = self.0.borrow_mut();
        let result = if script.fail_prepare {
            Err(Fault(PREPARE_CAUSE))
        } else if !script.registered {
            script.registered = true;
            script.rf_open = true;
            Ok(REGISTRATION)
        } else if !script.rf_open {
            script.rf_open = true;
            Ok(PhyPreparation::Woke { micros: 3 })
        } else {
            Ok(PhyPreparation::AlreadyOpen)
        };
        script.log.push(Event::Prepare(result));
        result
    }

    async fn close_rf(&mut self, last: LastClientLeft) -> Result<(), CloseFault> {
        let LastClientLeft(()) = last;
        let mut script = self.0.borrow_mut();
        script.log.push(Event::CloseRf);
        if script.close_recoverable {
            return Err(CloseFault::Recoverable);
        }
        script.rf_open = false;
        Ok(())
    }

    fn tracking_deadline(&self) -> Option<Instant> {
        self.0.borrow().deadline
    }

    async fn track_tick(&mut self) -> Result<(), Fault> {
        let mut script = self.0.borrow_mut();
        script.deadline = script
            .deadline
            .and_then(|deadline| deadline.checked_add(Duration::from_secs(1)));
        script.log.push(Event::Tick);
        script.ticks += 1;
        if script.fail_tick == Some(script.ticks) {
            return Err(Fault(TICK_CAUSE));
        }
        Ok(())
    }
}

impl ClientBackend<Wifi, RcSlow> for Model {
    async fn enter(
        &mut self,
        join: WifiJoin,
        phy: PhyPreparation,
        _lp: &LpClock<RcSlow>,
    ) -> Result<WifiJoined, Failed<WifiReturned>> {
        self.push(Event::Enter);
        let cause = {
            let mut script = self.0.borrow_mut();
            // A client failure happens once; the retry enters.
            let client_failed = core::mem::take(&mut script.client_fails_enter);
            if script.refuse_enter {
                Some(Failure::Refused)
            } else {
                client_failed.then_some(Failure::ClientFailed)
            }
        };
        if let Some(cause) = cause {
            return Err(Failed {
                returned: WifiReturned::Cold(join.wifi),
                cause,
            });
        }
        self.0.borrow_mut().wifi = true;
        Ok(WifiJoined {
            wifi: WifiClocked::for_host_test(join.wifi),
            phy,
        })
    }

    async fn track(&mut self, track: WifiTrack) -> Result<WifiTracked, Failed<WifiClocked>> {
        self.push(Event::Track);
        if self.0.borrow().poison_track {
            return Err(Failed {
                returned: track.wifi,
                cause: Failure::Poisoned(TRACK_POISON),
            });
        }
        Ok(WifiTracked {
            wifi: track.wifi,
            report: TrackReport {
                probe_priority: crate::mac_sources::probe_priority(
                    oer_espressif_coex::CoexPtiTable::VENDOR,
                ),
                tracking_micros: Some(7),
                temperature: PhyState::default().temperature_observation(),
                tx_power: PhyState::default().tx_target_power_profile(),
                mac: MacColdStartOutcome {
                    handshake_samples: 1,
                    handshake_observations: 2,
                },
            },
        })
    }

    async fn retune(&mut self, retune: WifiRetune) -> Result<WifiRetuned, Failed<WifiClocked>> {
        self.push(Event::Retune);
        if self.0.borrow().poison_retune {
            return Err(Failed {
                returned: retune.wifi,
                cause: Failure::Poisoned(RETUNE_POISON),
            });
        }
        Ok(WifiRetuned {
            wifi: retune.wifi,
            report: RetuneReport {
                probe_priority: crate::mac_sources::probe_priority(
                    oer_espressif_coex::CoexPtiTable::VENDOR,
                ),
                micros: 11,
                temperature: PhyState::default().temperature_observation(),
                tx_power: PhyState::default().tx_target_power_profile(),
            },
        })
    }

    async fn exit(&mut self, leave: WifiLeave) -> (WifiLeft, Exit) {
        self.push(Event::Exit);
        self.0.borrow_mut().wifi = false;
        let WifiLeave { wifi } = leave;
        (
            WifiLeft {
                wifi: WifiReturned::Clocked(wifi),
            },
            Exit {
                last: {
                    let script = self.0.borrow();
                    #[cfg(feature = "bluetooth")]
                    let bluetooth = script.bluetooth;
                    #[cfg(not(feature = "bluetooth"))]
                    let bluetooth = false;
                    !script.not_last && !bluetooth
                },
                health: DomainHealth::Healthy,
            },
        )
    }
}

fn cold() -> WifiCold {
    let (_domain, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_esp32s31_hal::domain::transaction::PhyIdentity::for_host_test(),
        )
        .expect("identity capacity");
    WifiCold::from_partition(partitions.wifi)
}

fn track_request(wifi: WifiClocked) -> WifiTrack {
    WifiTrack {
        wifi,
        channel: oer_radio_regulatory::TEST_DOMAIN
            .permit(WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).unwrap())
            .unwrap(),
        tx_ceiling: oer_radio_regulatory::TEST_DOMAIN
            .permit(WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz20).unwrap())
            .unwrap()
            .tx_ceiling(oer_radio_regulatory::PowerLimits {
                antenna_gain_mbm: 0,
                chip_maximum_mbm: 2000,
                owner_cap_mbm: None,
            }),
        mac: MacColdStartConfig {
            handshake_deadline: oer_time::Duration::from_millis(1),
            station_address: [2, 0, 0, 0, 0, 1],
            access_point_address: [2, 0, 0, 0, 0, 2],
        },
        slow_clock: MacSlowClockCalibration::Unavailable,
        delay_random: 0,
    }
}

/// Run `client` against an actor over `model` until `client` finishes.
fn run<F: Future<Output = ()>>(
    model: &Model,
    clock: &'static VirtualClock,
    client: impl FnOnce(ClientCells) -> F,
) {
    let (end, cells) = claim_for_host_test().expect("fresh radio client cells");
    let lp = LpClock::<RcSlow>::for_host_test();
    let mut actor = RadioSystem::new(end, model.clone(), clock, lp);
    match block_on(select(actor.run(), client(cells))) {
        Either::First(never) => match never {},
        Either::Second(()) => {}
    }
}

/// The instant `micros` past a default clock's start.
fn at(micros: u64) -> Instant {
    oer_time_virtual::START
        .checked_add(Duration::from_micros(micros))
        .unwrap()
}

fn clock() -> &'static VirtualClock {
    std::boxed::Box::leak(std::boxed::Box::new(VirtualClock::default()))
}

fn model(script: Script) -> Model {
    Model(Rc::new(RefCell::new(script)))
}

/// The model's registration; under `cfg(oer_trace)` its outcome carries no
/// RF calibration and no not-ready runs.
const REGISTRATION: PhyPreparation = PhyPreparation::Registered {
    calibration_path: PhyCalibrationPath::FullUncached,
    micros: 10,
    #[cfg(oer_trace)]
    outcome: oer_esp32s31_phy::PhyRegisterOutcome {
        rf_calibration: None,
        ready_waits: oer_esp32s31_phy::ReadyWaitDiagnostics {
            tx_dc_max_not_ready_samples: 0,
            pwdet_sar_max_not_ready_samples: 0,
        },
        full_calibration_performed: true,
        calibration_path: PhyCalibrationPath::FullUncached,
    },
};

const REGISTERED: Event = Event::Prepare(Ok(REGISTRATION));

#[test]
fn a_borrowed_run_keeps_membership_when_polled_again() {
    let model = model(Script::default());
    let (end, cells) = claim_for_host_test().expect("fresh radio client cells");
    let lp = LpClock::<RcSlow>::for_host_test();
    let mut actor = RadioSystem::new(end, model.clone(), clock(), lp);
    let Either::Second(Ok((cell, joined))) = block_on(select(
        actor.run(),
        cells.wifi.join(WifiJoin { wifi: cold() }),
    )) else {
        panic!("the actor runs until the client completes")
    };
    let Ok(joined) = joined else {
        panic!("a cold join registers")
    };

    // The completed select drops the first run future. The same actor
    // must still own the member and close RF on its last leave.
    let Either::Second(Ok((_cell, left))) = block_on(select(
        actor.run(),
        cell.leave(
            Leaving::Untracked(joined.member),
            WifiLeave {
                wifi: joined.payload.wifi,
            },
        ),
    )) else {
        panic!("the actor runs until the client completes")
    };
    assert_eq!(left.domain, DomainHealth::Healthy);
    assert_eq!(
        model.log(),
        vec![REGISTERED, Event::Enter, Event::Exit, Event::CloseRf]
    );
}

#[test]
fn join_track_and_leave_run_in_order_and_the_last_leave_closes_rf() {
    let model = model(Script::default());
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(joined) = joined else {
            panic!("a cold join registers")
        };
        assert_eq!(joined.epoch.get(), 0);
        assert!(matches!(
            joined.payload.phy,
            PhyPreparation::Registered { .. }
        ));
        let (cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let Ok(tracked) = tracked else {
            panic!("tracking runs")
        };
        assert_eq!(tracked.payload.report.tracking_micros, Some(7));
        let (_cell, left) = cell
            .leave(
                Leaving::Tracked(tracked.member),
                WifiLeave {
                    wifi: tracked.payload.wifi,
                },
            )
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Healthy);
    });
    assert_eq!(
        model.log(),
        vec![
            REGISTERED,
            Event::Enter,
            Event::Track,
            Event::Exit,
            Event::CloseRf,
        ]
    );
}

#[test]
fn a_join_after_rf_closed_wakes_rf_with_a_new_epoch() {
    let model = model(Script::default());
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(first) = joined else {
            panic!("a cold join registers")
        };
        let (cell, _left) = cell
            .leave(
                Leaving::Untracked(first.member),
                WifiLeave {
                    wifi: first.payload.wifi,
                },
            )
            .await
            .unwrap();
        let (_cell, joined) = cell.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(second) = joined else {
            panic!("a join wakes closed RF")
        };
        assert_eq!(second.payload.phy, PhyPreparation::Woke { micros: 3 });
        assert_eq!((first.epoch.get(), second.epoch.get()), (0, 1));
    });
    assert_eq!(
        model.log()[..5],
        [
            REGISTERED,
            Event::Enter,
            Event::Exit,
            Event::CloseRf,
            Event::Prepare(Ok(PhyPreparation::Woke { micros: 3 })),
        ]
    );
}

#[test]
fn a_refused_first_member_leaves_rf_closed_and_gets_its_partition_back() {
    let model = model(Script {
        refuse_enter: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (_cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Err(failed) = joined else {
            panic!("the backend refused")
        };
        assert_eq!(failed.cause, Failure::Refused);
        assert!(matches!(failed.returned, WifiReturned::Cold(_)));
    });
    assert_eq!(model.log(), vec![REGISTERED, Event::Enter, Event::CloseRf,]);
}

#[test]
fn a_failed_preparation_poisons_the_domain_for_every_later_request() {
    let model = model(Script {
        fail_prepare: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Err(failed) = joined else {
            panic!("the preparation failed")
        };
        assert_eq!(failed.cause, Failure::Poisoned(PREPARE_POISON));
        let (_cell, joined) = cell.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Err(failed) = joined else {
            panic!("the domain stays poisoned")
        };
        assert_eq!(failed.cause, Failure::Poisoned(PREPARE_POISON));
        assert!(matches!(failed.returned, WifiReturned::Cold(_)));
    });
    assert_eq!(model.log(), vec![Event::Prepare(Err(Fault(PREPARE_CAUSE)))]);
}

#[test]
fn a_poisoned_track_hands_back_the_member_and_the_client() {
    let model = model(Script {
        poison_track: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(joined) = joined else {
            panic!("a cold join registers")
        };
        let (cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let Err(Failed {
            returned: (member, wifi),
            cause,
        }) = tracked
        else {
            panic!("tracking failed")
        };
        assert_eq!(cause, Failure::Poisoned(TRACK_POISON));
        let (_cell, left) = cell
            .leave(Leaving::Untracked(member), WifiLeave { wifi })
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Poisoned(TRACK_POISON));
    });
    // A poisoned domain closes no RF.
    assert!(!model.log().contains(&Event::CloseRf));
}

#[test]
fn a_member_that_is_not_the_last_keeps_rf_open() {
    let model = model(Script {
        not_last: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(joined) = joined else {
            panic!("a cold join registers")
        };
        let (_cell, _left) = cell
            .leave(
                Leaving::Untracked(joined.member),
                WifiLeave {
                    wifi: joined.payload.wifi,
                },
            )
            .await
            .unwrap();
    });
    assert!(!model.log().contains(&Event::CloseRf));
}

#[test]
fn a_tracked_member_gets_periodic_ticks_when_they_fall_due() {
    let clock = clock();
    let model = model(Script {
        deadline: Some(at(1_000_000)),
        ..Script::default()
    });
    run(&model, clock, |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(joined) = joined else {
            panic!("a cold join registers")
        };
        let (_cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let Ok(tracked) = tracked else {
            panic!("tracking runs")
        };
        let _ = tracked;
        for _ in 0..4 {
            yield_now().await;
        }
        clock.advance_to(at(1_000_000));
        for _ in 0..4 {
            yield_now().await;
        }
        clock.advance_to(at(2_000_000));
        for _ in 0..4 {
            yield_now().await;
        }
        assert_eq!(clock.now(), at(2_000_000));
    });
    assert_eq!(
        model
            .log()
            .iter()
            .filter(|event| **event == Event::Tick)
            .count(),
        2
    );
}

#[test]
fn a_failed_tracking_tick_names_itself_in_the_next_retune_and_stops_tracking() {
    let clock = clock();
    let model = model(Script {
        deadline: Some(at(1_000_000)),
        fail_tick: Some(110),
        ..Script::default()
    });
    let tick = Poison {
        origin: PoisonOrigin::TrackingTick,
        cause: TICK_CAUSE,
    };
    run(&model, clock, |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("join");
        let (cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let tracked = tracked.ok().expect("track");
        // A long association: the tick falls due once a second.
        for second in 1..=120 {
            clock.advance_to(at(second * 1_000_000));
            for _ in 0..4 {
                yield_now().await;
            }
        }
        let config = track_request(tracked.payload.wifi);
        let (cell, answer) = cell
            .retune(
                tracked.member,
                WifiRetune {
                    wifi: config.wifi,
                    channel: config.channel,
                    tx_ceiling: config.tx_ceiling,
                },
            )
            .await
            .unwrap();
        let Err(Failed {
            returned: (member, wifi),
            cause,
        }) = answer
        else {
            panic!("the first request after the failed tick is refused");
        };
        assert_eq!(cause, Failure::Poisoned(tick));
        let (_, left) = cell
            .leave(Leaving::Tracked(member), WifiLeave { wifi })
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Poisoned(tick));
    });
    let log = model.log();
    // Tracking stops at the failed tick, and the refused retune reaches no
    // hardware.
    assert_eq!(
        log.iter().filter(|event| **event == Event::Tick).count(),
        110
    );
    assert!(!log.contains(&Event::Retune));
    assert!(!log.contains(&Event::CloseRf));
}

#[test]
fn a_recoverable_close_keeps_rf_open_in_the_same_epoch() {
    let model = model(Script {
        close_recoverable: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(first) = joined else {
            panic!("a cold join registers")
        };
        let (cell, left) = cell
            .leave(
                Leaving::Untracked(first.member),
                WifiLeave {
                    wifi: first.payload.wifi,
                },
            )
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Healthy);
        let (_cell, joined) = cell.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Ok(second) = joined else {
            panic!("the domain stays healthy")
        };
        assert_eq!(second.payload.phy, PhyPreparation::AlreadyOpen);
        assert_eq!(second.epoch, first.epoch);
    });
    assert_eq!(
        model.log(),
        vec![
            REGISTERED,
            Event::Enter,
            Event::Exit,
            Event::CloseRf,
            Event::Prepare(Ok(PhyPreparation::AlreadyOpen)),
            Event::Enter,
        ]
    );
}

#[test]
fn a_client_failure_keeps_the_domain_healthy_and_closes_rf_it_opened() {
    let model = model(Script {
        client_fails_enter: true,
        ..Script::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let Err(failed) = joined else {
            panic!("the client failed")
        };
        assert_eq!(failed.cause, Failure::ClientFailed);
        let (_cell, joined) = cell.join(WifiJoin { wifi: cold() }).await.unwrap();
        assert!(joined.is_ok(), "a client failure poisons nothing");
    });
    assert_eq!(
        model.log()[..4],
        [
            REGISTERED,
            Event::Enter,
            Event::CloseRf,
            Event::Prepare(Ok(PhyPreparation::Woke { micros: 3 }))
        ]
    );
}

#[test]
fn retune_preserves_membership_and_the_last_leave_still_closes_rf() {
    let model = model(Script::default());
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("join");
        let (cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let tracked = tracked.ok().expect("track");
        let config = track_request(tracked.payload.wifi);
        let (cell, tuned) = cell
            .retune(
                tracked.member,
                WifiRetune {
                    wifi: config.wifi,
                    channel: config.channel,
                    tx_ceiling: config.tx_ceiling,
                },
            )
            .await
            .unwrap();
        let tuned = tuned.ok().expect("retune");
        assert_eq!(tuned.payload.report.micros, 11);
        let (_, left) = cell
            .leave(
                Leaving::Tracked(tuned.member),
                WifiLeave {
                    wifi: tuned.payload.wifi,
                },
            )
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Healthy);
    });
    assert_eq!(
        model.log(),
        [
            REGISTERED,
            Event::Enter,
            Event::Track,
            Event::Retune,
            Event::Exit,
            Event::CloseRf
        ]
    );
}

#[test]
fn a_poisoned_retune_returns_custody_and_refuses_further_hardware_work() {
    let model = model(Script {
        poison_retune: true,
        ..Default::default()
    });
    run(&model, clock(), |cells| async move {
        let (cell, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("join");
        let (cell, tracked) = cell
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let tracked = tracked.ok().expect("track");
        let config = track_request(tracked.payload.wifi);
        let (cell, answer) = cell
            .retune(
                tracked.member,
                WifiRetune {
                    wifi: config.wifi,
                    channel: config.channel,
                    tx_ceiling: config.tx_ceiling,
                },
            )
            .await
            .unwrap();
        let Err(Failed {
            returned: (member, wifi),
            cause,
        }) = answer
        else {
            panic!("retune fails");
        };
        assert_eq!(cause, Failure::Poisoned(RETUNE_POISON));
        let config = track_request(wifi);
        let (cell, answer) = cell
            .retune(
                member,
                WifiRetune {
                    wifi: config.wifi,
                    channel: config.channel,
                    tx_ceiling: config.tx_ceiling,
                },
            )
            .await
            .unwrap();
        let Err(Failed {
            returned: (member, wifi),
            cause,
        }) = answer
        else {
            panic!("poison is retained");
        };
        assert_eq!(cause, Failure::Poisoned(RETUNE_POISON));
        let (_, left) = cell
            .leave(Leaving::Tracked(member), WifiLeave { wifi })
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Poisoned(RETUNE_POISON));
    });
    assert_eq!(
        model.log(),
        [
            REGISTERED,
            Event::Enter,
            Event::Track,
            Event::Retune,
            Event::Exit
        ]
    );
}

#[cfg(feature = "bluetooth")]
mod bluetooth;

#[test]
fn a_rung_doorbell_wins_over_an_already_due_tracking_deadline() {
    use core::{
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let model = model(Script::default());
    let clock = clock();
    let (end, cells) = claim_for_host_test().expect("fresh radio client cells");
    let actor = RadioSystem::new(end, model, clock, LpClock::<RcSlow>::for_host_test());
    let mut join = pin!(cells.wifi.join(WifiJoin { wifi: cold() }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(join.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        pin!(actor.wait(Some(clock.now()))).as_mut().poll(&mut cx),
        Poll::Ready(false)
    );
}

#[test]
fn no_tracking_deadline_waits_only_for_a_posted_request() {
    use core::{
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let model = model(Script::default());
    let clock = clock();
    let (end, cells) = claim_for_host_test().expect("fresh radio client cells");
    let actor = RadioSystem::new(end, model, clock, LpClock::<RcSlow>::for_host_test());
    let mut wait = pin!(actor.wait(None));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    assert_eq!(clock.next_deadline(), None);
    clock
        .advance(Duration::from_millis(1))
        .expect("virtual time fits");
    assert!(wait.as_mut().poll(&mut cx).is_pending());
    let mut join = pin!(cells.wifi.join(WifiJoin { wifi: cold() }));
    assert!(join.as_mut().poll(&mut cx).is_pending());
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(false));
}
