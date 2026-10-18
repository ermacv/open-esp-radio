//! `openthread_radio::Radio` over an IEEE 802.15.4 port on the owner shape.

use heapless::{Deque, Vec};
use oer_ieee802154::{
    AppliedSecurity, Channel, EventsLost, FrameCounterUpdate, FrameView, Ieee802154Instant,
    LinkMetrics, ProbingInitiator, ReceivedFrame, RequestId, TimeSync, TxMode, TxSecurity,
    owner::{
        ExtendedPending, Has, Ieee802154Caps, Ieee802154Client, Ieee802154Events, Operation,
        PhaseEvent, Promiscuous, RadioSettings, RecentRssi, ShortPending, SourceMatch,
        SupportedCcaThreshold, SupportedEnergyScan, SupportedPower, SupportedScheduledReceive,
        SupportedTransmit, Transmission,
    },
};
use oer_radio_port::{Seat, Slot};
use oer_time::Clock;
use openthread_radio::{
    AckSecurity, Capabilities, Config, CslConfig, ENH_ACK_PROBING_CAPACITY, EnhAckProbingConfig,
    MacCapabilities, MacKeys, PsduMeta, Radio, RadioCaps, RadioClock, RadioErrorKind, RadioRssi,
    SentAck, SrcMatchConfig, TxFrame,
};

use crate::frames::{
    TransmitFailure, csl_period, extended_address, pending_mode, probing_initiator, psdu_mac,
    radio_time, scan_micros, sent_ack_security, transmit_failure, tx_security,
    write_applied_security, write_psdu,
};

/// The PHY capabilities the radio reports, as ESP-IDF's OpenThread port
/// reports them (`otPlatRadioGetCaps`). Declare them to OpenThread before
/// its instance is built (`OtResources::set_radio_caps`).
pub const OPEN_THREAD_RADIO_CAPABILITIES: Capabilities = Capabilities::ACK_TIMEOUT
    .union(Capabilities::ENERGY_SCAN)
    .union(Capabilities::SLEEP_TO_TX)
    .union(Capabilities::TRANSMIT_SEC)
    .union(Capabilities::RECEIVE_TIMING)
    .union(Capabilities::TRANSMIT_TIMING);

/// The pending-table slots of the interface OpenThread drives.
pub struct PendingSlots<'id, const P: usize> {
    /// The short-address slots.
    pub short: [Slot<'id, ShortPending<0>>; P],
    /// The extended-address slots.
    pub extended: [Slot<'id, ExtendedPending<0>>; P],
}

/// An IEEE 802.15.4 port's running phase as an OpenThread radio: its one
/// events consumer, its control and the client of interface 0, which the
/// port's body lends for as long as OpenThread runs.
///
/// Frames that arrive while a transmission or energy scan runs wait in a
/// queue of `QUEUE` frames for [`Radio::receive`]; a full queue drops the
/// newest, as the trait allows. A transmission whose future OpenThread
/// drops finishes in the backend; the next operation first waits for its
/// end. The port loses only received frames, so [`EventsLost`] makes a
/// receive report `RxFailed`. A poisoned port fails every later operation.
///
/// The source-match table holds `P` addresses of each size, the
/// interface's pending-table slots; as ESP-IDF's port, an address past
/// them is no error.
// CAPABILITY: i-product-stacks-thread
pub struct OpenThreadRadio<'a, 'id, E, S, C, const QUEUE: usize, const P: usize>
where
    E: Ieee802154Events,
    S: RadioSettings,
    C: Ieee802154Client<'id, 0>,
{
    events: &'a mut E,
    control: &'a mut S,
    client: C,
    seat: Seat<'id, Operation>,
    short_free: Vec<Slot<'id, ShortPending<0>>, P>,
    short: Vec<(u16, C::ShortEntry), P>,
    extended_free: Vec<Slot<'id, ExtendedPending<0>>, P>,
    extended: Vec<(u64, C::ExtendedEntry), P>,
    /// Received-frame events.
    received: Deque<E::Event, QUEUE>,
    /// Received frames were lost since OpenThread last received.
    receive_lost: bool,
    /// An operation whose terminal event is still owed.
    pending: Option<RequestId>,
    /// The CCA threshold the radio holds, in dBm.
    cca_threshold: Option<i8>,
    /// Whether source matching is enabled.
    src_match_enabled: bool,
}

/// One frame to transmit and how.
struct Outgoing<'f> {
    psdu: &'f [u8],
    channel: u8,
    power: i8,
    /// Its presence asks for a CCA at the radio's own threshold.
    cca_threshold: Option<i8>,
    /// The start time in the low 32 bits of the radio clock.
    tx_at: Option<u32>,
    security: TxSecurity,
    /// The Time IE the radio fills at the SFD.
    time_sync: Option<TimeSync>,
}

