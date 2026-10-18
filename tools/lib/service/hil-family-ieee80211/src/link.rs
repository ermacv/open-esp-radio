//! What the Wi-Fi workloads read from a capture of the board under test
//! beyond the generic exchange: the station's lifecycle across a pause, its
//! beacon losses, the station epoch cycle, a finite monitor capture's
//! frames and the access point's airtime accounting.

use std::time::Duration;

use oer_hil_link::{CommandHandle, Received, SerialCapture, latest_boot_id_in};
use oer_hil_protocol::{
    system::StackUsage,
    wifi::{
        StationEpochEvidence, StationLifecycleEvent, WifiAirtimePeerEvidence, WifiMonitorEvidence,
        WifiMonitorFrameChunk,
    },
};

use crate::Result;

/// A station epoch cycle the device accepted.
#[derive(Clone, Copy, Debug)]
pub struct StationEpochHandle {
    request_id: u32,
    first_event: usize,
}

/// A finite monitor capture: its frame chunks and its summary.
pub struct MonitorCaptureEvidence {
    pub chunks: Vec<WifiMonitorFrameChunk>,
    pub summary: WifiMonitorEvidence,
}

/// The Wi-Fi evidence of a capture.
pub trait WifiCapture {
    /// Fail when the current boot's station lost a beacon.
    fn require_no_beacon_loss(&self) -> Result<()>;

    /// A pause must preserve the existing association, including when a
    /// reconnect happens quickly enough for the throughput floor to pass.
    fn require_station_unchanged_since(&self, first_event: usize) -> Result<()>;

    /// Cycle the station epoch; its completion is observed later.
    fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle>;

    /// The completion of the epoch cycle of `handle`, once published.
    fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence>;

    /// The completed finite monitor capture of `handle` and its frames.
    fn wait_monitor_capture(
        &self,
        handle: CommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence>;

    /// Call after the access point's stop completed: every airtime
    /// accounting record of the stop must precede it; missing evidence is an
    /// error, not a reason to wait for a guessed delay.
    fn require_access_point_airtime(&self, handle: CommandHandle) -> Result<()>;

    /// The device's stack watermarks, which must keep their headroom.
    fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage>;
}

impl WifiCapture for SerialCapture {
    fn require_no_beacon_loss(&self) -> Result<()> {
        let count = self.messages_since(0, beacon_loss_count_in)?;
        if count == 0 {
            Ok(())
        } else {
            Err(format!("observed {count} typed station beacon-loss event(s)").into())
        }
    }

    fn require_station_unchanged_since(&self, first_event: usize) -> Result<()> {
        self.messages_since(0, |messages| {
            station_unchanged_since_in(messages, first_event)
        })?
    }

    fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
        let first_event = self.event_cursor();
        let (reply, outcome) = self.call_answered(
            0,
            oer_hil_protocol::wifi::CycleStationEpoch,
            oer_hil_link::PROTOCOL_READY_TIMEOUT,
        )?;
        outcome.map_err(|reason| format!("device rejected station epoch cycle: {reason:?}"))?;
        Ok(StationEpochHandle {
            request_id: reply.request_id,
            first_event,
        })
    }

    fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence> {
        self.messages_since(handle.first_event, |messages| {
            messages
                .iter()
                .filter(|message| message.request_id == handle.request_id)
                .find_map(|message| {
                    message
                        .decode()
                        .map(|oer_hil_protocol::wifi::StationEpochCompleted(evidence)| evidence)
                })
        })
        .ok()
        .flatten()
    }

    fn wait_monitor_capture(
        &self,
        handle: CommandHandle,
        timeout: Duration,
    ) -> Result<MonitorCaptureEvidence> {
        let oer_hil_protocol::wifi::MonitorCaptureCompleted(summary) =
            self.wait_command(handle, timeout)?;
        let chunks = self.messages_since(handle.first_event(), |messages| {
            messages
                .iter()
                .filter(|message| message.request_id == handle.request_id())
                .filter_map(|message| {
                    message
                        .decode()
                        .map(|oer_hil_protocol::wifi::MonitorFrame(chunk)| chunk)
                })
                .collect()
        })?;
        Ok(MonitorCaptureEvidence { chunks, summary })
    }

