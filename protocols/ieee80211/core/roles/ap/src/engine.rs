//! The access point's frames in and out: one sans-IO engine over the peer
//! service and the beacon template.
//!
//! A received MPDU enters as bytes with its integrity evidence and leaves as
//! one [`ApReceived`]: a response the engine wrote into the caller's
//! buffer, a peer the AP no longer holds, an EAPOL-Key frame for the
//! authenticator, or an MSDU of an authorized peer for the network. A frame
//! the AP originates (a beacon, a handshake message, a peer's teardown, a
//! data MSDU) is written into the caller's buffer as one [`ApFrame`] that
//! names how it goes. The engine holds no port token, buffer or key slot:
//! the caller carries every frame through its port and returns its end.
//!
//! The beacon follows the access point's TSF: the caller names the instant
//! the TSF restarted ([`ApEngine::tsf_started`]); the TBTT schedule and the
//! Timestamp field are that clock's, measured in the caller's time since the
//! restart. The Timestamp is written when the beacon is prepared; the delay
//! between the preparation and the beacon's first symbol (queueing, channel
//! access) is not in it.

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_core_security_rsn::{
    OwnedEapolFrame, RsnInterface, frames::RsnTxFrame,
};
use oer_protocols_ieee80211_core_upper_mac::{
    ccmp::{CcmpRxReplayEpoch, InstalledCcmpTxKey},
    receive::{AdmittedData, Integrity, RxDropReason},
};
use oer_protocols_ieee80211_wire::{
    address::{AddressFlags, Bssid, IndividualAddress},
    ap::{
        ApAssociationResponseError, ApDataFrame, ApDataFrameError, ApManagementRequest,
        ApProtectedDataFrame, parse_ap_management_request, probe, profile::Advertisement,
        write_ap_peer_disconnect, write_ht_association_response_frame_for_security,
        write_open_authentication_response,
    },
    ccmp::{CCMP_HEADER_LEN, GroupKeyId},
    channel::Channel,
    data::MsduParts,
    frame::DecodedHeader,
    security::spp::SppScope,
};

pub use oer_protocols_ieee80211_wire::{
    ap::ApPeerDisconnectKind,
    security::{Spp as ApSpp, spp::SppScope as ApSppScope},
};

use crate::beacon::{ApBeacon, ApBeaconTxContext};
use crate::limits::AP_MAX_CLIENTS;
use crate::service::{
    AccessPointService, ApAssociationCapabilities, ApEapolTransmit, ApGroupTransmitKey,
    ApMlmeAction, ApPeerClose, ApPeerPhase, ApServiceError, ApWpa2Error, ApWpa2Progress,
    ApWpa2RetryProgress,
};

/// The EAPOL-Key frames the engine carries.
pub const AP_EAPOL_CAPACITY: usize =
    oer_protocols_ieee80211_core_security_rsn::frames::RSN_TX_EAPOL_CAPACITY;

/// The EtherType of EAPOL (IEEE Std 802.1X-2020, 11.1.4).
pub const EAPOL_ETHER_TYPE: u16 = 0x888e;

/// How long the access point answers no further Probe Request after one:
/// one response per interval, whatever its sender. The archive's policy.
pub const PROBE_RESPONSE_INTERVAL: Duration = Duration::from_millis(10);

/// The reason code of the access point's own teardown of a peer: the AP
/// leaves, or ends an inactive or failed association (IEEE Std
/// 802.11-2020, Table 9-49, 3: "deauthenticated because sending STA is
/// leaving (or has left) the BSS").
pub const AP_LEAVING_REASON: u16 = 3;

/// Why the engine could not take an input or write a frame. Nothing of the
/// peer table changed unless the variant says so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApEngineError {
    /// The peer service refused the step.
    Service(ApServiceError),
    /// The authenticator refused the step.
    Wpa2(ApWpa2Error),
    /// A management frame did not fit the caller's buffer.
    Management(ApAssociationResponseError),
    /// A data frame did not fit the caller's buffer or named a wrong peer.
    Data(ApDataFrameError),
    /// A probe response could not be written.
    Probe,
    /// The beacon template could not be stamped.
    Beacon,
    /// The caller's buffer is shorter than the frame.
    Output,
}