/// The terminal event of one operation.
enum Terminal<Ev> {
    Transmitted(Ev),
    Scanned(Option<i8>),
    /// The port is poisoned.
    Lost,
}

impl<'a, 'id, E, S, C, const QUEUE: usize, const P: usize>
    OpenThreadRadio<'a, 'id, E, S, C, QUEUE, P>
where
    E: Ieee802154Events,
    S: RadioSettings<Caps = C::Caps>,
    C: Ieee802154Client<'id, 0>,
{
    /// Drive the running phase of a port: its events consumer, its control,
    /// the client of interface 0 with its operation seat, and the
    /// interface's pending-table slots. OpenThread reads the radio clock
    /// through [`MonotonicRadioClock`] over the monotonic clock the port
    /// counts in, and the live RSSI through [`PortRssi`].
    pub fn new(
        events: &'a mut E,
        control: &'a mut S,
        client: C,
        seat: Seat<'id, Operation>,
        slots: PendingSlots<'id, P>,
    ) -> Self {
        Self {
            events,
            control,
            client,
            seat,
            short_free: Vec::from_array(slots.short),
            short: Vec::new(),
            extended_free: Vec::from_array(slots.extended),
            extended: Vec::new(),
            received: Deque::new(),
            receive_lost: false,
            pending: None,
            cca_threshold: None,
            src_match_enabled: false,
        }
    }

    /// Give back the seat and the pending-table slots, the entries
    /// removed, when OpenThread stops.
    pub fn into_parts(mut self) -> (Seat<'id, Operation>, PendingSlots<'id, P>) {
        while let Some((_, entry)) = self.short.pop() {
            let slot = self.client.remove_pending_short(entry);
            let _ = self.short_free.push(slot);
        }
        while let Some((_, entry)) = self.extended.pop() {
            let slot = self.client.remove_pending_extended(entry);
            let _ = self.extended_free.push(slot);
        }
        let short = self
            .short_free
            .into_array()
            .unwrap_or_else(|_| unreachable!("every slot is back"));
        let extended = self
            .extended_free
            .into_array()
            .unwrap_or_else(|_| unreachable!("every slot is back"));
        (self.seat, PendingSlots { short, extended })
    }

    /// Give the radio OpenThread's CCA threshold, as the port's
    /// `otPlatRadioSetCcaEnergyDetectThreshold` does, when it changed.
    fn apply_cca_threshold(&mut self, threshold: i8) -> Result<(), RadioErrorKind> {
        if self.cca_threshold != Some(threshold) {
            let supported =
                SupportedCcaThreshold::new(threshold).map_err(|_| RadioErrorKind::Other)?;
            self.control.set_cca_threshold(supported);
            self.cca_threshold = Some(threshold);
        }
        Ok(())
    }

    /// The full radio-clock instant of a 32-bit OpenThread radio time.
    async fn radio_timestamp(&self, low: u32) -> Result<Ieee802154Instant, RadioErrorKind> {
        match self.client.now().await {
            Ok(Ok(now)) => Ok(Ieee802154Instant::from_micros(radio_time(
                now.as_micros(),
                low,
            ))),
            _ => Err(RadioErrorKind::Other),
        }
    }

    /// Wait for the terminal event of `id`, queueing frames meanwhile.
    async fn terminal(&mut self, id: RequestId) -> Terminal<E::Event> {
        loop {
            let event = match self.events.next_event().await {
                Ok(Ok(event)) => event,
                // Only frames are lost; the terminal follows.
                Ok(Err(EventsLost)) => {
                    self.receive_lost = true;
                    continue;
                }
                Err(_) => return Terminal::Lost,
            };
            match E::view(&event) {
                PhaseEvent::Received(_) => {
                    if self.received.push_back(event).is_err() {
                        self.receive_lost = true;
                    }
                }
                PhaseEvent::TransmitDone { id: done, .. } if done == id => {
                    return Terminal::Transmitted(event);
                }
                PhaseEvent::EnergyScanDone {
                    id: done,
                    energy_dbm,
                    ..
                } if done == id => return Terminal::Scanned(Some(energy_dbm)),
                PhaseEvent::EnergyScanFailed { id: done, .. } if done == id => {
                    return Terminal::Scanned(None);
                }
                // The end of an operation OpenThread abandoned earlier.
                _ => {}
            }
        }
    }

    /// Wait for the end of an operation whose future was dropped.
    async fn settle(&mut self) {
        if let Some(id) = self.pending {
            let _ = self.terminal(id).await;
            self.pending = None;
        }
    }

    /// Transmit `psdu` under `security`; also report the security header
    /// fields the radio wrote, whatever the outcome.
    async fn send(
        &mut self,
        outgoing: Outgoing<'_>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> (
        Result<Option<PsduMeta>, RadioErrorKind>,
        Option<AppliedSecurity>,
    ) {
        let Outgoing {
            psdu,
            channel: number,
            power,
            cca_threshold,
            tx_at,
            security,
            time_sync,
        } = outgoing;
        self.settle().await;
        let channel = match channel(number) {
            Ok(channel) => channel,
            Err(error) => return (Err(error), None),
        };
        let Some(frame) = psdu_mac(psdu).and_then(|mac| FrameView::new(mac).ok()) else {
            return (Err(RadioErrorKind::TxInvalid), None);
        };
        if let Some(threshold) = cca_threshold
            && let Err(error) = self.apply_cca_threshold(threshold)
        {
            return (Err(error), None);
        }
        // The threshold's presence asks for a CCA. A delayed frame starts at
        // its time (`esp_i_transmit_at`).
        let mode = match (tx_at, cca_threshold.is_some()) {
            (Some(at), cca) => match self.radio_timestamp(at).await {
                Ok(at) => TxMode::Scheduled { at, cca },
                Err(error) => return (Err(error), None),
            },
            (None, true) => TxMode::ClearChannelAssessment,
            (None, false) => TxMode::Direct,
        };
        // OpenThread's power is an upper bound: above the port's levels the
        // radio sends at its highest, below them the transmission fails.
        let Ok(power) = SupportedPower::new(power) else {
            return (Err(RadioErrorKind::Other), None);
        };
        let Ok(transmission) = SupportedTransmit::new(Transmission {
            frame,
            channel,
            mode,
            power: Some(power),
            max_frame_retries: 0,
            security,
            time_sync,
        }) else {
            return (Err(RadioErrorKind::Other), None);
        };
        let permit = self.client.operation(&mut self.seat).await;
        let id = match self.client.transmit(permit, transmission) {
            Ok(work) => work.id(),
            Err(_) => return (Err(RadioErrorKind::Other), None),
        };
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        let Terminal::Transmitted(event) = terminal else {
            return (Err(RadioErrorKind::Other), None);
        };
        let PhaseEvent::TransmitDone {
            status,
            acknowledgement,
            security,
            ..
        } = E::view(&event)
        else {
            return (Err(RadioErrorKind::Other), None);
        };
        if let Some(failure) = transmit_failure(status) {
            let error = match failure {
                TransmitFailure::ChannelAccess => RadioErrorKind::TxFailed,
                TransmitFailure::NoAcknowledgement => RadioErrorKind::RxAckTimeout,
                TransmitFailure::InvalidAcknowledgement => RadioErrorKind::RxAckInvalid,
                TransmitFailure::InvalidFrame => RadioErrorKind::TxInvalid,
                TransmitFailure::Other => RadioErrorKind::Other,
            };
            return (Err(error), security);
        }
        let ack = match (acknowledgement, ack_psdu_buf) {
            (Some(ack), Some(buffer)) => {
                write_psdu(ack.frame.bytes(), buffer).map(|len| PsduMeta {
                    len,
                    channel: ack.metadata.channel.get(),
                    rssi: Some(ack.metadata.rssi_dbm),
                    lqi: Some(ack.metadata.link_quality),
                    ack: None,
                    timestamp: ack.metadata.timestamp.map(Ieee802154Instant::as_micros),
                })
            }
            _ => None,
        };
        (Ok(ack), security)
    }

    /// Bring the radio's source-match table to `config`, one entry at a
    /// time, removals first.
    fn match_sources(&mut self, config: &SrcMatchConfig) {
        let short_kept = |address: &u16| config.short_addrs.contains(address);
        let extended_kept = |address: &u64| config.ext_addrs.contains(address);
        let mut index = 0;
        while index < self.short.len() {
            if short_kept(&self.short[index].0) {
                index += 1;
            } else {
                let (_, entry) = self.short.swap_remove(index);
                let slot = self.client.remove_pending_short(entry);
                let _ = self.short_free.push(slot);
            }
        }
        let mut index = 0;
        while index < self.extended.len() {
            if extended_kept(&self.extended[index].0) {
                index += 1;
            } else {
                let (_, entry) = self.extended.swap_remove(index);
                let slot = self.client.remove_pending_extended(entry);
                let _ = self.extended_free.push(slot);
            }
        }
        for &address in config.short_addrs.iter() {
            if self.short.iter().any(|(held, _)| *held == address) {
                continue;
            }
            // As ESP-IDF's port, a full table is no error.
            let Some(slot) = self.short_free.pop() else {
                break;
            };
            let entry = self.client.add_pending_short(slot, address.to_le_bytes());
            let _ = self.short.push((address, entry));
        }
        for &address in config.ext_addrs.iter() {
            if self.extended.iter().any(|(held, _)| *held == address) {
                continue;
            }
            let Some(slot) = self.extended_free.pop() else {
                break;
            };
            let entry = self
                .client
                .add_pending_extended(slot, extended_address(address));
            let _ = self.extended.push((address, entry));
        }
    }
}

