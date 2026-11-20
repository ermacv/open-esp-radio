//! The access point the station joins: a test peer on the model's air, not
//! product access point code. It answers the station's probe with a
//! beacon, its open authentication and its association, runs the
//! authenticator's side of the four-way handshake with the real RSN
//! authenticator (`oer-protocols-ieee80211-core-security-rsn`), which checks
//! the station's MICs, and relays MSDUs between the station and the wired
//! side ([`Wired`]) once the station's port is open. Its frames to the
//! station arrive as the model's hardware hands them up: a protected one
//! decrypted and verified, its CCMP header kept.

use std::collections::VecDeque;
use std::rc::Rc;

use oer_portable_time::{Clock, Instant};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth, RxCryptoStatus, RxEvidence};
use oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
use oer_protocols_ieee80211_core_security_rsn::authenticator::{
    RsnAuthenticator, RsnAuthenticatorStep,
};
use oer_protocols_ieee80211_core_security_rsn::frames::{RsnGtk, RsnPlainKeyData};
use oer_protocols_ieee80211_core_security_rsn::state::RsnApState;
use oer_protocols_ieee80211_core_security_rsn::{OwnedEapolFrame, Pmk, RsnInterface};
use oer_protocols_ieee80211_testing_model::{AirFrame, ModelAir, ModelRxMeta};
use oer_protocols_ieee80211_wire::ccmp::{CcmpPacketNumber, GroupKeyId};
use oer_protocols_ieee80211_wire::security::rsn::Akm;

use super::wired::{Msdu, Wired};

pub const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
pub const SSID: &[u8] = b"open-esp-radio";
pub const CHANNEL: u8 = 6;
/// The network's pairwise master key: the station's profile names the same.
pub const PMK: [u8; 32] = [7; 32];
/// CCMP-128 pairwise and group ciphers with PSK, as the access point
/// advertises them (IEEE 802.11-2020, 9.4.2.24).
pub const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const ETHERTYPE_EAPOL: u16 = 0x888e;
/// The first Key Replay Counter of each association's handshake.
const REPLAY: u64 = 1;

/// Frame Control subtypes, IEEE 802.11-2020, 9.2.4.1.3, with the type bits.
pub mod subtype {
    pub const ASSOCIATION_REQUEST: u16 = 0x0000;
    pub const REASSOCIATION_REQUEST: u16 = 0x0020;
    pub const PROBE_REQUEST: u16 = 0x0040;
    pub const DISASSOCIATION: u16 = 0x00a0;
    pub const AUTHENTICATION: u16 = 0x00b0;
    pub const DEAUTHENTICATION: u16 = 0x00c0;
    pub const DATA: u16 = 0x0008;
    pub const QOS_DATA: u16 = 0x0088;
}

pub fn channel() -> Channel {
    Channel::ghz2_4(CHANNEL, ChannelWidth::Mhz20).expect("channel 6")
}

/// What the access point saw and did, in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApEvent {
    Probed,
    Authenticated,
    /// The station associated, the `n`th time.
    Associated(u32),
    /// The handshake's Message 4 verified: the station's port is open.
    Authorized(u32),
    /// The handshake failed: the access point dropped the station.
    HandshakeFailed,
    /// The station left with this reason.
    Left(u16),
    /// The access point deauthenticated the station.
    Dropped(u16),
}

/// One association: the station and its handshake.
struct Association {
    station: [u8; 6],
    authenticator: Option<RsnAuthenticator>,
    port_open: bool,
    /// The packet number of the access point's next protected frame.
    packet_number: u64,
}

/// How the access point answers.
#[derive(Clone, Copy, Debug)]
pub struct ApConfig {
    /// WPA2-Personal; otherwise open.
    pub protected: bool,
    /// It answers probes, authentications and associations.
    pub present: bool,
}

impl Default for ApConfig {
    fn default() -> Self {
        Self {
            protected: true,
            present: true,
        }
    }
}

/// The access point and the wired side behind it.
pub struct AccessPoint {
    air: Rc<ModelAir>,
    clock: Rc<VirtualClock>,
    pub config: ApConfig,
    /// The air frames taken so far.
    seen: usize,
    association: Option<Association>,
    associations: u32,
    /// The Sequence Number of the access point's next frame.
    sequence: u16,
    pub events: Vec<(Instant, ApEvent)>,
    pub wired: Wired,
    /// MSDUs the station sent before its port was open, which the access
    /// point dropped, and MSDUs the wired side had for no open station.
    pub dropped_from_station: usize,
    pub dropped_to_station: usize,
    /// Station frames whose MSDU the access point could not read.
    pub unreadable: usize,
}

