use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_pac::{WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::test_clocks::{ClockEvent, CountingPlatformClocks, abandoned, held, holding};
use crate::power::{PlatformClock, PlatformClockBackend, PlatformClockProvider};

use super::{
    BluetoothClient, ClockPort, CommonPower, CommonRadioPower, CommonRadioPowerError,
    Ieee802154Client, PlatformClockRefs, PowerEpoch, RadioClient, RadioClientKind, WifiClient,
    WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Acquire(PlatformClock),
    Release(PlatformClock),
    Abandon(PlatformClock, crate::power::PlatformClockAbandonment),
    RestorePower(WifiPowerBaseline),
    SelectWifiLowPowerClock,
    PowerSequence,
    CalibrationComplete(oer_time::Instant),
    ResetWifi(oer_time::Instant),
}

type Log = Rc<RefCell<Vec<Operation>>>;

struct Port {
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    /// Fail the sequence after it took the 160 MHz reference.
    power_sequence_failure: Option<crate::power::PowerError>,
    log: Log,
    clock: oer_time_virtual::VirtualClock,
    calibrated_window: bool,
}

impl Port {
    fn new(log: &Log) -> Self {
        Self {
            power: WifiPowerBaseline::for_host_test(false),
            power_readback: Ok(()),
            power_sequence_failure: None,
            log: log.clone(),
            clock: oer_time_virtual::VirtualClock::default(),
            calibrated_window: false,
        }
    }
}

impl ClockPort for Port {
    fn select_wifi_low_power_clock(&mut self) {
        self.log
            .borrow_mut()
            .push(Operation::SelectWifiLowPowerClock);
    }
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.power
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.log
            .borrow_mut()
            .push(Operation::RestorePower(baseline));
        self.power_readback
    }
    fn run_common_power_sequence<B: PlatformClockBackend>(
        &mut self,
        refs: &mut PlatformClockRefs<B>,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), crate::power::PowerError> {
        self.log.borrow_mut().push(Operation::PowerSequence);
        refs.acquire(PlatformClock::Mpll, platform).unwrap();
        refs.acquire(PlatformClock::Pll160m, platform).unwrap();
        if let Some(error) = self.power_sequence_failure {
            return Err(error);
        }
        refs.acquire(PlatformClock::AnalogI2cMaster, platform)
            .unwrap();
        Ok(())
    }
}

impl crate::ieee80211::client::WifiPowerPort for Port {
    fn pulse_wifi_baseband_and_mac_reset(&mut self) {
        use oer_time::Clock;
        self.log
            .borrow_mut()
            .push(Operation::ResetWifi(self.clock.now()));
        self.calibrated_window = false;
    }
}

fn try_enter<K: RadioClientKind>(
    power: &mut CommonRadioPower<crate::power::test_clocks::CountingBackend>,
    port: &mut Port,
    platform: &CountingPlatformClocks,
) -> Result<CommonPower<K>, CommonRadioPowerError> {
    let entry = power.prepare_entry::<K>(port)?;
    power.finish_entry(entry, port, platform)
}

/// Enter `K`, which the test expects to succeed.
fn entered<K: RadioClientKind>(
    power: &mut CommonRadioPower<crate::power::test_clocks::CountingBackend>,
    port: &mut Port,
    platform: &CountingPlatformClocks,
) -> CommonPower<K> {
    try_enter::<K>(power, port, platform)
        .unwrap_or_else(|error| panic!("common power entered: {error:?}"))
}

/// Leave with `proof`, which the test expects to succeed.
fn left<K: RadioClientKind>(
    power: &mut CommonRadioPower<crate::power::test_clocks::CountingBackend>,
    port: &mut Port,
    proof: CommonPower<K>,
) {
    if let Err(failure) = power.exit(port, proof) {
        panic!("common power left: {:?}", failure.error);
    }
}

fn setup() -> (Log, Port, CountingPlatformClocks) {
    let log = Log::default();
    let port = Port::new(&log);
    let sink = log.clone();
    let platform = CountingPlatformClocks::logging(move |event| {
        sink.borrow_mut().push(match event {
            ClockEvent::Acquire(clock) => Operation::Acquire(clock),
            ClockEvent::Release(clock) => Operation::Release(clock),
            ClockEvent::Abandon(clock, how) => Operation::Abandon(clock, how),
        });
    });
    (log, port, platform)
}

const COMMON: [PlatformClock; 3] = [
    PlatformClock::Mpll,
    PlatformClock::Pll160m,
    PlatformClock::AnalogI2cMaster,
];

#[test]
fn power_retry_preserves_the_original_cold_baseline_until_commit() {
    let original = WifiPowerBaseline::for_host_test(false);
    let retry_observation = WifiPowerBaseline::for_host_test(true);
    let (log, mut port, _) = setup();
    port.power = original;
    let mut epoch = PowerEpoch::default();

    epoch.prepare(&port);
    port.power = retry_observation;
    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(*log.borrow(), [Operation::RestorePower(original)]);

    epoch.prepare(&port);
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(
        log.borrow()[1..],
        [Operation::RestorePower(retry_observation)]
    );
}

