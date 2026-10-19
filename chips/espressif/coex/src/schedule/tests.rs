use super::*;
use CoexSchemeId as S;

/// One status per selectable scheme and the scheme the vendor selects,
/// found by emulating `coex_schm_status_change` of `coexist_scheme.o`
/// (esp-coex-lib c758e7b5, see [`super`]) over every combination of the
/// status bits it tests; each vector is a minimal status for its scheme.
/// The `bt_default` rows use a classic Bluetooth bit outside every tested
/// mask, which the vendor's final `else` selects.
const VENDOR_SELECTION: &[([u16; 5], CoexSchemeId)] = &[
    ([0x00, 0x00, 0x00, 0x00, 0x00], S::AllDefault),
    ([0x04, 0x00, 0x10, 0x00, 0x01], S::BleDefaultBtA2dpWifiConn),
    (
        [0x02, 0x00, 0x10, 0x00, 0x01],
        S::BleDefaultBtA2dpWifiConnecting,
    ),
    ([0x01, 0x00, 0x10, 0x00, 0x01], S::BleDefaultBtA2dpWifiScan),
    (
        [0x04, 0x00, 0x01, 0x00, 0x01],
        S::BleDefaultBtDefaultWifiConn,
    ),
    (
        [0x02, 0x00, 0x01, 0x00, 0x01],
        S::BleDefaultBtDefaultWifiConnecting,
    ),
    (
        [0x01, 0x00, 0x01, 0x00, 0x01],
        S::BleDefaultBtDefaultWifiScan,
    ),
    ([0x04, 0x00, 0x00, 0x00, 0x01], S::BleDefaultBtIdleWifiConn),
    (
        [0x02, 0x00, 0x00, 0x00, 0x01],
        S::BleDefaultBtIdleWifiConnecting,
    ),
    ([0x01, 0x00, 0x00, 0x00, 0x01], S::BleDefaultBtIdleWifiScan),
    (
        [0x04, 0x08, 0x20, 0x00, 0x00],
        S::BleMeshConfigBtA2dpPausedWifiConn,
    ),
    (
        [0x02, 0x08, 0x20, 0x00, 0x00],
        S::BleMeshConfigBtA2dpPausedWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x20, 0x00, 0x00],
        S::BleMeshConfigBtA2dpPausedWifiScan,
    ),
    (
        [0x04, 0x08, 0x10, 0x00, 0x00],
        S::BleMeshConfigBtA2dpWifiConn,
    ),
    (
        [0x02, 0x08, 0x10, 0x00, 0x00],
        S::BleMeshConfigBtA2dpWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x10, 0x00, 0x00],
        S::BleMeshConfigBtA2dpWifiScan,
    ),
    (
        [0x04, 0x08, 0x04, 0x00, 0x00],
        S::BleMeshConfigBtConnWifiConn,
    ),
    (
        [0x02, 0x08, 0x04, 0x00, 0x00],
        S::BleMeshConfigBtConnWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x04, 0x00, 0x00],
        S::BleMeshConfigBtConnWifiScan,
    ),
    (
        [0x04, 0x08, 0x02, 0x00, 0x00],
        S::BleMeshConfigBtDefaultWifiConn,
    ),
    (
        [0x02, 0x08, 0x02, 0x00, 0x00],
        S::BleMeshConfigBtDefaultWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x02, 0x00, 0x00],
        S::BleMeshConfigBtDefaultWifiScan,
    ),
    (
        [0x04, 0x08, 0x01, 0x00, 0x00],
        S::BleMeshConfigBtPiscanWifiConn,
    ),
    (
        [0x02, 0x08, 0x01, 0x00, 0x00],
        S::BleMeshConfigBtPiscanWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x01, 0x00, 0x00],
        S::BleMeshConfigBtPiscanWifiScan,
    ),
    (
        [0x04, 0x08, 0x08, 0x00, 0x00],
        S::BleMeshConfigBtSniffScoWifiConn,
    ),
    (
        [0x02, 0x08, 0x08, 0x00, 0x00],
        S::BleMeshConfigBtSniffScoWifiConnecting,
    ),
    (
        [0x01, 0x08, 0x08, 0x00, 0x00],
        S::BleMeshConfigBtSniffScoWifiScan,
    ),
    ([0x04, 0x08, 0x00, 0x00, 0x00], S::BleMeshConfigWifiConn),
    (
        [0x02, 0x08, 0x00, 0x00, 0x00],
        S::BleMeshConfigWifiConnecting,
    ),
    ([0x01, 0x08, 0x00, 0x00, 0x00], S::BleMeshConfigWifiScan),
    (
        [0x04, 0x20, 0x20, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpPausedWifiConn,
    ),
    (
        [0x02, 0x20, 0x20, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpPausedWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x20, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpPausedWifiScan,
    ),
    (
        [0x04, 0x20, 0x10, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpWifiConn,
    ),
    (
        [0x02, 0x20, 0x10, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x10, 0x00, 0x00],
        S::BleMeshStandbyBtA2dpWifiScan,
    ),
    (
        [0x04, 0x20, 0x04, 0x00, 0x00],
        S::BleMeshStandbyBtConnWifiConn,
    ),
    (
        [0x02, 0x20, 0x04, 0x00, 0x00],
        S::BleMeshStandbyBtConnWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x04, 0x00, 0x00],
        S::BleMeshStandbyBtConnWifiScan,
    ),
    (
        [0x04, 0x20, 0x02, 0x00, 0x00],
        S::BleMeshStandbyBtDefaultWifiConn,
    ),
    (
        [0x02, 0x20, 0x02, 0x00, 0x00],
        S::BleMeshStandbyBtDefaultWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x02, 0x00, 0x00],
        S::BleMeshStandbyBtDefaultWifiScan,
    ),
    (
        [0x04, 0x20, 0x01, 0x00, 0x00],
        S::BleMeshStandbyBtPiscanWifiConn,
    ),
    (
        [0x02, 0x20, 0x01, 0x00, 0x00],
        S::BleMeshStandbyBtPiscanWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x01, 0x00, 0x00],
        S::BleMeshStandbyBtPiscanWifiScan,
    ),
    (
        [0x04, 0x20, 0x08, 0x00, 0x00],
        S::BleMeshStandbyBtSniffScoWifiConn,
    ),
    (
        [0x02, 0x20, 0x08, 0x00, 0x00],
        S::BleMeshStandbyBtSniffScoWifiConnecting,
    ),
    (
        [0x01, 0x20, 0x08, 0x00, 0x00],
        S::BleMeshStandbyBtSniffScoWifiScan,
    ),
    ([0x04, 0x20, 0x00, 0x00, 0x00], S::BleMeshStandbyWifiConn),
    (
        [0x02, 0x20, 0x00, 0x00, 0x00],
        S::BleMeshStandbyWifiConnecting,
    ),
    ([0x01, 0x20, 0x00, 0x00, 0x00], S::BleMeshStandbyWifiScan),
    (
        [0x04, 0x02, 0x20, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpPausedWifiConn,
    ),
    (
        [0x02, 0x02, 0x20, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpPausedWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x20, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpPausedWifiScan,
    ),
    (
        [0x04, 0x02, 0x10, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpWifiConn,
    ),
    (
        [0x02, 0x02, 0x10, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x10, 0x00, 0x00],
        S::BleMeshTrafficBtA2dpWifiScan,
    ),
    (
        [0x04, 0x02, 0x04, 0x00, 0x00],
        S::BleMeshTrafficBtConnWifiConn,
    ),
    (
        [0x02, 0x02, 0x04, 0x00, 0x00],
        S::BleMeshTrafficBtConnWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x04, 0x00, 0x00],
        S::BleMeshTrafficBtConnWifiScan,
    ),
    (
        [0x04, 0x02, 0x02, 0x00, 0x00],
        S::BleMeshTrafficBtDefaultWifiConn,
    ),
    (
        [0x02, 0x02, 0x02, 0x00, 0x00],
        S::BleMeshTrafficBtDefaultWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x02, 0x00, 0x00],
        S::BleMeshTrafficBtDefaultWifiScan,
    ),
    (
        [0x04, 0x02, 0x01, 0x00, 0x00],
        S::BleMeshTrafficBtPiscanWifiConn,
    ),
    (
        [0x02, 0x02, 0x01, 0x00, 0x00],
        S::BleMeshTrafficBtPiscanWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x01, 0x00, 0x00],
        S::BleMeshTrafficBtPiscanWifiScan,
    ),
    (
        [0x04, 0x02, 0x08, 0x00, 0x00],
        S::BleMeshTrafficBtSniffScoWifiConn,
    ),
    (
        [0x02, 0x02, 0x08, 0x00, 0x00],
        S::BleMeshTrafficBtSniffScoWifiConnecting,
    ),
    (
        [0x01, 0x02, 0x08, 0x00, 0x00],
        S::BleMeshTrafficBtSniffScoWifiScan,
    ),
    ([0x04, 0x02, 0x00, 0x00, 0x00], S::BleMeshTrafficWifiConn),
    (
        [0x02, 0x02, 0x00, 0x00, 0x00],
        S::BleMeshTrafficWifiConnecting,
    ),
    ([0x01, 0x02, 0x00, 0x00, 0x00], S::BleMeshTrafficWifiScan),
    ([0x04, 0x00, 0x20, 0x00, 0x00], S::BtA2dpPausedWifiConn),
    (
        [0x02, 0x00, 0x20, 0x00, 0x00],
        S::BtA2dpPausedWifiConnecting,
    ),
    ([0x01, 0x00, 0x20, 0x00, 0x00], S::BtA2dpPausedWifiScan),
    ([0x04, 0x00, 0x10, 0x00, 0x00], S::BtA2dpWifiConn),
    ([0x02, 0x00, 0x10, 0x00, 0x00], S::BtA2dpWifiConnecting),
    ([0x01, 0x00, 0x10, 0x00, 0x00], S::BtA2dpWifiScan),
    ([0x04, 0x00, 0x04, 0x00, 0x00], S::BtConnWifiConn),
    ([0x02, 0x00, 0x04, 0x00, 0x00], S::BtConnWifiConnecting),
    ([0x01, 0x00, 0x04, 0x00, 0x00], S::BtConnWifiScan),
    ([0x04, 0x00, 0x00, 0x00, 0x00], S::BtIdleWifiConn),
    ([0x02, 0x00, 0x00, 0x00, 0x00], S::BtIdleWifiConnecting),
    ([0x01, 0x00, 0x00, 0x00, 0x00], S::BtIdleWifiScan),
    ([0x04, 0x00, 0x02, 0x00, 0x00], S::BtInqWifiConn),
    ([0x02, 0x00, 0x02, 0x00, 0x00], S::BtInqWifiConnecting),
    ([0x01, 0x00, 0x02, 0x00, 0x00], S::BtInqWifiScan),
    ([0x04, 0x00, 0x40, 0x00, 0x00], S::BtPageWifiConn),
    ([0x02, 0x00, 0x40, 0x00, 0x00], S::BtPageWifiConnecting),
    ([0x01, 0x00, 0x40, 0x00, 0x00], S::BtPageWifiScan),
    ([0x04, 0x00, 0x01, 0x00, 0x00], S::BtPiscanWifiConn),
    ([0x02, 0x00, 0x01, 0x00, 0x00], S::BtPiscanWifiConnecting),
    ([0x01, 0x00, 0x01, 0x00, 0x00], S::BtPiscanWifiScan),
    ([0x04, 0x00, 0x08, 0x00, 0x00], S::BtSniffScoWifiConn),
    ([0x02, 0x00, 0x08, 0x00, 0x00], S::BtSniffScoWifiConnecting),
    ([0x01, 0x00, 0x08, 0x00, 0x00], S::BtSniffScoWifiScan),
    (
        [0x02, 0x00, 0x00, 0x01, 0x00],
        S::ExternalCoexWifiConnecting,
    ),
    ([0x00, 0x00, 0x00, 0x01, 0x00], S::ExternalCoexWifiDefault),
    (
        [0x00, 0x00, 0x00, 0x03, 0x00],
        S::ExternalCoexWifiDefaultRxonly,
    ),
    ([0x01, 0x00, 0x00, 0x01, 0x00], S::ExternalCoexWifiScan),
    ([0x01, 0x00, 0x100, 0x00, 0x00], S::BtDefaultWifiScan),
    ([0x02, 0x00, 0x100, 0x00, 0x00], S::BtDefaultWifiConnecting),
    ([0x04, 0x00, 0x100, 0x00, 0x00], S::BtDefaultWifiConn),
];

