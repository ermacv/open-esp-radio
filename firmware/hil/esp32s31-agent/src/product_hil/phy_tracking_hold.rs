//! One forced shared-PHY tracking tick timed on the device: the RF hold of
//! the vendor-policy tracking ().
//!
//! The periodic timer is suspended, so this tick is the only tracking. The
//! vendor's temperature-tracking debug policy forces the requested branches
//! with the requested threshold for the tick and is cleared afterwards. The
//! tick's operations, the PHY grant-protect request and the Wi-Fi baseband
//! off window are timed with the PHY's own observation recorder.

use oer_esp32s31_phy::state::client::{PhyClientSnapshot, RadioClient};
use oer_esp32s31_phy::{
    ConcurrentTrackingTick, PhyTargetObserver,
    tracking::observation::{Event, Operation, Recorder, Report, Timing},
};
use oer_hil_protocol::phy::{
    PhyTrackingHoldClients, PhyTrackingHoldEvidence, PhyTrackingHoldRequest, PhyTrackingHoldTick,
    PhyTrackingHoldTiming,
};

use super::SharedRadio;

/// Serve hold requests on `radio` for as long as the future runs: the
/// server of an image mode without the tracking task.
pub(super) async fn serve(radio: &SharedRadio) -> core::convert::Infallible {
    let _server = crate::phy_tracking::HoldServer::register();
    loop {
        let request = crate::phy_tracking::hold_requested().await;
        crate::phy_tracking::hold_measured(measure(radio, request).await);
    }
}

/// Timestamps each observed operation edge into the recorder.
struct HoldObserver<'a> {
    recorder: &'a mut Recorder,
}

impl PhyTargetObserver for HoldObserver<'_> {
    fn tracking_operation(&mut self, operation: Operation, event: Event) {
        self.recorder
            .observe(operation, event, embassy_time::Instant::now().as_micros());
    }
}

/// Run one forced tick and report its timing.
pub(super) async fn measure(
    radio: &SharedRadio,
    request: PhyTrackingHoldRequest,
) -> PhyTrackingHoldEvidence {
    let mut guard = radio.lock().await;
    let clients = guard.lease().attachment().client_snapshot().map(clients);
    let forced = u8::from(request.rfpll) | u8::from(request.calibration) << 1;
    if guard
        .lease()
        .attachment_mut()
        .set_temperature_tracking_debug(forced, request.threshold)
        .is_err()
    {
        return PhyTrackingHoldEvidence {
            tick: PhyTrackingHoldTick::Skipped,
            clients,
            ..PhyTrackingHoldEvidence::default()
        };
    }
    let mut recorder = Recorder::default();
    let tick = guard
        .track_observed(HoldObserver {
            recorder: &mut recorder,
        })
        .await;
    // A failed tick poisons the domain, which then refuses the reset; the
    // policy no longer matters.
    let _ = guard
        .lease()
        .attachment_mut()
        .set_temperature_tracking_debug(0, 0);
    let tick = match tick {
        Ok(ConcurrentTrackingTick::Tracked(_)) => PhyTrackingHoldTick::Tracked,
        Ok(ConcurrentTrackingTick::NotDue) => PhyTrackingHoldTick::NotDue,
        Ok(ConcurrentTrackingTick::Unavailable(_)) => PhyTrackingHoldTick::Skipped,
        Err(_) => PhyTrackingHoldTick::Failed,
    };
    evidence(tick, clients, &recorder.report())
}

const fn clients(snapshot: PhyClientSnapshot) -> PhyTrackingHoldClients {
    PhyTrackingHoldClients {
        wifi: snapshot.contains(RadioClient::Wifi),
        bluetooth: snapshot.contains(RadioClient::Bluetooth),
        ieee802154: snapshot.contains(RadioClient::Ieee802154),
    }
}

fn evidence(
    tick: PhyTrackingHoldTick,
    clients: Option<PhyTrackingHoldClients>,
    report: &Report,
) -> PhyTrackingHoldEvidence {
    let timing = |operation| wire(report.timing(operation));
    PhyTrackingHoldEvidence {
        tick,
        clients,
        invalid: report.invalid,
        tracking: timing(Operation::Tracking),
        rfpll: timing(Operation::Rfpll),
        calibration: timing(Operation::Calibration),
        rx_gain: timing(Operation::RxGain),
        tx_dc_pwdet: timing(Operation::TxDcPwdet),
        grant_protect_rfpll: timing(Operation::GrantProtectRfpll),
        grant_protect_common: timing(Operation::GrantProtectCommon),
        grant_protect_transmit: timing(Operation::GrantProtectTransmit),
        wifi_baseband_off: timing(Operation::WifiBasebandOff),
    }
}

const fn wire(timing: Timing) -> PhyTrackingHoldTiming {
    PhyTrackingHoldTiming {
        completed: timing.completed,
        failed: timing.failed,
        maximum_micros: timing.maximum_micros,
        elapsed_micros: timing.elapsed_micros,
    }
}
