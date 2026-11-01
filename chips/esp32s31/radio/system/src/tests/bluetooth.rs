use super::*;
use oer_esp32s31_radio_client::bluetooth::{
    self, Bluetooth, BluetoothJoin, BluetoothJoined, BluetoothLeft, Installation, Permission,
};

const fn ble_poison(request: RequestKind, cause: PoisonCause) -> Poison {
    Poison::request(
        oer_esp32s31_hal::domain::RadioClient::Bluetooth,
        request,
        cause,
    )
}
const BLE_TRACK_POISON: Poison = ble_poison(RequestKind::Track, PoisonCause::Controller);
const BLE_LEAVE_POISON: Poison = ble_poison(RequestKind::Leave, PoisonCause::Custody);

struct ModelAttachment;
impl BluetoothBackend<Model, RcSlow> for ModelAttachment {
    fn attached(&mut self, _: &mut Model) {}
    fn detached(&mut self, _: &mut Model) {}
    fn is_stopped(&self, shared: &Model) -> bool {
        !shared.0.borrow().bluetooth
    }
    fn admits(&self, shared: &Model, _: &BluetoothJoin) -> bool {
        let script = shared.0.borrow();
        script.ble_available && !script.bluetooth
    }
    async fn enter(
        &mut self,
        shared: &mut Model,
        _: BluetoothJoin,
        phy: PhyPreparation,
        _: &LpClock<RcSlow>,
    ) -> Result<BluetoothJoined, Failed<BluetoothJoin>> {
        shared.push(Event::BleEnter);
        let mut script = shared.0.borrow_mut();
        assert!(
            !script.bluetooth,
            "a second role must use the existing Bluetooth client"
        );
        script.bluetooth = true;
        script.ble_stopped = false;
        script.ble_installation += 1;
        Ok(BluetoothJoined {
            installation: Installation(script.ble_installation),
            phy,
        })
    }
    async fn track(
        &mut self,
        shared: &mut Model,
        installation: Installation,
    ) -> Result<Installation, Failed<Installation>> {
        shared.push(Event::BleTrack);
        // Model an asynchronous initial tracking transaction, including a
        // suspension while the other client's request stays in its own cell.
        shared.0.borrow_mut().ble_tracking = true;
        yield_now().await;
        shared.0.borrow_mut().ble_tracking = false;
        if shared.0.borrow().ble_fault {
            Err(Failed {
                returned: installation,
                cause: Failure::Poisoned(BLE_TRACK_POISON),
            })
        } else {
            Ok(installation)
        }
    }
    async fn exit(
        &mut self,
        shared: &mut Model,
        installation: Installation,
    ) -> (BluetoothLeft, Exit) {
        shared.push(Event::BleExit);
        let mut script = shared.0.borrow_mut();
        if installation.0 != script.ble_installation || !script.ble_stopped {
            return (
                BluetoothLeft { stopped: false },
                Exit {
                    last: false,
                    health: DomainHealth::Poisoned(BLE_LEAVE_POISON),
                },
            );
        }
        script.bluetooth = false;
        (
            BluetoothLeft { stopped: true },
            Exit {
                last: !script.wifi,
                health: DomainHealth::Healthy,
            },
        )
    }
}

fn join() -> BluetoothJoin {
    BluetoothJoin::new(
        Permission::new(
            oer_radio_regulatory::TEST_DOMAIN
                .permit(oer_bluetooth_le_port::LeBand)
                .expect("LE test band"),
            oer_radio_regulatory::PowerLimits {
                antenna_gain_mbm: 0,
                chip_maximum_mbm: 2100,
                owner_cap_mbm: Some(1000),
            },
        ),
        bluetooth::Operation::DtmRx,
        None,
    )
    .expect("permitted receiver installation")
}

fn multi<F: Future<Output = ()>>(
    model: &Model,
    client: impl FnOnce(ClientCells, oer_esp32s31_radio_client::MembershipCell<Bluetooth>) -> F,
) {
    let (end, cells) = claim_for_host_test().expect("fresh radio client cells");
    let (inbox, ble) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let mut attachment = ModelAttachment;
    let mut attached = actor
        .attach_bluetooth(inbox, &mut attachment)
        .ok()
        .expect("one model attachment");
    match block_on(select(attached.run(), client(cells, ble))) {
        Either::First(fault) => panic!("the periodic tracking failed: {fault:?}"),
        Either::Second(()) => {}
    }
}