impl From<ApServiceError> for ApEngineError {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<ApWpa2Error> for ApEngineError {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

impl From<ApAssociationResponseError> for ApEngineError {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Management(error)
    }
}

impl From<ApDataFrameError> for ApEngineError {
    fn from(error: ApDataFrameError) -> Self {
        Self::Data(error)
    }
}

/// How a frame the engine wrote goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApFrameKind {
    /// An individually addressed management frame or an unprotected EAPOL
    /// frame to `peer`, acknowledged, at the BSS's lowest basic rate.
    Management {
        /// The receiver.
        peer: [u8; 6],
    },
    /// A data MSDU to an authorized `peer` under its pairwise key.
    Pairwise {
        /// The receiver.
        peer: [u8; 6],
        /// The highest legacy rate the peer and the BSS share.
        rate: oer_protocols_ieee80211_wire::phy::PhyRate,
    },
    /// A group-addressed data MSDU under the group key.
    Group,
    /// The beacon.
    Beacon,
}

/// One frame the engine wrote at the start of the caller's buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApFrame {
    /// Its length, header to body, without the MIC a protected frame's
    /// transmitter appends.
    pub len: usize,
    /// How it goes.
    pub kind: ApFrameKind,
}

/// What the caller does once a management response is on the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAfter {
    /// Nothing.
    Nothing,
    /// The association succeeded: start the four-way handshake with Message
    /// 1 ([`ApEngine::begin_wpa2`]) once the response is acknowledged.
    BeginWpa2 {
        /// The associated peer.
        peer: [u8; 6],
    },
}

/// One received MSDU of an authorized peer, located in the received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApMsdu<'f> {
    /// Its destination.
    pub destination: [u8; 6],
    /// Its source: the peer.
    pub source: [u8; 6],
    /// The EtherType of its LLC/SNAP header.
    pub ether_type: u16,
    /// Its payload, in the received frame.
    pub payload: &'f [u8],
}

impl<'f> ApMsdu<'f> {
    /// The MSDU's fields, as the network takes them.
    pub const fn parts(&self) -> MsduParts<'f> {
        MsduParts {
            destination: self.destination,
            source: self.source,
            ether_type: self.ether_type,
            payload: self.payload,
        }
    }
}

/// What a received frame was to the access point.
#[derive(Debug, Eq, PartialEq)]
pub enum ApReceived<'f> {
    /// Nothing the AP acts on: another BSS's frame, a duplicate, a frame of
    /// a peer it does not hold or a refused one.
    Nothing,
    /// The engine wrote a response at the start of the caller's buffer.
    Respond {
        /// The response.
        frame: ApFrame,
        /// What follows its terminal.
        after: ApAfter,
    },
    /// The peer left (Deauthentication or Disassociation): the caller
    /// removes its pairwise key if one is installed.
    PeerLeft {
        /// The peer.
        peer: [u8; 6],
    },
    /// An EAPOL-Key frame of an associated peer for the authenticator
    /// ([`ApEngine::eapol`]).
    Eapol {
        /// The peer.
        peer: [u8; 6],
        /// The EAPOL frame.
        body: &'f [u8],
    },
    /// An MSDU of an authorized peer for the network.
    Msdu(ApMsdu<'f>),
}

/// What an EAPOL-Key frame moved the handshake to.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "the next message moves once, by value, to its transmission"
)]
pub enum ApEapol {
    /// Nothing to do: a stale, duplicate or refused frame.
    Nothing,
    /// Send the next message.
    Transmit(ApEapolTransmit<AP_EAPOL_CAPACITY>),
    /// Message 2 is verified and derived a new PTK: install the peer's
    /// pairwise key ([`ApEngine::pairwise_key`]) with its controlled port
    /// closed, tell the engine on the key's terminal
    /// ([`ApEngine::pairwise_installed`]), and only then send Message 3,
    /// so the station's Message 4, protected or not, is readable.
    Install {
        /// The peer.
        peer: [u8; 6],
        /// Message 3, sent once the key is installed.
        message3: ApEapolTransmit<AP_EAPOL_CAPACITY>,
    },
    /// Message 4 is verified: [`ApEngine::authorize`] the peer, whose
    /// pairwise key is installed, which opens its controlled port.
    Authorize {
        /// The peer.
        peer: [u8; 6],
    },
    /// The handshake failed: tear the peer down ([`ApEngine::teardown`]).
    Deauthenticate {
        /// The peer.
        peer: [u8; 6],
    },
}

/// A peer the engine ends: its Disassociation and Deauthentication go, and
/// its pairwise key comes off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApTeardown {
    /// The peer.
    pub peer: [u8; 6],
    /// Whether it was associated: a Disassociation goes before the
    /// Deauthentication.
    pub associated: bool,
}

impl From<ApPeerClose> for ApTeardown {
    fn from(close: ApPeerClose) -> Self {
        Self {
            peer: close.peer,
            associated: close.was_associated,
        }
    }
}

