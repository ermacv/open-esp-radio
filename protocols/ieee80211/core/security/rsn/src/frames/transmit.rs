//! Bounded EAPOL transmission and typed handshake action encoding.

use super::*;
use crate::AkmKeys;
use oer_portable_crypto::block::Aes128Engine;
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::eapol::{
    EapolEncodeError, EapolKeyBuffer, EapolKeyFields, EapolKeyFixedFields, EapolProtocolVersion,
};

/// The Key Length field these EAPOL-Key frames carry: `RSN_GTK_LEN`
/// octets (IEEE Std 802.11-2020, 12.7.2).
const GTK_KEY_LENGTH: u16 = 16;
const _: () = {
    if GTK_KEY_LENGTH as usize != RSN_GTK_LEN {
        panic!("the Key Length field names the GTK's length");
    }
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnTxFrame<const N: usize = RSN_TX_EAPOL_CAPACITY> {
    interface: RsnInterface,
    peer: Eui48,
    pub(crate) retransmission: bool,
    packet: EapolKeyBuffer<N>,
}

impl<const N: usize> RsnTxFrame<N> {
    pub fn message1(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            EapolProtocolVersion::V2004,
            KeyInformation::EMPTY
                .with_descriptor_version(akm.key_descriptor_version())
                .with_pairwise(true)
                .with_ack(true),
            GTK_KEY_LENGTH,
            replay_counter,
            authenticator_nonce,
            [0; 8],
            &[],
        )
    }

    /// Message 1 of a nonce the caller proved nonzero. It cannot refuse:
    /// a Message 1 carries no Key Data, and its room in `N` is checked when
    /// `N` is chosen.
    pub(crate) fn authenticator_message1(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    ) -> Self {
        Self {
            interface: RsnInterface::AccessPoint,
            peer,
            retransmission: false,
            packet: EapolKeyBuffer::encode_without_key_data(&EapolKeyFixedFields {
                protocol_version: EapolProtocolVersion::V2004,
                key_information: KeyInformation::EMPTY
                    .with_descriptor_version(akm.key_descriptor_version())
                    .with_pairwise(true)
                    .with_ack(true),
                key_length: GTK_KEY_LENGTH,
                replay_counter,
                nonce: authenticator_nonce,
                key_rsc: [0; 8],
            }),
        }
    }

    pub fn message2<const R: usize>(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        rsn_ie: &OwnedRsnIe<R>,
    ) -> Result<Self, RsnFrameError> {
        let security_ies = OwnedAssociationSecurityIes::<R>::try_copy(rsn_ie, &[])?;
        Ok(Self::message2_with_security_ies(
            akm,
            peer,
            replay_counter,
            supplicant_nonce,
            &security_ies,
        ))
    }

    /// Message 2 echoing the association's security IEs. It cannot refuse:
    /// the IEs' bound is their type, and their room in `N` is checked when
    /// `N` and `R` are chosen.
    pub fn message2_with_security_ies<const R: usize>(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        security_ies: &OwnedAssociationSecurityIes<R>,
    ) -> Self {
        let (bytes, length) = security_ies.bounded();
        Self::station(
            EapolKeyBuffer::encode_with_bounded_key_data(
                &EapolKeyFixedFields {
                    protocol_version: EapolProtocolVersion::V2001,
                    key_information: KeyInformation::EMPTY
                        .with_descriptor_version(akm.key_descriptor_version())
                        .with_pairwise(true)
                        .with_mic(true),
                    key_length: 0,
                    replay_counter,
                    nonce: supplicant_nonce,
                    key_rsc: [0; 8],
                },
                bytes,
                length,
            ),
            peer,
        )
    }

    pub fn message3(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            EapolProtocolVersion::V2004,
            KeyInformation::EMPTY
                .with_descriptor_version(akm.key_descriptor_version())
                .with_pairwise(true)
                .with_install(true)
                .with_ack(true)
                .with_mic(true)
                .with_secure(true)
                .with_encrypted_key_data(true),
            GTK_KEY_LENGTH,
            replay_counter,
            authenticator_nonce,
            key_rsc,
            encrypted_key_data,
        )
    }

    /// Message 4. It cannot refuse: it carries no Key Data.
    pub fn message4(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    ) -> Self {
        Self::station(
            EapolKeyBuffer::encode_without_key_data(&EapolKeyFixedFields {
                protocol_version: EapolProtocolVersion::V2001,
                key_information: KeyInformation::EMPTY
                    .with_descriptor_version(akm.key_descriptor_version())
                    .with_pairwise(true)
                    .with_mic(true)
                    .with_secure(true),
                key_length: 0,
                replay_counter,
                nonce: oer_protocols_ieee80211_wire::eapol::Nonce::ZERO,
                key_rsc: [0; 8],
            }),
            peer,
        )
    }

    /// The station's response to one connected-state Group Message 1. It
    /// cannot refuse: it carries no Key Data.
    pub fn group_message2(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    ) -> Self {
        Self::station(
            EapolKeyBuffer::encode_without_key_data(&EapolKeyFixedFields {
                protocol_version: EapolProtocolVersion::V2001,
                key_information: KeyInformation::EMPTY
                    .with_descriptor_version(akm.key_descriptor_version())
                    .with_mic(true)
                    .with_secure(true),
                key_length: 0,
                replay_counter,
                nonce: oer_protocols_ieee80211_wire::eapol::Nonce::ZERO,
                key_rsc: [0; 8],
            }),
            peer,
        )
    }

    /// A station's frame of `packet` to `peer`.
    const fn station(packet: EapolKeyBuffer<N>, peer: Eui48) -> Self {
        Self {
            interface: RsnInterface::Station,
            peer,
            retransmission: false,
            packet,
        }
    }

    /// Build an authenticator Group Message 1 for protocol tests and the
    /// future AP authenticator. The caller supplies RFC3394-wrapped GTK data.
    pub fn group_message1(
        akm: Akm,
        peer: Eui48,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            EapolProtocolVersion::V2004,
            KeyInformation::EMPTY
                .with_descriptor_version(akm.key_descriptor_version())
                .with_ack(true)
                .with_mic(true)
                .with_secure(true)
                .with_encrypted_key_data(true),
            GTK_KEY_LENGTH,
            replay_counter,
            oer_protocols_ieee80211_wire::eapol::Nonce::ZERO,
            key_rsc,
            encrypted_key_data,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        interface: RsnInterface,
        peer: Eui48,
        protocol_version: EapolProtocolVersion,
        key_info: KeyInformation,
        key_length: u16,
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        key_rsc: [u8; 8],
        key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if key_info.pairwise() && key_info.ack() && nonce.is_zero() {
            return Err(RsnFrameError::ZeroNonce);
        }
        let packet = EapolKeyBuffer::encode(&EapolKeyFields {
            fixed: EapolKeyFixedFields {
                protocol_version,
                key_information: key_info,
                key_length,
                replay_counter,
                nonce,
                key_rsc,
            },
            key_data,
        })
        .map_err(|error| match error {
            EapolEncodeError::KeyDataTooLong | EapolEncodeError::BufferTooSmall { .. } => {
                RsnFrameError::CapacityExceeded
            }
        })?;
        Ok(Self {
            interface,
            peer,
            retransmission: false,
            packet,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub const fn peer(&self) -> Eui48 {
        self.peer
    }

    /// Semantic retry classification supplied by the WPA state transition.
    /// EAPOL-Key does not carry a standalone retransmission flag, so keeping
    /// this metadata prevents the TX-complete owner from guessing from bytes.
    pub const fn retransmission(&self) -> bool {
        self.retransmission
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.packet.as_bytes()
    }

    pub fn key_frame(
        &self,
    ) -> Result<EapolKeyFrame<'_>, oer_protocols_ieee80211_wire::eapol::EapolParseError> {
        EapolKeyFrame::parse(self.as_bytes())
    }

    /// Authenticate a supplicant or authenticator action with the pairwise
    /// KCK. The builder always initializes the MIC field to zero; clearing it
    /// here as well makes repeated authentication deterministic.
    /// An AES-CMAC suite runs on `engine`; its refusal is the error.
    pub fn authenticate<E: Aes128Engine>(
        mut self,
        ptk: &Ptk,
        engine: &mut E,
    ) -> Result<Self, E::Stop> {
        self.authenticate_with_kck(ptk.akm(), ptk.kck(), engine)?;
        Ok(self)
    }

    pub(crate) fn authenticate_with_confirmation_key<E: Aes128Engine>(
        mut self,
        key: &RsnKeyConfirmationKey,
        engine: &mut E,
    ) -> Result<Self, E::Stop> {
        self.authenticate_with_kck(key.akm(), key.kck(), engine)?;
        Ok(self)
    }

    fn authenticate_with_kck<E: Aes128Engine>(
        &mut self,
        akm: Akm,
        kck: &Kck,
        engine: &mut E,
    ) -> Result<(), E::Stop> {
        self.set_mic(&[0; RSN_MIC_LEN]);
        let mic = akm.mic(kck, engine, [self.as_bytes()])?;
        self.set_mic(&mic);
        Ok(())
    }

    pub(crate) const fn mark_retransmission(mut self) -> Self {
        self.retransmission = true;
        self
    }

    fn set_mic(&mut self, mic: &[u8; RSN_MIC_LEN]) {
        self.packet.set_mic(mic);
    }
}

#[cfg(test)]
mod tests;