    fn require_access_point_airtime(&self, handle: CommandHandle) -> Result<()> {
        self.messages_since(handle.first_event(), |messages| {
            validate(messages.iter().filter(|message| {
                message.request_id == handle.request_id() && message.session_id == 0
            }))
        })?
    }

    fn query_stack_usage(&self, timeout: Duration) -> Result<StackUsage> {
        let oer_hil_protocol::system::Stacks(usage) =
            self.request(0, oer_hil_protocol::system::GetStacks, timeout)?;
        oer_hil_net_traffic::validate_stack_usage(usage)?;
        Ok(usage)
    }
}

fn beacon_loss_count_in(messages: &[Received]) -> usize {
    let Some(boot_id) = latest_boot_id_in(messages) else {
        return 0;
    };
    messages
        .iter()
        .filter(|message| {
            message.boot_id == boot_id
                && matches!(
                    message.decode(),
                    Some(oer_hil_protocol::wifi::StationLifecycle(
                        StationLifecycleEvent::Disconnected {
                            reason: oer_hil_protocol::wifi::StationDisconnectReason::BeaconLoss,
                            ..
                        }
                    ))
                )
        })
        .count()
}

fn station_unchanged_since_in(messages: &[Received], first_event: usize) -> Result<()> {
    let subsequent = messages
        .get(first_event..)
        .ok_or("station lifecycle cursor exceeds captured events")?;
    let boot_id = latest_boot_id_in(&messages[..first_event])
        .filter(|boot_id| *boot_id != 0)
        .ok_or("station lifecycle cursor has no established boot identity")?;
    for message in subsequent {
        if message.boot_id != boot_id {
            return Err("device rebooted during station pause workload".into());
        }
        // A Hello answering a request is a capability reply; only an
        // unsolicited one restarts the greeting.
        if message.is::<oer_hil_protocol::base::Hello>() && message.request_id == 0 {
            return Err("device restarted its greeting during station pause workload".into());
        }
        if let Some(oer_hil_protocol::wifi::StationLifecycle(event)) = message.decode() {
            return Err(format!("station changed during pause workload: {event:?}").into());
        }
    }
    Ok(())
}

fn validate<'a>(messages: impl Iterator<Item = &'a Received>) -> Result<()> {
    let mut peers: Vec<WifiAirtimePeerEvidence> = Vec::new();
    let mut report = None;
    let mut stopped = false;
    for message in messages {
        if let Some(oer_hil_protocol::wifi::AirtimePeer(peer)) = message.decode() {
            if report.is_some() || stopped || peers.iter().any(|old| old.peer == peer.peer) {
                return Err("AP airtime has duplicate or out-of-order peer evidence".into());
            }
            let count = peer
                .settlements
                .checked_add(peer.cancellations)
                .and_then(|n| n.checked_add(u64::from(peer.outstanding)));
            let budget = peer
                .settled_grants_micros
                .checked_add(peer.cancelled_grants_micros)
                .and_then(|n| n.checked_add(peer.outstanding_micros));
            if count != Some(peer.grants)
                || budget != Some(peer.granted_micros)
                || peer.outstanding != 0
                || peer.outstanding_micros != 0
            {
                return Err(
                    format!("AP airtime reservations did not reconcile at stop: {peer:?}").into(),
                );
            }
            peers.push(peer);
        } else if let Some(oer_hil_protocol::wifi::AirtimeReport(value)) = message.decode() {
            if report.is_some()
                || stopped
                || usize::from(value.peer_records) != peers.len()
                || value.dropped_events != 0
                || value.saturated
            {
                return Err(format!("AP airtime report is incomplete: {value:?}").into());
            }
            report = Some(value);
        } else if let Some(oer_hil_protocol::wifi::AccessPointStopped(_)) = message.decode() {
            stopped = true;
        }
    }
    if report.is_none() || !stopped {
        return Err("AP stop is missing correlated airtime completeness evidence".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "link/airtime_tests.rs"]
mod airtime_tests;

#[cfg(test)]
mod tests;