/// An MSDU in a station's data MPDU: its addresses and LLC payload.
struct StationMsdu<'a> {
    source: [u8; 6],
    destination: [u8; 6],
    ether_type: u16,
    payload: &'a [u8],
}

impl AccessPoint {
    pub fn new(air: Rc<ModelAir>, clock: Rc<VirtualClock>, config: ApConfig, wired: Wired) -> Self {
        Self {
            air,
            clock,
            config,
            seen: 0,
            association: None,
            associations: 0,
            sequence: 1,
            events: Vec::new(),
            wired,
            dropped_from_station: 0,
            dropped_to_station: 0,
            unreadable: 0,
        }
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn log(&mut self, event: ApEvent) {
        let now = self.now();
        self.events.push((now, event));
    }

    /// The station's port is open at the access point.
    pub fn port_open(&self) -> bool {
        self.association
            .as_ref()
            .is_some_and(|association| association.port_open)
    }

    /// How many times the station associated.
    pub const fn associations(&self) -> u32 {
        self.associations
    }

    /// Take what the station put on the air since the last serve and answer
    /// it, then send what the wired side has for the station: whether it
    /// sent anything.
    pub fn serve(&mut self) -> bool {
        let sent = self.air.sent();
        let mut answered = false;
        while let Some(frame) = sent.get(self.seen) {
            self.seen += 1;
            answered |= self.station_frame(frame);
        }
        answered | self.flush()
    }

    /// Send the wired side's MSDUs for the station.
    fn flush(&mut self) -> bool {
        let pending: VecDeque<Msdu> = self.wired.take_outgoing();
        let mut sent = false;
        for msdu in pending {
            if self.port_open() {
                self.relay(&msdu);
                sent = true;
            } else {
                self.dropped_to_station += 1;
            }
        }
        sent
    }

    fn station_frame(&mut self, frame: &AirFrame) -> bool {
        let bytes = &frame.plaintext;
        let Some(control) = bytes.get(..2) else {
            return false;
        };
        let control = u16::from_le_bytes([control[0], control[1]]);
        let kind = control & 0x00fc;
        let Some(source) = bytes.get(10..16).and_then(|s| <[u8; 6]>::try_from(s).ok()) else {
            return false;
        };
        if bytes.get(4..10) != Some(&BSSID[..]) && kind != subtype::PROBE_REQUEST {
            return false;
        }
        match kind {
            subtype::PROBE_REQUEST if self.config.present => {
                self.log(ApEvent::Probed);
                self.receive(&beacon(self.config.protected), false);
                true
            }
            subtype::AUTHENTICATION if self.config.present => {
                // Open System, transaction 1, IEEE 802.11-2020, 9.3.3.12.
                if bytes.get(24..28) != Some(&[0, 0, 1, 0][..]) {
                    return false;
                }
                self.log(ApEvent::Authenticated);
                self.receive(&authentication_response(source), false);
                true
            }
            subtype::ASSOCIATION_REQUEST | subtype::REASSOCIATION_REQUEST
                if self.config.present =>
            {
                self.associate(source, bytes, kind == subtype::REASSOCIATION_REQUEST);
                true
            }
            subtype::DEAUTHENTICATION | subtype::DISASSOCIATION => {
                let reason = bytes
                    .get(24..26)
                    .map_or(0, |reason| u16::from_le_bytes([reason[0], reason[1]]));
                if self
                    .association
                    .as_ref()
                    .is_some_and(|association| association.station == source)
                {
                    self.association = None;
                    self.log(ApEvent::Left(reason));
                }
                false
            }
            subtype::DATA | subtype::QOS_DATA => self.data(bytes),
            _ => false,
        }
    }

    fn associate(&mut self, station: [u8; 6], request: &[u8], reassociation: bool) {
        // Capability Information and Listen Interval, and a reassociation's
        // Current AP Address, precede the elements (9.3.3.6, 9.3.3.8).
        let elements = request
            .get(if reassociation { 34 } else { 28 }..)
            .unwrap_or(&[]);
        let station_rsn = element(elements, 48).map(<[u8]>::to_vec);
        self.associations += 1;
        let count = self.associations;
        self.receive(&association_response(station, self.config.protected), false);
        self.log(ApEvent::Associated(count));
        if !self.config.protected {
            self.association = Some(Association {
                station,
                authenticator: None,
                port_open: true,
                packet_number: 1,
            });
            self.log(ApEvent::Authorized(count));
            return;
        }
        let Some(station_rsn) = station_rsn else {
            self.association = None;
            self.log(ApEvent::HandshakeFailed);
            return;
        };
        let mut anonce = [0x44; 32];
        anonce[0] = u8::try_from(count % 256).unwrap_or(0);
        let state = RsnApState::new(Akm::Psk, BSSID, station, anonce, REPLAY).expect("a state");
        let mut authenticator = RsnAuthenticator::new(
            state,
            Pmk::from_bytes(PMK).bind_association_security_ies(&station_rsn),
        );
        let (message1, _) = authenticator.message1::<512>().expect("Message 1");
        self.association = Some(Association {
            station,
            authenticator: Some(authenticator),
            port_open: false,
            packet_number: 1,
        });
        self.eapol(station, message1.as_bytes());
    }

    /// A data MPDU from the station: EAPOL to the authenticator, the rest
    /// to the wired side once the port is open.
    fn data(&mut self, bytes: &[u8]) -> bool {
        let Some(msdu) = station_msdu(bytes) else {
            self.unreadable += 1;
            return false;
        };
        let Some(association) = self.association.as_mut() else {
            self.dropped_from_station += 1;
            return false;
        };
        if msdu.source != association.station {
            return false;
        }
        if msdu.ether_type == ETHERTYPE_EAPOL {
            let station = association.station;
            let payload = msdu.payload.to_vec();
            return self.handshake(station, &payload);
        }
        if !association.port_open {
            self.dropped_from_station += 1;
            return false;
        }
        let now = self.clock.now();
        self.wired.take(
            Msdu {
                destination: msdu.destination,
                source: msdu.source,
                ether_type: msdu.ether_type,
                payload: msdu.payload.to_vec(),
            },
            now,
        );
        self.flush()
    }

    fn handshake(&mut self, station: [u8; 6], eapol: &[u8]) -> bool {
        let count = self.associations;
        let Some(association) = self.association.as_mut() else {
            return false;
        };
        let Some(authenticator) = association.authenticator.as_mut() else {
            return false;
        };
        let Ok(frame) = OwnedEapolFrame::<512>::try_copy(RsnInterface::AccessPoint, station, eapol)
        else {
            return false;
        };
        let step = authenticator.on_frame(&Pmk::from_bytes(PMK), frame, &mut SoftwareAes128);
        match step {
            Ok(RsnAuthenticatorStep::PrepareMessage3(preparation)) => {
                let gtk = RsnGtk::new(GroupKeyId::new(1).expect("key 1"), true, &[0x55; 16]);
                let key_data = RsnPlainKeyData::<512>::build(&RSN, &gtk, None).expect("key data");
                let (message3, _) = authenticator
                    .complete_message3::<512, 512, _>(
                        preparation,
                        &key_data,
                        CcmpPacketNumber::new(0).expect("a packet number"),
                        &mut SoftwareAes128,
                    )
                    .expect("Message 3");
                let message3 = message3.as_bytes().to_vec();
                self.eapol(station, &message3);
                true
            }
            Ok(RsnAuthenticatorStep::Authorized) => {
                authenticator.forget_handshake_keys();
                association.port_open = true;
                self.log(ApEvent::Authorized(count));
                self.flush();
                true
            }
            Ok(RsnAuthenticatorStep::None) => false,
            Ok(RsnAuthenticatorStep::Deauthenticate) | Err(_) => {
                self.association = None;
                self.log(ApEvent::HandshakeFailed);
                self.deauthenticate_frame(station, 15);
                true
            }
        }
    }

    /// Deauthenticate the station with `reason` and forget it: its link
    /// is lost.
    pub fn deauthenticate(&mut self, reason: u16) {
        if let Some(association) = self.association.take() {
            self.deauthenticate_frame(association.station, reason);
            self.log(ApEvent::Dropped(reason));
        }
    }

    fn deauthenticate_frame(&mut self, station: [u8; 6], reason: u16) {
        let mut frame = vec![0; 26];
        frame[0] = 0xc0;
        frame[4..10].copy_from_slice(&station);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&reason.to_le_bytes());
        self.receive(&frame, false);
    }

