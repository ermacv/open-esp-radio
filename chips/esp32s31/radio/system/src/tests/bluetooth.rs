use super::*;
use core::convert::Infallible;
use oer_esp32s31_radio_client::bluetooth::{
    self, Bluetooth, BluetoothJoin, BluetoothJoined, BluetoothLeft, Installation, Permission,
};

impl ClientBackend<Bluetooth, RcSlow> for Model {
    fn admits(&self, _: &BluetoothJoin) -> bool {
        self.0.borrow().ble_available
    }
    async fn enter(
        &mut self,
        _: BluetoothJoin,
        phy: PhyPreparation,
        _: &LpClock<RcSlow>,
    ) -> Result<BluetoothJoined, Failed<BluetoothJoin>> {
        self.push(Event::BleEnter);
        let mut script = self.0.borrow_mut();
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
        installation: Installation,
    ) -> Result<Installation, Failed<Installation>> {
        self.push(Event::BleTrack);
        // Model an asynchronous initial tracking transaction, including a
        // suspension while the other client's request stays in its own cell.
        yield_now().await;
        if self.0.borrow().ble_fault {
            Err(Failed {
                returned: installation,
                cause: Failure::Poisoned,
            })
        } else {
            Ok(installation)
        }
    }
    async fn retune(&mut self, never: Infallible) -> Result<Infallible, Failed<Infallible>> {
        match never {}
    }
    async fn exit(&mut self, installation: Installation) -> (BluetoothLeft, Exit) {
        self.push(Event::BleExit);
        let mut script = self.0.borrow_mut();
        if installation.0 != script.ble_installation || !script.ble_stopped {
            return (
                BluetoothLeft { stopped: false },
                Exit {
                    last: false,
                    health: DomainHealth::Poisoned,
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
    BluetoothJoin {
        permission: Permission::new(
            oer_radio_regulatory::TEST_DOMAIN
                .permit(oer_bluetooth_le_radio::LeBand)
                .expect("LE test band"),
            oer_radio_regulatory::PowerLimits {
                antenna_gain_mbm: 0,
                chip_maximum_mbm: 2100,
                owner_cap_mbm: Some(1000),
            },
        ),
    }
}

fn multi<F: Future<Output = ()>>(
    model: &Model,
    client: impl FnOnce(ClientCells, oer_esp32s31_radio_client::MembershipCell<Bluetooth>) -> F,
) {
    let (end, cells) = claim_for_host_test();
    let (inbox, ble) = bluetooth::claim_for_host_test(end.doorbell);
    let mut actor = RadioSystem::new(
        end,
        model.clone(),
        clock(),
        LpClock::<RcSlow>::for_host_test(),
    )
    .with_bluetooth(inbox);
    match block_on(select(actor.run(), client(cells, ble))) {
        Either::First(never) => match never {},
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
        let (wifi, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await;
        let joined = joined.ok().expect("Wi-Fi first");
        let epoch = joined.epoch;
        let (wifi, tracked) = wifi
            .track(joined.member, track_request(joined.payload.wifi))
            .await;
        let tracked = tracked.ok().expect("Wi-Fi tracking");
        let (ble, joined) = ble.join(join()).await;
        let joined = joined.ok().expect("BLE second");
        assert_eq!(joined.epoch, epoch);
        assert_eq!(joined.payload.phy, PhyPreparation::AlreadyOpen);
        let (ble, bt) = ble.track(joined.member, joined.payload.installation).await;
        let bt = bt.ok().expect("BLE initial tracking");
        model.0.borrow_mut().ble_stopped = true;
        let (_, left) = ble.leave(Leaving::Tracked(bt.member), bt.payload).await;
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
            .await;
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
        let (ble, joined) = ble.join(join()).await;
        let joined = joined.ok().expect("BLE first");
        let epoch = joined.epoch;
        let (ble, bt) = ble.track(joined.member, joined.payload.installation).await;
        let bt = bt.ok().expect("BLE tracking");
        let (wifi, joined) = cells.wifi.join(WifiJoin { wifi: cold() }).await;
        let joined = joined.ok().expect("Wi-Fi second");
        assert_eq!(joined.epoch, epoch);
        let (wifi, tracked) = wifi
            .track(joined.member, track_request(joined.payload.wifi))
            .await;
        let tracked = tracked.ok().expect("Wi-Fi tracking");
        let (_, left) = wifi
            .leave(
                Leaving::Tracked(tracked.member),
                WifiLeave {
                    wifi: tracked.payload.wifi,
                },
            )
            .await;
        assert_eq!(left.domain, DomainHealth::Healthy);
        assert!(model.0.borrow().bluetooth);
        assert!(!model.log().contains(&Event::CloseRf));
        model.0.borrow_mut().ble_stopped = true;
        let (_, left) = ble.leave(Leaving::Tracked(bt.member), bt.payload).await;
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
        let (ble, joined) = ble.join(join()).await;
        let joined = joined.ok().expect("BLE joined");
        let (_, tracked) = ble.track(joined.member, joined.payload.installation).await;
        assert_eq!(
            tracked.err().expect("RF write failed").cause,
            Failure::Poisoned
        );
        let (_, wifi) = cells.wifi.join(WifiJoin { wifi: cold() }).await;
        assert_eq!(
            wifi.err().expect("shared domain poisoned").cause,
            Failure::Poisoned
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
    multi(&model, |_, ble| async {
        let (ble, first) = ble.join(join()).await;
        let first = first.ok().expect("first installation");
        let (ble, first) = ble.track(first.member, first.payload.installation).await;
        let first = first.ok().expect("first tracking");
        let old = first.payload;
        model.0.borrow_mut().ble_stopped = true;
        let (ble, _) = ble.leave(Leaving::Tracked(first.member), old).await;
        let (ble, next) = ble.join(join()).await;
        let next = next.ok().expect("reinstall");
        assert_ne!(next.payload.installation, old);
        let (ble, next) = ble.track(next.member, next.payload.installation).await;
        let next = next.ok().expect("new tracking");
        let (_, late) = ble.leave(Leaving::Tracked(next.member), old).await;
        assert!(!late.payload.stopped);
        assert_eq!(late.domain, DomainHealth::Poisoned);
        assert!(model.0.borrow().bluetooth);
    });
    assert_eq!(
        model.log().iter().filter(|e| **e == Event::CloseRf).count(),
        1
    );
}

#[test]
fn preflight_refusal_never_registers_or_wakes_rf() {
    let model = model(Script::default());
    multi(&model, |_, ble| async {
        let (_, joined) = ble.join(join()).await;
        assert_eq!(
            joined.err().expect("no available controller").cause,
            Failure::Refused
        );
    });
    assert!(model.log().is_empty());
}