fn channel(number: u8) -> Result<Channel, RadioErrorKind> {
    Channel::new(number).map_err(|_| RadioErrorKind::Other)
}

impl<'id, E, S, C, const QUEUE: usize, const P: usize> Radio
    for OpenThreadRadio<'_, 'id, E, S, C, QUEUE, P>
where
    E: Ieee802154Events,
    S: RadioSettings<Caps = C::Caps>,
    C: Ieee802154Client<'id, 0>,
{
    type Error = RadioErrorKind;

    /// The port's body enabled the radio before OpenThread runs; `init`
    /// installs OpenThread's starting state.
    async fn init(&mut self) -> Result<RadioCaps, Self::Error> {
        let caps = <C::Caps as Ieee802154Caps>::CAPS;
        let (Some(transmit_power), Some(cca_threshold), Some(csl)) =
            (caps.transmit_power, caps.cca_threshold, caps.csl)
        else {
            return Err(RadioErrorKind::Other);
        };
        self.settle().await;
        // The radio secures frames and enhanced ACKs with OpenThread's keys,
        // starting from zeroed ones: raising the frame counter to zero
        // installs them and keeps keys already set.
        self.client
            .update_frame_counter(FrameCounterUpdate::SetIfLarger(0));
        // Link margins are measured from the receive sensitivity the radio
        // reports; ESP-IDF's port leaves the noise floor at zero.
        self.control
            .set_enhanced_ack(Some(oer_ieee802154::EnhancedAckGeneration {
                noise_floor_dbm: caps.receive_sensitivity_dbm,
            }));
        // The radio holds the threshold OpenThread starts from.
        self.cca_threshold = None;
        self.apply_cca_threshold(cca_threshold.reset_dbm)?;
        // And the empty source-match table it starts from: every poll is
        // answered with frame pending.
        self.match_sources(&SrcMatchConfig::new());
        self.set_pending(false)?;
        Ok(RadioCaps {
            phy: OPEN_THREAD_RADIO_CAPABILITIES,
            mac: MacCapabilities::all(),
            receive_sensitivity: caps.receive_sensitivity_dbm,
            default_tx_power: transmit_power.reset_dbm,
            default_cca_threshold: cca_threshold.reset_dbm,
            csl_accuracy: csl.accuracy_ppm,
            csl_uncertainty: csl.uncertainty_10us,
        })
    }

    async fn set_config(&mut self, config: &Config) -> Result<(), Self::Error> {
        self.settle().await;
        if let Some(pan_id) = config.pan_id {
            self.client.set_pan_id(pan_id);
        }
        if let Some(short) = config.short_addr {
            self.client.set_short_address(short);
        }
        if let Some(extended) = config.ext_addr {
            self.client.set_extended_address(extended_address(extended));
        }
        let has = Has::<C::Caps, Promiscuous>::new().ok_or(RadioErrorKind::Other)?;
        self.control.set_promiscuous(has, config.promiscuous);
        Ok(())
    }

    async fn set_src_match_config(&mut self, config: &SrcMatchConfig) -> Result<(), Self::Error> {
        self.settle().await;
        if config.enabled != self.src_match_enabled {
            self.set_pending(config.enabled)?;
        }
        self.match_sources(config);
        Ok(())
    }

    async fn set_receive(&mut self, number: u8) -> Result<(), Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        self.client
            .receive(channel)
            .await
            .map_err(|_| RadioErrorKind::Other)
    }

    async fn set_sleep(&mut self) -> Result<(), Self::Error> {
        self.settle().await;
        self.client.sleep();
        Ok(())
    }

    async fn energy_scan(&mut self, number: u8, duration_millis: u16) -> Result<i8, Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let scan = SupportedEnergyScan::new(channel, scan_micros(duration_millis))
            .ok_or(RadioErrorKind::Other)?;
        let permit = self.client.operation(&mut self.seat).await;
        let id = self
            .client
            .energy_scan(permit, scan)
            .map_err(|_| RadioErrorKind::Other)?
            .id();
        self.pending = Some(id);
        let terminal = self.terminal(id).await;
        self.pending = None;
        match terminal {
            Terminal::Scanned(Some(energy)) => Ok(energy),
            _ => Err(RadioErrorKind::Other),
        }
    }

    /// Transmit `psdu` as given: the radio does not secure it.
    async fn transmit(
        &mut self,
        psdu: &[u8],
        number: u8,
        power: i8,
        cca_threshold: Option<i8>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        let outgoing = Outgoing {
            psdu,
            channel: number,
            power,
            cca_threshold,
            tx_at: None,
            security: TxSecurity::Processed,
            time_sync: None,
        };
        self.send(outgoing, ack_psdu_buf).await.0
    }

    async fn transmit_frame(
        &mut self,
        frame: &mut TxFrame<'_>,
        ack_psdu_buf: Option<&mut [u8]>,
    ) -> Result<Option<PsduMeta>, Self::Error> {
        let security = tx_security(frame.retransmission, frame.security_processed);
        let (result, applied) = self
            .send(
                Outgoing {
                    psdu: frame.psdu,
                    channel: frame.channel,
                    power: frame.power,
                    cca_threshold: frame.cca_threshold,
                    tx_at: frame.tx_at,
                    time_sync: frame.time_sync.map(|ie| TimeSync {
                        ie_offset: ie.ie_offset,
                        sequence: ie.sequence,
                        network_time_offset: ie.network_time_offset,
                    }),
                    security,
                },
                ack_psdu_buf,
            )
            .await;
        if let Some(applied) = applied {
            frame.header_updated = write_applied_security(applied, frame.psdu);
        }
        result
    }

    /// A scheduled receive window is a wish of the interface, admitted at
    /// once; it owes no event.
    async fn receive_at(
        &mut self,
        number: u8,
        start: u32,
        duration: u32,
    ) -> Result<(), Self::Error> {
        self.settle().await;
        let channel = channel(number)?;
        let start = self.radio_timestamp(start).await?;
        let window = SupportedScheduledReceive::new(channel, start, duration)
            .ok_or(RadioErrorKind::Other)?;
        self.client.scheduled_receive(window);
        Ok(())
    }

    async fn set_csl(&mut self, csl: CslConfig) -> Result<(), Self::Error> {
        self.control.set_csl(oer_ieee802154::CslReceiver {
            period: csl_period(csl.period),
            sample_time: csl.sample_time,
        });
        Ok(())
    }

    async fn set_enh_ack_probing(
        &mut self,
        config: &EnhAckProbingConfig,
    ) -> Result<(), Self::Error> {
        let initiators: Vec<ProbingInitiator, ENH_ACK_PROBING_CAPACITY> = config
            .initiators
            .iter()
            .map(|initiator| {
                probing_initiator(
                    initiator.short_address,
                    initiator.ext_address,
                    LinkMetrics {
                        pdu_count: initiator.metrics.pdu_count,
                        lqi: initiator.metrics.lqi,
                        link_margin: initiator.metrics.link_margin,
                        rssi: initiator.metrics.rssi,
                    },
                )
            })
            .collect();
        self.control.set_enhanced_ack_probing(&initiators);
        Ok(())
    }

    async fn set_mac_keys(&mut self, keys: &MacKeys) -> Result<(), Self::Error> {
        self.client
            .set_mac_keys(keys.key_id, keys.previous, keys.current, keys.next);
        Ok(())
    }

    async fn set_mac_frame_counter(
        &mut self,
        update: openthread_radio::FrameCounterUpdate,
    ) -> Result<(), Self::Error> {
        let update = match update {
            openthread_radio::FrameCounterUpdate::Set(counter) => FrameCounterUpdate::Set(counter),
            openthread_radio::FrameCounterUpdate::SetIfLarger(counter) => {
                FrameCounterUpdate::SetIfLarger(counter)
            }
        };
        self.client.update_frame_counter(update);
        Ok(())
    }

    /// A loss of received frames is reported once as `RxFailed`, after the
    /// frames queued before it.
    async fn receive(&mut self, psdu_buf: &mut [u8]) -> Result<PsduMeta, Self::Error> {
        let event = loop {
            if let Some(event) = self.received.pop_front() {
                break event;
            }
            if core::mem::take(&mut self.receive_lost) {
                return Err(RadioErrorKind::RxFailed);
            }
            match self.events.next_event().await {
                Ok(Ok(event)) => match E::view(&event) {
                    PhaseEvent::Received(_) => break event,
                    // The end of an abandoned operation.
                    view => {
                        if let Some((_, id)) = view.ends()
                            && self.pending == Some(id)
                        {
                            self.pending = None;
                        }
                    }
                },
                // Only frames are in the gap.
                Ok(Err(EventsLost)) => return Err(RadioErrorKind::RxFailed),
                Err(_) => return Err(RadioErrorKind::Other),
            }
        };
        match E::view(&event) {
            PhaseEvent::Received(frame) => received_meta(frame, psdu_buf),
            // Only received frames are kept.
            _ => Err(RadioErrorKind::RxInvalid),
        }
    }
}

