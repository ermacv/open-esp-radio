//! The test fixtures shared by the station service's tests: the access
//! point's frames, the station's configuration and the model's contexts.

use std::boxed::Box;
use std::vec::Vec;

use oer_portable_time::Duration;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth, SupportedChannel};
use oer_protocols_ieee80211_core_roles_sta::machine::{StaConfig, StaSecurity};
use oer_protocols_ieee80211_core_security_rsn::frames::RsnTxFrame;
use oer_protocols_ieee80211_testing_model::{AirFrame, ModelCaps24, ModelRxMeta};
use oer_protocols_ieee80211_wire::phy::{LegacyRate, PhyRate};
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::security::rsn::Akm;
use oer_protocols_ieee80211_wire::station::AssociationCapabilities;
use oer_protocols_radio_port::SlotIdentity;

use crate::station::Outgoing;

pub(crate) mod block_ack;

pub(crate) const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
pub(crate) const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
pub(crate) const SSID: &[u8] = b"open-net";
pub(crate) const SNONCE: [u8; 32] = [3; 32];
pub(crate) const ANONCE: [u8; 32] = [4; 32];
pub(crate) const PMK: [u8; 32] = [7; 32];
/// CCMP-128 with PSK.
pub(crate) const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

pub(crate) struct ServiceContexts;

impl oer_protocols_ieee80211_testing_model::ModelContext for ServiceContexts {
    type Body = core::convert::Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = oer_protocols_radio_port::ContextChoice<
        (),
        oer_protocols_ieee80211_core_roles_sta::epoch::EpochContext<
            oer_protocols_ieee80211_testing_model::ModelPool<Id, 4>,
            (),
        >,
    >;
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> =
        oer_protocols_ieee80211_core_roles_sta::machine::StaConfigurationContext<
            'p,
            oer_protocols_ieee80211_testing_model::ModelPool<Id, 4>,
        >;
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

pub(crate) static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: None,
    wmm: [0; 9],
};

pub(crate) struct Callbacks(
    pub(crate) oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
);
impl oer_protocols_ieee80211_core_roles_sta::machine::StaCallbacks for Callbacks {
    type AesEngine = oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
    fn nonce(&mut self) -> [u8; 32] {
        SNONCE
    }
    fn aes_engine(&mut self) -> &mut Self::AesEngine {
        &mut self.0
    }
    fn he_packet_padding(
        &self,
        _: &[u8],
    ) -> oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding {
        oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding::None
    }
}

/// The network's memory the station's data MSDUs come in.
pub(crate) type NetworkMemory = oer_protocols_ieee80211_testing_model::ModelNetworkMemory;

/// A network frame of user `priority`, written into fresh network memory
/// where the data machine reads it.
pub(crate) fn network_frame(
    parts: oer_protocols_ieee80211_wire::data::MsduParts<'_>,
    priority: WmmUserPriority,
) -> Outgoing<NetworkMemory> {
    use oer_protocols_ieee80211_core_roles_sta::data::{MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET};
    let mut memory: NetworkMemory =
        Box::new([0; oer_protocols_ieee80211_testing_model::MODEL_MAX_MPDU]);
    memory[MSDU_ETHERNET_OFFSET..][..6].copy_from_slice(&parts.destination);
    memory[MSDU_ETHERNET_OFFSET + 6..][..6].copy_from_slice(&parts.source);
    memory[MSDU_ETHERNET_OFFSET + 12..][..2].copy_from_slice(&parts.ether_type.to_be_bytes());
    memory[MSDU_PAYLOAD_OFFSET..][..parts.payload.len()].copy_from_slice(parts.payload);
    Outgoing {
        memory,
        length: u16::try_from(14 + parts.payload.len()).unwrap(),
        priority,
    }
}

pub(crate) fn channel6() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

pub(crate) type TestConfig = (StaConfig<ModelCaps24, Callbacks>, StaSecurity);

pub(crate) fn config(security: StaSecurity) -> TestConfig {
    let channels = heapless::Vec::from_slice(&[SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel6()))
            .unwrap(),
    )
    .unwrap()])
    .unwrap();
    (
        StaConfig {
            address: oer_protocols_ieee80211_wire::address::OwnAddress::new(
                oer_protocols_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
            ),
            channels,
            wide_channels: heapless::Vec::new(),
            dwell: Duration::from_millis(10),
            skipped_absences: 3,
            access_point_margin_db: 6,
            supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
            association: &CAPABILITIES,
            listen_interval:
                oer_protocols_ieee80211_core_roles_sta::request::StationListenInterval::new(10)
                    .unwrap(),
            phy: oer_protocols_ieee80211_core_roles_sta::association::StaPhyRequest::Legacy,
            tx_block_ack: None,
            callbacks: Callbacks(oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128),
        },
        security,
    )
}

/// A beacon of the network on channel 6, protected or not.
pub(crate) fn beacon(protected: bool) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    let capability: u16 = if protected { 0x0011 } else { 0x0001 };
    frame[34..36].copy_from_slice(&capability.to_le_bytes());
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = 6;
    if protected {
        frame.extend_from_slice(&RSN);
    }
    frame
}

pub(crate) fn authentication_response(request: &[u8]) -> Option<[u8; 30]> {
    let (request, _) = request.split_first_chunk::<30>()?;
    if u16::from_le_bytes([request[0], request[1]]) & !0x0800 != 0x00b0
        || request[4..10] != BSSID
        || request[10..16] != LOCAL
        || request[16..22] != BSSID
        || request[24..30] != [0, 0, 1, 0, 0, 0]
    {
        return None;
    }
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    Some(frame)
}

