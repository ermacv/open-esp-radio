use std::{cell::RefCell, rc::Rc, vec::Vec};

use crate::radio::modem::WifiPowerBaseline;
use oer_chips_family_radio_power::baseline::PowerRestoreCheckpoint;

use crate::power::platform::PlatformClock;
use crate::power::test_gates::{GateEvent, RecordingGates};
use oer_chips_family_radio_power::power::PowerClock;

use super::{
    ClockPort, CommonPower, CommonRadioPower, CommonRadioPowerError, PlatformClockRefs,
    RadioClient, RadioClientKind, WifiClient, WifiPowerRestoreCheckpoint,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Acquire(PlatformClock),
    Release(PlatformClock),
    WifiPower(bool),
    RestorePower(WifiPowerBaseline),
    SelectWifiLowPowerClock,
    PowerSequence,
    CalibrationComplete(oer_portable_time::Instant),
    ResetWifi(oer_portable_time::Instant),
}

type Log = Rc<RefCell<Vec<Operation>>>;

struct Port {
    power: WifiPowerBaseline,
    power_readback: Result<(), PowerRestoreCheckpoint>,
    /// Fail the sequence after it took the 160 MHz reference.
    power_sequence_failure: Option<oer_chips_family_radio_power::power::PowerError>,
    log: Log,
    clock: oer_portable_time_virtual::VirtualClock,
    calibrated_window: bool,
}

impl Port {
    fn new(log: &Log) -> Self {
        Self {
            power: WifiPowerBaseline::for_host_test(
                oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Disabled,
            ),
            power_readback: Ok(()),
            power_sequence_failure: None,
            log: log.clone(),
            clock: oer_portable_time_virtual::VirtualClock::default(),
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
    ) -> Result<(), PowerRestoreCheckpoint> {
        self.log
            .borrow_mut()
            .push(Operation::RestorePower(baseline));
        self.power_readback
    }
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &mut impl crate::power::platform::PlatformGates,
    ) -> Result<(), oer_chips_family_radio_power::power::PowerError> {
        self.log.borrow_mut().push(Operation::PowerSequence);
        refs.acquire(PowerClock::Pll160m, platform);
        if let Some(error) = self.power_sequence_failure {
            return Err(error);
        }
        refs.acquire(PowerClock::AnalogI2cMaster, platform);
        Ok(())
    }
}

impl crate::radio::ieee80211::client::WifiPowerPort for Port {
    fn pulse_wifi_baseband_and_mac_reset(&mut self) {
        use oer_portable_clock::Clock;
        self.log
            .borrow_mut()
            .push(Operation::ResetWifi(self.clock.now()));
        self.calibrated_window = false;
    }
}

fn try_enter<K: RadioClientKind>(
    power: &mut CommonRadioPower,
    port: &mut Port,
    platform: &mut RecordingGates,
) -> Result<CommonPower<K>, CommonRadioPowerError> {
    power.enter::<K>(port, platform)
}

/// Enter `K`, which the test expects to succeed.
fn entered<K: RadioClientKind>(
    power: &mut CommonRadioPower,
    port: &mut Port,
    platform: &mut RecordingGates,
) -> CommonPower<K> {
    try_enter::<K>(power, port, platform)
        .unwrap_or_else(|error| panic!("common power entered: {error:?}"))
}

/// Leave with `proof`, which the test expects to succeed.
fn left<K: RadioClientKind>(
    power: &mut CommonRadioPower,
    port: &mut Port,
    platform: &mut RecordingGates,
    proof: CommonPower<K>,
) {
    if let Err(failure) = power.exit(port, platform, proof) {
        panic!("common power left: {:?}", failure.error);
    }
}

fn setup() -> (Log, Port, RecordingGates) {
    let log = Log::default();
    let port = Port::new(&log);
    let sink = log.clone();
    let platform = RecordingGates::logging(move |event| {
        sink.borrow_mut().push(match event {
            GateEvent::Acquire(clock) => Operation::Acquire(clock),
            GateEvent::Release(clock) => Operation::Release(clock),
            GateEvent::OpenWifiPower => Operation::WifiPower(true),
            GateEvent::CloseWifiPower => Operation::WifiPower(false),
        });
    });
    (log, port, platform)
}

const COMMON: [PlatformClock; 2] = [PlatformClock::Pll160m, PlatformClock::AnalogI2cMaster];

#[test]
fn common_power_holds_one_platform_reference_of_each_shared_gate() {
    let cold = WifiPowerBaseline::for_host_test(
        oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Disabled,
    );
    let (log, mut port, mut platform) = setup();
    port.power = cold;
    let mut power = CommonRadioPower::default();

    let wifi = entered::<WifiClient>(&mut power, &mut port, &mut platform);
    assert_eq!(
        *log.borrow(),
        [
            Operation::WifiPower(true),
            Operation::SelectWifiLowPowerClock,
            Operation::PowerSequence,
            Operation::Acquire(PlatformClock::Pll160m),
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ]
    );
    assert!(matches!(
        try_enter::<WifiClient>(&mut power, &mut port, &mut platform),
        Err(CommonRadioPowerError::AlreadyEntered)
    ));
    assert_eq!(platform.held(), COMMON);

    log.borrow_mut().clear();
    left(&mut power, &mut port, &mut platform, wifi);
    // The last client closes the analog-I2C gate, restores the baseline
    // captured before the first edge, gives back the 160 MHz source and
    // closes Wi-Fi's power gate last.
    assert_eq!(
        *log.borrow(),
        [
            Operation::Release(PlatformClock::AnalogI2cMaster),
            Operation::RestorePower(cold),
            Operation::Release(PlatformClock::Pll160m),
            Operation::WifiPower(false),
        ]
    );
    assert!(platform.held().is_empty());
    assert!(!power.any_client());
}

