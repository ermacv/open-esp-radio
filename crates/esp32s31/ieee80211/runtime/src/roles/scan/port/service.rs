#![expect(
    clippy::manual_async_fn,
    reason = "scan port implementations keep explicit borrowed Future contracts"
)]

use oer_esp32s31_ieee80211_mac::init::{
    MacRuntimeStopHardware, MacSnifferHardware, activate_promiscuous_receive,
    deactivate_promiscuous_receive,
};

use super::*;

/// One scan dwell tick.
const SCAN_DWELL_TICK: oer_time::Duration = oer_time::Duration::from_millis(1);

impl<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, const RECORDS: usize> StaScanPort
    for ScanPort<'resources, 'sequence, 'ssid, 'rates, P, H, R, T, W, O, RECORDS>
where
    P: ScanPhyPort<H>,
    H: MacSnifferHardware + MacRuntimeStopHardware,
    R: ScanReceivePort<H>,
    T: ScanTransmitPort<H>,
    W: oer_time::Timer + crate::mac_clock::ReceptionClock,
    O: ScanFrameObserver,
{
    type Channel = u8;
    type Candidate = ScanRecord;
    type Error = ScanPortError<P::Error, R::Error, T::Error>;

    fn begin_scan(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.storage.table.clear();
            self.telemetry = ScanTelemetry::default();
            self.radio.tx.begin_scan();
            self.radio
                .rx
                .prepare_initial(&mut self.radio.hardware)
                .map_err(ScanPortError::Receive)
        }
    }

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
        requested_dwell_ticks: u16,
    ) -> impl Future<Output = Result<u16, Self::Error>> + '_ {
        // One production dwell tick is one millisecond.
        async move {
            self.radio
                .phy
                .switch_channel(
                    &mut self.radio.hardware,
                    context.channel,
                    requested_dwell_ticks,
                )
                .await
                .map_err(ScanPortError::ChannelSwitch)
        }
    }

    fn start_receive(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            activate_promiscuous_receive(&mut self.radio.hardware);
            let started = self
                .radio
                .rx
                .start(&mut self.radio.hardware)
                .await
                .map_err(ScanPortError::Receive);
            if started.is_err() {
                deactivate_promiscuous_receive(&mut self.radio.hardware);
            } else {
                self.radio.hardware.resume_mac_runtime();
            }
            started
        }
    }

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<ActiveProbeOutcome, Self::Error>> + '_ {
        let request = ScanProbeRequest {
            source: self.station.station_address,
            sequence_number: self.storage.sequence.take(),
            ssid: b"",
            supported_rates: self.station.supported_rates,
            current_channel: Some(context.channel),
            descriptor_capacity: self.station.descriptor_capacity,
        };
        async move {
            let mut coex = self.radio.phy.connection_coex();
            self.radio
                .tx
                .transmit_probe_request(&mut self.radio.hardware, request, &mut coex)
                .await
                .map(ScanProbeReport::outcome)
                .map_err(ScanPortError::Transmit)
        }
    }

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.observe_scan_rx(context.channel).map(|_| ())
    }

    fn wait_dwell_tick(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            crate::time::wait_for(&self.timer, SCAN_DWELL_TICK).await;
            Ok(())
        }
    }

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            deactivate_promiscuous_receive(&mut self.radio.hardware);
            self.radio.hardware.request_mac_runtime_stop();
            crate::time::wait_for(&self.timer, crate::time::MAC_STOP_SETTLE).await;
            while self.radio.hardware.mac_runtime_active_state() != 0 {
                crate::time::wait_for(&self.timer, crate::time::MAC_STOP_POLL).await;
            }
            loop {
                let progress = self.observe_scan_rx(context.channel)?;
                if progress.completed_descriptors == 0 {
                    break;
                }
            }
            self.radio.rx.park().map_err(ScanPortError::Receive)
        }
    }

    fn prepare_next_ring(
        &mut self,
        _context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        self.radio
            .rx
            .prepare_next_channel(&mut self.radio.hardware)
            .map_err(ScanPortError::Receive)
    }

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
        if !self.station.select_candidate {
            return Ok(None);
        }
        Ok(best_matching_ssid_and_security(
            self.storage.table.records(),
            self.station.target_ssid,
            self.station.security,
        )
        .copied())
    }
}