impl<'id, E, S, C, const QUEUE: usize, const P: usize> OpenThreadRadio<'_, 'id, E, S, C, QUEUE, P>
where
    E: Ieee802154Events,
    S: RadioSettings<Caps = C::Caps>,
    C: Ieee802154Client<'id, 0>,
{
    /// Enable or disable source matching (`otPlatRadioEnableSrcMatch`).
    fn set_pending(&mut self, enabled: bool) -> Result<(), RadioErrorKind> {
        let has = Has::<C::Caps, SourceMatch>::new().ok_or(RadioErrorKind::Other)?;
        self.client.set_pending_mode(has, pending_mode(enabled));
        self.src_match_enabled = enabled;
        Ok(())
    }
}

/// Copy a received frame into OpenThread's PSDU buffer with its metadata.
fn received_meta(
    frame: ReceivedFrame<'_>,
    psdu_buf: &mut [u8],
) -> Result<PsduMeta, RadioErrorKind> {
    let len = write_psdu(frame.frame.bytes(), psdu_buf).ok_or(RadioErrorKind::RxInvalid)?;
    let sent = frame.metadata.sent_acknowledgement;
    Ok(PsduMeta {
        len,
        channel: frame.metadata.channel.get(),
        rssi: Some(frame.metadata.rssi_dbm),
        lqi: Some(frame.metadata.link_quality),
        timestamp: frame.metadata.timestamp.map(Ieee802154Instant::as_micros),
        ack: Some(SentAck {
            frame_pending: sent.frame_pending,
            security: sent_ack_security(sent).map(|(frame_counter, key_id)| AckSecurity {
                frame_counter,
                key_id,
            }),
        }),
    })
}

/// The radio clock for OpenThread's synchronous `otPlatRadioGetNow` and
/// `otPlatTimeGet`: the monotonic clock the composition binds the port to,
/// whose epoch is the port's radio time (its scheduled operations and
/// receive timestamps).
pub struct MonotonicRadioClock<C>(C);

impl<C: Clock> MonotonicRadioClock<C> {
    /// The radio time of a port whose epoch is `clock`'s.
    pub const fn new(clock: C) -> Self {
        Self(clock)
    }
}

impl<C: Clock> RadioClock for MonotonicRadioClock<C> {
    fn now_micros(&self) -> u64 {
        self.0.now().as_micros()
    }
}

/// The live RSSI of an IEEE 802.15.4 port for OpenThread's synchronous
/// `otPlatRadioGetRssi`: the install's recent-RSSI reader.
pub struct PortRssi<'r, R>(&'r R);

impl<'r, R: RecentRssi> PortRssi<'r, R> {
    /// The live RSSI `reader` reads.
    pub const fn new(reader: &'r R) -> Self {
        Self(reader)
    }
}

impl<R: RecentRssi> RadioRssi for PortRssi<'_, R> {
    fn rssi(&self) -> Option<i8> {
        Some(self.0.recent_rssi())
    }
}
