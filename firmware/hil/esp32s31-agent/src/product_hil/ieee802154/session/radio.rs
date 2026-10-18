//! The session's radio operations: its commands to the running system and
//! the frames received between collections.

use embassy_time::{Duration, with_timeout};
use heapless::Vec;
use oer_esp32s31_ieee802154_system::Esp32s31Ieee802154;
use oer_espressif_ieee802154_engine::engine::PENDING_TABLE_SIZE;
use oer_espressif_ieee802154_runtime::{
    Ieee802154OwnedFrame, Ieee802154PortEvent, Ieee802154RadioEvent,
};
use oer_hil_protocol::{
    ieee802154::IEEE802154_SESSION_RECORDED_FRAMES, ieee802154::Ieee802154AirCcaOutcome,
    ieee802154::Ieee802154AirEnergyOutcome, ieee802154::Ieee802154AirTxOutcome,
    ieee802154::Ieee802154SessionAck, ieee802154::Ieee802154SessionAssessRequest,
    ieee802154::Ieee802154SessionAssessment, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionFrame, ieee802154::Ieee802154SessionPendingMode,
    ieee802154::Ieee802154SessionPendingRequest, ieee802154::Ieee802154SessionReceiveEvidence,
    ieee802154::Ieee802154SessionReceivedFrame, ieee802154::Ieee802154SessionRecentRssi,
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154SessionTransmitEvidence,
    ieee802154::Ieee802154SessionTransmitRequest, ieee802154::Ieee802154SessionTxMode,
    ieee802154::ieee802154_frame_crc32c,
};
use oer_ieee802154::{
    AutoPendingMode, Channel, EnhancedAckGeneration, EventsLost, FrameView, RequestId, TxMode,
    TxSecurity,
    owner::{
        Has, Ieee802154Client, Operation, RadioSettings, RecentRssi, ShortPending, SupportedCca,
        SupportedEnergyScan, SupportedTransmit, Transmission,
    },
};
use oer_ieee802154_port::RadioPortEvents;
use oer_radio_port::{Seat, Slot};

use super::super::client::tx_outcome;

/// Bound on one transmission's terminal event, over its retries.
pub(super) const TRANSMIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Bound on the terminal event of one energy scan beyond its duration, and
/// of one clear-channel assessment.
pub(super) const ASSESS_TIMEOUT: Duration = Duration::from_millis(100);

/// Frames received since the previous collection.
#[derive(Default)]
pub(super) struct Received {
    total: u16,
    frames: heapless::Vec<Ieee802154SessionReceivedFrame, IEEE802154_SESSION_RECORDED_FRAMES>,
}

impl Received {
    pub(super) fn record(&mut self, frame: &Ieee802154OwnedFrame) {
        self.total = self.total.saturating_add(1);
        let bytes = frame.frame.as_bytes();
        let _ = self.frames.push(Ieee802154SessionReceivedFrame {
            length: bytes.len() as u8,
            crc32c: ieee802154_frame_crc32c(bytes),
            rssi_dbm: frame.metadata.rssi_dbm,
            lqi: frame.metadata.link_quality,
        });
    }

    pub(super) fn take(&mut self) -> Ieee802154SessionReceiveEvidence {
        let Self { total, frames } = core::mem::take(self);
        Ieee802154SessionReceiveEvidence {
            result: Ieee802154SessionResult::Done,
            total,
            frames,
        }
    }
}

/// The session's view of a running port phase: its events consumer, its
/// control, the client of interface 0 with its operation seat, the
/// interface's pending-table slots and the install's RSSI reader.
pub(super) struct Session<'s, 'id, E, S, C, Rs>
where
    C: Ieee802154Client<'id, 0>,
{
    pub(super) events: &'s mut E,
    pub(super) control: &'s mut S,
    pub(super) client: &'s C,
    pub(super) seat: &'s mut Seat<'id, Operation>,
    pub(super) rssi: &'s Rs,
    pub(super) short_free: Vec<Slot<'id, ShortPending<0>>, PENDING_TABLE_SIZE>,
    pub(super) short: Vec<(u16, C::ShortEntry), PENDING_TABLE_SIZE>,
    pub(super) channel: Channel,
    /// Frames received since the previous collection; they survive a
    /// restart of the radio.
    pub(super) received: &'s mut Received,
    pub(super) lost: bool,
}