#[test]
fn wifi_then_ble_share_one_rf_epoch_and_only_the_last_leave_closes_it() {
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    multi(&model, |cells, ble| async {
        let (wifi, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("Wi-Fi first");
        let epoch = joined.epoch;
        let (wifi, tracked) = wifi
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let tracked = tracked.ok().expect("Wi-Fi tracking");
        let (ble, joined) = ble.join(join()).await.unwrap();
        let joined = joined.ok().expect("BLE second");
        assert_eq!(joined.epoch, epoch);
        assert_eq!(joined.payload.phy, PhyPreparation::AlreadyOpen);
        let (ble, bt) = ble
            .track(joined.member, joined.payload.installation)
            .await
            .unwrap();
        let bt = bt.ok().expect("BLE initial tracking");
        model.0.borrow_mut().ble_stopped = true;
        let (_, left) = ble
            .leave(Leaving::Tracked(bt.member), bt.payload)
            .await
            .unwrap();
        assert!(left.payload.stopped);
        assert_eq!(left.domain, DomainHealth::Healthy);
        assert!(!model.log().contains(&Event::CloseRf));
        let (_, left) = wifi
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
            Event::Prepare(Ok(PhyPreparation::AlreadyOpen)),
            Event::BleEnter,
            Event::BleTrack,
            Event::BleExit,
            Event::Exit,
            Event::CloseRf
        ]
    );
}

#[test]
fn ble_then_wifi_preserve_ble_membership_through_wifi_stop() {
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    multi(&model, |cells, ble| async {
        let (ble, joined) = ble.join(join()).await.unwrap();
        let joined = joined.ok().expect("BLE first");
        let epoch = joined.epoch;
        let (ble, bt) = ble
            .track(joined.member, joined.payload.installation)
            .await
            .unwrap();
        let bt = bt.ok().expect("BLE tracking");
        let (wifi, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("Wi-Fi second");
        assert_eq!(joined.epoch, epoch);
        let (wifi, tracked) = wifi
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let tracked = tracked.ok().expect("Wi-Fi tracking");
        let (_, left) = wifi
            .leave(
                Leaving::Tracked(tracked.member),
                WifiLeave {
                    wifi: tracked.payload.wifi,
                },
            )
            .await
            .unwrap();
        assert_eq!(left.domain, DomainHealth::Healthy);
        assert!(model.0.borrow().bluetooth);
        assert!(!model.log().contains(&Event::CloseRf));
        model.0.borrow_mut().ble_stopped = true;
        let (_, left) = ble
            .leave(Leaving::Tracked(bt.member), bt.payload)
            .await
            .unwrap();
        assert!(left.payload.stopped);
    });
    assert_eq!(model.log().last(), Some(&Event::CloseRf));
    assert_eq!(
        model
            .log()
            .iter()
            .filter(|event| matches!(event, Event::Prepare(Ok(PhyPreparation::Registered { .. }))))
            .count(),
        1
    );
}

#[test]
fn failed_ble_rf_work_poisons_the_same_domain_seen_by_wifi() {
    let model = model(Script {
        ble_available: true,
        ble_fault: true,
        ..Script::default()
    });
    multi(&model, |cells, ble| async {
        let (ble, joined) = ble.join(join()).await.unwrap();
        let joined = joined.ok().expect("BLE joined");
        let (_, tracked) = ble
            .track(joined.member, joined.payload.installation)
            .await
            .unwrap();
        assert_eq!(
            tracked.err().expect("RF write failed").cause,
            Failure::Poisoned(BLE_TRACK_POISON)
        );
        let (_, wifi) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        assert_eq!(
            wifi.err().expect("shared domain poisoned").cause,
            Failure::Poisoned(BLE_TRACK_POISON)
        );
    });
    assert_eq!(
        model.log(),
        vec![REGISTERED, Event::BleEnter, Event::BleTrack]
    );
}

#[test]
fn late_retirement_after_reinstall_never_releases_the_new_graph() {
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    multi(&model, |cells, ble| async {
        let (wifi, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await.unwrap();
        let joined = joined.ok().expect("Wi-Fi holds the common RF epoch");
        let epoch = joined.epoch;
        let (_wifi, wifi) = wifi
            .track(joined.member, track_request(joined.payload.wifi))
            .await
            .unwrap();
        let _wifi = wifi.ok().expect("Wi-Fi tracked through BLE reinstall");
        let (ble, first) = ble.join(join()).await.unwrap();
        let first = first.ok().expect("first installation");
        assert_eq!(first.epoch, epoch);
        let (ble, first) = ble
            .track(first.member, first.payload.installation)
            .await
            .unwrap();
        let first = first.ok().expect("first tracking");
        let old = first.payload;
        model.0.borrow_mut().ble_stopped = true;
        let (ble, _) = ble
            .leave(Leaving::Tracked(first.member), old)
            .await
            .unwrap();
        let (ble, next) = ble.join(join()).await.unwrap();
        let next = next.ok().expect("reinstall");
        assert_eq!(
            next.epoch, epoch,
            "Controller reinstall does not reopen common RF"
        );
        assert_ne!(next.payload.installation, old);
        let (ble, next) = ble
            .track(next.member, next.payload.installation)
            .await
            .unwrap();
        let next = next.ok().expect("new tracking");
        let (_, late) = ble.leave(Leaving::Tracked(next.member), old).await.unwrap();
        assert!(!late.payload.stopped);
        assert_eq!(late.domain, DomainHealth::Poisoned(BLE_LEAVE_POISON));
        assert!(model.0.borrow().bluetooth);
        assert!(model.0.borrow().wifi);
    });
    assert_eq!(
        model.log().iter().filter(|e| **e == Event::CloseRf).count(),
        0
    );
}

#[test]
fn preflight_refusal_never_registers_or_wakes_rf() {
    let model = model(Script::default());
    multi(&model, |_, ble| async {
        let (_, joined) = ble.join(join()).await.unwrap();
        assert_eq!(
            joined.err().expect("no available controller").cause,
            Failure::Refused
        );
    });
    assert!(model.log().is_empty());
}

#[test]
fn a_second_role_cannot_join_another_bluetooth_client() {
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    multi(&model, |_, ble| async {
        let (ble, first) = ble.join(join()).await.unwrap();
        let first = first.ok().expect("one Bluetooth installation");
        let (ble, first) = ble
            .track(first.member, first.payload.installation)
            .await
            .unwrap();
        let first = first.ok().expect("tracked Bluetooth");
        let (ble, duplicate) = ble.join(join()).await.unwrap();
        assert_eq!(
            duplicate
                .err()
                .expect("one Bluetooth client for all roles")
                .cause,
            Failure::Refused
        );
        model.0.borrow_mut().ble_stopped = true;
        let (_, left) = ble
            .leave(Leaving::Tracked(first.member), first.payload)
            .await
            .unwrap();
        assert!(left.payload.stopped);
    });
    assert_eq!(
        model.log(),
        vec![
            REGISTERED,
            Event::BleEnter,
            Event::BleTrack,
            Event::BleExit,
            Event::CloseRf
        ]
    );
}

#[test]
fn runtime_handover_waits_for_the_entered_tracking_transaction() {
    use core::pin::pin;
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    let (end, _) = claim_for_host_test().expect("fresh radio client cells");
    let (inbox, ble) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let mut attachment = ModelAttachment;
    let mut attached = actor
        .attach_bluetooth(inbox, &mut attachment)
        .ok()
        .expect("one model attachment");
    let mut client = pin!(async {
        let (ble, joined) = ble.join(join()).await.unwrap();
        let joined = joined.expect("accepted BLE join");
        let (ble, tracked) = ble
            .track(joined.member, joined.payload.installation)
            .await
            .unwrap();
        (ble, tracked.expect("completed initial tracking"))
    });
    let boundary = poll_fn(|cx| {
        assert!(
            !model.0.borrow().ble_tracking,
            "a runtime handover never polls through an entered RF transaction"
        );
        client.as_mut().poll(cx)
    });
    let (ble, tracked) =
        block_on(attached.run_until(boundary)).expect("the periodic tracking holds");
    assert!(!model.0.borrow().ble_tracking);
    assert!(matches!(model.log().last(), Some(Event::BleTrack)));
    // The same actor, RF epoch and member now perform the ordinary leave.
    model.0.borrow_mut().ble_stopped = true;
    let left =
        block_on(attached.run_until(ble.leave(Leaving::Tracked(tracked.member), tracked.payload)))
            .expect("the periodic tracking holds")
            .unwrap();
    assert!(left.1.payload.stopped);
    assert_eq!(model.log().last(), Some(&Event::CloseRf));
    let out =
        block_on(attached.run_until(core::future::ready(42))).expect("the periodic tracking holds");
    assert_eq!(out, 42);
}

#[test]
fn detach_returns_the_inbox_only_after_stop_and_never_moves_the_shared_actor() {
    let model = model(Script {
        ble_available: true,
        ..Script::default()
    });
    let (end, _) = claim_for_host_test().expect("fresh radio client cells");
    let (inbox, ble) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let backend = core::ptr::from_ref(&actor.backend);
    let core = core::ptr::from_ref(&actor.core);
    let mut attachment = ModelAttachment;
    let mut attached = actor
        .attach_bluetooth(inbox, &mut attachment)
        .ok()
        .expect("one model attachment");
    let (ble, joined) = block_on(attached.run_until(ble.join(join())))
        .expect("the periodic tracking holds")
        .unwrap();
    let joined = joined.expect("first installation");
    let epoch = joined.epoch;
    let mut attached = attached
        .detach()
        .err()
        .expect("a live installation retains its loan");
    model.0.borrow_mut().ble_stopped = true;
    let (ble, left) = block_on(attached.run_until(ble.leave(
        Leaving::Untracked(joined.member),
        joined.payload.installation,
    )))
    .expect("the periodic tracking holds")
    .unwrap();
    assert!(left.payload.stopped);
    let inbox = attached.detach().ok().expect("normal stop detaches");
    assert_eq!(backend, core::ptr::from_ref(&actor.backend));
    assert_eq!(core, core::ptr::from_ref(&actor.core));
    let mut attached = actor
        .attach_bluetooth(inbox, &mut attachment)
        .ok()
        .expect("reinstall same actor");
    let (ble, joined) = block_on(attached.run_until(ble.join(join())))
        .expect("the periodic tracking holds")
        .unwrap();
    let joined = joined.expect("fresh installation");
    assert_ne!(epoch, joined.epoch);
    model.0.borrow_mut().ble_stopped = true;
    let (_, left) = block_on(attached.run_until(ble.leave(
        Leaving::Untracked(joined.member),
        joined.payload.installation,
    )))
    .expect("the periodic tracking holds")
    .unwrap();
    assert!(left.payload.stopped);
    let _ = attached.detach().ok().expect("second normal stop");
}

#[test]
fn an_abandoned_attachment_loan_cannot_open_a_second_seat() {
    let model = model(Script::default());
    let (end, _) = claim_for_host_test().expect("fresh radio client cells");
    let (one, _) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let (two, _) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let mut a = ModelAttachment;
    let mut b = ModelAttachment;
    {
        let _loan = actor
            .attach_bluetooth(one, &mut a)
            .ok()
            .expect("first seat");
    }
    let _returned = actor
        .attach_bluetooth(two, &mut b)
        .err()
        .expect("no fresh authority after abandonment");
    assert!(model.log().is_empty());
}

#[test]
fn detach_and_reattach_do_not_clear_a_poisoned_common_domain() {
    let model = model(Script {
        ble_available: true,
        fail_prepare: true,
        ..Script::default()
    });
    let (end, _) = claim_for_host_test().expect("fresh radio client cells");
    let (inbox, ble) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let mut a = ModelAttachment;
    let mut loan = actor
        .attach_bluetooth(inbox, &mut a)
        .ok()
        .expect("first loan");
    let (ble, failed) = block_on(loan.run_until(ble.join(join())))
        .expect("the periodic tracking holds")
        .unwrap();
    assert_eq!(
        failed.err().expect("failed prepare").cause,
        Failure::Poisoned(PREPARE_POISON)
    );
    let inbox = loan.detach().ok().expect("no Bluetooth hardware entered");
    let mut loan = actor
        .attach_bluetooth(inbox, &mut a)
        .ok()
        .expect("same common domain");
    let (_, failed) = block_on(loan.run_until(ble.join(join())))
        .expect("the periodic tracking holds")
        .unwrap();
    assert_eq!(
        failed.err().expect("poison persists").cause,
        Failure::Poisoned(PREPARE_POISON)
    );
    assert_eq!(model.log(), vec![Event::Prepare(Err(Fault(PREPARE_CAUSE)))]);
}

#[test]
fn a_tracking_deadline_the_clock_cannot_hold_ends_the_attached_run() {
    use core::pin::pin;
    let overflow =
        PoisonCause::TrackingRefused(oer_esp32s31_phy::concurrent::ConcurrentPhyError::Time(
            oer_esp32s31_phy::state::client::PhyTrackTimeError::DeadlineOverflow {
                class: oer_esp32s31_phy::state::client::PhyPllTrackClass::BluetoothIeee802154,
            },
        ));
    let model = model(Script {
        ble_available: true,
        deadline_fault: Some(overflow),
        ..Script::default()
    });
    let (end, _) = claim_for_host_test().expect("fresh radio client cells");
    let (inbox, ble) =
        bluetooth::claim_for_host_test(end.doorbell).expect("fresh Bluetooth client cell");
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    );
    let mut attachment = ModelAttachment;
    let mut attached = actor
        .attach_bluetooth(inbox, &mut attachment)
        .ok()
        .expect("one model attachment");
    let mut client = pin!(async {
        let (ble, joined) = ble.join(join()).await.unwrap();
        let joined = joined.expect("accepted BLE join");
        let (_ble, tracked) = ble
            .track(joined.member, joined.payload.installation)
            .await
            .unwrap();
        let _tracked = tracked.expect("completed initial tracking");
        core::future::pending::<()>().await
    });
    // Once a member is tracked the deadline is asked for, and its overflow
    // ends the run instead of ending tracking silently.
    assert_eq!(
        block_on(attached.run_until(client.as_mut())),
        Err(TrackingFault {
            stage: TrackingStage::Deadline,
            cause: overflow,
        })
    );
    assert!(!model.log().contains(&Event::Tick));
}