fn words([wifi, ble, bt, external_coex, ieee802154]: [u16; 5]) -> CoexStatusWords {
    CoexStatusWords {
        wifi,
        ble,
        bt,
        external_coex,
        ieee802154,
    }
}

#[test]
fn every_selectable_scheme_follows_the_vendor_selection() {
    for &(status, expected) in VENDOR_SELECTION {
        assert_eq!(words(status).select(), expected, "{status:x?}");
    }
    let selected = |scheme| VENDOR_SELECTION.iter().any(|&(_, s)| s == scheme);
    let never_selected: std::vec::Vec<_> = CoexSchemeId::ALL
        .into_iter()
        .filter(|&scheme| !selected(scheme))
        .collect();
    assert_eq!(
        never_selected,
        [
            S::BleDefaultBtA2dpWifiDefault,
            S::BleDefaultBtIdleWifiDefault,
            S::BleIdleBtIdleWifiDefault,
        ],
        "the vendor defines these schemes but never selects them"
    );
}

#[test]
fn every_scheme_has_a_phase_and_a_unique_name() {
    for scheme in CoexSchemeId::ALL {
        assert!(!scheme.scheme().phases().is_empty(), "{}", scheme.name());
        let same_name = CoexSchemeId::ALL
            .into_iter()
            .filter(|other| other.name() == scheme.name())
            .count();
        assert_eq!(same_name, 1);
    }
}