#[test]
fn a_failed_power_sequence_admits_no_client_and_retries_without_a_second_reference() {
    let (log, mut port, mut platform) = setup();
    let error = oer_chips_family_radio_power::power::PowerError {
        checkpoint: oer_chips_family_radio_power::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    };
    port.power_sequence_failure = Some(error);
    let mut power = CommonRadioPower::default();
    assert!(matches!(
        try_enter::<WifiClient>(&mut power, &mut port, &mut platform),
        Err(CommonRadioPowerError::Power(failed)) if failed == error
    ));
    assert!(!power.holds(RadioClient::Wifi));

    port.power_sequence_failure = None;
    let _wifi = entered::<WifiClient>(&mut power, &mut port, &mut platform);
    let acquisitions = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Acquire(PlatformClock::Pll160m))
        .count();
    assert_eq!(acquisitions, 1);
}

#[test]
fn a_failed_restore_keeps_the_last_client_for_retry() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    let wifi = entered::<WifiClient>(&mut power, &mut port, &mut platform);
    port.power_readback = Err(PowerRestoreCheckpoint::ModemSyscon);
    let Err(failure) = power.exit(&mut port, &mut platform, wifi) else {
        panic!("the restore failed");
    };
    assert_eq!(
        failure.error,
        CommonRadioPowerError::Restore(WifiPowerRestoreCheckpoint::ModemSyscon)
    );
    assert!(power.holds(RadioClient::Wifi));
    // The analog-I2C gate closed before the restore; the source stays held.
    assert_eq!(platform.held(), [PlatformClock::Pll160m]);
    port.power_readback = Ok(());
    left(&mut power, &mut port, &mut platform, failure.power);
    let releases = log
        .borrow()
        .iter()
        .filter(|operation| **operation == Operation::Release(PlatformClock::AnalogI2cMaster))
        .count();
    assert_eq!(releases, 1);
    assert!(platform.held().is_empty());
}

#[test]
fn a_failed_power_sequence_keeps_its_sources_for_the_retry() {
    let (_, mut port, mut platform) = setup();
    port.power_sequence_failure = Some(oer_chips_family_radio_power::power::PowerError {
        checkpoint: oer_chips_family_radio_power::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(try_enter::<WifiClient>(&mut power, &mut port, &mut platform).is_err());
    // The retry reuses the source taken before the failed checkpoint.
    assert_eq!(platform.held(), [PlatformClock::Pll160m]);
    port.power_sequence_failure = None;
    let wifi = entered::<WifiClient>(&mut power, &mut port, &mut platform);
    assert_eq!(platform.held(), COMMON);
    left(&mut power, &mut port, &mut platform, wifi);
    assert!(platform.held().is_empty());
}

#[test]
fn the_wifi_power_gate_opens_first_and_closes_last() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    let wifi = power.enter_wifi(&mut port, &mut platform).unwrap();
    assert_eq!(log.borrow().first(), Some(&Operation::WifiPower(true)));
    left(&mut power, &mut port, &mut platform, wifi);
    assert_eq!(log.borrow().last(), Some(&Operation::WifiPower(false)));
}

#[test]
fn repeated_common_power_does_not_repeat_global_clock_selection() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    let first = entered::<WifiClient>(&mut power, &mut port, &mut platform);
    left(&mut power, &mut port, &mut platform, first);
    let _second = entered::<WifiClient>(&mut power, &mut port, &mut platform);
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
}

#[test]
fn a_failed_initial_domain_entry_retries_the_clock_selection() {
    let (log, mut port, mut platform) = setup();
    port.power_sequence_failure = Some(oer_chips_family_radio_power::power::PowerError {
        checkpoint: oer_chips_family_radio_power::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(try_enter::<WifiClient>(&mut power, &mut port, &mut platform).is_err());
    port.power_sequence_failure = None;
    let _wifi = entered::<WifiClient>(&mut power, &mut port, &mut platform);
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
}

fn calibrated(port: &mut Port) -> oer_portable_time::Instant {
    use oer_portable_clock::Clock;
    use oer_portable_time::Duration;
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
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    let at = calibrated(&mut port);
    let first = power.enter_wifi(&mut port, &mut platform).unwrap();
    assert!(!port.calibrated_window);
    assert_eq!(
        &log.borrow()[..5],
        &[
            Operation::CalibrationComplete(at),
            Operation::WifiPower(true),
            Operation::SelectWifiLowPowerClock,
            Operation::ResetWifi(at),
            Operation::PowerSequence,
        ]
    );
    left(&mut power, &mut port, &mut platform, first);
    // A retained wake republishes PHY state before a repeated client entry.
    calibrated(&mut port);
    let _second = power.enter_wifi(&mut port, &mut platform).unwrap();
    assert!(port.calibrated_window);
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|op| matches!(op, Operation::ResetWifi(_)))
            .count(),
        1
    );
}

#[test]
fn wifi_reset_is_retried_after_power_failure_but_not_on_duplicate_admission() {
    let (log, mut port, mut platform) = setup();
    let mut power = CommonRadioPower::default();
    calibrated(&mut port);
    port.power_sequence_failure = Some(oer_chips_family_radio_power::power::PowerError {
        checkpoint: oer_chips_family_radio_power::power::PowerCheckpoint::ModemClockSource,
        expected: true,
        observed: false,
    });
    assert!(power.enter_wifi(&mut port, &mut platform).is_err());
    port.power_sequence_failure = None;
    let _entered = power.enter_wifi(&mut port, &mut platform).unwrap();
    let before = log.borrow().len();
    assert!(matches!(
        power.enter_wifi(&mut port, &mut platform),
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
}
