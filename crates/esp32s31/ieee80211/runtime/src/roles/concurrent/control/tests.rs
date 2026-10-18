use core::{convert::Infallible, future::pending};

use oer_time_virtual::SkipClock;

use super::*;

struct Station {
    calls: usize,
    progress: DatapathControlProgress<u8>,
}

impl StaApStationControlRole<(), ()> for Station {
    type Error = Infallible;
    type Exit = u8;

    fn service_station_control<'a>(
        &'a mut self,
        _hardware: &'a mut (),
        _physical_tx: &'a mut (),
        _context: DatapathControlContext,
        _retain_physical_tx: bool,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            self.calls += 1;
            Ok(self.progress)
        }
    }

    fn wait_station_control_ready<'a>(
        &'a mut self,
        _timer: &'a impl Timer,
    ) -> impl Future<Output = ()> + 'a {
        pending()
    }

    fn station_control_ready(&self, _now: oer_time::Instant) -> bool {
        false
    }

    fn station_required_before_stop(&self) -> bool {
        false
    }
}

struct AccessPoint {
    due: bool,
    calls: usize,
    progress: StaApAccessPointControlProgress,
}

impl StaApAccessPointControlRole<(), ()> for AccessPoint {
    type Error = Infallible;

    fn beacon_publication_due(&self, _now: Instant) -> bool {
        self.due
    }

    fn service_access_point_control(
        &mut self,
        _hardware: &mut (),
        _physical_tx: &mut (),
        _now: oer_time::Instant,
        _retain_physical_tx: bool,
    ) -> Result<StaApAccessPointControlProgress, Self::Error> {
        self.calls += 1;
        Ok(self.progress)
    }

    fn service_access_point_stop(
        &mut self,
        _hardware: &mut (),
        _physical_tx: &mut (),
    ) -> Result<DatapathPairedStopProgress, Self::Error> {
        Ok(DatapathPairedStopProgress::Stopped)
    }

    fn next_access_point_control_deadline(&self) -> Result<Instant, Self::Error> {
        Ok(Instant::from_micros(1_000))
    }
}

fn service(
    station: &mut Station,
    access_point: &mut AccessPoint,
) -> DatapathPairedControlProgress<StaApControlExit<u8>> {
    service_with_retained(station, access_point, None)
}

fn service_with_retained(
    station: &mut Station,
    access_point: &mut AccessPoint,
    retained: Option<DatapathPairRole>,
) -> DatapathPairedControlProgress<StaApControlExit<u8>> {
    embassy_futures::block_on(DatapathPairedControlService::service(
        &mut StaApControlArbiter::new(SkipClock::new()),
        &mut (),
        &mut (),
        station,
        access_point,
        DatapathControlContext::IDLE,
        retained,
    ))
    .unwrap()
}

#[test]
fn due_beacon_owns_tx_before_station_control_is_polled() {
    let mut station = Station {
        calls: 0,
        progress: DatapathControlProgress::TxPending,
    };
    let mut access_point = AccessPoint {
        due: true,
        calls: 0,
        progress: StaApAccessPointControlProgress::TxPending,
    };

    assert_eq!(
        service(&mut station, &mut access_point),
        DatapathPairedControlProgress::TxPending(DatapathPairRole::Second)
    );
    assert_eq!(station.calls, 0);
    assert_eq!(access_point.calls, 1);
}

#[test]
fn station_control_tx_retains_first_role_identity() {
    let mut station = Station {
        calls: 0,
        progress: DatapathControlProgress::TxPending,
    };
    let mut access_point = AccessPoint {
        due: false,
        calls: 0,
        progress: StaApAccessPointControlProgress::TxPending,
    };

    assert_eq!(
        service(&mut station, &mut access_point),
        DatapathPairedControlProgress::TxPending(DatapathPairRole::First)
    );
    assert_eq!(station.calls, 1);
    assert_eq!(access_point.calls, 0);
}

#[test]
fn idle_station_yields_same_turn_to_access_point_control() {
    let mut station = Station {
        calls: 0,
        progress: DatapathControlProgress::Idle,
    };
    let mut access_point = AccessPoint {
        due: false,
        calls: 0,
        progress: StaApAccessPointControlProgress::TxPending,
    };

    assert_eq!(
        service(&mut station, &mut access_point),
        DatapathPairedControlProgress::TxPending(DatapathPairRole::Second)
    );
    assert_eq!(station.calls, 1);
    assert_eq!(access_point.calls, 1);
}

#[test]
fn retained_station_tx_excludes_access_point_control() {
    let mut station = Station {
        calls: 0,
        progress: DatapathControlProgress::Idle,
    };
    let mut access_point = AccessPoint {
        due: true,
        calls: 0,
        progress: StaApAccessPointControlProgress::TxPending,
    };

    assert_eq!(
        service_with_retained(
            &mut station,
            &mut access_point,
            Some(DatapathPairRole::First)
        ),
        DatapathPairedControlProgress::Idle
    );
    assert_eq!(station.calls, 1);
    assert_eq!(access_point.calls, 0);
}

#[test]
fn retained_access_point_tx_excludes_station_control() {
    let mut station = Station {
        calls: 0,
        progress: DatapathControlProgress::TxPending,
    };
    let mut access_point = AccessPoint {
        due: false,
        calls: 0,
        progress: StaApAccessPointControlProgress::Idle,
    };

    assert_eq!(
        service_with_retained(
            &mut station,
            &mut access_point,
            Some(DatapathPairRole::Second)
        ),
        DatapathPairedControlProgress::Idle
    );
    assert_eq!(station.calls, 0);
    assert_eq!(access_point.calls, 1);
}

#[test]
fn absolute_access_point_deadline_is_an_o1_readiness_edge() {
    let arbiter = StaApControlArbiter {
        timer: SkipClock::new(),
        next_access_point_deadline: Instant::from_micros(10_000),
    };
    let station = Station {
        calls: 0,
        progress: DatapathControlProgress::Idle,
    };
    let access_point = AccessPoint {
        due: false,
        calls: 0,
        progress: StaApAccessPointControlProgress::Idle,
    };

    assert!(!DatapathPairedControlService::ready(
        &arbiter,
        &station,
        &access_point,
        oer_time::Instant::from_micros(9_999),
    ));
    assert!(DatapathPairedControlService::ready(
        &arbiter,
        &station,
        &access_point,
        oer_time::Instant::from_micros(10_000),
    ));
}