#[test]
fn phases_loop_only_while_two_radio_groups_publish_status() {
    let mut status = CoexStatusWords::default();
    assert!(!status.loop_allowed());
    status.wifi = wifi_status::SCAN;
    assert!(!status.loop_allowed());
    status.ble = 1;
    assert!(status.loop_allowed());
    status.wifi = 0;
    status.bt = 1;
    assert!(
        !status.loop_allowed(),
        "BLE and classic Bluetooth are one group"
    );
    status.ieee802154 = 1;
    assert!(status.loop_allowed());
}

/// Wi-Fi scanning beside BLE: two phases of the ble-default scan scheme.
fn scanning_beside_ble() -> CoexSchedule {
    let mut schedule = CoexSchedule::new();
    schedule.set_interval(1024);
    assert_eq!(
        schedule.set_status_bits(CoexStatusType::Wifi, wifi_status::SCAN),
        None,
        "one radio alone does not loop"
    );
    schedule
}

#[test]
fn the_first_looping_status_restarts_the_phases_of_a_scanning_wifi() {
    let mut schedule = scanning_beside_ble();
    let step = schedule
        .set_status_bits(CoexStatusType::Ble, 1)
        .expect("BLE beside a scanning Wi-Fi starts the phases");
    assert_eq!(schedule.scheme(), S::BleDefaultBtIdleWifiScan);
    let scheme = S::BleDefaultBtIdleWifiScan.scheme();
    assert_eq!(schedule.phase_index(), 0);
    assert_eq!(step.phase, scheme.phases()[0]);
    assert_eq!(
        step.timer_micros,
        Some(u32::from(scheme.period()) * 1024 * u32::from(scheme.phases()[0].share_percent()))
    );
    assert!(step.notify_wifi);
    assert!(!step.notify_bluetooth);

    // A repeated status bit changes nothing.
    assert_eq!(schedule.set_status_bits(CoexStatusType::Ble, 1), None);
}

