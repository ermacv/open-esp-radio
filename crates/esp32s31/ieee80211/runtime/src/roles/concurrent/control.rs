#![expect(
    clippy::manual_async_fn,
    reason = "the concurrent control adapter keeps its borrowed Future service contract explicit"
)]

//! Typed control arbitration for one same-channel station plus SoftAP pair.

use core::future::Future;

use crate::datapath::{
    DatapathControlContext, DatapathControlProgress,
    paired::{
        DatapathPairRole, DatapathPairedControlProgress, DatapathPairedControlService,
        DatapathPairedStopProgress,
    },
};

use embassy_futures::select::{Either, select};

use oer_time::{Instant, Timer};

pub trait StaApStationControlRole<H, PhysicalTx> {
    type Error: 'static;
    type Exit: 'static;

    fn service_station_control<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        context: DatapathControlContext,
        retain_physical_tx: bool,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a;

    fn station_control_ready(&self, now: oer_time::Instant) -> bool;

    /// Whether a controlled stop must still run station control, for the
    /// leaving station's Deauthentication.
    fn station_required_before_stop(&self) -> bool;

    /// Wait for role-local work without borrowing the shared physical TX.
    /// Holding that owner across a sleep would prevent the AP beacon timer
    /// from beginning its own finite transaction.
    fn wait_station_control_ready<'a>(
        &'a mut self,
        timer: &'a impl Timer,
    ) -> impl Future<Output = ()> + 'a;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointControlProgress {
    Idle,
    More,
    TxPending,
}

pub trait StaApAccessPointControlRole<H, PhysicalTx> {
    type Error: 'static;

    fn beacon_publication_due(&self, now: Instant) -> bool;

    fn service_access_point_control(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        now: oer_time::Instant,
        retain_physical_tx: bool,
    ) -> Result<StaApAccessPointControlProgress, Self::Error>;

    fn service_access_point_stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
    ) -> Result<DatapathPairedStopProgress, Self::Error>;

    fn next_access_point_control_deadline(&self) -> Result<Instant, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApControlError<StationError, AccessPointError> {
    Station(StationError),
    AccessPoint(AccessPointError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApControlExit<StationExit> {
    Station(StationExit),
}

/// Minimal policy owner above the two role-local control machines.
///
/// A beacon whose publication edge is already due is serviced before other
/// control work. At all other idle boundaries the station gets one finite
/// step followed by one AP step. The arbiter never inspects protocol internals
/// and reports the exact role for every resulting hardware TX transaction.
pub struct StaApControlArbiter<T> {
    timer: T,
    next_access_point_deadline: Instant,
}

impl<T> StaApControlArbiter<T> {
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            next_access_point_deadline: Instant::EPOCH,
        }
    }

    fn map_access_point_progress<E>(
        progress: StaApAccessPointControlProgress,
    ) -> DatapathPairedControlProgress<E> {
        match progress {
            StaApAccessPointControlProgress::Idle => DatapathPairedControlProgress::Idle,
            StaApAccessPointControlProgress::More => DatapathPairedControlProgress::More,
            StaApAccessPointControlProgress::TxPending => {
                DatapathPairedControlProgress::TxPending(DatapathPairRole::Second)
            }
        }
    }
}

impl<H, PhysicalTx, Station, AccessPoint, T>
    DatapathPairedControlService<H, PhysicalTx, Station, AccessPoint> for StaApControlArbiter<T>