#[test]
fn power_restore_failure_retains_the_baseline_for_retry() {
    let (log, mut port, _) = setup();
    let mut epoch = PowerEpoch::default();
    epoch.prepare(&port);

    port.power_readback = Err(WifiPowerRestoreReadback::ModemSourceClocks);
    assert_eq!(
        epoch.restore(&mut port),
        Err(WifiPowerRestoreCheckpoint::ModemSourceClocks)
    );
    port.power_readback = Ok(());
    assert_eq!(epoch.restore(&mut port), Ok(()));
    assert_eq!(log.borrow().len(), 2);
}

#[test]
fn common_power_holds_one_platform_reference_of_each_shared_gate() {
    let cold = WifiPowerBaseline::for_host_test(false);
    let (log, mut port, platform) = setup();
    port.power = cold;
    let mut power = CommonRadioPower::default();

    let wifi = entered::<WifiClient>(&mut power, &mut port, &platform);
    // A later protocol must not pulse the Wi-Fi resets or take references.
    port.power = WifiPowerBaseline::for_host_test(true);
    let bluetooth = entered::<BluetoothClient>(&mut power, &mut port, &platform);
    assert_eq!(
        *log.borrow(),
        [
            Operation::SelectWifiLowPowerClock,
            Operation::PowerSequence,
            Operation::Acquire(PlatformClock::Mpll),
            Operation::Acquire(PlatformClock::Pll160m),
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ]
    );
    assert!(matches!(
        try_enter::<BluetoothClient>(&mut power, &mut port, &platform),
        Err(CommonRadioPowerError::AlreadyEntered)
    ));
    assert_eq!(held(), holding(&COMMON));
    let mut counted = crate::power::PlatformClockHolds::default();
    power.count_platform_clocks(&mut counted);
    assert_eq!(counted, held());

    log.borrow_mut().clear();
    left(&mut power, &mut port, wifi);
    assert!(log.borrow().is_empty());
    left(&mut power, &mut port, bluetooth);
    // The last client closes the analog-I2C gate, restores the baseline
    // captured before the first edge, then gives back the 160 MHz source and MPLL.
    assert_eq!(
        *log.borrow(),
        [
            Operation::Release(PlatformClock::AnalogI2cMaster),
            Operation::RestorePower(cold),
            Operation::Release(PlatformClock::Pll160m),
            Operation::Release(PlatformClock::Mpll),
        ]
    );
    assert!(held().is_empty());
    assert!(!power.any_client());
}

#[test]
fn a_failed_power_sequence_admits_no_client_and_retries_without_a_second_reference() {
    let (log, mut port, platform) = setup();
    let error = crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    };
    port.power_sequence_failure = Some(error);
    let mut power = CommonRadioPower::default();
    assert!(matches!(
        try_enter::<Ieee802154Client>(&mut power, &mut port, &platform),
        Err(CommonRadioPowerError::Power(failed)) if failed == error
    ));
    assert!(!power.holds(RadioClient::Ieee802154));

    port.power_sequence_failure = None;
    let _ieee802154 = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    let acquisitions = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Acquire(PlatformClock::Pll160m))
        .count();
    assert_eq!(acquisitions, 1);
    power.abandon_for_test();
}

#[test]
fn a_failed_restore_keeps_the_last_client_for_retry() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    let wifi = entered::<WifiClient>(&mut power, &mut port, &platform);
    port.power_readback = Err(WifiPowerRestoreReadback::ModemSyscon);
    let Err(failure) = power.exit(&mut port, wifi) else {
        panic!("the restore failed");
    };
    assert_eq!(
        failure.error,
        CommonRadioPowerError::Restore(WifiPowerRestoreCheckpoint::ModemSyscon)
    );
    assert!(power.holds(RadioClient::Wifi));
    // The analog-I2C gate closed before the restore; the sources stay held.
    assert_eq!(
        held(),
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    port.power_readback = Ok(());
    left(&mut power, &mut port, failure.power);
    let releases = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Release(PlatformClock::AnalogI2cMaster))
        .count();
    assert_eq!(releases, 1);
    assert!(held().is_empty());
}