    /// An EAPOL frame to the station, never protected: Messages 1 and 3.
    fn eapol(&mut self, station: [u8; 6], eapol: &[u8]) {
        let frame = self.data_frame(station, BSSID, ETHERTYPE_EAPOL, eapol, None);
        self.receive(&frame, false);
    }

    /// Relay `msdu` to the open station, protected on a protected link.
    fn relay(&mut self, msdu: &Msdu) {
        let protected = self.config.protected;
        let Some(association) = self.association.as_mut() else {
            return;
        };
        let packet_number = protected.then(|| {
            let number = association.packet_number;
            association.packet_number += 1;
            number
        });
        let frame = self.data_frame(
            msdu.destination,
            msdu.source,
            msdu.ether_type,
            &msdu.payload,
            packet_number,
        );
        self.receive(&frame, protected);
    }

    /// A data MPDU from the DS (9.3.2.1): to `destination` from `source`,
    /// under CCMP's header of `packet_number` when protected.
    fn data_frame(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
        packet_number: Option<u64>,
    ) -> Vec<u8> {
        let mut frame = vec![0_u8; 24];
        let control: u16 = if packet_number.is_some() {
            0x4208
        } else {
            0x0208
        };
        frame[0..2].copy_from_slice(&control.to_le_bytes());
        frame[4..10].copy_from_slice(&destination);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&source);
        frame[22..24].copy_from_slice(&(self.sequence << 4).to_le_bytes());
        self.sequence = (self.sequence + 1) % 4096;
        if let Some(number) = packet_number {
            // 12.5.3.2: PN0, PN1, reserved, Key ID octet with Ext IV, PN2..PN5.
            let pn = number.to_le_bytes();
            frame.extend_from_slice(&[pn[0], pn[1], 0, 0x20, pn[2], pn[3], pn[4], pn[5]]);
        }
        frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
        frame.extend_from_slice(&ether_type.to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    /// Put `frame` on the station's air.
    fn receive(&self, frame: &[u8], decrypted: bool) {
        let meta = ModelRxMeta::new(channel(), -40, -95);
        let meta = if decrypted {
            meta.with_crypto(RxEvidence::HardwareObserved(
                RxCryptoStatus::DecryptedAndIntegrityVerified,
            ))
        } else {
            meta
        };
        self.air.receive(frame, meta);
    }
}

/// The MSDU of a station's data MPDU to the DS: its header, a QoS Control
/// field, the CCMP header the DMA image keeps, then LLC/SNAP.
fn station_msdu(bytes: &[u8]) -> Option<StationMsdu<'_>> {
    let control = u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]);
    let qos = control & 0x0080 != 0;
    let to_ds = control & 0x0100 != 0;
    let protected = control & 0x4000 != 0;
    if !to_ds {
        return None;
    }
    let mut header = 24;
    if qos {
        header += 2;
    }
    if protected {
        header += 8;
    }
    let source = bytes.get(10..16)?.try_into().ok()?;
    let destination = bytes.get(16..22)?.try_into().ok()?;
    let llc = bytes.get(header..)?;
    if llc.get(..6)? != [0xaa, 0xaa, 0x03, 0, 0, 0] {
        return None;
    }
    let ether_type = u16::from_be_bytes([*llc.get(6)?, *llc.get(7)?]);
    Some(StationMsdu {
        source,
        destination,
        ether_type,
        payload: llc.get(8..)?,
    })
}