/// The receive replay owner of one authorized peer.
struct PeerReplay {
    peer: [u8; 6],
    replay: CcmpRxReplayEpoch,
}

/// The access point's sans-IO engine.
pub struct ApEngine<'s, S: SppScope> {
    service: AccessPointService<'s, S>,
    beacon: ApBeacon<'s>,
    advertisement: Advertisement,
    channel: Channel,
    /// The group key the BSS announces, for each peer's replay lanes.
    group_key_id: GroupKeyId,
    /// The instant the TSF restarted, once it did.
    tsf_epoch: Option<Instant>,
    /// No Probe Response before this instant.
    probe_after: Option<Instant>,
    replay: [Option<PeerReplay>; AP_MAX_CLIENTS],
}

impl<'s, S: SppScope> ApEngine<'s, S> {
    /// An engine over `service` and the template `beacon` of the BSS on
    /// `channel`, which advertises `advertisement` and announces its group
    /// key as `group_key_id`.
    pub fn new(
        service: AccessPointService<'s, S>,
        beacon: ApBeacon<'s>,
        advertisement: Advertisement,
        channel: Channel,
        group_key_id: GroupKeyId,
    ) -> Self {
        Self {
            service,
            beacon,
            advertisement,
            channel,
            group_key_id,
            tsf_epoch: None,
            probe_after: None,
            replay: [const { None }; AP_MAX_CLIENTS],
        }
    }