pub(crate) fn association_response(protected: bool) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&0x0010_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    let capability: u16 = if protected { 0x0431 } else { 0x0421 };
    frame[24..26].copy_from_slice(&capability.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

/// An EAPOL frame from the access point, in a data frame from the DS.
pub(crate) fn eapol_frame(eapol: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame.extend_from_slice(eapol);
    frame
}

/// Group Message 1 of key `key_id` with replay counter `replay`.
pub(crate) fn group_message1(
    ptk: &oer_protocols_ieee80211_core_security_rsn::Ptk,
    replay: u64,
    key_id: u8,
) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, key_id, 0x00]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0, 0, 0, 0, 0]);
    let wrapped = oer_protocols_ieee80211_core_security_cipher::aes::software_aes128_key_wrap(
        ptk.kek(),
        &plain,
    )
    .unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(
                ptk,
                &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
            )
            .unwrap();
    eapol_frame(message.as_bytes())
}

pub(crate) fn protected_group_message1(
    ptk: &oer_protocols_ieee80211_core_security_rsn::Ptk,
    replay: u64,
    key_id: u8,
) -> Vec<u8> {
    use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    let mut frame = group_message1(ptk, replay, key_id);
    frame[1] |= 0x40;
    let header = CcmpHeader::new(
        CcmpPacketNumber::new(replay).unwrap(),
        CcmpKeyId::new(0).unwrap(),
    );
    frame.splice(24..24, header.encode());
    frame
}

/// What the port reported of a frame the access point sent: its signal
/// over the noise floor, 55 dB.
pub(crate) fn meta() -> ModelRxMeta {
    ModelRxMeta::new(channel6(), -40, -95)
}

/// The Frame Control subtype bits of a frame.
pub(crate) fn kind(frame: &AirFrame) -> u16 {
    u16::from_le_bytes([frame.plaintext[0], frame.plaintext[1]]) & 0x00fc
}

pub(crate) const PROBE_REQUEST: u16 = 0x0040;
pub(crate) const AUTHENTICATION: u16 = 0x00b0;
pub(crate) const ASSOCIATION_REQUEST: u16 = 0x0000;
pub(crate) const DEAUTHENTICATION: u16 = 0x00c0;
pub(crate) const DATA: u16 = 0x0008;

pub(crate) fn dsss1() -> PhyRate {
    PhyRate::Legacy(LegacyRate::Dsss1M)
}

pub(crate) fn message3(ptk: &oer_protocols_ieee80211_core_security_rsn::Ptk) -> Vec<u8> {
    let mut plain = Vec::new();
    plain.extend_from_slice(&RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_protocols_ieee80211_core_security_cipher::aes::software_aes128_key_wrap(
        ptk.kek(),
        &plain,
    )
    .unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(
                ptk,
                &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
            )
            .unwrap();
    eapol_frame(message3.as_bytes())
}

/// A protected data MPDU from the DS to the station, as the hardware
/// hands it up once it decrypted and verified it: the CCMP header of
/// `packet_number` stays, the MIC is gone.
pub(crate) fn protected_data(packet_number: u8, source: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x4208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&(u16::from(packet_number) << 4).to_le_bytes());
    frame.extend_from_slice(&[packet_number, 0, 0, 0x20, 0, 0, 0, 0]);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn decrypted() -> ModelRxMeta {
    meta().with_crypto(
        oer_protocols_ieee80211_contracts_port::RxEvidence::HardwareObserved(
            oer_protocols_ieee80211_contracts_port::RxCryptoStatus::DecryptedAndIntegrityVerified,
        ),
    )
}

pub(crate) const NULL: u16 = 0x0048;

pub(crate) const QOS_DATA: u16 = 0x0088;

/// A WMM Parameter element: the network is a QoS BSS.
pub(crate) const WMM: [u8; 26] = [
    0xdd, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x00, 0x00, 0x03, 0xa4, 0x00, 0x00, 0x27, 0xa4,
    0x00, 0x00, 0x42, 0x43, 0x5e, 0x00, 0x62, 0x32, 0x2f, 0x00,
];

/// Whether `frame` is a protected QoS Data frame carrying EAPOL.
pub(crate) fn carries_eapol(frame: &AirFrame) -> bool {
    kind(frame) == QOS_DATA && frame.plaintext.get(40..42) == Some(&[0x88, 0x8e][..])
}

/// The Sequence Number of a QoS Data frame.
pub(crate) fn qos_sequence(frame: &AirFrame) -> u16 {
    u16::from_le_bytes([frame.plaintext[22], frame.plaintext[23]]) >> 4
}

/// An unprotected QoS Data A-MSDU from `source` to the station, one
/// subframe per payload.
pub(crate) fn open_amsdu(source: [u8; 6], payloads: &[&[u8]]) -> Vec<u8> {
    let mut frame = protected_data(1, source, &[]);
    frame.truncate(24);
    frame[0] = 0x88;
    frame[1] &= !0x40;
    // QoS Control: TID 0, A-MSDU present.
    frame.extend_from_slice(&[0x80, 0]);
    for (index, payload) in payloads.iter().enumerate() {
        if index != 0 {
            while !(frame.len() - 26).is_multiple_of(4) {
                frame.push(0);
            }
        }
        frame.extend_from_slice(&LOCAL);
        frame.extend_from_slice(&source);
        frame.extend_from_slice(&u16::try_from(8 + payload.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
        frame.extend_from_slice(payload);
    }
    frame
}
