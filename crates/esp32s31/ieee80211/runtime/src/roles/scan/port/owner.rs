#![expect(
    clippy::type_complexity,
    reason = "the scan owner result exposes every exact phase owner without type erasure"
)]

use super::*;
impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize>
    ScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
{
    pub const fn new(
        radio: ScanRadio<P, H, R, T>,
        storage: ScanStorage<'resources, 'sequence, O, RECORDS>,
        station: ScanStation<'ssid, 'rates>,
        timer: W,
    ) -> Self {
        Self {
            radio,
            storage,
            station,
            timer,
            telemetry: ScanTelemetry {
                raw_frames: 0,
                ring_epochs: 0,
            },
        }
    }

    pub fn into_parts(self) -> ScanPortParts<'resources, 'sequence, P, H, R, T, W, O, RECORDS> {
        let Self {
            radio,
            storage,
            station: _,
            timer,
            telemetry,
        } = self;
        let ScanRadio {
            phy,
            hardware,
            rx,
            tx,
        } = radio;
        let ScanStorage {
            table,
            frame,
            observer,
            sequence,
        } = storage;
        ScanPortParts {
            phy,
            hardware,
            rx,
            tx,
            timer,
            observer,
            table,
            frame,
            sequence,
            telemetry,
        }
    }

    pub(super) fn observe_scan_rx(
        &mut self,
        channel: u8,
    ) -> Result<ScanRxProgress, ScanPortError<P::Error, R::Error, T::Error>>
    where
        P: ScanPhyPort<H>,
        R: ScanReceivePort<H>,
        T: ScanTransmitPort<H>,
        O: ScanFrameObserver,
        W: crate::mac_clock::ReceptionClock,
    {
        let mut context = ScanObservationContext::new(
            channel,
            self.timer.snapshot(),
            self.storage.frame,
            self.storage.table,
            &mut self.storage.observer,
        );
        let progress = self
            .radio
            .rx
            .observe_management(&mut self.radio.hardware, &mut context)
            .map_err(ScanPortError::Receive)?;
        self.telemetry.raw_frames = self
            .telemetry
            .raw_frames
            .saturating_add(progress.completed_descriptors);
        if progress.recycled_descriptors != 0 {
            self.telemetry.ring_epochs = self.telemetry.ring_epochs.saturating_add(1);
        }
        Ok(progress)
    }
}
