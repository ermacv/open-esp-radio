//! Legacy undirected advertising: non-connectable, scannable or connectable.
//!
//! Enable configures the backend's advertising set with the `ADV_NONCONN_IND`
//! PDU, or with `ADV_SCAN_IND` or `ADV_IND` and its `SCAN_RSP`, and then
//! schedules one event per
//! advertising interval plus a pseudo-random advertising delay of 0 to 10 ms
//! (Core Vol 6 Part B 4.4.2.2.1). Each event transmits on the selected
//! primary channels in order. A non-scannable channel reserves the radio's
//! preparation lead plus the PDU's air time, following the vendor item
//! geometry; a response-capable channel also covers the vendor's
//! four-microsecond response-capable tail and the longest exchange that
//! follows: a scan request answered by the scan response, or, for a
//! connectable set, a connection indication. An
//! event the arbiter cannot place within the delay range is skipped. Disable
//! cancels the event in progress, waits for it to end and removes the set
//! before it completes.
//!
//! A valid connection indication addressed to a connectable set ends
//! advertising: the remaining channels are cancelled, the set is removed and
//! the indication passes to the peripheral connection. A scannable set
//! ignores connection indications.

use bt_hci::param::{Error as HciError, Status};
use core::num::{NonZeroU32, NonZeroU64};
use oer_bluetooth_hci::{LeLegacyAdvertisingAddress, LeLegacyAdvertisingEnableRequest};
use oer_bluetooth_ll::{
    LeDeviceAddress, LeDeviceAddressKind,
    advertising::{
        LegacyAdvertisingData, LegacyNonconnectableAdvertisement, LegacyNonconnectableKind,
    },
    connectable_advertising::{
        LeChannelSelectionAlgorithmTwoSupport, LegacyConnectableAdvertisement,
        LegacyDirectedAdvertisement, LegacyScanResponseData,
    },
    connection::{
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeLegacyConnectionRequest,
        LeLegacyConnectionRequestError,
    },
};
use oer_bluetooth_radio::LeRadio;

use oer_bluetooth_radio::{
    AdvertisingChannels, AdvertisingConfiguration, AdvertisingEvent, AdvertisingPdu,
    AdvertisingReception, AdvertisingSetId, Capability, EventId, LeCapabilities, LeInstant, LePhy,
    LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioDuration, RadioOutcome, RadioRequest,
    RadioTiming, Supported, TxPower,
};

use oer_bluetooth_trace::{AdvertiserStep, AdvertiserStepKind, IndicationVerdict};

use crate::{
    RadioWork,
    arbiter::{Proposal, reservation},
    coexistence,
    planning::{
        EpochExhausted, PlanningCalculation as C, PlanningOperation as O, PlanningRole as R,
    },
};

/// Longest legacy advertising PDU: header, AdvA and 31 data octets.
pub(crate) const ADVERTISING_PDU_CAPACITY: usize = 2 + 6 + 31;
/// Upper bound of the advertising delay.
pub(crate) const ADVERTISING_DELAY_MAX: RadioDuration<LeRadio> = RadioDuration::from_micros(10_000);
const SET: AdvertisingSetId = AdvertisingSetId::new(0);
/// Inter-frame space.
const T_IFS_MICROS: u32 = 150;
/// LE 1M air time of `SCAN_REQ`.
const SCAN_REQ_AIR_MICROS: u32 = (1 + 4 + 2 + 12 + 3) * 8;
/// The vendor's response-capable scheduler tail after `ADV_IND`.
const RESPONSE_CAPABLE_TAIL_MICROS: u32 = 4;
/// Longest spacing of high duty cycle directed advertising events (Core
/// Vol 6 Part B 4.4.2.4.3).
const HIGH_DUTY_INTERVAL: RadioDuration<LeRadio> = RadioDuration::from_micros(3_750);
/// How long high duty cycle directed advertising lasts.
const HIGH_DUTY_DURATION: RadioDuration<LeRadio> = RadioDuration::from_micros(1_280_000);