impl<'id, E, S, C, Rs> Session<'_, 'id, E, S, C, Rs>
where
    E: RadioPortEvents<Event = Ieee802154PortEvent>,
    S: RadioSettings<Caps = Esp32s31Ieee802154>,
    C: Ieee802154Client<'id, 0, Caps = Esp32s31Ieee802154>,
    Rs: RecentRssi,
{
    /// Apply the host's identity and filter to the enabled radio, and
    /// install the enhanced-ACK generator the host asked for.
    pub(super) fn configure(
        &mut self,
        config: Ieee802154SessionConfig,
    ) -> Result<(), Ieee802154SessionResult> {
        // A generator measuring link margins from a zero noise floor, as
        // ESP-IDF's port leaves it.
        let generation = config
            .enhanced_ack
            .then_some(EnhancedAckGeneration { noise_floor_dbm: 0 });
        self.control.set_enhanced_ack(generation);
        self.client.set_pan_id(config.pan_id);
        self.client.set_short_address(config.short_address);
        self.client.set_extended_address(config.extended_address);
        let has = Has::new().ok_or(Ieee802154SessionResult::UnsupportedSetup)?;
        self.control.set_promiscuous(has, config.promiscuous);
        Ok(())
    }

    /// Receive on the session's channel.
    pub(super) async fn receive(&mut self) -> Result<(), Ieee802154SessionResult> {
        self.client
            .receive(self.channel)
            .await
            .map_err(|_| Ieee802154SessionResult::CommandRejected)
    }

    /// The port's next event.
    pub(super) async fn taken(&mut self) -> Result<Ieee802154RadioEvent, EventsLost> {
        match self.events.next_event().await {
            Ok(Ok(event)) => Ok(event.event),
            // A poisoned port reports nothing more; the session sees a gap.
            Ok(Err(EventsLost)) | Err(_) => Err(EventsLost),
        }
    }

    /// Take one runtime event; received frames are recorded.
    pub(super) fn observe(
        &mut self,
        event: Result<Ieee802154RadioEvent, EventsLost>,
    ) -> Option<Ieee802154RadioEvent> {
        match event {
            Ok(Ieee802154RadioEvent::Received(frame)) => {
                self.received.record(&frame);
                None
            }
            Ok(event) => Some(event),
            Err(_) => {
                self.lost = true;
                None
            }
        }
    }

    pub(super) async fn transmit(
        &mut self,
        request: &Ieee802154SessionTransmitRequest,
    ) -> Ieee802154SessionTransmitEvidence {
        let mut evidence = Ieee802154SessionTransmitEvidence::default();
        let Ok(frame) = FrameView::new(&request.frame) else {
            evidence.result = Ieee802154SessionResult::CommandRejected;
            return evidence;
        };
        let mode = match request.mode {
            Ieee802154SessionTxMode::Direct => TxMode::Direct,
            Ieee802154SessionTxMode::ClearChannelAssessment => TxMode::ClearChannelAssessment,
            Ieee802154SessionTxMode::CsmaCa { max_backoffs } => TxMode::CsmaCa { max_backoffs },
        };
        let Ok(transmission) = SupportedTransmit::new(Transmission {
            frame,
            channel: self.channel,
            mode,
            power: None,
            max_frame_retries: request.max_frame_retries,
            security: TxSecurity::Radio,
            time_sync: None,
        }) else {
            evidence.result = Ieee802154SessionResult::CommandRejected;
            return evidence;
        };
        let permit = self.client.operation(self.seat).await;
        let id = match self.client.transmit(permit, transmission) {
            Ok(work) => work.id(),
            Err(_) => {
                evidence.result = Ieee802154SessionResult::CommandRejected;
                return evidence;
            }
        };
        loop {
            let Ok(event) = with_timeout(TRANSMIT_TIMEOUT, self.taken()).await else {
                evidence.result = Ieee802154SessionResult::EventTimeout;
                return evidence;
            };
            match self.observe(event) {
                None => continue,
                Some(Ieee802154RadioEvent::TransmitDone {
                    id: done,
                    status,
                    acknowledgement,
                    ..
                }) if done == id => {
                    evidence.result = Ieee802154SessionResult::Done;
                    evidence.outcome = tx_outcome(status);
                    evidence.acknowledgement = acknowledgement.map(|ack| Ieee802154SessionAck {
                        frame: Ieee802154SessionFrame::from_slice(ack.frame.as_bytes())
                            .unwrap_or_default(),
                        rssi_dbm: ack.metadata.rssi_dbm,
                        lqi: ack.metadata.link_quality,
                    });
                    return evidence;
                }
                Some(_) => {
                    evidence.result = Ieee802154SessionResult::UnexpectedEvent;
                    evidence.outcome = Ieee802154AirTxOutcome::NotRun;
                    return evidence;
                }
            }
        }
    }

    /// The terminal event of `id` within `timeout`; received frames are
    /// recorded on the way.
    async fn terminal_of(
        &mut self,
        id: RequestId,
        timeout: Duration,
    ) -> Result<Ieee802154RadioEvent, Ieee802154SessionResult> {
        loop {
            let Ok(event) = with_timeout(timeout, self.taken()).await else {
                return Err(Ieee802154SessionResult::EventTimeout);
            };
            match self.observe(event) {
                Some(event) if terminal_id(&event) == Some(id) => return Ok(event),
                Some(_) => return Err(Ieee802154SessionResult::UnexpectedEvent),
                None => {}
            }
        }
    }

    /// Scan the energy on the requested channel, then assess it once, as
    /// two radio operations on that channel.
    pub(super) async fn assess(
        &mut self,
        request: Ieee802154SessionAssessRequest,
    ) -> Ieee802154SessionAssessment {
        let mut assessment = Ieee802154SessionAssessment::default();
        let Ok(channel) = Channel::new(request.channel) else {
            assessment.result = Ieee802154SessionResult::CommandRejected;
            return assessment;
        };
        let (Some(scan), Some(cca)) = (
            SupportedEnergyScan::new(channel, request.energy_scan_micros),
            SupportedCca::new(channel),
        ) else {
            assessment.result = Ieee802154SessionResult::UnsupportedSetup;
            return assessment;
        };
        let permit = self.client.operation(self.seat).await;
        let Ok(id) = self.client.energy_scan(permit, scan).map(|work| work.id()) else {
            assessment.result = Ieee802154SessionResult::CommandRejected;
            return assessment;
        };
        let scan_timeout =
            Duration::from_micros(u64::from(request.energy_scan_micros)) + ASSESS_TIMEOUT;
        assessment.energy = match self.terminal_of(id, scan_timeout).await {
            Ok(Ieee802154RadioEvent::EnergyScanDone { energy_dbm, .. }) => {
                Ieee802154AirEnergyOutcome::Energy(energy_dbm)
            }
            Ok(_) => Ieee802154AirEnergyOutcome::Failed,
            Err(result) => {
                assessment.result = result;
                return assessment;
            }
        };
        let permit = self.client.operation(self.seat).await;
        let Ok(id) = self
            .client
            .clear_channel_assessment(permit, cca)
            .map(|work| work.id())
        else {
            assessment.result = Ieee802154SessionResult::CommandRejected;
            return assessment;
        };
        assessment.cca = match self.terminal_of(id, ASSESS_TIMEOUT).await {
            Ok(Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: true, .. }) => {
                Ieee802154AirCcaOutcome::Clear
            }
            Ok(Ieee802154RadioEvent::ClearChannelAssessmentDone { idle: false, .. }) => {
                Ieee802154AirCcaOutcome::Busy
            }
            Ok(_) => Ieee802154AirCcaOutcome::Failed,
            Err(result) => {
                assessment.result = result;
                return assessment;
            }
        };
        assessment.result = Ieee802154SessionResult::Done;
        assessment
    }

    /// The live RSSI of the most recent baseband reception.
    pub(super) fn recent_rssi(&self) -> Ieee802154SessionRecentRssi {
        Ieee802154SessionRecentRssi {
            result: Ieee802154SessionResult::Done,
            rssi_dbm: self.rssi.recent_rssi(),
        }
    }

    /// Set the pending mode and add the host's source in a pending-table
    /// slot; the pending table is software the next operation publishes.
    pub(super) fn pending(&mut self, request: Ieee802154SessionPendingRequest) -> bool {
        let mode = match request.mode {
            Ieee802154SessionPendingMode::Disabled => AutoPendingMode::Disable,
            Ieee802154SessionPendingMode::Enabled => AutoPendingMode::Enable,
            Ieee802154SessionPendingMode::Enhanced => AutoPendingMode::Enhanced,
            Ieee802154SessionPendingMode::Zigbee => AutoPendingMode::Zigbee,
        };
        let Some(has) = Has::new() else {
            return false;
        };
        self.client.set_pending_mode(has, mode);
        match request.short_address {
            None => true,
            Some(short) if self.short.iter().any(|(held, _)| *held == short) => true,
            Some(short) => {
                // A full table refuses the source.
                let Some(slot) = self.short_free.pop() else {
                    return false;
                };
                let entry = self.client.add_pending_short(slot, short.to_le_bytes());
                self.short.push((short, entry)).is_ok()
            }
        }
    }

    /// Clear every pending entry this session added, giving the slots back.
    pub(super) fn clear_pending(&mut self) {
        while let Some((_, entry)) = self.short.pop() {
            let slot = self.client.remove_pending_short(entry);
            let _ = self.short_free.push(slot);
        }
    }
}

/// The identity of the operation `event` ends.
fn terminal_id(event: &Ieee802154RadioEvent) -> Option<RequestId> {
    match *event {
        Ieee802154RadioEvent::TransmitDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanFailed { id }
        | Ieee802154RadioEvent::ClearChannelAssessmentDone { id, .. }
        | Ieee802154RadioEvent::ClearChannelAssessmentFailed { id } => Some(id),
        _ => None,
    }
}