    /// The peer service.
    pub const fn service(&self) -> &AccessPointService<'s, S> {
        &self.service
    }

    /// The access point's address, its BSSID.
    pub const fn address(&self) -> [u8; 6] {
        self.service.address()
    }

    /// The TSF restarted at `now`: the beacon schedule starts again on it.
    pub fn tsf_started(&mut self, now: Instant) {
        self.tsf_epoch = Some(now);
        // The TBTTs are the TSF's multiples of the beacon interval.
        self.beacon.start_schedule(Instant::from_micros(0));
    }

    /// The TSF stopped: no beacon is due.
    pub fn tsf_stopped(&mut self) {
        self.tsf_epoch = None;
    }

    /// The TSF's value at `now`, in its microseconds, while it runs.
    fn tsf(&self, now: Instant) -> Option<Instant> {
        self.tsf_epoch
            .map(|epoch| Instant::from_micros(now.saturating_duration_since(epoch).as_micros()))
    }

    /// The instant the next beacon is due, while the TSF runs: its TBTT.
    pub fn next_beacon(&self) -> Option<Instant> {
        let epoch = self.tsf_epoch?;
        let next = self.beacon.next_publication()?;
        Some(epoch.saturating_add(Duration::from_micros(next.as_micros())))
    }

    /// The beacon interval.
    pub const fn beacon_interval(&self) -> Duration {
        self.beacon.interval()
    }

    /// Write the beacon due at `now` into `output`: the frame, the instant it
    /// stays fresh until (the next TBTT) and its DTIM continuation, which
    /// its terminal completes. `None` while no beacon is due.
    ///
    /// # Errors
    ///
    /// [`ApEngineError::Beacon`] when the template could not be stamped;
    /// [`ApEngineError::Output`] when `output` is shorter than the frame.
    pub fn beacon(
        &mut self,
        now: Instant,
        output: &mut [u8],
    ) -> Result<Option<(ApFrame, Instant, ApBeaconTxContext)>, ApEngineError> {
        let (Some(tsf), Some(due)) = (self.tsf(now), self.next_beacon()) else {
            return Ok(None);
        };
        if now < due {
            return Ok(None);
        }
        let sequence = self.service.next_management_sequence();
        let bitmap = self
            .service
            .unicast_tim_bitmap()
            .map_err(|_| ApEngineError::Beacon)?;
        let buffered = self.service.buffered_group_frames();
        let publication = self
            .beacon
            .prepare(tsf, sequence, buffered, bitmap.partial())
            .ok_or(ApEngineError::Beacon)?;
        let frame = publication.frame();
        let len = frame.len();
        let destination = output.get_mut(..len).ok_or(ApEngineError::Output)?;
        for (octet, source) in destination.iter_mut().zip(frame) {
            *octet = *source;
        }
        let (_, context) = publication.into_parts();
        let fresh = self
            .next_beacon()
            .unwrap_or_else(|| now.saturating_add(self.beacon.interval()));
        Ok(Some((
            ApFrame {
                len,
                kind: ApFrameKind::Beacon,
            },
            fresh,
            context,
        )))
    }

    /// Take one received MPDU with its `integrity` evidence at `now`,
    /// writing a response into `output`. `nonce` is a fresh ANonce and
    /// `replay_counter` the first replay counter of a handshake an
    /// association starts; without a nonce no association is admitted.
    ///
    /// # Errors
    ///
    /// A step the service, the authenticator or a frame writer refused.
    pub fn receive<'f>(
        &mut self,
        frame: &'f [u8],
        integrity: Integrity,
        now: Instant,
        nonce: Option<[u8; 32]>,
        replay_counter: u64,
        output: &mut [u8],
    ) -> Result<ApReceived<'f>, ApEngineError> {
        let Some(header) = DecodedHeader::decode(frame) else {
            return Ok(ApReceived::Nothing);
        };
        if header.control().is_data() {
            return Ok(self.uplink(header, integrity, now));
        }
        self.management(frame, now, nonce, replay_counter, output)
    }

    /// One data uplink: EAPOL of an associated peer, an MSDU of an
    /// authorized one.
    fn uplink<'f>(
        &mut self,
        header: DecodedHeader<'f>,
        integrity: Integrity,
        now: Instant,
    ) -> ApReceived<'f> {
        let peer = header.transmitter();
        let Some(status) = self.service.peer_status(peer) else {
            return ApReceived::Nothing;
        };
        let Some(bssid) = IndividualAddress::new(self.address()).and_then(Bssid::new) else {
            return ApReceived::Nothing;
        };
        let replay = self
            .replay
            .iter_mut()
            .flatten()
            .find(|replay| replay.peer == peer)
            .map(|replay| &mut replay.replay);
        let admitted = match AdmittedData::admit_uplink(header, bssid, peer, integrity, replay) {
            Ok(admitted) => admitted,
            Err(RxDropReason::Replay | RxDropReason::Integrity | _) => return ApReceived::Nothing,
        };
        let Ok(mut msdus) = admitted.msdus() else {
            return ApReceived::Nothing;
        };
        let Some(Ok(MsduParts {
            destination,
            source,
            ether_type,
            payload,
        })) = msdus.next()
        else {
            return ApReceived::Nothing;
        };
        match status.phase {
            ApPeerPhase::Securing if ether_type == EAPOL_ETHER_TYPE => ApReceived::Eapol {
                peer,
                body: payload,
            },
            ApPeerPhase::Authorized if admitted.protected() || !self.protects() => {
                let _ = self.service.observe_activity(peer, now);
                if ether_type == EAPOL_ETHER_TYPE {
                    // A rekey is not served; the frame changes nothing.
                    return ApReceived::Nothing;
                }
                ApReceived::Msdu(ApMsdu {
                    destination,
                    source,
                    ether_type,
                    payload,
                })
            }
            _ => ApReceived::Nothing,
        }
    }

    /// Whether the BSS protects its data.
    fn protects(&self) -> bool {
        self.service.link_protection()
            == oer_protocols_ieee80211_wire::security::LinkProtection::Ccmp
    }

    /// One management request addressed to the AP.
    fn management<'f>(
        &mut self,
        frame: &'f [u8],
        now: Instant,
        nonce: Option<[u8; 32]>,
        replay_counter: u64,
        output: &mut [u8],
    ) -> Result<ApReceived<'f>, ApEngineError> {
        let Some(request) = parse_ap_management_request(&self.advertisement, frame, self.address())
        else {
            return Ok(ApReceived::Nothing);
        };
        let retry = frame.get(1).is_some_and(|byte| byte & 0x08 != 0);
        let address = self.address();
        match request {
            ApManagementRequest::Probe { peer, ssid } => {
                let advertisement = self
                    .beacon
                    .advertisement()
                    .map_err(|_| ApEngineError::Probe)?;
                if !probe::matches_ssid(advertisement, ssid)
                    || self.probe_after.is_some_and(|after| now < after)
                {
                    return Ok(ApReceived::Nothing);
                }
                let sequence = self.service.next_management_sequence();
                let advertisement = self
                    .beacon
                    .advertisement()
                    .map_err(|_| ApEngineError::Probe)?;
                // The Timestamp of a probe response is the TSF at its
                // preparation, as the beacon's.
                let tsf = self.tsf(now).map_or(0, Instant::as_micros);
                let len = probe::write_response(advertisement, peer, sequence, tsf, output)
                    .map_err(|_| ApEngineError::Probe)?;
                self.probe_after = Some(now.saturating_add(PROBE_RESPONSE_INTERVAL));
                Ok(ApReceived::Respond {
                    frame: ApFrame {
                        len,
                        kind: ApFrameKind::Management { peer },
                    },
                    after: ApAfter::Nothing,
                })
            }
            ApManagementRequest::OpenAuthentication { peer } => {
                let status = match self.service.peer_status(peer) {
                    // A retried request whose response was lost: the same
                    // success again, the peer's state kept.
                    Some(status) if retry && status.phase != ApPeerPhase::Authenticated => 0,
                    Some(_) => {
                        // A new authentication ends the peer's earlier
                        // association: its key comes off first.
                        self.forget_replay(peer);
                        self.authenticate(peer, now)?
                    }
                    None => self.authenticate(peer, now)?,
                };
                let sequence = self.service.next_management_sequence();
                let len =
                    write_open_authentication_response(output, address, peer, status, sequence)?;
                Ok(ApReceived::Respond {
                    frame: ApFrame {
                        len,
                        kind: ApFrameKind::Management { peer },
                    },
                    after: ApAfter::Nothing,
                })
            }
            ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
                short_preamble,
            } => {
                let Some(status) = self.service.peer_status(peer) else {
                    return Ok(ApReceived::Nothing);
                };
                let (status_code, association_id, after) = if self
                    .service
                    .matches_association_security(security)
                    .unwrap_or(false)
                    && (status.phase == ApPeerPhase::Securing
                        || (!self.protects() && status.phase == ApPeerPhase::Authorized))
                {
                    // A repeated request whose response was lost: the same
                    // success again, no second Message 1.
                    (0, status.association_id, ApAfter::Nothing)
                } else if status.phase != ApPeerPhase::Authenticated {
                    return Ok(ApReceived::Nothing);
                } else {
                    let capabilities = ApAssociationCapabilities {
                        maximum_legacy_rate_500kbps,
                        short_preamble,
                        ht: ht_capabilities,
                        qos_supported,
                    };
                    let action = if self.protects() {
                        let Some(nonce) = nonce else {
                            return Ok(ApReceived::Nothing);
                        };
                        self.service.associate_rsn(
                            peer,
                            security,
                            capabilities,
                            nonce,
                            replay_counter,
                            now,
                        )?
                    } else {
                        self.service
                            .associate_open(peer, security, capabilities, now)?
                    };
                    let ApMlmeAction::AssociationResponse {
                        status,
                        association_id,
                        ..
                    } = action
                    else {
                        return Ok(ApReceived::Nothing);
                    };
                    let after = match association_id {
                        Some(_) if self.protects() => ApAfter::BeginWpa2 { peer },
                        _ => ApAfter::Nothing,
                    };
                    (status, association_id.unwrap_or(0), after)
                };
                let sequence = self.service.next_management_sequence();
                let protection = self
                    .service
                    .bss_protection(self.channel.bandwidth_mhz() == 40);
                let len = write_ht_association_response_frame_for_security(
                    &self.advertisement,
                    output,
                    address,
                    peer,
                    status_code,
                    association_id,
                    sequence,
                    self.channel,
                    ht_capabilities,
                    self.service.security_policy().advertisement(),
                    protection,
                )?;
                Ok(ApReceived::Respond {
                    frame: ApFrame {
                        len,
                        kind: ApFrameKind::Management { peer },
                    },
                    after,
                })
            }
            ApManagementRequest::Disassociation { peer, .. }
            | ApManagementRequest::Deauthentication { peer, .. } => {
                let Some(status) = self.service.peer_status(peer) else {
                    return Ok(ApReceived::Nothing);
                };
                // The AP's own teardown of the peer is in flight: it ends it.
                if status.phase == ApPeerPhase::Closing {
                    return Ok(ApReceived::Nothing);
                }
                self.forget_replay(peer);
                self.service.remove_peer(peer)?;
                Ok(ApReceived::PeerLeft { peer })
            }
            // No SAE, no Block Ack agreement and no management protection
            // is offered: these requests change nothing.
            ApManagementRequest::SaeAuthentication { .. }
            | ApManagementRequest::BlockAck { .. }
            | ApManagementRequest::SaQuery { .. } => Ok(ApReceived::Nothing),
        }
    }

    /// Authenticate `peer` by Open System: the status its slot allows.
    fn authenticate(&mut self, peer: [u8; 6], now: Instant) -> Result<u16, ApEngineError> {
        match self.service.authenticate_open(peer, now)? {
            ApMlmeAction::AuthenticationResponse { status, .. } => Ok(status),
            _ => Err(ApEngineError::Service(ApServiceError::WrongPeerPhase)),
        }
    }

    /// Forget the receive replay owner of `peer`.
    fn forget_replay(&mut self, peer: [u8; 6]) {
        for slot in &mut self.replay {
            if slot.as_ref().is_some_and(|replay| replay.peer == peer) {
                *slot = None;
            }
        }
    }

    /// Message 1 to `peer`, once its successful Association Response was
    /// acknowledged.
    ///
    /// # Errors
    ///
    /// The authenticator refused it.
    pub fn begin_wpa2(
        &mut self,
        peer: [u8; 6],
    ) -> Result<ApEapolTransmit<AP_EAPOL_CAPACITY>, ApEngineError> {
        Ok(self.service.begin_wpa2_frame(peer)?)
    }

    /// Take one EAPOL-Key frame of `peer` into its handshake. Message 3
    /// carries `group_key`'s last packet number as its Key RSC.
    ///
    /// # Errors
    ///
    /// The authenticator refused the step for a reason other than the
    /// frame's own.
    pub fn eapol<K: ApGroupTransmitKey>(
        &mut self,
        peer: [u8; 6],
        body: &[u8],
        group_key: &InstalledCcmpTxKey<K>,
    ) -> Result<ApEapol, ApEngineError> {
        let Ok(frame) =
            OwnedEapolFrame::<AP_EAPOL_CAPACITY>::try_copy(RsnInterface::AccessPoint, peer, body)
        else {
            return Ok(ApEapol::Nothing);
        };
        let before = self.temporal_key(peer);
        Ok(match self.service.on_eapol(peer, frame, group_key)? {
            ApWpa2Progress::None => ApEapol::Nothing,
            // A Message 3 for a PTK the peer had not derived before: a
            // retried Message 3 or a repeated Message 2 of the same PTK
            // installs nothing again.
            ApWpa2Progress::Transmit(transmit)
                if self
                    .temporal_key(peer)
                    .is_some_and(|key| Some(key) != before) =>
            {
                ApEapol::Install {
                    peer,
                    message3: transmit,
                }
            }
            ApWpa2Progress::Transmit(transmit) => ApEapol::Transmit(transmit),
            ApWpa2Progress::AuthorizePeer => ApEapol::Authorize { peer },
            ApWpa2Progress::DeauthenticatePeer => ApEapol::Deauthenticate { peer },
        })
    }

    /// The temporal key `peer`'s handshake derived, if any.
    fn temporal_key(&self, peer: [u8; 6]) -> Option<[u8; 16]> {
        self.service
            .pending_ptk(peer)
            .ok()
            .map(|ptk| *ptk.temporal_key())
    }

    /// `peer`'s pairwise key is installed: a new receive replay owner reads
    /// its protected frames, while its controlled port stays closed until
    /// [`Self::authorize`].
    ///
    /// # Errors
    ///
    /// No replay owner fits.
    pub fn pairwise_installed(&mut self, peer: [u8; 6]) -> Result<(), ApEngineError> {
        self.forget_replay(peer);
        let replay = CcmpRxReplayEpoch::new([0; 8], self.group_key_id, [0; 8])
            .map_err(|_| ApEngineError::Service(ApServiceError::WrongPeerPhase))?;
        let slot = self
            .replay
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(ApServiceError::BufferedTrafficOverflow)?;
        *slot = Some(PeerReplay { peer, replay });
        Ok(())
    }

    /// The temporal key of `peer`'s verified handshake, for its pairwise
    /// installation, and the association identifier its key slot follows.
    ///
    /// # Errors
    ///
    /// The peer holds no verified handshake.
    pub fn pairwise_key(
        &self,
        peer: [u8; 6],
    ) -> Result<
        (
            oer_protocols_ieee80211_contracts_security::Ccmp128Key,
            IndividualAddress,
        ),
        ApEngineError,
    > {
        let ptk = self.service.pending_ptk(peer)?;
        let address = IndividualAddress::new(peer).ok_or(ApServiceError::UnknownPeer)?;
        Ok((
            oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(ptk.temporal_key()),
            address,
        ))
    }

    /// Whether the network takes `msdu`: addressed to the access point or
    /// to a group. An MSDU of one peer for another is not relayed.
    pub fn delivers(&self, msdu: &ApMsdu<'_>) -> bool {
        let [first, ..] = msdu.destination;
        msdu.destination == self.address() || AddressFlags::from_bytes([first]).group()
    }

    /// Open the controlled port of `peer`, whose pairwise key is installed
    /// ([`Self::pairwise_installed`]); its replay owner continues.
    ///
    /// # Errors
    ///
    /// The peer's handshake is not verified, or its key is not installed.
    pub fn authorize(&mut self, peer: [u8; 6], now: Instant) -> Result<(), ApEngineError> {
        if !self
            .replay
            .iter()
            .flatten()
            .any(|replay| replay.peer == peer)
        {
            return Err(ApEngineError::Service(ApServiceError::WrongPeerPhase));
        }
        self.service.authorize(peer, now)?;
        Ok(())
    }

    /// Record the terminal of an EAPOL-Key frame's transmission.
    ///
    /// # Errors
    ///
    /// The authenticator refused the context.
    pub fn eapol_sent(
        &mut self,
        context: crate::service::ApEapolTxContext,
        acknowledged: bool,
        now: Instant,
    ) -> Result<(), ApEngineError> {
        self.service
            .observe_wpa2_transmit(context, acknowledged, now)?;
        Ok(())
    }

    /// Write `transmit`'s EAPOL frame into `output` as an unprotected data
    /// frame to its peer.
    ///
    /// # Errors
    ///
    /// `output` is too short.
    pub fn write_eapol(
        &mut self,
        frame: &RsnTxFrame<AP_EAPOL_CAPACITY>,
        output: &mut [u8],
    ) -> Result<ApFrame, ApEngineError> {
        let peer = *frame.peer();
        let sequence = self.service.next_data_sequence();
        let len = ApDataFrame {
            access_point: self.address(),
            destination: peer,
            sequence_number: sequence,
            ether_type: EAPOL_ETHER_TYPE,
            payload: frame.as_bytes(),
        }
        .encode(output)?;
        Ok(ApFrame {
            len,
            kind: ApFrameKind::Management { peer },
        })
    }

    /// The next instant the engine has work: the next beacon, a handshake
    /// retry or a peer's inactivity.
    pub fn deadline(&self) -> Option<Instant> {
        [
            self.next_beacon(),
            self.service.next_wpa2_retry_deadline(),
            self.service.next_peer_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// A handshake retry due at `now`, or a peer whose handshake ran out.
    ///
    /// # Errors
    ///
    /// The authenticator refused the retry.
    pub fn due_retry<K: ApGroupTransmitKey>(
        &mut self,
        now: Instant,
        group_key: &InstalledCcmpTxKey<K>,
    ) -> Result<Option<ApDue>, ApEngineError> {
        Ok(match self.service.take_due_wpa2_retry(now, group_key)? {
            ApWpa2RetryProgress::None => None,
            ApWpa2RetryProgress::Transmit(transmit) => Some(ApDue::Transmit(transmit)),
            ApWpa2RetryProgress::Close(close) => {
                self.forget_replay(close.peer);
                Some(ApDue::Teardown(close.into()))
            }
        })
    }

    /// A peer inactive at `now`, which the engine now ends.
    pub fn due_inactive(&mut self, now: Instant) -> Option<ApTeardown> {
        let close = self.service.begin_due_peer_close(now)?;
        self.forget_replay(close.peer);
        Some(close.into())
    }

    /// The next peer the AP's stop ends, or `None` once it holds none.
    pub fn stopping_peer(&mut self) -> Option<ApTeardown> {
        let close = self.service.begin_stop_peer()?;
        self.forget_replay(close.peer);
        Some(close.into())
    }

    /// End `peer` for a failed handshake.
    ///
    /// # Errors
    ///
    /// The peer holds no handshake in progress.
    pub fn fail_handshake(&mut self, peer: [u8; 6]) -> Result<ApTeardown, ApEngineError> {
        let close = self.service.begin_wpa2_failure_close(peer)?;
        self.forget_replay(close.peer);
        Ok(close.into())
    }

    /// Write `peer`'s `kind` of teardown into `output`.
    ///
    /// # Errors
    ///
    /// `output` is too short.
    pub fn write_teardown(
        &mut self,
        peer: [u8; 6],
        kind: ApPeerDisconnectKind,
        output: &mut [u8],
    ) -> Result<ApFrame, ApEngineError> {
        let sequence = self.service.next_management_sequence();
        let len = write_ap_peer_disconnect(
            output,
            self.address(),
            peer,
            kind,
            AP_LEAVING_REASON,
            sequence,
        )?;
        Ok(ApFrame {
            len,
            kind: ApFrameKind::Management { peer },
        })
    }

    /// The torn-down `peer` is gone: the AP holds nothing of it.
    ///
    /// # Errors
    ///
    /// The service holds no such peer.
    pub fn forget(&mut self, peer: [u8; 6]) -> Result<(), ApEngineError> {
        self.forget_replay(peer);
        self.service.remove_peer(peer)?;
        Ok(())
    }

    /// Write one MSDU of the network into `output` as a protected data
    /// frame: to its authorized destination under the pairwise key, or to
    /// the BSS under the group key. The caller numbers the frame's packet
    /// number with the key [`ApFrame::kind`] names. `None` when the
    /// destination is no authorized peer: the MSDU does not go.
    ///
    /// # Errors
    ///
    /// `output` is too short or the MSDU's addresses are invalid.
    pub fn encapsulate(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
        output: &mut [u8],
    ) -> Result<Option<ApFrame>, ApEngineError> {
        let group = AddressFlags::from_bytes([destination[0]]).group();
        let (peer, qos, kind) = if group {
            (destination, false, ApFrameKind::Group)
        } else {
            let Some(status) = self
                .service
                .peer_status(destination)
                .filter(|status| status.phase == ApPeerPhase::Authorized)
            else {
                return Ok(None);
            };
            (
                destination,
                status.qos_supported,
                ApFrameKind::Pairwise {
                    peer: destination,
                    rate: oer_protocols_ieee80211_wire::phy::PhyRate::Legacy(legacy_rate(
                        status.maximum_legacy_rate_500kbps,
                    )),
                },
            )
        };
        let sequence = if qos {
            self.service
                .next_qos_sequence(peer, 0)
                .ok_or(ApServiceError::UnknownPeer)?
        } else {
            self.service.next_data_sequence()
        };
        let len = ApProtectedDataFrame {
            access_point: self.address(),
            peer,
            sequence_number: sequence,
            user_priority: 0,
            peer_qos: qos,
            more_data: false,
            // The transmitter writes the CCMP header from the packet
            // number the caller numbers.
            ccmp_header: [0; CCMP_HEADER_LEN],
            msdu: MsduParts {
                destination,
                source,
                ether_type,
                payload,
            },
        }
        .encode(output)?;
        Ok(Some(ApFrame { len, kind }))
    }

    /// The peer service, for the caller's status reads and its teardown.
    pub fn into_service(self) -> AccessPointService<'s, S> {
        self.service
    }
}

