use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_pac::{WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::test_clocks::{ClockEvent, CountingPlatformClocks, abandoned, held, holding};
use crate::power::{PlatformClock, PlatformClockBackend, PlatformClockProvider, PowerEntry};

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
    PowerSequence(PowerEntry),
}

type Log = Rc<RefCell<Vec<Operation>>>;

struct Port {
    power: WifiPowerBaseline,
    power_readback: Result<(), WifiPowerRestoreReadback>,
    /// Fail the sequence after it took the 160 MHz reference.
    power_sequence_failure: Option<crate::power::PowerError>,
    log: Log,
}

impl Port {
    fn new(log: &Log) -> Self {
        Self {
            power: WifiPowerBaseline::for_host_test(false),
            power_readback: Ok(()),
            power_sequence_failure: None,
            log: log.clone(),
        }
    }
}

impl ClockPort for Port {
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
        entry: PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        self.log.borrow_mut().push(Operation::PowerSequence(entry));
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

/// Enter `K`, which the test expects to succeed.
fn entered<K: RadioClientKind>(
    power: &mut CommonRadioPower<crate::power::test_clocks::CountingBackend>,
    port: &mut Port,
    platform: &CountingPlatformClocks,
) -> CommonPower<K> {
    power
        .enter::<K>(port, platform)
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
            Operation::PowerSequence(PowerEntry::FirstSinceBoot),
            Operation::Acquire(PlatformClock::Mpll),
            Operation::Acquire(PlatformClock::Pll160m),
            Operation::Acquire(PlatformClock::AnalogI2cMaster),
        ]
    );
    assert!(matches!(
        power.enter::<BluetoothClient>(&mut port, &platform),
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
        power.enter::<Ieee802154Client>(&mut port, &platform),
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
    assert!(
        power
            .enter::<BluetoothClient>(&mut port, &platform)
            .is_err()
    );
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
fn power_up_after_every_client_left_keeps_the_wifi_resets() {
    let (log, mut port, platform) = setup();
    let mut power = CommonRadioPower::default();

    let first = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    left(&mut power, &mut port, first);
    let _second = entered::<Ieee802154Client>(&mut power, &mut port, &platform);
    let sequences: Vec<_> = log
        .borrow()
        .iter()
        .filter_map(|operation| match operation {
            Operation::PowerSequence(entry) => Some(*entry),
            _ => None,
        })
        .collect();
    assert_eq!(
        sequences,
        [PowerEntry::FirstSinceBoot, PowerEntry::Repeated],
    );
    power.abandon_for_test();
}

#[test]
fn a_failed_first_power_up_still_pulses_the_wifi_resets_on_retry() {
    let (log, mut port, platform) = setup();
    port.power_sequence_failure = Some(crate::power::PowerError {
        checkpoint: crate::power::PowerCheckpoint::ResetReleased,
        expected: true,
        observed: false,
    });
    let mut power = CommonRadioPower::default();
    assert!(power.enter::<WifiClient>(&mut port, &platform).is_err());
    port.power_sequence_failure = None;
    let _wifi = entered::<WifiClient>(&mut power, &mut port, &platform);
    let sequences: Vec<_> = log
        .borrow()
        .iter()
        .filter_map(|operation| match operation {
            Operation::PowerSequence(entry) => Some(*entry),
            _ => None,
        })
        .collect();
    assert_eq!(
        sequences,
        [PowerEntry::FirstSinceBoot, PowerEntry::FirstSinceBoot]
    );
    power.abandon_for_test();
}