/// The element `id` among `elements`, whole: identifier, length and body.
fn element(elements: &[u8], id: u8) -> Option<&[u8]> {
    let mut rest = elements;
    while let [found, length, ..] = rest {
        let end = 2 + usize::from(*length);
        let whole = rest.get(..end)?;
        if *found == id {
            return Some(whole);
        }
        rest = rest.get(end..)?;
    }
    None
}

/// A beacon of the network on channel 6 (9.3.3.2): SSID, the DSSS rates,
/// the DS Parameter Set, and the RSN element when protected.
fn beacon(protected: bool) -> Vec<u8> {
    let mut frame = vec![0_u8; 36];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    let capability: u16 = if protected { 0x0011 } else { 0x0001 };
    frame[34..36].copy_from_slice(&capability.to_le_bytes());
    frame.extend_from_slice(&[0, u8::try_from(SSID.len()).expect("an SSID")]);
    frame.extend_from_slice(SSID);
    frame.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    frame.extend_from_slice(&[3, 1, CHANNEL]);
    if protected {
        frame.extend_from_slice(&RSN);
    }
    frame
}

/// A successful Open System authentication, transaction 2 (9.3.3.12).
fn authentication_response(station: [u8; 6]) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&station);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame
}

/// A successful Association Response of AID 42 (9.3.3.7).
fn association_response(station: [u8; 6], protected: bool) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&station);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    let capability: u16 = if protected { 0x0431 } else { 0x0421 };
    frame[24..26].copy_from_slice(&capability.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}
