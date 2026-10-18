//! Concrete ESP32-S31 active-scan probe TX ownership.
//!
//! The executor-independent scan transaction decides when an active probe is
//! attempted. This module binds that edge to the polling control-TX owner and
//! retains the exact descriptor across successful and passive-fallback paths.

use crate::connection_coex::{ConnectionFrame, ConnectionFrameCoex};
use crate::{
    control_tx::{ControlTransmitter, ControlTxError},
    scan::ActiveProbeOutcome,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

use oer_esp32s31_ieee80211_mac::tx::{TxCompletion, TxHardware};

use oer_ieee80211_mac::management::ProbeRequest;

/// Complete inputs for one active-scan Probe Request publication.
pub struct ScanProbeRequest<'a> {
    pub source: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub ssid: &'a [u8],
    pub supported_rates: &'a [u8],
    pub current_channel: Option<u8>,
    pub descriptor_capacity: Option<u32>,
}

/// Detailed terminal observation retained for applications and HIL telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanProbeReport {
    Transmitted(TxCompletion),
    PassiveWithoutAttempt,
    PassiveAfterCompletion(TxCompletion),
    PassiveAfterError(ControlTxError),
}

impl ScanProbeReport {
    pub const fn outcome(self) -> ActiveProbeOutcome {
        match self {
            Self::Transmitted(_) => ActiveProbeOutcome::Transmitted,
            Self::PassiveWithoutAttempt
            | Self::PassiveAfterCompletion(_)
            | Self::PassiveAfterError(_) => ActiveProbeOutcome::PassiveFallback,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanTxSummary {
    pub completions: u32,
    pub failures: u32,
}

/// Shared state machine for cold and running active-probe publication.
pub struct ScanTxState {
    active_probe_available: bool,
    summary: ScanTxSummary,
}

impl ScanTxState {
    pub const fn new() -> Self {
        Self {
            active_probe_available: true,
            summary: ScanTxSummary {
                completions: 0,
                failures: 0,
            },
        }
    }

    pub fn begin_scan(&mut self) {
        self.active_probe_available = true;
        self.summary = ScanTxSummary::default();
    }

    pub const fn active_probe_available(&self) -> bool {
        self.active_probe_available
    }

    pub fn classify(
        &mut self,
        result: Result<TxCompletion, ControlTxError>,
    ) -> Result<ScanProbeReport, ControlTxError> {
        match result {
            Ok(completion) => {
                self.summary.completions = self.summary.completions.saturating_add(1);
                if completion.status() == 0 {
                    Ok(ScanProbeReport::Transmitted(completion))
                } else {
                    self.summary.failures = self.summary.failures.saturating_add(1);
                    self.active_probe_available = false;
                    Ok(ScanProbeReport::PassiveAfterCompletion(completion))
                }
            }
            Err(error) if error.retains_quiescent_owner() => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Ok(ScanProbeReport::PassiveAfterError(error))
            }
            Err(error) => {
                self.summary.failures = self.summary.failures.saturating_add(1);
                self.active_probe_available = false;
                Err(error)
            }
        }
    }

    pub const fn summary(&self) -> ScanTxSummary {
        self.summary
    }
}

impl Default for ScanTxState {
    fn default() -> Self {
        Self::new()
    }
}

/// Polling TX owner used by initial and running scans.
///
/// Probe completion is derived from the ordinary descriptor transaction, not
/// from ownership of the MAC interrupt setup token. The shared hard handler
/// may remain installed across a logical scan epoch and only acknowledges its
/// level source; no WDEV bottom half owns this control descriptor.
pub struct RunningScanTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    control: ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
    state: ScanTxState,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> RunningScanTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    pub const fn new(control: ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            state: ScanTxState::new(),
        }
    }

    #[cfg(test)]
    const fn new_for_test(control: ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        Self {
            control,
            state: ScanTxState::new(),
        }
    }

    pub fn begin_scan(&mut self) {
        self.state.begin_scan();
    }

    /// Send one Probe Request when active probing remains available, after
    /// its per-frame coexistence request.
    pub async fn transmit_probe_request<H: TxHardware, C: ConnectionFrameCoex>(
        &mut self,
        hardware: &mut H,
        request: ScanProbeRequest<'_>,
        coex: &mut C,
    ) -> Result<ScanProbeReport, ControlTxError> {
        if !self.state.active_probe_available() {
            return Ok(ScanProbeReport::PassiveWithoutAttempt);
        }
        // A Probe Request only requests the air; its priorities stay.
        let _ = coex.connection_frame(ConnectionFrame::ProbeRequest).await;
        let ScanProbeRequest {
            source,
            sequence_number,
            ssid,
            supported_rates,
            current_channel,
            descriptor_capacity,
        } = request;
        let result = self
            .control
            .transmit_probe_request(
                hardware,
                ProbeRequest {
                    destination: oer_ieee80211_mac::management::BROADCAST_ADDRESS,
                    source,
                    bssid: oer_ieee80211_mac::management::BROADCAST_ADDRESS,
                    sequence_number,
                    ssid,
                    supported_rates,
                },
                current_channel,
                descriptor_capacity,
            )
            .await;
        self.state.classify(result)
    }

    pub fn into_parts(
        self,
    ) -> (
        ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
        ScanTxSummary,
    ) {
        (self.control, self.state.summary())
    }
}

#[cfg(test)]
mod tests;