#[test]
fn a_timer_looping_wifi_wraps_from_the_last_phase_to_the_first() {
    let mut schedule = scanning_beside_ble();
    let _ = schedule.set_status_bits(CoexStatusType::Ble, 1);
    let scheme = schedule.scheme().scheme();
    let last = scheme.phases().len() - 1;
    for index in 1..=last {
        let step = schedule.timeout().expect("the phases loop");
        assert_eq!(usize::from(schedule.phase_index()), index);
        assert_eq!(step.phase, scheme.phases()[index]);
        assert!(step.timer_micros.is_some(), "a looping schedule re-arms");
        assert_eq!(
            step.notify_bluetooth,
            scheme.phases()[index].bluetooth() != [0, 0]
        );
    }
    let step = schedule.timeout().expect("the last phase wraps");
    assert_eq!(schedule.phase_index(), 0);
    assert_eq!(step.phase, scheme.phases()[0]);
}

#[test]
fn a_connected_wifi_stops_at_the_last_phase_until_it_restarts_them() {
    let mut schedule = CoexSchedule::new();
    schedule.set_interval(1024);
    let _ = schedule.set_status_bits(CoexStatusType::Wifi, wifi_status::CONNECTED);
    assert_eq!(
        schedule.set_status_bits(CoexStatusType::Ble, 1),
        None,
        "a connected Wi-Fi restarts the phases itself"
    );
    assert_eq!(schedule.scheme(), S::BleDefaultBtIdleWifiConn);
    let scheme = schedule.scheme().scheme();
    let last = scheme.phases().len() - 1;

    let step = schedule.restart().expect("a restart always steps");
    assert_eq!(schedule.phase_index(), 0);
    assert!(step.timer_micros.is_some());
    for _ in 1..last {
        let _ = schedule.timeout().expect("inner phases step");
    }
    let step = schedule.timeout().expect("the last phase is entered");
    assert_eq!(usize::from(schedule.phase_index()), last);
    assert_eq!(step.timer_micros, None, "the last phase is not timed");
    assert_eq!(schedule.timeout(), Err(CoexScheduleIdle::LastPhase));
    assert_eq!(usize::from(schedule.phase_index()), last);
}

