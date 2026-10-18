use core::{
    future::{Future, ready},
    pin::{Pin, pin},
    task::{Context, Poll},
};

use crate::control_tx::{ControlTxConfig, WifiTxResources};

use oer_esp32s31_hal::types::{
    MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};

use oer_esp32s31_ieee80211::ordinary_tx::WifiTxPowerPair;

use oer_esp32s31_ieee80211_mac::tx::{
    HardwareOwnedTxDma, PreparedTxDma, TxSlot, runtime::WifiTxRuntimePolicy,
};

use crate::connection_coex::{ConnectionFrame, ConnectionFrameCoex};

use super::*;

/// Records the connection frames that requested the air.
#[derive(Default)]
struct Coex(std::vec::Vec<ConnectionFrame>);

impl ConnectionFrameCoex for Coex {
    fn connection_frame(
        &mut self,
        frame: ConnectionFrame,
    ) -> impl Future<Output = Option<crate::connection_coex::ReconnectFramePriority>> + '_ {
        self.0.push(frame);
        ready(None)
    }
}

#[derive(Default)]
struct ScanTxHardware {
    publications: u8,
    completion: Option<MacTxCompletionObservation>,
}

impl TxHardware for ScanTxHardware {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        _queue: u8,
        _program: MacLegacyTxProgram,
    ) -> bool {
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.publications = self.publications.saturating_add(1);
    }

    fn take_tx_completion(&mut self, _queue: u8) -> Option<MacTxCompletionObservation> {
        self.completion.take()
    }

    fn begin_tx_timeout_abort(&mut self, _queue: u8) -> bool {
        false
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        _queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        match reason {
            MacTxDetachReason::Completed => MacTxDetachOutcome::Detached(detached(
                MacTxQueueDetached::new_model(expected_descriptor_head),
            )),
            MacTxDetachReason::Collision | MacTxDetachReason::Timeout => {
                MacTxDetachOutcome::NoEvent
            }
        }
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }
}

#[derive(Clone, Copy)]
struct ScanTxPower;

impl WifiTxPowerProfile for ScanTxPower {
    fn power_pair(&self, _rate_code: u8) -> WifiTxPowerPair {
        WifiTxPowerPair {
            primary: 5,
            alternate: 6,
        }
    }
}

type ScanTxTimer = oer_time_virtual::SkipClock;

fn scan_tx_completion(status: u8) -> MacTxCompletionObservation {
    MacTxCompletionObservation::new_model(status, 0)
}

fn running_scan_tx<'a>(
    slot: Pin<&'a mut TxSlot<256>>,
) -> RunningScanTx<'a, ScanTxPower, fn() -> u32, ScanTxTimer, 256> {
    fn entropy() -> u32 {
        0x1234_5678
    }
    RunningScanTx::new_for_test(ControlTransmitter::new(
        WifiTxResources {
            slot,
            policy: WifiTxRuntimePolicy::vendor_defaults(),
            power: ScanTxPower,
            entropy,
            timer: ScanTxTimer::default(),
        },
        ControlTxConfig {
            completion_timeout: oer_time::Duration::from_micros(10),
            poll_interval: oer_time::Duration::from_micros(1),
        },
    ))
}

fn scan_probe_request() -> ScanProbeRequest<'static> {
    ScanProbeRequest {
        source: [2, 3, 4, 5, 6, 7],
        sequence_number: SequenceNumber::new(9).unwrap(),
        ssid: b"",
        supported_rates: &[0x82, 0x84],
        current_channel: Some(6),
        descriptor_capacity: Some(256),
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn running_scan_tx_returns_the_control_owner_after_a_probe() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let mut hardware = ScanTxHardware {
        completion: Some(scan_tx_completion(0)),
        ..ScanTxHardware::default()
    };
    let mut tx = running_scan_tx(slot.as_mut());
    tx.begin_scan();

    let mut coex = Coex::default();
    let report =
        block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request(), &mut coex))
            .expect("completed running probe");
    assert_eq!(coex.0, [ConnectionFrame::ProbeRequest]);
    assert!(matches!(report, ScanProbeReport::Transmitted(_)));
    let (mut control, summary) = tx.into_parts();
    assert_eq!(summary.completions, 1);
    assert_eq!(summary.failures, 0);

    hardware.completion = Some(scan_tx_completion(0));
    block_on(control.transmit_probe_request(
        &mut hardware,
        ProbeRequest {
            destination: oer_ieee80211_mac::management::BROADCAST_ADDRESS,
            source: [2, 3, 4, 5, 6, 7],
            bssid: oer_ieee80211_mac::management::BROADCAST_ADDRESS,
            sequence_number: SequenceNumber::new(10).unwrap(),
            ssid: b"",
            supported_rates: &[0x82, 0x84],
        },
        Some(6),
        Some(256),
    ))
    .expect("returned control owner remains usable");
    assert_eq!(hardware.publications, 2);
}

#[test]
fn failed_running_probe_disables_further_active_attempts() {
    let mut slot = pin!(TxSlot::<256>::new_model());
    let mut hardware = ScanTxHardware {
        completion: Some(scan_tx_completion(1)),
        ..ScanTxHardware::default()
    };
    let mut tx = running_scan_tx(slot.as_mut());
    tx.begin_scan();

    let mut coex = Coex::default();
    let first = block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request(), &mut coex))
        .expect("nonzero completion is a safe passive fallback");
    assert!(matches!(first, ScanProbeReport::PassiveAfterCompletion(_)));
    let second =
        block_on(tx.transmit_probe_request(&mut hardware, scan_probe_request(), &mut coex))
            .expect("disabled active probe remains passive");
    assert_eq!(second, ScanProbeReport::PassiveWithoutAttempt);
    // Only the probe that reached the air requested it.
    assert_eq!(coex.0, [ConnectionFrame::ProbeRequest]);
    assert_eq!(hardware.publications, 1);

    let (_control, summary) = tx.into_parts();
    assert_eq!(summary.completions, 1);
    assert_eq!(summary.failures, 1);
}