/// The legacy rate of `rate_500kbps`, the highest the peer and the BSS
/// share in 500 kbit/s units, of the OFDM set; the lowest OFDM rate when
/// the peer named none of it.
const fn legacy_rate(rate_500kbps: u8) -> oer_protocols_ieee80211_wire::phy::LegacyRate {
    use oer_protocols_ieee80211_wire::phy::LegacyRate;
    match rate_500kbps {
        108.. => LegacyRate::Ofdm54M,
        96..=107 => LegacyRate::Ofdm48M,
        72..=95 => LegacyRate::Ofdm36M,
        48..=71 => LegacyRate::Ofdm24M,
        36..=47 => LegacyRate::Ofdm18M,
        24..=35 => LegacyRate::Ofdm12M,
        18..=23 => LegacyRate::Ofdm9M,
        _ => LegacyRate::Ofdm6M,
    }
}

/// The port's description of a frame the engine wrote in `bytes`: an
/// acknowledged individual frame by `plan`, or a group frame at the plan's
/// first rate. `None` for a header or length the port cannot take.
pub fn port_frame<
    'p,
    K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities,
    C: oer_protocols_ieee80211_contracts_port::TxBufferClass,
>(
    bytes: &[u8],
    frame: ApFrame,
    plan: oer_protocols_ieee80211_contracts_port::RetryPlan<
        oer_protocols_ieee80211_contracts_port::PortRate<'p, K>,
    >,
) -> Option<oer_protocols_ieee80211_contracts_port::ClassFrame<'p, C, K>> {
    use oer_protocols_ieee80211_contracts_port::{
        AckHeader, ClassFrame, GroupAddress, HeaderSpec, MpduLength, SizedFrame, TxFrame,
    };
    let header = oer_protocols_ieee80211_wire::frame::MacHeader::decode(bytes.get(..frame.len)?)?;
    let length = MpduLength::new(frame.len)?;
    let tx = match frame.kind {
        ApFrameKind::Management { .. } | ApFrameKind::Pairwise { .. } => TxFrame::Individual {
            frame: SizedFrame::new(
                AckHeader::new(HeaderSpec::from_mac_header(header, IndividualAddress::new)?)?,
                length,
            )?,
            plan,
        },
        ApFrameKind::Group | ApFrameKind::Beacon => TxFrame::Group {
            frame: SizedFrame::new(
                HeaderSpec::from_mac_header(header, GroupAddress::new)?,
                length,
            )?,
            rate: plan.first().rate,
        },
    };
    ClassFrame::new(tx).ok()
}

/// A due handshake edge.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "the retransmission moves once, by value, to its transmission"
)]
pub enum ApDue {
    /// A retransmission of the handshake's current message.
    Transmit(ApEapolTransmit<AP_EAPOL_CAPACITY>),
    /// The handshake ran out: tear the peer down.
    Teardown(ApTeardown),
}

#[cfg(test)]
mod tests;