where
    T: Timer,
    Station: StaApStationControlRole<H, PhysicalTx>,
    AccessPoint: StaApAccessPointControlRole<H, PhysicalTx>,
{
    type Error = StaApControlError<Station::Error, AccessPoint::Error>;
    type Exit = StaApControlExit<Station::Exit>;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        context: DatapathControlContext,
        retained_tx: Option<DatapathPairRole>,
    ) -> impl Future<Output = Result<DatapathPairedControlProgress<Self::Exit>, Self::Error>> + 'a
    {
        async move {
            // A controlled stop gives the station its leaving turn first; the
            // access point's own stop runs after it.
            if context.stop_pending && station.station_required_before_stop() {
                return match station
                    .service_station_control(hardware, physical_tx, context, false)
                    .await
                    .map_err(StaApControlError::Station)?
                {
                    DatapathControlProgress::Idle => Ok(DatapathPairedControlProgress::Idle),
                    DatapathControlProgress::More => Ok(DatapathPairedControlProgress::More),
                    DatapathControlProgress::TxPending => Ok(
                        DatapathPairedControlProgress::TxPending(DatapathPairRole::First),
                    ),
                    DatapathControlProgress::Exit(exit) => Ok(DatapathPairedControlProgress::Exit(
                        StaApControlExit::Station(exit),
                    )),
                };
            }
            let now = self.timer.now();
            if retained_tx == Some(DatapathPairRole::Second) {
                let progress = access_point
                    .service_access_point_control(hardware, physical_tx, now, true)
                    .map_err(StaApControlError::AccessPoint)?;
                if progress == StaApAccessPointControlProgress::Idle {
                    self.next_access_point_deadline = access_point
                        .next_access_point_control_deadline()
                        .map_err(StaApControlError::AccessPoint)?;
                }
                return Ok(Self::map_access_point_progress(progress));
            }
            if retained_tx == Some(DatapathPairRole::First) {
                return match station
                    .service_station_control(hardware, physical_tx, context, true)
                    .await
                    .map_err(StaApControlError::Station)?
                {
                    DatapathControlProgress::Idle => Ok(DatapathPairedControlProgress::Idle),
                    DatapathControlProgress::More => Ok(DatapathPairedControlProgress::More),
                    DatapathControlProgress::TxPending => Ok(
                        DatapathPairedControlProgress::TxPending(DatapathPairRole::First),
                    ),
                    DatapathControlProgress::Exit(exit) => Ok(DatapathPairedControlProgress::Exit(
                        StaApControlExit::Station(exit),
                    )),
                };
            }
            if access_point.beacon_publication_due(now) {
                let progress = access_point
                    .service_access_point_control(hardware, physical_tx, now, false)
                    .map_err(StaApControlError::AccessPoint)?;
                if progress != StaApAccessPointControlProgress::Idle {
                    return Ok(Self::map_access_point_progress(progress));
                }
            }

            match station
                .service_station_control(hardware, physical_tx, context, false)
                .await
                .map_err(StaApControlError::Station)?
            {
                DatapathControlProgress::Idle => {}
                DatapathControlProgress::More => return Ok(DatapathPairedControlProgress::More),
                DatapathControlProgress::TxPending => {
                    return Ok(DatapathPairedControlProgress::TxPending(
                        DatapathPairRole::First,
                    ));
                }
                DatapathControlProgress::Exit(exit) => {
                    return Ok(DatapathPairedControlProgress::Exit(
                        StaApControlExit::Station(exit),
                    ));
                }
            }

            let now = self.timer.now();
            let progress = access_point
                .service_access_point_control(hardware, physical_tx, now, false)
                .map_err(StaApControlError::AccessPoint)?;
            if progress == StaApAccessPointControlProgress::Idle {
                self.next_access_point_deadline = access_point
                    .next_access_point_control_deadline()
                    .map_err(StaApControlError::AccessPoint)?;
            }
            Ok(Self::map_access_point_progress(progress))
        }
    }

    fn ready(
        &self,
        station: &Station,
        _access_point: &AccessPoint,
        now: oer_time::Instant,
    ) -> bool {
        station.station_control_ready(now) || now >= self.next_access_point_deadline
    }

    fn required_before_stop(&self, station: &Station, _access_point: &AccessPoint) -> bool {
        station.station_required_before_stop()
    }

    fn stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        _station: &mut Station,
        access_point: &mut AccessPoint,
    ) -> Result<DatapathPairedStopProgress, Self::Error> {
        access_point
            .service_access_point_stop(hardware, physical_tx)
            .map_err(StaApControlError::AccessPoint)
    }

    fn wait_ready<'a>(
        &'a mut self,
        _physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        _access_point: &'a mut AccessPoint,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            match select(
                station.wait_station_control_ready(&self.timer),
                self.timer.wait_until(self.next_access_point_deadline),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