#[test]
fn a_failed_power_sequence_is_abandoned_with_its_references_on() {
    let (_, mut port, platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(try_enter::<BluetoothClient>(&mut power, &mut port, &platform).is_err());
    // The retry reuses the sources taken before the failed checkpoint.
    assert_eq!(
        held(),
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    // Abandoning the failed owner gives nothing back: the sequence stays
    // fail-stop with its sources on, and the abandonment is recorded.
    power.abandon_for_test();
    assert_eq!(
        held(),
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    let (abandoned, dropped) = abandoned();
    assert_eq!(
        abandoned,
        holding(&[PlatformClock::Mpll, PlatformClock::Pll160m])
    );
    assert!(dropped.is_empty());
}

#[test]
fn repeated_common_power_does_not_repeat_global_clock_selection() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    let first = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    left(&mut power, &mut port, first);
    let _second = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    let observed = log.borrow().clone();
    assert_eq!(
        observed
            .iter()
            .filter(|op| **op == Operation::PowerSequence)
            .count(),
        2
    );
    assert_eq!(
        observed
            .iter()
            .filter(|op| **op == Operation::SelectWifiLowPowerClock)
            .count(),
        1
    );
    power.abandon_for_test();
}

#[test]
fn a_failed_initial_domain_entry_retries_the_clock_selection() {
    let (log, mut port, platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(try_enter::<WifiClient>(&mut power, &mut port, &platform).is_err());
    port.power_sequence_failure = None;
    let _wifi = entered::<WifiClient>(&mut power, &mut port, &platform);
    let observed = log.borrow().clone();
    assert_eq!(
        observed
            .iter()
            .filter(|op| **op == Operation::PowerSequence)
            .count(),
        2
    );
    assert_eq!(
        observed
            .iter()
            .filter(|op| **op == Operation::SelectWifiLowPowerClock)
            .count(),
        2
    );
    power.abandon_for_test();
}

fn calibrated(port: &mut Port) -> oer_time::Instant {
    use oer_time::{Clock, Duration};
    port.clock.advance(Duration::from_micros(10)).unwrap();
    port.calibrated_window = true;
    let at = port.clock.now();
    port.log
        .borrow_mut()
        .push(Operation::CalibrationComplete(at));
    at
}

#[test]
fn wifi_entry_resets_after_calibration_before_domain_power_and_only_once() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    let mut wifi = crate::ieee80211::client::WifiPowerEntry::default();
    let at = calibrated(&mut port);
    let first = wifi.enter(&mut power, &mut port, &platform).unwrap();
    assert!(!port.calibrated_window);
    assert_eq!(
        &log.borrow()[..4],
        &[
            Operation::CalibrationComplete(at),
            Operation::SelectWifiLowPowerClock,
            Operation::ResetWifi(at),
            Operation::PowerSequence,
        ]
    );
    left(&mut power, &mut port, first);
    // A retained wake republishes PHY state before a repeated client entry.
    calibrated(&mut port);
    let _second = wifi.enter(&mut power, &mut port, &platform).unwrap();
    assert!(port.calibrated_window);
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|op| matches!(op, Operation::ResetWifi(_)))
            .count(),
        1
    );
    power.abandon_for_test();
}

#[test]
fn ieee_and_ble_initial_and_repeated_entries_preserve_the_calibrated_mac_window() {
    fn check<K: RadioClientKind>() {
        let (log, mut port, platform) = setup();
        let mut power = CommonRadioPower::default();
        calibrated(&mut port);
        let first = entered::<K>(&mut power, &mut port, &platform);
        assert!(port.calibrated_window);
        left(&mut power, &mut port, first);
        let _second = entered::<K>(&mut power, &mut port, &platform);
        assert!(port.calibrated_window);
        assert!(
            !log.borrow()
                .iter()
                .any(|op| matches!(op, Operation::ResetWifi(_)))
        );
        power.abandon_for_test();
    }
    check::<Ieee802154Client>();
    check::<BluetoothClient>();
}

#[test]
fn wifi_reset_is_retried_after_power_failure_but_not_on_duplicate_admission() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    let mut wifi = crate::ieee80211::client::WifiPowerEntry::default();
    calibrated(&mut port);
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    });
    assert!(wifi.enter(&mut power, &mut port, &platform).is_err());
    port.power_sequence_failure = None;
    let _entered = wifi.enter(&mut power, &mut port, &platform).unwrap();
    let before = log.borrow().len();
    assert!(matches!(
        wifi.enter(&mut power, &mut port, &platform),
        Err(CommonRadioPowerError::AlreadyEntered)
    ));
    assert_eq!(log.borrow().len(), before);
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|op| matches!(op, Operation::ResetWifi(_)))
            .count(),
        2
    );
    power.abandon_for_test();
}

#[test]
fn wifi_has_its_own_initial_reset_after_an_ieee_power_epoch() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();
    let mut wifi = crate::ieee80211::client::WifiPowerEntry::default();
    calibrated(&mut port);
    let ieee = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    left(&mut power, &mut port, ieee);
    assert!(port.calibrated_window);
    let _wifi = wifi.enter(&mut power, &mut port, &platform).unwrap();
    assert!(!port.calibrated_window);
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|op| matches!(op, Operation::ResetWifi(_)))
            .count(),
        1
    );
    power.abandon_for_test();
}