#[test]
fn clearing_status_reselects_without_moving_the_phase() {
    let mut schedule = scanning_beside_ble();
    let _ = schedule.set_status_bits(CoexStatusType::Ble, 1);
    let _ = schedule.timeout();
    let index = schedule.phase_index();
    schedule.clear_status_bits(CoexStatusType::Ble, 1);
    assert_eq!(schedule.scheme(), S::BtIdleWifiScan);
    assert_eq!(schedule.phase_index(), index);
    assert_eq!(
        schedule.current_phase(),
        schedule.phase_by_index(index),
        "an index beyond the new scheme reads no phase"
    );
    assert_eq!(schedule.timeout(), Err(CoexScheduleIdle::LastPhase));
}

#[test]
fn flexible_period_is_kept_without_changing_phase_durations() {
    let mut schedule = scanning_beside_ble();
    schedule.set_flexible_period(3);
    let first = schedule.set_status_bits(CoexStatusType::Ble, 1);
    assert_eq!(schedule.flexible_period(), 3);
    let mut plain = scanning_beside_ble();
    assert_eq!(plain.set_status_bits(CoexStatusType::Ble, 1), first);
}

#[test]
fn only_the_other_radios_status_counts_for_a_radio() {
    let wifi_only = words([wifi_status::SCAN, 0, 0, 0, 0]);
    assert!(!wifi_only.others_publish(CoexStatusType::Wifi));
    assert!(wifi_only.others_publish(CoexStatusType::Ble));
    let with_ieee802154 = words([wifi_status::SCAN, 0, 0, 0, 1]);
    assert!(with_ieee802154.others_publish(CoexStatusType::Wifi));
    assert!(!words([0; 5]).others_publish(CoexStatusType::Ieee802154));
}