/// A connection indication accepted by a connectable set.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConnectionIndication {
    pub(crate) request: LeLegacyConnectionRequest,
    /// On-air start of the indication.
    pub(crate) at: LeInstant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    /// Enable waits for the backend to take the set.
    Configuring {
        sent: bool,
    },
    Running,
    /// Disable cancels the event in progress.
    Cancelling {
        sent: bool,
    },
    /// Disable removes the set.
    Removing {
        sent: bool,
    },
}

/// What the set answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetKind {
    Nonconnectable,
    Scannable,
    Connectable,
    /// Connectable by `target` only.
    Directed {
        target: LeDeviceAddress,
        high_duty: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct Outstanding {
    id: EventId,
    reservation: LeWindow,
    /// A connection event took its place; its cancel is sent.
    preempted: bool,
}

#[derive(Debug)]
// CAPABILITY: bluetooth-legacy-non-connectable-advertising-adv-nonconn-ind, bluetooth-legacy-connectable-advertising-adv-ind, bluetooth-advertiser-scan-response-scan-rsp, bluetooth-scannable-non-connectable-advertising-adv-scan-ind, bluetooth-low-duty-high-duty-directed-advertising-adv-direct-ind, bluetooth-static-random-advertiser-address, bluetooth-broadcaster-observer
pub(crate) struct Advertiser {
    phase: Phase,
    kind: SetKind,
    pdu: [u8; ADVERTISING_PDU_CAPACITY],
    pdu_len: usize,
    /// The scan response of a response-capable set.
    scan_response: [u8; ADVERTISING_PDU_CAPACITY],
    scan_response_len: Option<usize>,
    advertiser: LeDeviceAddress,
    channels: AdvertisingChannels,
    interval: RadioDuration<LeRadio>,
    next_anchor: Option<LeInstant>,
    outstanding: Option<Outstanding>,
    /// Events between raised coexistence levels, from the interval.
    coexistence_period: u16,
    /// Events of the running set that ended.
    ended: u16,
    /// The removal in progress is followed by a new configuration.
    restart: bool,
    completion: Option<Status>,
    connection: Option<ConnectionIndication>,
    /// Advertising stops because a connection was created.
    connected: bool,
    /// When high duty cycle directed advertising ends.
    expires: Option<LeInstant>,
    /// High duty cycle directed advertising ended without a connection.
    expired: bool,
    /// The expired set was removed; the Host learns it once.
    timed_out: bool,
}

impl Advertiser {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            kind: SetKind::Nonconnectable,
            pdu: [0; ADVERTISING_PDU_CAPACITY],
            pdu_len: 0,
            scan_response: [0; ADVERTISING_PDU_CAPACITY],
            scan_response_len: None,
            advertiser: LeDeviceAddress::from_wire_bytes([0; 6], LeDeviceAddressKind::Public),
            channels: AdvertisingChannels::ALL,
            interval: RadioDuration::from_micros(0),
            next_anchor: None,
            outstanding: None,
            coexistence_period: 1,
            ended: 0,
            restart: false,
            completion: None,
            connection: None,
            connected: false,
            expires: None,
            expired: false,
            timed_out: false,
        }
    }

    /// Whether advertising is enabled or still stopping.
    pub(crate) const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Start a set; the completion follows once the backend took it.
    pub(crate) fn enable<K: LeCapabilities>(
        &mut self,
        request: LeLegacyAdvertisingEnableRequest,
    ) -> Result<(), HciError> {
        self.load(request)?;
        // The backend sends only the types `K` serves; the Host asked for
        // another.
        if !self.configuration().supported(&K::CAPS) {
            return Err(HciError::UNSUPPORTED);
        }
        self.next_anchor = None;
        self.outstanding = None;
        self.expires = None;
        self.expired = false;
        self.phase = Phase::Configuring { sent: false };
        Ok(())
    }

    /// Replace the running set's PDU. The set is removed and configured
    /// again; the completion follows once the backend took the new one.
    pub(crate) fn update<K: LeCapabilities>(
        &mut self,
        request: LeLegacyAdvertisingEnableRequest,
    ) -> Result<(), HciError> {
        self.load(request)?;
        // New data keeps the running set's type, which `K` served when the
        // set was enabled.
        debug_assert!(self.configuration().supported(&K::CAPS));
        self.restart = true;
        self.disable();
        Ok(())
    }

    fn load(&mut self, request: LeLegacyAdvertisingEnableRequest) -> Result<(), HciError> {
        let (set_kind, parameters, advertiser, data, scan_response) = match request {
            LeLegacyAdvertisingEnableRequest::Nonconnectable(request) => (
                SetKind::Nonconnectable,
                request.parameters(),
                request.advertiser(),
                request.data(),
                None,
            ),
            LeLegacyAdvertisingEnableRequest::Scannable(request) => (
                SetKind::Scannable,
                request.parameters(),
                request.advertiser(),
                request.data(),
                Some(request.scan_response_data()),
            ),
            LeLegacyAdvertisingEnableRequest::Connectable(request) => (
                SetKind::Connectable,
                request.parameters(),
                request.advertiser(),
                request.data(),
                Some(request.scan_response_data()),
            ),
            LeLegacyAdvertisingEnableRequest::Directed(request) => {
                let peer = request.parameters().peer();
                let bytes: [u8; 6] = peer
                    .address()
                    .raw()
                    .try_into()
                    .expect("a device address has six octets");
                let target = LeDeviceAddress::from_wire_bytes(
                    bytes,
                    if peer.is_random() {
                        LeDeviceAddressKind::Random
                    } else {
                        LeDeviceAddressKind::Public
                    },
                );
                (
                    SetKind::Directed {
                        target,
                        high_duty: request.is_high_duty(),
                    },
                    request.parameters(),
                    request.advertiser(),
                    // Directed PDUs carry no data.
                    oer_bluetooth_hci::LeLegacyAdvertisingData::EMPTY,
                    None,
                )
            }
        };
        let kind = match advertiser {
            LeLegacyAdvertisingAddress::Public(_) => LeDeviceAddressKind::Public,
            LeLegacyAdvertisingAddress::Random(_) => LeDeviceAddressKind::Random,
        };
        let bytes: [u8; 6] = advertiser
            .wire_address()
            .raw()
            .try_into()
            .expect("a device address has six octets");
        let advertiser = LeDeviceAddress::from_wire_bytes(bytes, kind);
        let data = LegacyAdvertisingData::new(data.as_bytes())
            .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
        let scan_response_len = match scan_response {
            Some(response) => Some(
                LegacyScanResponseData::new(response.as_bytes())
                    .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?
                    .encode(advertiser, &mut self.scan_response)
                    .map_err(|_| HciError::INVALID_HCI_PARAMETERS)?,
            ),
            None => None,
        };
        let pdu_len = match set_kind {
            SetKind::Nonconnectable => LegacyNonconnectableAdvertisement::new(
                LegacyNonconnectableKind::NonScannable,
                advertiser,
                data,
            )
            .encode(&mut self.pdu),
            SetKind::Scannable => LegacyNonconnectableAdvertisement::new(
                LegacyNonconnectableKind::Scannable,
                advertiser,
                data,
            )
            .encode(&mut self.pdu),
            SetKind::Connectable => LegacyConnectableAdvertisement::new(
                advertiser,
                data,
                LeChannelSelectionAlgorithmTwoSupport::Supported,
            )
            .encode(&mut self.pdu),
            SetKind::Directed { target, .. } => LegacyDirectedAdvertisement::new(
                advertiser,
                target,
                LeChannelSelectionAlgorithmTwoSupport::Supported,
            )
            .encode(&mut self.pdu),
        };
        self.kind = set_kind;
        self.pdu_len = pdu_len.map_err(|_| HciError::INVALID_HCI_PARAMETERS)?;
        self.scan_response_len = scan_response_len;
        self.advertiser = advertiser;
        let channels = parameters.channels();
        self.channels = AdvertisingChannels::new(
            channels.channel_37(),
            channels.channel_38(),
            channels.channel_39(),
        )
        .ok_or(HciError::INVALID_HCI_PARAMETERS)?;
        self.interval = match set_kind {
            SetKind::Directed {
                high_duty: true, ..
            } => HIGH_DUTY_INTERVAL,
            _ => RadioDuration::from_micros(
                u64::from(parameters.interval().minimum_units_625_us()) * 625,
            ),
        };
        Ok(())
    }

    /// Stop the set; the completion follows once it is removed.
    pub(crate) fn disable(&mut self) {
        self.phase = match self.outstanding {
            Some(_) => Phase::Cancelling { sent: false },
            None => Phase::Removing { sent: false },
        };
    }

    /// Whether the role has a request for the radio.
    pub(crate) fn wants_radio(&self) -> bool {
        self.has_control_request() || (self.phase == Phase::Running && self.outstanding.is_none())
    }

    pub(crate) fn has_control_request(&self) -> bool {
        matches!(
            self.phase,
            Phase::Configuring { sent: false }
                | Phase::Cancelling { sent: false }
                | Phase::Removing { sent: false }
        )
    }

    /// The set as the backend configures it.
    fn configuration(&self) -> AdvertisingConfiguration<'_> {
        AdvertisingConfiguration {
            set: SET,
            pdu: AdvertisingPdu::new(&self.pdu[..self.pdu_len])
                .expect("the encoded PDU fits the legacy bounds"),
            reception: match (self.kind, self.scan_response_len) {
                (SetKind::Directed { .. }, _) => AdvertisingReception::Report,
                (_, Some(length)) => AdvertisingReception::ScanResponse(
                    AdvertisingPdu::new(&self.scan_response[..length])
                        .expect("the encoded response fits the legacy bounds"),
                ),
                (_, None) => AdvertisingReception::None,
            },
            tx_power: TxPower::from_dbm(0),
            phy: LePhy::Le1M,
        }
    }

    /// A configuration request the role needs now.
    pub(crate) fn control_request<K: LeCapabilities>(&mut self) -> Option<RadioWork<'_, K>> {
        match &mut self.phase {
            Phase::Configuring { sent: sent @ false } => {
                *sent = true;
                Some(RadioWork::Submit(RadioRequest::ConfigureAdvertising(
                    Supported::new(self.configuration())
                        .expect("the set's type was checked against K when it was enabled"),
                )))
            }
            Phase::Cancelling { sent: sent @ false } => {
                *sent = true;
                let id = self.outstanding.expect("an event is cancelled").id;
                Some(RadioWork::Cancel(id))
            }
            Phase::Removing { sent: sent @ false } => {
                *sent = true;
                Some(RadioWork::Submit(RadioRequest::RemoveAdvertising(SET)))
            }
            _ => None,
        }
    }

    /// The backend's answer to this role's control request.
    pub(crate) fn control_done(&mut self, accepted: bool) {
        if accepted {
            let step = match self.phase {
                Phase::Configuring { .. } => Some(AdvertiserStepKind::Configured),
                Phase::Removing { .. } => Some(AdvertiserStepKind::Removed),
                Phase::Idle | Phase::Running | Phase::Cancelling { .. } => None,
            };
            if let Some(step) = step {
                crate::trace::emit(|| AdvertiserStep { step, id: 0 });
            }
        }
        match self.phase {
            Phase::Configuring { .. } if accepted => {
                self.phase = Phase::Running;
                self.coexistence_period = coexistence::advertising_period(self.interval);
                self.ended = 0;
                self.completion = Some(Status::SUCCESS);
            }
            Phase::Configuring { .. } => {
                self.phase = Phase::Idle;
                self.completion = Some(HciError::HARDWARE_FAILURE.to_status());
            }
            // A cancelled event ends either way; its outcome follows.
            Phase::Cancelling { .. } => {}
            Phase::Removing { .. } if accepted && self.restart => {
                self.restart = false;
                self.phase = Phase::Configuring { sent: false };
            }
            // Advertising that ended in a connection completes no command.
            Phase::Removing { .. } if self.connected => {
                self.connected = false;
                self.phase = Phase::Idle;
            }
            // Expired directed advertising completes no command either; the
            // Host learns it from LE Connection Complete.
            Phase::Removing { .. } if self.expired => {
                self.expired = false;
                self.timed_out = true;
                self.phase = Phase::Idle;
            }
            Phase::Removing { .. } => {
                self.restart = false;
                self.phase = Phase::Idle;
                self.completion = Some(if accepted {
                    Status::SUCCESS
                } else {
                    HciError::HARDWARE_FAILURE.to_status()
                });
            }
            Phase::Idle | Phase::Running => {}
        }
    }

    /// Air time of one channel's packet and the responses it awaits.
    fn channel_air(&self) -> NonZeroRadioDuration<LeRadio> {
        // Legacy wire lengths are bounded to 39 bytes, so every sum below
        // stays under 10,000 us: none saturates.
        let air = air_micros(self.pdu_len);
        let responses = if let SetKind::Directed { .. } = self.kind {
            RESPONSE_CAPABLE_TAIL_MICROS + T_IFS_MICROS + LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS
        } else if let Some(response) = self.scan_response_len {
            let scan = SCAN_REQ_AIR_MICROS + T_IFS_MICROS + air_micros(response).get();
            let exchange = match self.kind {
                SetKind::Connectable => scan.max(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS),
                SetKind::Nonconnectable | SetKind::Scannable | SetKind::Directed { .. } => scan,
            };
            RESPONSE_CAPABLE_TAIL_MICROS + T_IFS_MICROS + exchange
        } else {
            0
        };
        NonZeroRadioDuration::from_micros(NonZeroU64::from(air.saturating_add(responses)))
    }

    /// Air time of one channel of the event and its responses, plus the
    /// lead.
    fn channel_spacing(
        &self,
        timing: RadioTiming,
    ) -> Result<RadioDuration<LeRadio>, EpochExhausted> {
        // A spacing past `u64::MAX` cannot lie in the epoch.
        self.channel_air()
            .get()
            .checked_add(timing.preparation_lead)
            .ok_or(error(O::Event, C::ChannelSpacing))
    }

    fn event_duration(
        &self,
        timing: RadioTiming,
    ) -> Result<NonZeroRadioDuration<LeRadio>, EpochExhausted> {
        let spacing = self.channel_spacing(timing)?;
        let channels = self.channels.iter().count() as u64;
        // The last channel needs no lead after it.
        spacing
            .checked_mul(channels - 1)
            .and_then(|prefix| self.channel_air().checked_add(prefix))
            .ok_or(error(O::Event, C::EventDuration))
    }

    pub(crate) fn wants_event(&self) -> bool {
        self.phase == Phase::Running && self.outstanding.is_none() && !self.expired
    }

    /// The next event, when the role needs one.
    pub(crate) fn proposal(
        &self,
        earliest: LeInstant,
        timing: RadioTiming,
        progress: Progress,
    ) -> Result<Option<Proposal>, EpochExhausted> {
        if !self.wants_event() || progress.expired {
            return Ok(None);
        }
        let earliest = progress
            .next_anchor
            .map_or(earliest, |anchor| anchor.max(earliest));
        Ok(Some(Proposal {
            earliest,
            latest: earliest
                .checked_add(ADVERTISING_DELAY_MAX)
                .ok_or(error(O::Event, C::LatestAnchor))?,
            duration: self.event_duration(timing)?,
        }))
    }

    /// The next nominal reservation. Absence never hides failed geometry.
    pub(crate) fn planned(
        &self,
        timing: RadioTiming,
        progress: Progress,
    ) -> Result<Option<LeWindow>, EpochExhausted> {
        if self.phase != Phase::Running || progress.expired {
            return Ok(None);
        }
        let Some(anchor) = progress.next_anchor else {
            return Ok(None);
        };
        reservation(anchor, self.event_duration(timing)?, timing)
            .map(Some)
            .map_err(|OutsideEpoch| error(O::FutureReservation, C::Reservation))
    }

    pub(crate) fn busy(&self) -> Option<LeWindow> {
        self.outstanding.map(|event| event.reservation)
    }

    /// The submitted event a connection event may take the place of: one
    /// of a running set, not cancelled yet.
    pub(crate) fn preemptible(&self) -> Option<(EventId, LeWindow)> {
        self.outstanding
            .filter(|event| self.phase == Phase::Running && !event.preempted)
            .map(|event| (event.id, event.reservation))
    }

    /// Whether a cancelled event still holds the radio until its end.
    pub(crate) fn is_preempted(&self) -> bool {
        self.outstanding.is_some_and(|event| event.preempted)
    }

    /// A connection event takes the submitted event's place: its cancel is
    /// sent, and its end follows as usual.
    pub(crate) fn preempt(&mut self) {
        if let Some(event) = &mut self.outstanding {
            event.preempted = true;
        }
    }

    /// Validate the whole event and its required successor before committing.
    pub(crate) fn build(
        &mut self,
        id: EventId,
        anchor: LeInstant,
        timing: RadioTiming,
        delay: RadioDuration<LeRadio>,
        progress: Progress,
    ) -> Result<AdvertisingEvent, EpochExhausted> {
        let channel_spacing = self.channel_spacing(timing)?;
        let reserved = reservation(anchor, self.event_duration(timing)?, timing)
            .map_err(|OutsideEpoch| error(O::Event, C::Reservation))?;
        let continuation = self.continuation(anchor, delay, progress)?;
        crate::trace::emit(|| AdvertiserStep {
            step: AdvertiserStepKind::EventPlanned,
            id: id.get(),
        });
        self.outstanding = Some(Outstanding {
            id,
            reservation: reserved,
            preempted: false,
        });
        self.commit(continuation);
        Ok(AdvertisingEvent {
            id,
            set: SET,
            anchor,
            channels: self.channels,
            channel_spacing,
            coexistence: coexistence::advertising_level(self.coexistence_period, self.ended),
        })
    }

    /// The backend's answer to the event request. A refused event counts as
    /// skipped; the next one keeps its planned anchor.
    pub(crate) fn event_done(&mut self, accepted: bool) {
        if !accepted {
            self.outstanding = None;
            if self.expired && self.phase == Phase::Running {
                self.phase = Phase::Removing { sent: false };
            }
        }
    }

    /// Skip the event the arbiter could not place.
    pub(crate) fn progress(&self) -> Progress {
        Progress {
            next_anchor: self.next_anchor,
            expires: self.expires,
            expired: self.expired,
        }
    }

    pub(crate) fn skip(
        &self,
        earliest: LeInstant,
        delay: RadioDuration<LeRadio>,
        progress: Progress,
    ) -> Result<Progress, EpochExhausted> {
        let anchor = progress.next_anchor.unwrap_or(earliest);
        self.continuation(anchor, delay, progress)
    }

    fn continuation(
        &self,
        anchor: LeInstant,
        delay: RadioDuration<LeRadio>,
        progress: Progress,
    ) -> Result<Progress, EpochExhausted> {
        let high_duty = matches!(
            self.kind,
            SetKind::Directed {
                high_duty: true,
                ..
            }
        );
        let step = if high_duty {
            self.interval
        } else {
            // A step past `u64::MAX` cannot lie in the epoch.
            self.interval
                .checked_add(delay)
                .ok_or(error(O::Event, C::Recurrence))?
        };
        let next = anchor
            .checked_add(step)
            .ok_or(error(O::Event, C::Recurrence))?;
        let expires = if high_duty {
            Some(match progress.expires {
                Some(expires) => expires,
                None => anchor
                    .checked_add(HIGH_DUTY_DURATION)
                    .ok_or(error(O::Event, C::Expiry))?,
            })
        } else {
            progress.expires
        };
        Ok(Progress {
            next_anchor: Some(next),
            expires,
            expired: high_duty && expires.is_some_and(|expires| next >= expires),
        })
    }

    pub(crate) fn commit(&mut self, continuation: Progress) {
        self.next_anchor = continuation.next_anchor;
        self.expires = continuation.expires;
        self.expired = continuation.expired;
        if self.expired && self.outstanding.is_none() && self.phase == Phase::Running {
            self.phase = Phase::Removing { sent: false };
        }
    }

    /// Judge a PDU the set received as a connection indication, and open
    /// the connection it accepts.
    fn indication(&mut self, pdu: &[u8], captured_at: Option<LeInstant>) -> IndicationVerdict {
        if self.phase != Phase::Running {
            return IndicationVerdict::NotRunning;
        }
        let Some(at) = captured_at else {
            return IndicationVerdict::NoCapture;
        };
        let request = match LeLegacyConnectionRequest::decode(pdu) {
            Ok(request) => request,
            Err(LeLegacyConnectionRequestError::UnexpectedPduType) => {
                return IndicationVerdict::OtherPdu;
            }
            Err(_) => return IndicationVerdict::Malformed,
        };
        let admitted = match self.kind {
            SetKind::Connectable => true,
            SetKind::Directed { target, .. } => request.initiator() == target,
            SetKind::Nonconnectable | SetKind::Scannable => false,
        };
        if !admitted {
            return IndicationVerdict::NotAdmitted;
        }
        if !request.is_addressed_to(self.advertiser) {
            return IndicationVerdict::NotAddressed;
        }
        self.connection = Some(ConnectionIndication { request, at });
        self.connected = true;
        self.phase = Phase::Cancelling { sent: false };
        IndicationVerdict::Accepted
    }

    /// Account one outcome.
    pub(crate) fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        match outcome {
            RadioOutcome::Received { id, pdu } if self.owns(id) => {
                let verdict = self.indication(pdu.pdu, pdu.captured_at);
                crate::trace::emit(|| oer_bluetooth_trace::ConnectionIndication { verdict });
            }
            RadioOutcome::EventEnded { id, .. } if self.owns(id) => {
                crate::trace::emit(|| AdvertiserStep {
                    step: AdvertiserStepKind::EventEnded,
                    id: id.get(),
                });
                self.outstanding = None;
                self.ended = self.ended.wrapping_add(1);
                match self.phase {
                    Phase::Cancelling { .. } => self.phase = Phase::Removing { sent: false },
                    Phase::Running if self.expired => {
                        self.phase = Phase::Removing { sent: false };
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Whether high duty cycle directed advertising ended without a
    /// connection since the last call.
    pub(crate) fn take_timeout(&mut self) -> bool {
        core::mem::take(&mut self.timed_out)
    }

    /// The connection indication that ended advertising, once.
    pub(crate) fn take_connection(&mut self) -> Option<ConnectionIndication> {
        self.connection.take()
    }

    pub(crate) fn owns(&self, id: EventId) -> bool {
        self.outstanding.is_some_and(|event| event.id == id)
    }

    pub(crate) fn take_completion(&mut self) -> Option<Status> {
        self.completion.take()
    }
}

/// LE 1M air time of a legacy PDU of `length` bytes, header included.
const fn air_micros(length: usize) -> NonZeroU32 {
    // Preamble, access address, header and CRC.
    const OVERHEAD: NonZeroU32 = NonZeroU32::new((1 + 4 + 2 + 3) * 8).unwrap();
    // Legacy PDUs carry at most 37 payload bytes: it never saturates.
    OVERHEAD.saturating_add((length as u32 - 2) * 8)
}

#[derive(Clone, Copy)]
pub(crate) struct Progress {
    next_anchor: Option<LeInstant>,
    expires: Option<LeInstant>,
    expired: bool,
}

fn error(operation: O, calculation: C) -> EpochExhausted {
    EpochExhausted::at(R::Advertising, operation, calculation)
}
